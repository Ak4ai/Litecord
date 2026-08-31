// Teste: Hardware Encoder Direct3D 11 com entrada ARGB/BGRA direta (Zero-CPU Color Conversion)

use std::time::Instant;
use windows::core::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;

fn main() -> Result<()> {
    println!("============================================================");
    println!("🚀 TESTE: DIRECT GPU COLOR CONVERSION (ARGB -> H.264 NA GPU)");
    println!("============================================================");

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)?;

        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut selected_adapter: Option<IDXGIAdapter1> = None;
        let mut selected_gpu_name = String::new();
        let mut best_vram: usize = 0;

        let mut adapter_index = 0u32;
        while let Ok(adapter) = factory.EnumAdapters1(adapter_index) {
            let desc = adapter.GetDesc1()?;
            let name = String::from_utf16_lossy(&desc.Description).trim_matches('\0').trim().to_string();
            let vram_mb = desc.DedicatedVideoMemory / (1024 * 1024);
            let is_software = (desc.Flags & 2) != 0;

            if !is_software && (desc.VendorId == 0x10DE || desc.VendorId == 0x1002 || vram_mb > best_vram) {
                selected_adapter = Some(adapter);
                selected_gpu_name = name;
                best_vram = vram_mb;
            }
            adapter_index += 1;
        }

        println!("🎯 GPU: '{}' ({} MB VRAM)", selected_gpu_name, best_vram);

        let mut d3d11_device: Option<ID3D11Device> = None;
        let mut d3d11_context: Option<ID3D11DeviceContext> = None;
        let mut feature_level = D3D_FEATURE_LEVEL_11_0;

        D3D11CreateDevice(
            selected_adapter.as_ref().map(|a| a as &IDXGIAdapter),
            D3D_DRIVER_TYPE_UNKNOWN,
            None,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut d3d11_device),
            Some(&mut feature_level),
            Some(&mut d3d11_context),
        )?;

        let d3d11_device = d3d11_device.unwrap();
        let mut reset_token = 0u32;
        let mut device_manager: Option<IMFDXGIDeviceManager> = None;
        MFCreateDXGIDeviceManager(&mut reset_token, &mut device_manager)?;
        let device_manager = device_manager.unwrap();
        device_manager.ResetDevice(&d3d11_device, reset_token)?;

        let mut writer_attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut writer_attributes, 4)?;
        let writer_attributes = writer_attributes.unwrap();

        writer_attributes.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &device_manager)?;
        writer_attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        writer_attributes.SetUINT32(&MF_LOW_LATENCY, 1)?;

        let byte_stream: IMFByteStream = MFCreateTempFile(MF_ACCESSMODE_READWRITE, MF_OPENMODE_DELETE_IF_EXIST, MF_FILEFLAGS_NONE)?;
        let sink_writer = MFCreateSinkWriterFromURL(w!(".mp4"), &byte_stream, &writer_attributes)?;

        fn set_size(attrs: &IMFAttributes, key: &windows::core::GUID, w: u32, h: u32) -> Result<()> {
            unsafe { attrs.SetUINT64(key, ((w as u64) << 32) | (h as u64)) }
        }
        fn set_ratio(attrs: &IMFAttributes, key: &windows::core::GUID, num: u32, den: u32) -> Result<()> {
            unsafe { attrs.SetUINT64(key, ((num as u64) << 32) | (den as u64)) }
        }

        // Saída H.264
        let out_media_type = MFCreateMediaType()?;
        out_media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out_media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        out_media_type.SetUINT32(&MF_MT_AVG_BITRATE, 4_000_000)?;
        set_size(&out_media_type.cast()?, &MF_MT_FRAME_SIZE, 1920, 1080)?;
        set_ratio(&out_media_type.cast()?, &MF_MT_FRAME_RATE, 60, 1)?;
        set_ratio(&out_media_type.cast()?, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
        out_media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        out_media_type.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Main.0 as u32)?;

        let stream_index = sink_writer.AddStream(&out_media_type)?;

        // Testar Entrada ARGB32 direta (Zero-CPU Color Conversion)
        let in_media_type = MFCreateMediaType()?;
        in_media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        in_media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_ARGB32)?;
        set_size(&in_media_type.cast()?, &MF_MT_FRAME_SIZE, 1920, 1080)?;
        set_ratio(&in_media_type.cast()?, &MF_MT_FRAME_RATE, 60, 1)?;
        set_ratio(&in_media_type.cast()?, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
        in_media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;

        let hr_in = sink_writer.SetInputMediaType(stream_index, &in_media_type, None);
        println!("👉 SetInputMediaType (MFVideoFormat_ARGB32): {:?}", hr_in);

        if hr_in.is_err() {
            // Tentar RGB32
            let in_media_type_rgb32 = MFCreateMediaType()?;
            in_media_type_rgb32.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            in_media_type_rgb32.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            set_size(&in_media_type_rgb32.cast()?, &MF_MT_FRAME_SIZE, 1920, 1080)?;
            set_ratio(&in_media_type_rgb32.cast()?, &MF_MT_FRAME_RATE, 60, 1)?;
            set_ratio(&in_media_type_rgb32.cast()?, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
            in_media_type_rgb32.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            let hr_rgb32 = sink_writer.SetInputMediaType(stream_index, &in_media_type_rgb32, None);
            println!("👉 SetInputMediaType (MFVideoFormat_RGB32): {:?}", hr_rgb32);
        }

        sink_writer.BeginWriting()?;
        println!("🚀 IMFSinkWriter pronto!");

        let bgra_frame_size = 1920 * 1080 * 4;
        let dummy_bgra = vec![128u8; bgra_frame_size];

        let in_buffer = MFCreateMemoryBuffer(bgra_frame_size as u32)?;
        let mut p_buf_data: *mut u8 = std::ptr::null_mut();
        let mut max_len = 0u32;
        let mut cur_len = 0u32;
        in_buffer.Lock(&mut p_buf_data, Some(&mut max_len), Some(&mut cur_len))?;
        std::ptr::copy_nonoverlapping(dummy_bgra.as_ptr(), p_buf_data, bgra_frame_size);
        in_buffer.Unlock()?;
        in_buffer.SetCurrentLength(bgra_frame_size as u32)?;

        let in_sample = MFCreateSample()?;
        in_sample.AddBuffer(&in_buffer)?;
        in_sample.SetSampleTime(0)?;
        in_sample.SetSampleDuration(166_666)?;

        let t0 = Instant::now();
        let hr_write = sink_writer.WriteSample(stream_index, &in_sample);
        let el = t0.elapsed();

        println!("✅ WriteSample ARGB direto na GPU: {:?} | Tempo: {:?}", hr_write, el);
    }
    Ok(())
}
