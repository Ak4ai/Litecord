//! NVENC through the driver's own API (nvEncodeAPI64.dll), fed with NV12 textures that
//! never leave the GPU. No FFmpeg: the structs are built from the layout the official
//! header gives (`nvenc_api.rs`), and only the fields we set are written.
//!
//! The encoder runs on the D3D11 device the textures belong to (the capture device), so
//! nothing is copied between devices. Encoding is synchronous: `encode` returns the
//! frame's Annex B bytes, with SPS/PPS before every IDR.

use std::collections::HashMap;
use std::ffi::c_void;

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};

use super::nvenc_api as api;

type Status = i32;
type FnCreateInstance = unsafe extern "system" fn(*mut u8) -> Status;
type FnMaxVersion = unsafe extern "system" fn(*mut u32) -> Status;
type FnOpenSessionEx = unsafe extern "system" fn(*mut u8, *mut *mut c_void) -> Status;
type FnPresetConfigEx = unsafe extern "system" fn(*mut c_void, Guid, Guid, u32, *mut u8) -> Status;
type FnWithParams = unsafe extern "system" fn(*mut c_void, *mut u8) -> Status;
type FnWithHandle = unsafe extern "system" fn(*mut c_void, *mut c_void) -> Status;
type FnDestroy = unsafe extern "system" fn(*mut c_void) -> Status;
type FnLastError = unsafe extern "system" fn(*mut c_void) -> *const std::ffi::c_char;

#[repr(C)]
#[derive(Clone, Copy)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

impl Guid {
    fn from_bytes(b: [u8; 16]) -> Self {
        Self {
            data1: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            data2: u16::from_le_bytes([b[4], b[5]]),
            data3: u16::from_le_bytes([b[6], b[7]]),
            data4: [b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]],
        }
    }
}

/// A zeroed, 8-byte aligned C struct of `size` bytes with its version word set.
struct Blob(Vec<u64>);

impl Blob {
    fn new(size: usize, version: u32) -> Self {
        let mut b = Blob(vec![0u64; size.div_ceil(8)]);
        b.u32(0, version);
        b
    }
    fn ptr(&mut self) -> *mut u8 {
        self.0.as_mut_ptr() as *mut u8
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        let len = self.0.len() * 8;
        unsafe { std::slice::from_raw_parts_mut(self.ptr(), len) }
    }
    fn bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.0.as_ptr() as *const u8, self.0.len() * 8) }
    }
    fn u32(&mut self, off: usize, v: u32) {
        self.bytes_mut()[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, off: usize, v: u64) {
        self.bytes_mut()[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }
    fn ptr_at(&mut self, off: usize, p: *mut c_void) {
        self.u64(off, p as u64);
    }
    fn guid(&mut self, off: usize, g: [u8; 16]) {
        self.bytes_mut()[off..off + 16].copy_from_slice(&g);
    }
    fn bit(&mut self, (off, bit): (usize, u32), on: bool) {
        let b = &mut self.bytes_mut()[off];
        if on { *b |= 1 << bit } else { *b &= !(1 << bit) }
    }
    fn get_u32(&self, off: usize) -> u32 {
        u32::from_le_bytes(self.bytes()[off..off + 4].try_into().unwrap())
    }
    fn get_ptr(&self, off: usize) -> *mut c_void {
        u64::from_le_bytes(self.bytes()[off..off + 8].try_into().unwrap()) as *mut c_void
    }
}

/// The function table the driver fills in.
struct Functions(Blob);

impl Functions {
    fn get<T: Copy>(&self, off: usize) -> Option<T> {
        let p = self.0.get_ptr(off);
        (!p.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
    }
}

pub struct NvencD3d11Encoder {
    _device: ID3D11Device,
    f: Functions,
    encoder: *mut c_void,
    bitstream: *mut c_void,
    /// Textures registered with the encoder, by COM pointer.
    registered: HashMap<usize, *mut c_void>,
    init: Blob,
    config: Blob,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: u32,
    force_idr: bool,
    frame_idx: u32,
}

// The encoder is driven from one thread at a time (the stream's send thread).
unsafe impl Send for NvencD3d11Encoder {}

fn check(s: Status, what: &str) -> Result<(), String> {
    if s == 0 { Ok(()) } else { Err(format!("{what}: NVENCSTATUS {s}")) }
}

impl NvencD3d11Encoder {
    /// Opens an encoder on `device` (it must be an NVIDIA device) for NV12 frames of this size.
    pub fn new(device: &ID3D11Device, width: u32, height: u32, fps: u32, bitrate: u32) -> Result<Self, String> {
        unsafe {
            let dll = windows::Win32::System::LibraryLoader::LoadLibraryA(windows::core::s!("nvEncodeAPI64.dll"))
                .map_err(|e| format!("nvEncodeAPI64.dll: {e}"))?;
            let proc = |name: windows::core::PCSTR| windows::Win32::System::LibraryLoader::GetProcAddress(dll, name);
            let max_version: FnMaxVersion = std::mem::transmute(proc(windows::core::s!("NvEncodeAPIGetMaxSupportedVersion")).ok_or("no NvEncodeAPIGetMaxSupportedVersion")?);
            let create: FnCreateInstance = std::mem::transmute(proc(windows::core::s!("NvEncodeAPICreateInstance")).ok_or("no NvEncodeAPICreateInstance")?);

            // The driver reports (major << 4) | minor; this code speaks 12.1.
            let mut v = 0u32;
            check(max_version(&mut v), "NvEncodeAPIGetMaxSupportedVersion")?;
            if v < (12 << 4 | 1) {
                return Err(format!("driver supports NVENC API {}.{}, needs 12.1", v >> 4, v & 0xf));
            }

            let mut f = Functions(Blob::new(api::SIZE_NV_ENCODE_API_FUNCTION_LIST, api::FUNCTION_LIST_VER));
            check(create(f.0.ptr()), "NvEncodeAPICreateInstance")?;

            let mut open = Blob::new(api::SIZE_NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS, api::OPEN_SESSION_EX_VER);
            open.u32(api::OPEN_DEVICE_TYPE, api::DEVICE_TYPE_DIRECTX);
            open.ptr_at(api::OPEN_DEVICE, device.as_raw());
            open.u32(api::OPEN_API_VERSION, api::NVENCAPI_VERSION);
            let open_fn: FnOpenSessionEx = f.get(api::FN_OPEN_SESSION_EX).ok_or("no nvEncOpenEncodeSessionEx")?;
            let mut encoder = std::ptr::null_mut();
            check(open_fn(open.ptr(), &mut encoder), "nvEncOpenEncodeSessionEx")?;

            let mut me = Self {
                _device: device.clone(),
                f,
                encoder,
                bitstream: std::ptr::null_mut(),
                registered: HashMap::new(),
                init: Blob::new(api::SIZE_NV_ENC_INITIALIZE_PARAMS, api::INITIALIZE_PARAMS_VER),
                config: Blob::new(api::SIZE_NV_ENC_CONFIG, api::CONFIG_VER),
                width,
                height,
                fps: fps.max(1),
                bitrate,
                force_idr: true,
                frame_idx: 0,
            };
            // From here on Drop releases what was created.
            me.initialize().map_err(|e| format!("{e} ({})", me.last_error()))?;
            Ok(me)
        }
    }

    unsafe fn initialize(&mut self) -> Result<(), String> {
        // Start from the driver's low-latency preset, then pin what the stream needs.
        let mut preset = Blob::new(api::SIZE_NV_ENC_PRESET_CONFIG, api::PRESET_CONFIG_VER);
        preset.u32(api::PRESET_CFG, api::CONFIG_VER);
        preset.u32(api::PRESET_CFG + api::CFG_RC + api::RC_VERSION, api::RC_PARAMS_VER);
        let preset_fn: FnPresetConfigEx = self.f.get(api::FN_GET_PRESET_CONFIG_EX).ok_or("no nvEncGetEncodePresetConfigEx")?;
        check(
            preset_fn(self.encoder, Guid::from_bytes(api::CODEC_H264_GUID), Guid::from_bytes(api::PRESET_P1_GUID), api::TUNING_ULTRA_LOW_LATENCY, preset.ptr()),
            "nvEncGetEncodePresetConfigEx",
        )?;
        let cfg_bytes = preset.bytes()[api::PRESET_CFG..api::PRESET_CFG + api::SIZE_NV_ENC_CONFIG].to_vec();
        self.config.bytes_mut()[..api::SIZE_NV_ENC_CONFIG].copy_from_slice(&cfg_bytes);
        self.config.u32(0, api::CONFIG_VER);
        self.write_config();

        let init = &mut self.init;
        init.guid(api::INIT_ENCODE_GUID, api::CODEC_H264_GUID);
        init.guid(api::INIT_PRESET_GUID, api::PRESET_P1_GUID);
        init.u32(api::INIT_WIDTH, self.width);
        init.u32(api::INIT_HEIGHT, self.height);
        init.u32(api::INIT_DAR_WIDTH, self.width);
        init.u32(api::INIT_DAR_HEIGHT, self.height);
        init.u32(api::INIT_FRAME_RATE_NUM, self.fps);
        init.u32(api::INIT_FRAME_RATE_DEN, 1);
        init.u32(api::INIT_ENCODE_ASYNC, 0);
        init.u32(api::INIT_ENABLE_PTD, 1);
        init.ptr_at(api::INIT_ENCODE_CONFIG, self.config.ptr() as *mut c_void);
        init.u32(api::INIT_MAX_WIDTH, self.width);
        init.u32(api::INIT_MAX_HEIGHT, self.height);
        init.u32(api::INIT_TUNING_INFO, api::TUNING_ULTRA_LOW_LATENCY);
        let init_fn: FnWithParams = self.f.get(api::FN_INITIALIZE).ok_or("no nvEncInitializeEncoder")?;
        check(init_fn(self.encoder, self.init.ptr()), "nvEncInitializeEncoder")?;

        let mut bs = Blob::new(api::SIZE_NV_ENC_CREATE_BITSTREAM_BUFFER, api::CREATE_BITSTREAM_BUFFER_VER);
        let create_bs: FnWithParams = self.f.get(api::FN_CREATE_BITSTREAM).ok_or("no nvEncCreateBitstreamBuffer")?;
        check(create_bs(self.encoder, bs.ptr()), "nvEncCreateBitstreamBuffer")?;
        self.bitstream = bs.get_ptr(api::CREATE_BS_BUFFER);
        Ok(())
    }

    /// The parts of NV_ENC_CONFIG this stream decides (also used when reconfiguring).
    fn write_config(&mut self) {
        let (fps, bitrate) = (self.fps, self.bitrate);
        let c = &mut self.config;
        c.guid(api::CFG_PROFILE_GUID, api::H264_PROFILE_MAIN_GUID);
        // Two-second GOP, no B-frames (the receivers decode in order, with low latency).
        c.u32(api::CFG_GOP_LENGTH, fps * 2);
        c.u32(api::CFG_FRAME_INTERVAL_P, 1);
        let rc = api::CFG_RC;
        c.u32(rc + api::RC_MODE, api::PARAMS_RC_CBR);
        c.u32(rc + api::RC_AVG_BITRATE, bitrate);
        c.u32(rc + api::RC_MAX_BITRATE, bitrate);
        // One frame of buffer: steady packets, no bursts after keyframes.
        c.u32(rc + api::RC_VBV_BUFFER, bitrate / fps);
        c.u32(rc + api::RC_VBV_INITIAL_DELAY, bitrate / fps);
        let h = api::CFG_CODEC;
        c.u32(h + api::H264_IDR_PERIOD, fps * 2);
        c.bit((h + api::H264_REPEAT_SPSPPS_BIT.0, api::H264_REPEAT_SPSPPS_BIT.1), true);
        c.bit((h + api::H264_OUTPUT_AUD_BIT.0, api::H264_OUTPUT_AUD_BIT.1), false);
        // The textures are BT.709 limited range (gpu_tx's video processor writes them so).
        let vui = h + api::H264_VUI;
        c.u32(vui + api::VUI_SIGNAL_TYPE_PRESENT, 1);
        c.u32(vui + api::VUI_VIDEO_FORMAT, 5);
        c.u32(vui + api::VUI_FULL_RANGE, 0);
        c.u32(vui + api::VUI_COLOUR_DESCRIPTION_PRESENT, 1);
        c.u32(vui + api::VUI_COLOUR_PRIMARIES, 1);
        c.u32(vui + api::VUI_TRANSFER, 1);
        c.u32(vui + api::VUI_MATRIX, 1);
    }

    fn last_error(&self) -> String {
        unsafe {
            let Some(f) = self.f.get::<FnLastError>(api::FN_GET_LAST_ERROR) else { return String::new() };
            let p = f(self.encoder);
            if p.is_null() { String::new() } else { std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned() }
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Encodes one NV12 texture of the encoder's size (on the encoder's device).
    pub fn encode_texture(&mut self, texture: &ID3D11Texture2D) -> Result<Vec<u8>, String> {
        unsafe {
            let key = texture.as_raw() as usize;
            let resource = match self.registered.get(&key) {
                Some(&r) => r,
                None => {
                    let mut reg = Blob::new(api::SIZE_NV_ENC_REGISTER_RESOURCE, api::REGISTER_RESOURCE_VER);
                    reg.u32(api::REG_RESOURCE_TYPE, api::INPUT_RESOURCE_TYPE_DIRECTX);
                    reg.u32(api::REG_WIDTH, self.width);
                    reg.u32(api::REG_HEIGHT, self.height);
                    reg.u32(api::REG_PITCH, 0);
                    reg.u32(api::REG_SUBRESOURCE, 0);
                    reg.ptr_at(api::REG_RESOURCE, texture.as_raw());
                    reg.u32(api::REG_BUFFER_FORMAT, api::BUFFER_FORMAT_NV12);
                    reg.u32(api::REG_BUFFER_USAGE, api::INPUT_IMAGE);
                    let f: FnWithParams = self.f.get(api::FN_REGISTER_RESOURCE).ok_or("no nvEncRegisterResource")?;
                    check(f(self.encoder, reg.ptr()), "nvEncRegisterResource").map_err(|e| format!("{e} ({})", self.last_error()))?;
                    let r = reg.get_ptr(api::REG_REGISTERED);
                    self.registered.insert(key, r);
                    r
                }
            };

            let mut map = Blob::new(api::SIZE_NV_ENC_MAP_INPUT_RESOURCE, api::MAP_INPUT_RESOURCE_VER);
            map.ptr_at(api::MAP_REGISTERED, resource);
            let map_fn: FnWithParams = self.f.get(api::FN_MAP_INPUT).ok_or("no nvEncMapInputResource")?;
            check(map_fn(self.encoder, map.ptr()), "nvEncMapInputResource")?;
            let mapped = map.get_ptr(api::MAP_MAPPED);

            let result = self.encode_mapped(mapped, map.get_u32(api::MAP_BUFFER_FORMAT));

            if let Some(unmap) = self.f.get::<FnWithHandle>(api::FN_UNMAP_INPUT) {
                unmap(self.encoder, mapped);
            }
            result
        }
    }

    unsafe fn encode_mapped(&mut self, mapped: *mut c_void, format: u32) -> Result<Vec<u8>, String> {
        let mut pic = Blob::new(api::SIZE_NV_ENC_PIC_PARAMS, api::PIC_PARAMS_VER);
        pic.u32(api::PIC_INPUT_WIDTH, self.width);
        pic.u32(api::PIC_INPUT_HEIGHT, self.height);
        if std::mem::take(&mut self.force_idr) {
            pic.u32(api::PIC_FLAGS, api::PIC_FLAG_FORCEIDR | api::PIC_FLAG_OUTPUT_SPSPPS);
        }
        pic.u32(api::PIC_FRAME_IDX, self.frame_idx);
        pic.u64(api::PIC_TIMESTAMP, self.frame_idx as u64);
        self.frame_idx = self.frame_idx.wrapping_add(1);
        pic.ptr_at(api::PIC_INPUT_BUFFER, mapped);
        pic.ptr_at(api::PIC_OUTPUT_BITSTREAM, self.bitstream);
        pic.u32(api::PIC_BUFFER_FORMAT, format);
        pic.u32(api::PIC_STRUCT, api::PIC_STRUCT_FRAME);
        let encode: FnWithParams = self.f.get(api::FN_ENCODE_PICTURE).ok_or("no nvEncEncodePicture")?;
        check(encode(self.encoder, pic.ptr()), "nvEncEncodePicture").map_err(|e| format!("{e} ({})", self.last_error()))?;

        let mut lock = Blob::new(api::SIZE_NV_ENC_LOCK_BITSTREAM, api::LOCK_BITSTREAM_VER);
        lock.ptr_at(api::LOCK_OUTPUT_BITSTREAM, self.bitstream);
        let lock_fn: FnWithParams = self.f.get(api::FN_LOCK_BITSTREAM).ok_or("no nvEncLockBitstream")?;
        check(lock_fn(self.encoder, lock.ptr()), "nvEncLockBitstream")?;
        let size = lock.get_u32(api::LOCK_SIZE) as usize;
        let data = lock.get_ptr(api::LOCK_PTR) as *const u8;
        let out = if data.is_null() { Vec::new() } else { std::slice::from_raw_parts(data, size).to_vec() };
        if let Some(unlock) = self.f.get::<FnWithHandle>(api::FN_UNLOCK_BITSTREAM) {
            unlock(self.encoder, self.bitstream);
        }
        Ok(out)
    }

    pub fn force_idr(&mut self) {
        self.force_idr = true;
    }

    pub fn bitrate(&self) -> u32 {
        self.bitrate
    }

    /// New target bitrate, without restarting the stream.
    pub fn set_bitrate(&mut self, bitrate: u32) -> Result<(), String> {
        if bitrate == self.bitrate {
            return Ok(());
        }
        self.bitrate = bitrate;
        self.write_config();
        unsafe {
            let mut reconf = Blob::new(api::SIZE_NV_ENC_RECONFIGURE_PARAMS, api::RECONFIGURE_PARAMS_VER);
            let init = self.init.bytes()[..api::SIZE_NV_ENC_INITIALIZE_PARAMS].to_vec();
            reconf.bytes_mut()[api::RECONF_INIT_PARAMS..api::RECONF_INIT_PARAMS + init.len()].copy_from_slice(&init);
            reconf.bit(api::RECONF_RESET_BIT, false);
            reconf.bit(api::RECONF_FORCE_IDR_BIT, false);
            let f: FnWithParams = self.f.get(api::FN_RECONFIGURE).ok_or("no nvEncReconfigureEncoder")?;
            check(f(self.encoder, reconf.ptr()), "nvEncReconfigureEncoder").map_err(|e| format!("{e} ({})", self.last_error()))
        }
    }
}

impl Drop for NvencD3d11Encoder {
    fn drop(&mut self) {
        unsafe {
            if let Some(unreg) = self.f.get::<FnWithHandle>(api::FN_UNREGISTER_RESOURCE) {
                for (_, r) in self.registered.drain() {
                    unreg(self.encoder, r);
                }
            }
            if !self.bitstream.is_null() {
                if let Some(f) = self.f.get::<FnWithHandle>(api::FN_DESTROY_BITSTREAM) {
                    f(self.encoder, self.bitstream);
                }
            }
            if !self.encoder.is_null() {
                if let Some(f) = self.f.get::<FnDestroy>(api::FN_DESTROY_ENCODER) {
                    f(self.encoder);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC};
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};

    fn nvidia_device() -> Option<ID3D11Device> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
            let mut i = 0;
            while let Ok(adapter) = factory.EnumAdapters1(i) {
                i += 1;
                if adapter.GetDesc1().ok()?.VendorId != 0x10de {
                    continue;
                }
                let mut device = None;
                D3D11CreateDevice(&adapter, D3D_DRIVER_TYPE_UNKNOWN, HMODULE::default(), D3D11_CREATE_DEVICE_VIDEO_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, None).ok()?;
                return device;
            }
            None
        }
    }

    fn nv12_texture(device: &ID3D11Device, w: u32, h: u32, (y, u, v): (u8, u8, u8)) -> ID3D11Texture2D {
        let mut data = vec![y; (w * h) as usize];
        for _ in 0..(w * h / 4) {
            data.push(u);
            data.push(v);
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let init = D3D11_SUBRESOURCE_DATA { pSysMem: data.as_ptr() as *const _, SysMemPitch: w, SysMemSlicePitch: 0 };
        let mut tex = None;
        unsafe { device.CreateTexture2D(&desc, Some(&init), Some(&mut tex)).unwrap() };
        tex.unwrap()
    }

    /// Needs an NVIDIA GPU: cargo test --release --lib nvenc_d3d11 -- --ignored
    #[test]
    #[ignore]
    fn encodes_textures_that_decode_back_to_the_same_colours() {
        use openh264::formats::YUVSource;
        let Some(device) = nvidia_device() else {
            eprintln!("no NVIDIA adapter");
            return;
        };
        let (w, h) = (1280, 720);
        let mut enc = NvencD3d11Encoder::new(&device, w, h, 60, 6_000_000).unwrap();
        let colours = [(100u8, 80u8, 200u8), (40, 160, 90), (200, 128, 128)];
        let textures: Vec<_> = colours.iter().map(|&c| nv12_texture(&device, w, h, c)).collect();
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        let mut stream = Vec::new();
        for (i, tex) in textures.iter().enumerate() {
            for n in 0..5 {
                if i == 2 && n == 0 {
                    enc.set_bitrate(4_000_000).unwrap();
                    enc.force_idr();
                }
                let bytes = enc.encode_texture(tex).unwrap();
                assert!(!bytes.is_empty());
                stream.extend_from_slice(&bytes);
                let pic = decoder.decode(&bytes).unwrap().expect("a picture per frame (no B-frames)");
                let (dw, dh) = pic.dimensions();
                assert_eq!((dw as u32, dh as u32), (w, h));
                let (ys, us, _) = pic.strides();
                let at = |plane: &[u8], stride: usize, x: usize, y: usize| plane[y * stride + x] as i32;
                let (ey, eu, ev) = colours[i];
                let got = (at(pic.y(), ys, 640, 360), at(pic.u(), us, 320, 180), at(pic.v(), us, 320, 180));
                assert!((got.0 - ey as i32).abs() <= 3 && (got.1 - eu as i32).abs() <= 3 && (got.2 - ev as i32).abs() <= 3,
                    "frame {i}/{n}: expected {:?}, decoded {:?}", (ey, eu, ev), got);
            }
        }
        // The SPS declares what the textures hold: BT.709, limited range.
        let sps = crate::screen_capture::hw_decode::h264_state::nal_ranges(&stream)
            .into_iter()
            .find(|r| stream[r.start] & 0x1f == 7)
            .map(|r| stream[r].to_vec())
            .expect("an SPS");
        use h264_reader::nal::Nal;
        let nal = h264_reader::nal::RefNal::new(&sps, &[], true);
        let sps = h264_reader::nal::sps::SeqParameterSet::from_bits(nal.rbsp_bits()).unwrap();
        let signal = sps.vui_parameters.as_ref().and_then(|v| v.video_signal_type.as_ref()).expect("a video signal type");
        assert!(!signal.video_full_range_flag, "limited range");
        let c = signal.colour_description.as_ref().expect("a colour description");
        assert_eq!((c.colour_primaries, c.transfer_characteristics, c.matrix_coefficients), (1, 1, 1), "BT.709");
    }
}
