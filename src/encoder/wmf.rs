#![allow(non_snake_case, non_camel_case_types)]

//! H.264 encoding with a GPU's Media Foundation encoder (Intel Quick Sync, NVIDIA, AMD),
//! used directly as an asynchronous transform: raw H.264 with SPS/PPS before every
//! keyframe, keyframes on request, no B-frames, low latency.
//!
//! (It used to go through a sink writer into an MP4 file and send the new bytes; an MP4
//! only gets its header, where SPS/PPS live, when the file is finished, so receivers could
//! never decode it.)

#[cfg(target_os = "windows")]
use super::VideoEncoder;
#[cfg(target_os = "windows")]
use log::{info, warn};
#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};
#[cfg(target_os = "windows")]
use windows::core::*;
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct3D::*;
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct3D11::*;
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Dxgi::Common::*;
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Dxgi::*;
#[cfg(target_os = "windows")]
use windows::Win32::Media::MediaFoundation::*;
#[cfg(target_os = "windows")]
use windows::Win32::System::Com::*;

/// Input surfaces in flight: the encoder may still read one while we fill the next.
#[cfg(target_os = "windows")]
const RING: usize = 4;
/// How long encode() waits for the picture it just sent before returning (it is then
/// returned with the next call).
#[cfg(target_os = "windows")]
const OUTPUT_WAIT: Duration = Duration::from_millis(12);

#[cfg(target_os = "windows")]
pub struct WmfGpuEncoder {
    mft: IMFTransform,
    events: IMFMediaEventGenerator,
    codec: ICodecAPI,
    context: ID3D11DeviceContext,
    device: ID3D11Device,
    _manager: IMFDXGIDeviceManager,
    ring: Vec<ID3D11Texture2D>,
    ring_pos: usize,
    nv12: Vec<u8>,
    need_input: u32,
    in_flight: u32,
    pub current_width: u32,
    pub current_height: u32,
    pub target_fps: u32,
    pub bitrate_bps: u32,
    pub frame_count: u64,
    pub gpu_name: String,
    pub needs_keyframe: bool,
    /// (primaries, transfer, matrix) written into the SPS: the encoder leaves them out.
    colour: (u8, u8, u8),
    /// Finished pictures, one access unit each, handed out one per call: a picture that
    /// comes late must not be glued to the next one (receivers would drop one of them).
    ready: std::collections::VecDeque<Vec<u8>>,
}

#[cfg(target_os = "windows")]
unsafe impl Send for WmfGpuEncoder {}

#[cfg(target_os = "windows")]
impl WmfGpuEncoder {
    pub fn try_new(target_fps: u32, is_screen_content: bool) -> Result<Self> {
        let bitrate = if is_screen_content { 4_000_000 } else { 3_500_000 };
        Self::with_size(1920, 1080, target_fps, bitrate)
    }

    fn with_size(width: u32, height: u32, target_fps: u32, bitrate_bps: u32) -> Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_LITE)?;

            // Hardware H.264 encoders, best first; each must run on its own GPU's device.
            let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
            let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
            let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER, Some(&input), Some(&output), &mut list, &mut count)?;
            let activates: Vec<IMFActivate> = std::slice::from_raw_parts(list, count as usize).iter().flatten().cloned().collect();
            CoTaskMemFree(Some(list as *const _));

            let mut last_err = Error::from(windows::Win32::Foundation::E_FAIL);
            for act in &activates {
                let name = string_attr(act, &MFT_FRIENDLY_NAME_Attribute);
                let vendor = string_attr(act, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute);
                match Self::open(act, &vendor, width, height, target_fps, bitrate_bps) {
                    Ok(mut enc) => {
                        enc.gpu_name = name.clone();
                        info!("🚀 [WMF GPU ENGINE] Encoder de hardware H.264 pronto: '{}' ({}x{} {} FPS, {:.1} Mbps)", name, width, height, target_fps, bitrate_bps as f64 / 1e6);
                        return Ok(enc);
                    }
                    Err(e) => {
                        warn!("⚠️ [WMF GPU ENGINE] '{}' indisponível: {}", name, e);
                        last_err = e;
                    }
                }
            }
            Err(last_err)
        }
    }

    unsafe fn open(act: &IMFActivate, vendor: &str, width: u32, height: u32, target_fps: u32, bitrate_bps: u32) -> Result<Self> {
        let adapter = adapter_of_vendor(vendor)?;
        let mut device = None;
        D3D11CreateDevice(&adapter, D3D_DRIVER_TYPE_UNKNOWN, None, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, None)?;
        let device: ID3D11Device = device.ok_or_else(|| Error::from(windows::Win32::Foundation::E_FAIL))?;
        // encode() converts on the CPU with BT.601 coefficients.
        Self::open_on(act, device, false, width, height, target_fps, bitrate_bps)
    }

    /// An encoder for textures of `device` (the capture device), which already hold NV12
    /// in BT.709 limited range (gpu_tx): `encode_texture` only copies them on the GPU.
    pub fn for_device(device: &ID3D11Device, vendor_id: u32, width: u32, height: u32, target_fps: u32, bitrate_bps: u32) -> Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_LITE)?;
            let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
            let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
            let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER, Some(&input), Some(&output), &mut list, &mut count)?;
            let activates: Vec<IMFActivate> = std::slice::from_raw_parts(list, count as usize).iter().flatten().cloned().collect();
            CoTaskMemFree(Some(list as *const _));
            // Only the encoder of the GPU the textures are on.
            let wanted = format!("VEN_{:04X}", vendor_id);
            let mut last_err = Error::from(windows::Win32::Foundation::E_FAIL);
            for act in activates.iter().filter(|a| string_attr(a, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute).eq_ignore_ascii_case(&wanted)) {
                let name = string_attr(act, &MFT_FRIENDLY_NAME_Attribute);
                match Self::open_on(act, device.clone(), true, width, height, target_fps, bitrate_bps) {
                    Ok(mut enc) => {
                        enc.gpu_name = name;
                        return Ok(enc);
                    }
                    Err(e) => {
                        warn!("⚠️ [WMF GPU ENGINE] '{}' indisponível para texturas: {}", name, e);
                        last_err = e;
                    }
                }
            }
            Err(last_err)
        }
    }

    unsafe fn open_on(act: &IMFActivate, device: ID3D11Device, bt709: bool, width: u32, height: u32, target_fps: u32, bitrate_bps: u32) -> Result<Self> {
        let context = device.GetImmediateContext()?;
        let _ = context.cast::<ID3D11Multithread>()?.SetMultithreadProtected(true);
        let mut token = 0u32;
        let mut manager = None;
        MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
        let manager = manager.ok_or_else(|| Error::from(windows::Win32::Foundation::E_FAIL))?;
        manager.ResetDevice(&device, token)?;

        let mft: IMFTransform = act.ActivateObject()?;
        let attrs = mft.GetAttributes()?;
        attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
        let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
        mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)?;

        let codec: ICodecAPI = mft.cast()?;
        // Some encoders lack some of these; the rest still applies.
        let gop = (target_fps * 2).max(1);
        for (api, value) in [
            (CODECAPI_AVEncCommonRateControlMode, eAVEncCommonRateControlMode_CBR.0 as u32),
            (CODECAPI_AVEncCommonMeanBitRate, bitrate_bps),
            (CODECAPI_AVEncMPVDefaultBPictureCount, 0),
            (CODECAPI_AVEncMPVGOPSize, gop),
            (CODECAPI_AVLowLatencyMode, 1),
        ] {
            let _ = codec.SetValue(&api, &VARIANT::from(value));
        }

        // Output first (encoders want it before the input), then NV12 input.
        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate_bps)?;
        out.SetUINT64(&MF_MT_FRAME_SIZE, (width as u64) << 32 | height as u64)?;
        out.SetUINT64(&MF_MT_FRAME_RATE, (target_fps as u64) << 32 | 1)?;
        out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        out.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
        // What the pictures hold, limited range: BT.601 from encode(), BT.709 from gpu_tx.
        let matrix = if bt709 { MFVideoTransferMatrix_BT709 } else { MFVideoTransferMatrix_BT601 };
        let _ = out.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32);
        let _ = out.SetUINT32(&MF_MT_YUV_MATRIX, matrix.0 as u32);
        if bt709 {
            let _ = out.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32);
            let _ = out.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32);
        }
        mft.SetOutputType(0, &out, 0)?;
        let mut i = 0;
        let input = loop {
            let t = mft.GetInputAvailableType(0, i)?;
            if t.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
                break t;
            }
            i += 1;
        };
        input.SetUINT64(&MF_MT_FRAME_SIZE, (width as u64) << 32 | height as u64)?;
        input.SetUINT64(&MF_MT_FRAME_RATE, (target_fps as u64) << 32 | 1)?;
        let _ = input.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32);
        let _ = input.SetUINT32(&MF_MT_YUV_MATRIX, matrix.0 as u32);
        if bt709 {
            let _ = input.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32);
            let _ = input.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32);
        }
        mft.SetInputType(0, &input, 0)?;
        mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
        mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        let events: IMFMediaEventGenerator = mft.cast()?;

        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut ring = Vec::with_capacity(RING);
        for _ in 0..RING {
            let mut t = None;
            device.CreateTexture2D(&desc, None, Some(&mut t))?;
            ring.push(t.ok_or_else(|| Error::from(windows::Win32::Foundation::E_FAIL))?);
        }
        Ok(Self {
            mft,
            events,
            codec,
            context,
            device,
            _manager: manager,
            ring,
            ring_pos: 0,
            nv12: vec![0; (width * height * 3 / 2) as usize],
            need_input: 0,
            in_flight: 0,
            current_width: width,
            current_height: height,
            target_fps,
            bitrate_bps,
            frame_count: 0,
            gpu_name: String::new(),
            needs_keyframe: true,
            colour: if bt709 { (1, 1, 1) } else { (6, 6, 6) },
            ready: std::collections::VecDeque::new(),
        })
    }

    /// Handles whatever the encoder has signalled: input slots and finished pictures.
    unsafe fn drain_events(&mut self) {
        while let Ok(ev) = self.events.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
            let Ok(kind) = ev.GetType() else { continue };
            if kind == METransformNeedInput.0 as u32 {
                self.need_input += 1;
            } else if kind == METransformHaveOutput.0 as u32 {
                self.take_output();
            }
        }
    }

    unsafe fn take_output(&mut self) {
        let mut buf = [MFT_OUTPUT_DATA_BUFFER::default()];
        let mut status = 0u32;
        let mut r = self.mft.ProcessOutput(0, &mut buf, &mut status);
        if matches!(&r, Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE) {
            // The encoder completed its output type (sequence header): take it, retry.
            if let Ok(t) = self.mft.GetOutputAvailableType(0, 0) {
                let _ = self.mft.SetOutputType(0, &t, 0);
            }
            buf = [MFT_OUTPUT_DATA_BUFFER::default()];
            r = self.mft.ProcessOutput(0, &mut buf, &mut status);
        }
        let sample = std::mem::ManuallyDrop::take(&mut buf[0].pSample);
        let _ = std::mem::ManuallyDrop::take(&mut buf[0].pEvents);
        if r.is_err() {
            return;
        }
        let Some(sample) = sample else { return };
        let Ok(mb) = sample.ConvertToContiguousBuffer() else { return };
        let mut p = std::ptr::null_mut();
        let mut len = 0u32;
        if mb.Lock(&mut p, None, Some(&mut len)).is_ok() {
            self.ready.push_back(std::slice::from_raw_parts(p, len as usize).to_vec());
            let _ = mb.Unlock();
            self.in_flight = self.in_flight.saturating_sub(1);
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for WmfGpuEncoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            if let Ok(s) = self.mft.cast::<IMFShutdown>() {
                let _ = s.Shutdown();
            }
        }
    }
}

#[cfg(target_os = "windows")]
impl VideoEncoder for WmfGpuEncoder {
    fn encode(&mut self, bgra_data: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
        let (w, h) = (width & !1, height & !1);
        if w == 0 || h == 0 || bgra_data.len() < (width * height * 4) as usize {
            return None;
        }
        if (w, h) != (self.current_width, self.current_height) {
            // New size: start a new encoder (its first picture is a keyframe).
            match Self::with_size(w, h, self.target_fps, self.bitrate_bps) {
                Ok(mut enc) => {
                    enc.gpu_name = std::mem::take(&mut self.gpu_name);
                    *self = enc;
                }
                Err(e) => {
                    warn!("⚠️ [WMF GPU ENGINE] Não foi possível mudar para {}x{}: {}", w, h, e);
                    return None;
                }
            }
        }
        unsafe {
            self.drain_events();
            if self.need_input == 0 {
                // Still busy with earlier pictures: skip this one.
                return self.next_output();
            }
            bgra_to_nv12(bgra_data, width as usize, &mut self.nv12, w as usize, h as usize);
            let tex = self.ring[self.ring_pos].clone();
            self.ring_pos = (self.ring_pos + 1) % RING;
            self.context.UpdateSubresource(&tex, 0, None, self.nv12.as_ptr() as *const _, w, 0);
            self.submit(&tex);
        }
        self.next_output()
    }

    fn force_intra_frame(&mut self) {
        self.needs_keyframe = true;
    }

    fn set_bitrate_bps(&mut self, bitrate_bps: u32) {
        self.bitrate_bps = bitrate_bps.clamp(1_500_000, 8_000_000);
        unsafe {
            let _ = self.codec.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &VARIANT::from(self.bitrate_bps));
        }
    }

    fn get_bitrate_bps(&self) -> u32 {
        self.bitrate_bps
    }

    fn name(&self) -> &'static str {
        "Media Foundation GPU H.264 (Intel Quick Sync / NVIDIA / AMD)"
    }

    fn is_hardware_accelerated(&self) -> bool {
        true
    }
}

#[cfg(target_os = "windows")]
impl WmfGpuEncoder {
    /// Encodes an NV12 texture of the encoder device and size: a GPU copy into the
    /// encoder textures (it reads them asynchronously), nothing through the CPU.
    pub fn encode_texture(&mut self, texture: &ID3D11Texture2D) -> std::result::Result<Vec<u8>, String> {
        unsafe {
            self.drain_events();
            if self.need_input == 0 {
                return Ok(self.next_output().unwrap_or_default());
            }
            let tex = self.ring[self.ring_pos].clone();
            self.ring_pos = (self.ring_pos + 1) % RING;
            self.context.CopyResource(&tex, texture);
            if !self.submit(&tex) {
                return Err("ProcessInput failed".into());
            }
        }
        Ok(self.next_output().unwrap_or_default())
    }

    /// The oldest finished picture, with its colours declared in the SPS (Quick Sync
    /// writes only the range).
    fn next_output(&mut self) -> Option<Vec<u8>> {
        let au = self.ready.pop_front()?;
        let (p, t, m) = self.colour;
        Some(super::h264_vui::declare_colour(&au, p, t, m))
    }

    /// Hands `tex` to the encoder and waits briefly for the picture.
    unsafe fn submit(&mut self, tex: &ID3D11Texture2D) -> bool {
        let Ok(buffer) = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, tex, 0, false) else { return false };
        let Ok(sample) = MFCreateSample() else { return false };
        let _ = sample.AddBuffer(&buffer);
        let duration = 10_000_000 / self.target_fps.max(1) as i64;
        let _ = sample.SetSampleTime(self.frame_count as i64 * duration);
        let _ = sample.SetSampleDuration(duration);
        if self.needs_keyframe {
            let _ = self.codec.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &VARIANT::from(1u32));
            self.needs_keyframe = false;
        }
        if self.mft.ProcessInput(0, &sample, 0).is_err() {
            return false;
        }
        self.need_input -= 1;
        self.in_flight += 1;
        self.frame_count += 1;
        // Usually the picture is ready within a few milliseconds.
        let start = Instant::now();
        while self.in_flight > 0 && start.elapsed() < OUTPUT_WAIT {
            self.drain_events();
            if self.in_flight > 0 {
                std::thread::sleep(Duration::from_micros(500));
            }
        }
        true
    }
}

#[cfg(target_os = "windows")]
unsafe fn string_attr(act: &IMFActivate, key: &GUID) -> String {
    let mut s = PWSTR::null();
    let mut len = 0u32;
    if act.GetAllocatedString(key, &mut s, &mut len).is_err() {
        return String::new();
    }
    let out = s.to_string().unwrap_or_default();
    CoTaskMemFree(Some(s.0 as *const _));
    out
}

/// The DXGI adapter of a hardware vendor ("VEN_8086"), for the encoder's device.
#[cfg(target_os = "windows")]
unsafe fn adapter_of_vendor(vendor: &str) -> Result<IDXGIAdapter1> {
    let id = vendor.trim_start_matches("VEN_").trim_start_matches("ven_");
    let id = u32::from_str_radix(id, 16).map_err(|_| Error::from(windows::Win32::Foundation::E_INVALIDARG))?;
    let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
    let mut i = 0;
    while let Ok(a) = factory.EnumAdapters1(i) {
        if a.GetDesc1()?.VendorId == id {
            return Ok(a);
        }
        i += 1;
    }
    Err(Error::from(windows::Win32::Foundation::E_FAIL))
}

/// BGRA (stride `src_w` pixels) to NV12 `w` x `h`, BT.601 limited range, like the other
/// encoders.
#[cfg(target_os = "windows")]
fn bgra_to_nv12(bgra: &[u8], src_w: usize, nv12: &mut [u8], w: usize, h: usize) {
    use rayon::prelude::*;
    let (y_plane, uv_plane) = nv12.split_at_mut(w * h);
    y_plane.par_chunks_mut(2 * w).zip(uv_plane.par_chunks_mut(w)).enumerate().for_each(|(pair, (ys, uv))| {
        let j = pair * 2;
        let (y0, y1) = ys.split_at_mut(w);
        let row0 = &bgra[j * src_w * 4..];
        let row1 = &bgra[(j + 1) * src_w * 4..];
        for i in (0..w).step_by(2) {
            let px = |row: &[u8], x: usize| (row[x * 4 + 2] as i32, row[x * 4 + 1] as i32, row[x * 4] as i32);
            let (r0, g0, b0) = px(row0, i);
            let (r1, g1, b1) = px(row0, i + 1);
            let (r2, g2, b2) = px(row1, i);
            let (r3, g3, b3) = px(row1, i + 1);
            let luma = |r: i32, g: i32, b: i32| (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;
            y0[i] = luma(r0, g0, b0);
            y0[i + 1] = luma(r1, g1, b1);
            y1[i] = luma(r2, g2, b2);
            y1[i + 1] = luma(r3, g3, b3);
            let (r, g, b) = ((r0 + r1 + r2 + r3) >> 2, (g0 + g1 + g2 + g3) >> 2, (b0 + b1 + b2 + b3) >> 2);
            uv[i] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
            uv[i + 1] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
        }
    });
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn nv12_conversion_matches_bt601_limited() {
        // 4x2 of white then black then red (BT.601 limited: Y 235/16/81, red U=90 V=240).
        let (w, h) = (4usize, 2usize);
        let mut bgra = Vec::new();
        for _ in 0..h {
            bgra.extend_from_slice(&[255, 255, 255, 255, 255, 255, 255, 255]);
            bgra.extend_from_slice(&[0, 0, 255, 255, 0, 0, 255, 255]);
        }
        let mut nv12 = vec![0u8; w * h * 3 / 2];
        bgra_to_nv12(&bgra, w, &mut nv12, w, h);
        assert_eq!(&nv12[..4], &[235, 235, 82, 82]);
        let uv = &nv12[w * h..];
        assert_eq!((uv[0], uv[1]), (128, 128), "white chroma");
        assert!((uv[2] as i32 - 90).abs() <= 1 && (uv[3] as i32 - 240).abs() <= 1, "red chroma {:?}", &uv[2..4]);
    }
}
