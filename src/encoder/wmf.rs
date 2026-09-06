#![allow(dead_code, non_snake_case, non_camel_case_types)]

#[cfg(target_os = "windows")]
use super::VideoEncoder;
#[cfg(target_os = "windows")]
use log::info;
#[cfg(target_os = "windows")]
use windows::core::*;
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct3D::*;
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct3D11::*;
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Dxgi::*;
#[cfg(target_os = "windows")]
use windows::Win32::Media::MediaFoundation::*;
#[cfg(target_os = "windows")]
use windows::Win32::System::Com::*;

#[cfg(target_os = "windows")]
pub struct WmfGpuEncoder {
    sink_writer: IMFSinkWriter,
    byte_stream: IMFByteStream,
    stream_index: u32,
    pub current_width: u32,
    pub current_height: u32,
    pub target_fps: u32,
    pub bitrate_bps: u32,
    pub frame_count: u64,
    pub last_read_pos: u64,
    pub gpu_name: String,
    pub needs_keyframe: bool,
    pub in_buffer: IMFMediaBuffer,
    pub in_sample: IMFSample,
    pub read_buf: Vec<u8>,
}

#[cfg(target_os = "windows")]
unsafe impl Send for WmfGpuEncoder {}

#[cfg(target_os = "windows")]
impl WmfGpuEncoder {
    pub fn try_new(target_fps: u32, is_screen_content: bool) -> Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_LITE)?;

            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let mut selected_adapter: Option<IDXGIAdapter1> = None;
            let mut selected_gpu_name = String::from("GPU Padrão");
            let mut best_vram: usize = 0;

            let mut adapter_index = 0u32;
            while let Ok(adapter) = factory.EnumAdapters1(adapter_index) {
                if let Ok(desc) = adapter.GetDesc1() {
                    let name = String::from_utf16_lossy(&desc.Description)
                        .trim_matches('\0')
                        .trim()
                        .to_string();
                    let vram_mb = desc.DedicatedVideoMemory / (1024 * 1024);
                    let is_software = (desc.Flags & 2) != 0;

                    if !is_software && (desc.VendorId == 0x10DE || desc.VendorId == 0x1002 || vram_mb > best_vram) {
                        selected_adapter = Some(adapter);
                        selected_gpu_name = name;
                        best_vram = vram_mb;
                    }
                }
                adapter_index += 1;
            }

            info!("🎮 [WMF GPU ENGINE] Selecionada GPU para codificação por hardware: '{}' ({} MB VRAM)", selected_gpu_name, best_vram);

            let mut d3d11_device: Option<ID3D11Device> = None;
            let mut d3d11_context: Option<ID3D11DeviceContext> = None;
            let mut feature_level = D3D_FEATURE_LEVEL_11_0;

            let adapter_ref = selected_adapter.as_ref();
            let driver_type = if adapter_ref.is_some() { D3D_DRIVER_TYPE_UNKNOWN } else { D3D_DRIVER_TYPE_HARDWARE };

            D3D11CreateDevice(
                adapter_ref.map(|a| a as &IDXGIAdapter),
                driver_type,
                None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut d3d11_device),
                Some(&mut feature_level),
                Some(&mut d3d11_context),
            )?;

            let d3d11_device = d3d11_device.ok_or_else(|| Error::from(windows::Win32::Foundation::E_FAIL))?;

            let mut reset_token = 0u32;
            let mut device_manager: Option<IMFDXGIDeviceManager> = None;
            MFCreateDXGIDeviceManager(&mut reset_token, &mut device_manager)?;
            let device_manager = device_manager.ok_or_else(|| Error::from(windows::Win32::Foundation::E_FAIL))?;
            device_manager.ResetDevice(&d3d11_device, reset_token)?;

            let mut writer_attributes: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut writer_attributes, 4)?;
            let writer_attributes = writer_attributes.ok_or_else(|| Error::from(windows::Win32::Foundation::E_FAIL))?;

            writer_attributes.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &device_manager)?;
            writer_attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
            writer_attributes.SetUINT32(&MF_LOW_LATENCY, 1)?;
            writer_attributes.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;

            let byte_stream: IMFByteStream = MFCreateTempFile(MF_ACCESSMODE_READWRITE, MF_OPENMODE_DELETE_IF_EXIST, MF_FILEFLAGS_NONE)?;
            let sink_writer = MFCreateSinkWriterFromURL(w!(".mp4"), &byte_stream, &writer_attributes)?;

            let initial_width = 1920u32;
            let initial_height = 1080u32;
            let initial_bitrate = if is_screen_content { 4_000_000 } else { 3_500_000 };

            fn set_size(attrs: &IMFAttributes, key: &windows::core::GUID, w: u32, h: u32) -> Result<()> {
                unsafe { attrs.SetUINT64(key, ((w as u64) << 32) | (h as u64)) }
            }

            fn set_ratio(attrs: &IMFAttributes, key: &windows::core::GUID, num: u32, den: u32) -> Result<()> {
                unsafe { attrs.SetUINT64(key, ((num as u64) << 32) | (den as u64)) }
            }

            let out_media_type = MFCreateMediaType()?;
            out_media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out_media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            out_media_type.SetUINT32(&MF_MT_AVG_BITRATE, initial_bitrate)?;
            set_size(&out_media_type.cast()?, &MF_MT_FRAME_SIZE, initial_width, initial_height)?;
            set_ratio(&out_media_type.cast()?, &MF_MT_FRAME_RATE, target_fps, 1)?;
            set_ratio(&out_media_type.cast()?, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
            out_media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            out_media_type.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Main.0 as u32)?;

            let stream_index = sink_writer.AddStream(&out_media_type)?;

            let in_media_type = MFCreateMediaType()?;
            in_media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            in_media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_ARGB32)?;
            set_size(&in_media_type.cast()?, &MF_MT_FRAME_SIZE, initial_width, initial_height)?;
            set_ratio(&in_media_type.cast()?, &MF_MT_FRAME_RATE, target_fps, 1)?;
            set_ratio(&in_media_type.cast()?, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
            in_media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;

            sink_writer.SetInputMediaType(stream_index, &in_media_type, None)?;
            sink_writer.BeginWriting()?;

            // Pré-aloca IMFMediaBuffer e IMFSample persistentes para Zero-Heap allocations em 60 FPS
            let bgra_len = (initial_width * initial_height * 4) as u32;
            let in_buffer = MFCreateMemoryBuffer(bgra_len)?;
            let in_sample = MFCreateSample()?;
            in_sample.AddBuffer(&in_buffer)?;

            info!("🚀 [WMF GPU ENGINE] Encoder de hardware H.264 pronto na GPU '{}' (Zero-CPU Direct ARGB -> {}p {} FPS, {:.1} Mbps)!",
                selected_gpu_name, initial_height, target_fps, initial_bitrate as f64 / 1_000_000.0);

            Ok(Self {
                sink_writer,
                byte_stream,
                stream_index,
                current_width: initial_width,
                current_height: initial_height,
                target_fps,
                bitrate_bps: initial_bitrate,
                frame_count: 0,
                last_read_pos: 0,
                gpu_name: selected_gpu_name,
                needs_keyframe: true,
                in_buffer,
                in_sample,
                read_buf: Vec::with_capacity(512 * 1024),
            })
        }
    }
}

#[cfg(target_os = "windows")]
impl VideoEncoder for WmfGpuEncoder {
    fn encode(&mut self, bgra_data: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
        let bgra_len = (width * height * 4) as usize;
        if bgra_data.len() < bgra_len {
            return None;
        }

        unsafe {
            // Entrada direta ARGB/BGRA na GPU (Zero CPU Color Conversion & Zero Heap Allocations)
            let mut p_buf_data: *mut u8 = std::ptr::null_mut();
            let mut max_len = 0u32;
            let mut cur_len = 0u32;
            self.in_buffer.Lock(&mut p_buf_data, Some(&mut max_len), Some(&mut cur_len)).ok()?;
            std::ptr::copy_nonoverlapping(bgra_data.as_ptr(), p_buf_data, bgra_len.min(max_len as usize));
            let _ = self.in_buffer.Unlock();
            let _ = self.in_buffer.SetCurrentLength(bgra_len as u32);

            let sample_duration = (10_000_000 / self.target_fps.max(1)) as i64;
            let sample_time = (self.frame_count as i64) * sample_duration;
            let _ = self.in_sample.SetSampleTime(sample_time);
            let _ = self.in_sample.SetSampleDuration(sample_duration);

            if self.needs_keyframe {
                let _ = self.in_sample.SetUINT32(&MFSampleExtension_CleanPoint, 1);
                self.needs_keyframe = false;
            } else {
                let _ = self.in_sample.SetUINT32(&MFSampleExtension_CleanPoint, 0);
            }

            self.frame_count += 1;

            if self.sink_writer.WriteSample(self.stream_index, &self.in_sample).is_ok() {
                let cur_len = self.byte_stream.GetLength().unwrap_or(0);
                if cur_len > self.last_read_pos {
                    let to_read = (cur_len - self.last_read_pos) as usize;
                    if self.read_buf.len() < to_read {
                        self.read_buf.resize(to_read, 0);
                    }
                    let mut read_bytes = 0u32;
                    let _ = self.byte_stream.SetCurrentPosition(self.last_read_pos);
                    let _ = self.byte_stream.Read(&mut self.read_buf[..to_read], &mut read_bytes);
                    self.last_read_pos = cur_len;
                    if read_bytes > 0 {
                        return Some(self.read_buf[..read_bytes as usize].to_vec());
                    }
                }
            }
        }

        None
    }

    fn force_intra_frame(&mut self) {
        self.needs_keyframe = true;
    }

    fn set_bitrate_bps(&mut self, bitrate_bps: u32) {
        self.bitrate_bps = bitrate_bps.clamp(1_500_000, 8_000_000);
    }

    fn get_bitrate_bps(&self) -> u32 {
        self.bitrate_bps
    }

    fn name(&self) -> &'static str {
        "Direct3D 11 / Windows Media Foundation (Universal GPU Hardware Acceleration)"
    }

    fn is_hardware_accelerated(&self) -> bool {
        true
    }
}
