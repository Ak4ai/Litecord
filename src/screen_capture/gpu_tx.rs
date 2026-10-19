//! Sending a stream without the CPU touching its pictures (Windows).
//!
//! The captured screen texture stays on the GPU: a video processor scales it into the
//! stream's size (aspect kept, centred on the dark background the CPU path uses) and
//! converts it to NV12, BT.709 limited range, into one of a few textures the encoder reads
//! directly. Everything runs on the capture's own D3D11 device, so nothing is copied
//! between devices; the device is made multithread-safe because the capture thread
//! converts while the send thread encodes.
//!
//! The local preview is the only thing read back: a small picture a few times a second.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use slint::{Rgba8Pixel, SharedPixelBuffer};
use windows::core::Interface;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

/// Stream pictures in flight at once: the one being encoded (and re-sent while the screen
/// does not change), the newest one, and room for the capture to write the next.
const RING: usize = 4;

/// Behind the picture when its shape differs from the stream's (#111214, as the CPU path).
const BACKGROUND: (f32, f32, f32) = (17.0 / 255.0, 18.0 / 255.0, 20.0 / 255.0);
/// The same colour in BT.709 limited range (Y 31, Cb 129, Cr 128).
const BACKGROUND_YCBCR: (f32, f32, f32) = (31.0 / 255.0, 129.0 / 255.0, 128.0 / 255.0);

/// A converted picture, ready for the encoder. Its texture returns to the ring when the
/// last clone is dropped.
#[derive(Clone)]
pub struct GpuFrame {
    pub texture: ID3D11Texture2D,
    pub device: ID3D11Device,
    pub width: u32,
    pub height: u32,
    _slot: Arc<SlotGuard>,
}

// COM objects of a multithread-protected device, used by one thread at a time.
unsafe impl Send for GpuFrame {}

struct SlotGuard(Arc<AtomicBool>);

impl Drop for SlotGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

struct Slot {
    texture: ID3D11Texture2D,
    view: ID3D11VideoProcessorOutputView,
    busy: Arc<AtomicBool>,
}

struct Processor {
    key: (u32, u32, u32, u32),
    enumerator: ID3D11VideoProcessorEnumerator,
    vp: ID3D11VideoProcessor,
}

struct Preview {
    target: ID3D11Texture2D,
    view: ID3D11VideoProcessorOutputView,
    staging: ID3D11Texture2D,
}

pub struct GpuStage {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    vdev: ID3D11VideoDevice,
    vctx: ID3D11VideoContext,
    out: (u32, u32),
    stream: Option<Processor>,
    preview_proc: Option<Processor>,
    slots: Vec<Slot>,
    preview: Option<Preview>,
    /// Input views of the capture's textures (its frame pool reuses a few), by pointer.
    in_views: HashMap<(usize, usize), ID3D11VideoProcessorInputView>,
}

unsafe impl Send for GpuStage {}

type Result<T> = std::result::Result<T, windows::core::Error>;

impl GpuStage {
    /// A stage on `device` (the capture's) producing `width` x `height` pictures.
    pub fn new(device: &ID3D11Device, width: u32, height: u32) -> Result<Self> {
        unsafe {
            if let Ok(mt) = device.cast::<ID3D11Multithread>() {
                let _ = mt.SetMultithreadProtected(true);
            }
            let context = device.GetImmediateContext()?;
            Ok(Self {
                vdev: device.cast()?,
                vctx: context.cast()?,
                device: device.clone(),
                context,
                out: (width & !1, height & !1),
                stream: None,
                preview_proc: None,
                slots: Vec::new(),
                preview: None,
                in_views: HashMap::new(),
            })
        }
    }

    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }

    /// Converts the captured `source` (BGRA) into a free stream texture. `None` when the
    /// encoder still holds every texture (the picture is dropped, as a full queue would).
    pub fn convert(&mut self, source: &ID3D11Texture2D) -> Result<Option<GpuFrame>> {
        unsafe {
            let (in_w, in_h) = texture_size(source);
            let (out_w, out_h) = self.out;
            let key = (in_w, in_h, out_w, out_h);
            if self.stream.as_ref().map(|p| p.key != key).unwrap_or(true) {
                self.stream = Some(self.processor(key, true)?);
                // Output views are tied to the enumerator. Pictures still in flight keep
                // their old textures alive until dropped.
                self.slots.clear();
                self.create_slots()?;
                self.in_views.retain(|k, _| k.1 != 0);
            }
            let Some(i) = self.slots.iter().position(|s| !s.busy.load(Ordering::Acquire)) else {
                return Ok(None);
            };
            let input = self.input_view(source, 0)?;
            blt(&self.vctx, self.stream.as_ref().unwrap(), &input, &self.slots[i].view, (in_w, in_h), (out_w, out_h))?;
            let slot = &self.slots[i];
            slot.busy.store(true, Ordering::Release);
            Ok(Some(GpuFrame {
                texture: slot.texture.clone(),
                device: self.device.clone(),
                width: out_w,
                height: out_h,
                _slot: Arc::new(SlotGuard(slot.busy.clone())),
            }))
        }
    }

    /// A small RGBA copy of `source` for the local preview (at most `max_w` wide).
    pub fn preview(&mut self, source: &ID3D11Texture2D, max_w: u32) -> Result<SharedPixelBuffer<Rgba8Pixel>> {
        unsafe {
            let (in_w, in_h) = texture_size(source);
            let w = max_w.min(in_w).max(2) & !1;
            let h = ((in_h as u64 * w as u64 / in_w.max(1) as u64) as u32).max(2) & !1;
            let key = (in_w, in_h, w, h);
            if self.preview_proc.as_ref().map(|p| p.key != key).unwrap_or(true) {
                let p = self.processor(key, false)?;
                self.preview = Some(self.create_preview(&p, w, h)?);
                self.preview_proc = Some(p);
                self.in_views.retain(|k, _| k.1 != 1);
            }
            let input = self.input_view(source, 1)?;
            let pv = self.preview.as_ref().unwrap();
            blt(&self.vctx, self.preview_proc.as_ref().unwrap(), &input, &pv.view, (in_w, in_h), (w, h))?;
            self.context.CopyResource(&pv.staging, &pv.target);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(&pv.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
            let dst = buf.make_mut_slice();
            for y in 0..h as usize {
                let row = std::slice::from_raw_parts((mapped.pData as *const u8).add(y * mapped.RowPitch as usize), w as usize * 4);
                for (x, px) in row.chunks_exact(4).enumerate() {
                    dst[y * w as usize + x] = Rgba8Pixel::new(px[2], px[1], px[0], 255);
                }
            }
            self.context.Unmap(&pv.staging, 0);
            Ok(buf)
        }
    }

    /// A video processor for `key` = (in w, in h, out w, out h): stream (NV12, BT.709
    /// limited) or preview (BGRA).
    unsafe fn processor(&self, key: (u32, u32, u32, u32), to_nv12: bool) -> Result<Processor> {
        let (in_w, in_h, out_w, out_h) = key;
        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL { Numerator: 60, Denominator: 1 },
            InputWidth: in_w,
            InputHeight: in_h,
            OutputFrameRate: DXGI_RATIONAL { Numerator: 60, Denominator: 1 },
            OutputWidth: out_w,
            OutputHeight: out_h,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        let enumerator = self.vdev.CreateVideoProcessorEnumerator(&desc)?;
        let vp = self.vdev.CreateVideoProcessor(&enumerator, 0)?;
        let vctx = &self.vctx;
        // Only scale and convert: no driver "enhancements" (they cost GPU time).
        vctx.VideoProcessorSetStreamAutoProcessingMode(&vp, 0, false);
        vctx.VideoProcessorSetStreamFrameFormat(&vp, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
        vctx.VideoProcessorSetStreamOutputRate(&vp, 0, D3D11_VIDEO_PROCESSOR_OUTPUT_RATE_NORMAL, false, None);
        // Given as YCbCr for the stream: drivers read an RGB background differently (NVIDIA
        // takes it as limited-range RGB).
        let bg = if to_nv12 {
            let (y, cb, cr) = BACKGROUND_YCBCR;
            D3D11_VIDEO_COLOR { Anonymous: D3D11_VIDEO_COLOR_0 { YCbCr: D3D11_VIDEO_COLOR_YCbCrA { Y: y, Cb: cb, Cr: cr, A: 1.0 } } }
        } else {
            let (r, g, b) = BACKGROUND;
            D3D11_VIDEO_COLOR { Anonymous: D3D11_VIDEO_COLOR_0 { RGBA: D3D11_VIDEO_COLOR_RGBA { R: r, G: g, B: b, A: 1.0 } } }
        };
        vctx.VideoProcessorSetOutputBackgroundColor(&vp, to_nv12, &bg);
        // The screen is full-range RGB; the stream BT.709 limited range (what the encoders declare).
        let out_space = if to_nv12 { DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709 } else { DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709 };
        if let Ok(vctx1) = vctx.cast::<ID3D11VideoContext1>() {
            vctx1.VideoProcessorSetStreamColorSpace1(&vp, 0, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709);
            vctx1.VideoProcessorSetOutputColorSpace1(&vp, out_space);
        } else {
            // Usage 0, RGB full range; output: YCbCr_Matrix (bit 2) = BT.709, Nominal_Range (bits 4-5) = 16-235.
            vctx.VideoProcessorSetStreamColorSpace(&vp, 0, &D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 });
            let out_bits = if to_nv12 { 1 << 2 | 1 << 4 } else { 0 };
            vctx.VideoProcessorSetOutputColorSpace(&vp, &D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: out_bits });
        }
        Ok(Processor { key, enumerator, vp })
    }

    unsafe fn create_slots(&mut self) -> Result<()> {
        let Some(p) = self.stream.as_ref() else { return Ok(()) };
        let (w, h) = self.out;
        for _ in 0..RING {
            let texture = create_texture(&self.device, w, h, DXGI_FORMAT_NV12, D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET.0 as u32, 0)?;
            let view = output_view(&self.vdev, &texture, &p.enumerator)?;
            self.slots.push(Slot { texture, view, busy: Arc::new(AtomicBool::new(false)) });
        }
        Ok(())
    }

    unsafe fn create_preview(&self, p: &Processor, w: u32, h: u32) -> Result<Preview> {
        let target = create_texture(&self.device, w, h, DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET.0 as u32, 0)?;
        let staging = create_texture(&self.device, w, h, DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32)?;
        let view = output_view(&self.vdev, &target, &p.enumerator)?;
        Ok(Preview { target, view, staging })
    }

    /// The input view of a capture texture for processor 0 (stream) or 1 (preview).
    unsafe fn input_view(&mut self, source: &ID3D11Texture2D, which: usize) -> Result<ID3D11VideoProcessorInputView> {
        let key = (source.as_raw() as usize, which);
        if let Some(v) = self.in_views.get(&key) {
            return Ok(v.clone());
        }
        if self.in_views.len() > 16 {
            self.in_views.clear();
        }
        let enumerator = if which == 0 { &self.stream.as_ref().unwrap().enumerator } else { &self.preview_proc.as_ref().unwrap().enumerator };
        let mut desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC { FourCC: 0, ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D, ..Default::default() };
        desc.Anonymous.Texture2D = D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 };
        let mut v = None;
        self.vdev.CreateVideoProcessorInputView(source, enumerator, &desc, Some(&mut v))?;
        let v = v.unwrap();
        self.in_views.insert(key, v.clone());
        Ok(v)
    }
}

unsafe fn texture_size(t: &ID3D11Texture2D) -> (u32, u32) {
    let mut d = D3D11_TEXTURE2D_DESC::default();
    t.GetDesc(&mut d);
    (d.Width, d.Height)
}

unsafe fn create_texture(device: &ID3D11Device, w: u32, h: u32, format: DXGI_FORMAT, usage: D3D11_USAGE, bind: u32, cpu: u32) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: usage,
        BindFlags: bind,
        CPUAccessFlags: cpu,
        MiscFlags: 0,
    };
    let mut t = None;
    device.CreateTexture2D(&desc, None, Some(&mut t))?;
    Ok(t.unwrap())
}

unsafe fn output_view(vdev: &ID3D11VideoDevice, t: &ID3D11Texture2D, e: &ID3D11VideoProcessorEnumerator) -> Result<ID3D11VideoProcessorOutputView> {
    let desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC { ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D, ..Default::default() };
    let mut v = None;
    vdev.CreateVideoProcessorOutputView(t, e, &desc, Some(&mut v))?;
    Ok(v.unwrap())
}

/// Scales `input` (in_w x in_h) into `output` (out_w x out_h), aspect kept, centred.
unsafe fn blt(vctx: &ID3D11VideoContext, p: &Processor, input: &ID3D11VideoProcessorInputView, output: &ID3D11VideoProcessorOutputView, (in_w, in_h): (u32, u32), (out_w, out_h): (u32, u32)) -> Result<()> {
    let scale = (out_w as f64 / in_w as f64).min(out_h as f64 / in_h as f64);
    let (dw, dh) = (((in_w as f64 * scale).round() as i32) & !1, ((in_h as f64 * scale).round() as i32) & !1);
    let (dx, dy) = (((out_w as i32 - dw) / 2) & !1, ((out_h as i32 - dh) / 2) & !1);
    vctx.VideoProcessorSetStreamSourceRect(&p.vp, 0, true, Some(&RECT { left: 0, top: 0, right: in_w as i32, bottom: in_h as i32 }));
    vctx.VideoProcessorSetStreamDestRect(&p.vp, 0, true, Some(&RECT { left: dx, top: dy, right: dx + dw, bottom: dy + dh }));
    vctx.VideoProcessorSetOutputTargetRect(&p.vp, true, Some(&RECT { left: 0, top: 0, right: out_w as i32, bottom: out_h as i32 }));
    let streams = [D3D11_VIDEO_PROCESSOR_STREAM { Enable: true.into(), pInputSurface: std::mem::ManuallyDrop::new(Some(input.clone())), ..Default::default() }];
    let result = vctx.VideoProcessorBlt(&p.vp, output, 0, &streams);
    let [mut s] = streams;
    std::mem::ManuallyDrop::drop(&mut s.pInputSurface);
    result
}

// ---- The stream being sent: capture threads hand pictures to the send thread --------

use std::sync::atomic::AtomicU8;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const UNDECIDED: u8 = 0;
const GPU: u8 = 1;
const CPU: u8 = 2;

/// Whether this stream's pictures go through the GPU: tried first when allowed, and given
/// up for good (back to the CPU copies) as soon as something in the chain fails.
static MODE: AtomicU8 = AtomicU8::new(CPU);
static LATEST: Mutex<Option<GpuFrame>> = Mutex::new(None);
static PREVIEW: Mutex<Option<SharedPixelBuffer<Rgba8Pixel>>> = Mutex::new(None);
static ENCODER_CHOICE: Mutex<Option<String>> = Mutex::new(None);

/// How often the local preview is refreshed from the GPU (as the CPU path does).
const PREVIEW_EVERY: Duration = Duration::from_millis(250);
const PREVIEW_WIDTH: u32 = 480;

/// A new stream: try the GPU path if `allowed` (screen capture with a hardware encoder).
pub fn begin_stream(allowed: bool) {
    *ENCODER_CHOICE.lock().unwrap_or_else(|e| e.into_inner()) = Some(crate::video_settings::get_video_encoder());
    MODE.store(if allowed { UNDECIDED } else { CPU }, Ordering::Relaxed);
    *NEW_DEVICE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *PREVIEW.lock().unwrap_or_else(|e| e.into_inner()) = None;
    super::gpu_capture::clear_status();
}

/// Settings changes take effect on the next stream, including its CPU fallback.
pub fn encoder_choice() -> String {
    ENCODER_CHOICE.lock().unwrap_or_else(|e| e.into_inner()).clone()
        .unwrap_or_else(crate::video_settings::get_video_encoder)
}

/// Release queued pictures at stop; no texture or device is kept for a later stream.
pub fn end_stream() {
    MODE.store(CPU, Ordering::Relaxed);
    *NEW_DEVICE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *PREVIEW.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The GPU path is (still) in use or being tried.
pub fn active() -> bool {
    MODE.load(Ordering::Relaxed) != CPU
}

/// The send thread encoded its first picture from a texture.
pub fn confirm() {
    let _ = MODE.compare_exchange(UNDECIDED, GPU, Ordering::Relaxed, Ordering::Relaxed);
}

/// Back to the CPU path for the rest of this stream.
pub fn give_up(why: &str) {
    let changed = MODE.swap(CPU, Ordering::Relaxed) != CPU;
    if changed {
        log::warn!("🎞️ [GPU TX] Transmissão volta a passar pela CPU: {}", why);
    }
    *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = None;
    if changed { super::gpu_capture::status_changed(); }
}

/// The capture device of this stream, once, when the capture starts using it.
static NEW_DEVICE: Mutex<Option<(ID3D11Device, u32, u32)>> = Mutex::new(None);

pub fn take_new_device() -> Option<(ID3D11Device, u32, u32)> {
    NEW_DEVICE.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// The newest converted picture, if one arrived since the last call.
pub fn take_latest() -> Option<GpuFrame> {
    LATEST.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// The newest local preview, if one arrived since the last call.
pub fn take_preview() -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    PREVIEW.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// The PCI vendor id of the GPU a device runs on.
pub fn vendor_of(device: &ID3D11Device) -> Option<u32> {
    unsafe {
        let dxgi: windows::Win32::Graphics::Dxgi::IDXGIDevice = device.cast().ok()?;
        Some(dxgi.GetAdapter().ok()?.GetDesc().ok()?.VendorId)
    }
}

/// Per capture thread: turns captured textures into stream pictures on the GPU.
pub struct CaptureSide {
    out: (u32, u32),
    stage: Option<(usize, GpuStage)>,
    last_preview: Option<Instant>,
}

unsafe impl Send for CaptureSide {}

impl CaptureSide {
    pub fn new(width: u32, height: u32) -> Self {
        Self { out: (width, height), stage: None, last_preview: None }
    }

    /// Handles a captured texture on the GPU. `device` and `texture` are raw COM pointers
    /// (the capture library uses another version of the `windows` crate). Returns false
    /// when the CPU path must take the picture instead.
    pub fn handle(&mut self, device: *mut std::ffi::c_void, texture: *mut std::ffi::c_void, watched: bool) -> bool {
        if !active() || device.is_null() || texture.is_null() {
            return false;
        }
        let (Some(device), Some(texture)) = (unsafe { ID3D11Device::from_raw_borrowed(&device) }, unsafe { ID3D11Texture2D::from_raw_borrowed(&texture) }) else {
            return false;
        };
        self.handle_typed(device, texture, watched)
    }

    pub fn handle_typed(&mut self, device: &ID3D11Device, texture: &ID3D11Texture2D, watched: bool) -> bool {
        if !active() { return false; }
        if self.stage.as_ref().map(|(k, _)| *k != device.as_raw() as usize).unwrap_or(true) {
            match vendor_of(device) {
                Some(v) if crate::encoder::gpu_encoder_supports(v) => {}
                v => {
                    give_up(&format!("nenhum codificador lê texturas desta placa (fabricante {:04x?})", v));
                    return false;
                }
            }
            match GpuStage::new(device, self.out.0, self.out.1) {
                Ok(stage) => {
                    self.stage = Some((device.as_raw() as usize, stage));
                    // The send thread opens its encoder now, before anyone watches.
                    *NEW_DEVICE.lock().unwrap_or_else(|e| e.into_inner()) = Some((device.clone(), self.out.0 & !1, self.out.1 & !1));
                }
                Err(e) => {
                    give_up(&format!("conversão pela GPU indisponível ({e})"));
                    return false;
                }
            }
        }
        let stage = &mut self.stage.as_mut().unwrap().1;
        if self.last_preview.map_or(true, |t| t.elapsed() >= PREVIEW_EVERY) {
            self.last_preview = Some(Instant::now());
            match stage.preview(texture, PREVIEW_WIDTH) {
                Ok(p) => {
                    let mut preview = PREVIEW.lock().unwrap_or_else(|e| e.into_inner());
                    if active() { *preview = Some(p); }
                }
                Err(e) => log::debug!("preview: {e}"),
            }
        }
        if watched {
            match stage.convert(texture) {
                Ok(Some(frame)) => {
                    let mut latest = LATEST.lock().unwrap_or_else(|e| e.into_inner());
                    if active() { *latest = Some(frame); }
                }
                // The encoder still holds every texture: this picture is skipped.
                Ok(None) => {}
                Err(e) => {
                    give_up(&format!("falha ao converter na GPU ({e})"));
                    return false;
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};

    fn device(vendor: u32) -> Option<ID3D11Device> {
        device_with(vendor, D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT)
    }

    fn device_with(vendor: u32, flags: D3D11_CREATE_DEVICE_FLAG) -> Option<ID3D11Device> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
            let mut i = 0;
            while let Ok(adapter) = factory.EnumAdapters1(i) {
                i += 1;
                if adapter.GetDesc1().ok()?.VendorId != vendor {
                    continue;
                }
                let mut device = None;
                D3D11CreateDevice(&adapter, D3D_DRIVER_TYPE_UNKNOWN, HMODULE::default(), flags, None, D3D11_SDK_VERSION, Some(&mut device), None, None).ok()?;
                return device;
            }
            None
        }
    }

    /// 4:3 BGRA picture: left half white, right half red.
    fn source(device: &ID3D11Device) -> ID3D11Texture2D {
        let (w, h) = (1600u32, 1200u32);
        let mut px: Vec<u8> = Vec::with_capacity((w * h * 4) as usize);
        for _y in 0..h {
            for x in 0..w {
                px.extend_from_slice(if x < w / 2 { &[255, 255, 255, 255] } else { &[0, 0, 255, 255] });
            }
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let init = D3D11_SUBRESOURCE_DATA { pSysMem: px.as_ptr() as *const _, SysMemPitch: w * 4, SysMemSlicePitch: 0 };
        let mut t = None;
        unsafe { device.CreateTexture2D(&desc, Some(&init), Some(&mut t)).unwrap() };
        t.unwrap()
    }

    /// (Y, U, V) at (x, y) of an NV12 texture, read back through a staging copy.
    fn nv12_at(device: &ID3D11Device, tex: &ID3D11Texture2D, points: &[(u32, u32)]) -> Vec<(i32, i32, i32)> {
        unsafe {
            let (w, h) = texture_size(tex);
            let staging = create_texture(device, w, h, DXGI_FORMAT_NV12, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32).unwrap();
            let ctx = device.GetImmediateContext().unwrap();
            ctx.CopyResource(&staging, tex);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m)).unwrap();
            let base = m.pData as *const u8;
            let pitch = m.RowPitch as usize;
            let out = points
                .iter()
                .map(|&(x, y)| {
                    let yv = *base.add(y as usize * pitch + x as usize) as i32;
                    let uv = base.add(pitch * h as usize + (y as usize / 2) * pitch + (x as usize & !1));
                    (yv, *uv as i32, *uv.add(1) as i32)
                })
                .collect();
            ctx.Unmap(&staging, 0);
            out
        }
    }

    fn close(a: (i32, i32, i32), b: (i32, i32, i32)) -> bool {
        (a.0 - b.0).abs() <= 3 && (a.1 - b.1).abs() <= 3 && (a.2 - b.2).abs() <= 3
    }

    /// Needs an NVIDIA GPU: cargo test --release --lib gpu_tx -- --ignored
    #[test]
    #[ignore]
    fn scales_converts_and_letterboxes_on_the_gpu_then_encodes() {
        use openh264::formats::YUVSource;
        let Some(dev) = device(0x10de) else { return };
        let src = source(&dev);
        let mut stage = GpuStage::new(&dev, 1280, 720).unwrap();
        let frame = stage.convert(&src).unwrap().expect("a free slot");
        // 4:3 into 16:9: a 960x720 picture at x = 160, dark bars on both sides.
        let bar = (31, 129, 128);
        let white = (235, 128, 128);
        let red = (63, 102, 240);
        let got = nv12_at(&dev, &frame.texture, &[(50, 360), (400, 360), (1000, 360), (1230, 360)]);
        for (g, want) in got.iter().zip([bar, white, red, bar]) {
            assert!(close(*g, want), "NV12 {:?}, expected {:?} (all: {:?})", g, want, got);
        }

        // The ring: four pictures in flight at most; dropping one frees its texture.
        let mut held = vec![frame];
        for _ in 0..3 {
            held.push(stage.convert(&src).unwrap().expect("a free slot"));
        }
        assert!(stage.convert(&src).unwrap().is_none(), "ring full");
        held.pop();
        let again = stage.convert(&src).unwrap().expect("the dropped picture's texture");

        // Encoded with NVENC straight from the texture, it decodes to the same colours.
        let mut enc = crate::encoder::nvenc_d3d11::NvencD3d11Encoder::new(&dev, 1280, 720, 60, 8_000_000).unwrap();
        let bytes = enc.encode_texture(&again.texture).unwrap();
        let mut dec = openh264::decoder::Decoder::new().unwrap();
        let pic = dec.decode(&bytes).unwrap().unwrap();
        let (ys, us, _) = pic.strides();
        let at = |x: usize, y: usize| (pic.y()[y * ys + x] as i32, pic.u()[(y / 2) * us + x / 2] as i32, pic.v()[(y / 2) * us + x / 2] as i32);
        for ((x, y), want) in [((50, 360), bar), ((400, 360), white), ((1000, 360), red)] {
            assert!(close(at(x, y), want), "decoded {:?} at {:?}, expected {:?}", at(x, y), (x, y), want);
        }

        // The preview: 480 wide, same shape as the screen, RGBA.
        let pv = stage.preview(&src, 480).unwrap();
        assert_eq!((pv.width(), pv.height()), (480, 360));
        let px = |x: usize, y: usize| { let p = pv.as_slice()[y * 480 + x]; (p.r, p.g, p.b) };
        let (wr, wg, wb) = px(100, 180);
        assert!(wr > 250 && wg > 250 && wb > 250, "white {:?}", px(100, 180));
        let (rr, rg, rb) = px(380, 180);
        assert!(rr > 250 && rg < 5 && rb < 5, "red {:?}", px(380, 180));
    }

    /// Capture libraries create their device without D3D11_CREATE_DEVICE_VIDEO_SUPPORT:
    /// the stage must still work there, on every GPU of this machine.
    #[test]
    #[ignore]
    fn converts_on_devices_created_like_the_capture_library_does() {
        for vendor in [0x10de, 0x8086, 0x1002] {
            let Some(dev) = device_with(vendor, D3D11_CREATE_DEVICE_BGRA_SUPPORT) else { continue };
            let src = source(&dev);
            let r = GpuStage::new(&dev, 1280, 720).and_then(|mut st| st.convert(&src));
            match r {
                Ok(Some(f)) => eprintln!("vendor {vendor:04x}: {:?}", nv12_at(&dev, &f.texture, &[(50, 360), (400, 360), (1000, 360)])),
                other => panic!("vendor {vendor:04x}: {:?}", other.map(|f| f.is_some())),
            }
        }
    }

    /// Decodes an encoded stream and checks colours at three points, plus what its SPS
    /// declares; returns (full range, matrix) from the VUI.
    fn check_stream(units: &[Vec<u8>]) -> (Option<bool>, Option<u8>) {
        use h264_reader::nal::Nal;
        use openh264::formats::YUVSource;
        let mut dec = openh264::decoder::Decoder::new().unwrap();
        let mut last = None;
        for unit in units.iter().filter(|u| !u.is_empty()) {
            if let Ok(Some(pic)) = dec.decode(unit) {
                let (ys, us, _) = pic.strides();
                let at = |x: usize, y: usize| (pic.y()[y * ys + x] as i32, pic.u()[(y / 2) * us + x / 2] as i32, pic.v()[(y / 2) * us + x / 2] as i32);
                last = Some([at(50, 360), at(400, 360), at(1000, 360)]);
            }
        }
        let got = last.expect("decoded pictures");
        let stream: Vec<u8> = units.concat();
        let stream = &stream[..];
        for (g, want) in got.iter().zip([(31, 129, 128), (235, 128, 128), (63, 102, 240)]) {
            assert!(close(*g, want), "decoded {:?}, expected {:?} (all {:?})", g, want, got);
        }
        let sps = crate::screen_capture::hw_decode::h264_state::nal_ranges(stream).into_iter().find(|r| stream[r.start] & 0x1f == 7).expect("an SPS");
        let nal = h264_reader::nal::RefNal::new(&stream[sps], &[], true);
        let sps = h264_reader::nal::sps::SeqParameterSet::from_bits(nal.rbsp_bits()).unwrap();
        let signal = sps.vui_parameters.as_ref().and_then(|v| v.video_signal_type.as_ref());
        (signal.map(|s| s.video_full_range_flag), signal.and_then(|s| s.colour_description.as_ref()).map(|c| c.matrix_coefficients))
    }

    /// Quick Sync through Media Foundation, from the stage's textures on a device created
    /// like the capture library's. Needs an Intel GPU.
    #[test]
    #[ignore]
    fn quick_sync_encodes_the_stage_textures() {
        let Some(dev) = device_with(0x8086, D3D11_CREATE_DEVICE_BGRA_SUPPORT) else { return };
        let src = source(&dev);
        let mut stage = GpuStage::new(&dev, 1280, 720).unwrap();
        let mut enc = crate::encoder::wmf::WmfGpuEncoder::for_device(&dev, 0x8086, 1280, 720, 60, 8_000_000).unwrap();
        let mut units = Vec::new();
        for _ in 0..30 {
            let f = stage.convert(&src).unwrap().unwrap();
            units.push(enc.encode_texture(&f.texture).unwrap());
        }
        let (full, matrix) = check_stream(&units);
        eprintln!("Quick Sync VUI: full range {:?}, matrix {:?}", full, matrix);
        assert_eq!(full, Some(false), "limited range declared");
        assert_eq!(matrix, Some(1), "BT.709 declared");
    }

    /// The receiver's parser (h264-reader, as h264_state uses it) must read the SPS and
    /// PPS Quick Sync writes at 1080p (cropped height) after the colour rewrite.
    #[test]
    #[ignore]
    fn quick_sync_parameter_sets_parse_at_1080p() {
        use h264_reader::nal::{Nal, RefNal};
        let Some(dev) = device_with(0x8086, D3D11_CREATE_DEVICE_BGRA_SUPPORT) else { return };
        let src = source(&dev);
        let mut stage = GpuStage::new(&dev, 1920, 1080).unwrap();
        let mut enc = crate::encoder::wmf::WmfGpuEncoder::for_device(&dev, 0x8086, 1920, 1080, 60, 8_000_000).unwrap();
        let mut stream = Vec::new();
        for _ in 0..10 {
            let f = stage.convert(&src).unwrap().unwrap();
            stream.extend(enc.encode_texture(&f.texture).unwrap());
        }
        let mut ctx = h264_reader::Context::new();
        let (mut sps_n, mut pps_n) = (0, 0);
        for r in crate::screen_capture::hw_decode::h264_state::nal_ranges(&stream) {
            let nal = RefNal::new(&stream[r.clone()], &[], true);
            match stream[r.start] & 0x1f {
                7 => {
                    let sps = h264_reader::nal::sps::SeqParameterSet::from_bits(nal.rbsp_bits()).unwrap_or_else(|e| panic!("SPS {:02x?}: {e:?}", &stream[r.clone()]));
                    ctx.put_seq_param_set(sps);
                    sps_n += 1;
                }
                8 => {
                    h264_reader::nal::pps::PicParameterSet::from_bits(&ctx, nal.rbsp_bits()).unwrap_or_else(|e| panic!("PPS {:02x?}: {e:?}", &stream[r.clone()]));
                    pps_n += 1;
                }
                _ => {}
            }
        }
        assert!(sps_n > 0 && pps_n > 0);
    }

    /// A keyframe forced mid-stream (a viewer arriving) must carry SPS and PPS: the
    /// viewer starts decoding from it.
    #[test]
    #[ignore]
    fn quick_sync_forced_keyframes_carry_parameter_sets() {
        use crate::encoder::VideoEncoder;
        let Some(dev) = device_with(0x8086, D3D11_CREATE_DEVICE_BGRA_SUPPORT) else { return };
        let src = source(&dev);
        let mut stage = GpuStage::new(&dev, 1280, 720).unwrap();
        let mut enc = crate::encoder::wmf::WmfGpuEncoder::for_device(&dev, 0x8086, 1280, 720, 60, 8_000_000).unwrap();
        let types = |au: &[u8]| -> Vec<u8> { crate::screen_capture::hw_decode::h264_state::nal_ranges(au).into_iter().map(|r| au[r.start] & 0x1f).collect() };
        let mut log = Vec::new();
        for i in 0..40 {
            if i == 20 {
                enc.force_intra_frame();
            }
            let f = stage.convert(&src).unwrap().unwrap();
            let au = enc.encode_texture(&f.texture).unwrap();
            log.push((i, types(&au)));
        }
        eprintln!("{:?}", log);
        for (i, t) in &log {
            if t.contains(&5) {
                assert!(t.contains(&7) && t.contains(&8), "IDR at {i} without SPS/PPS: {:?}", t);
            }
        }
        assert!(log.iter().filter(|(_, t)| t.contains(&5)).count() >= 2, "the forced keyframe came out");
    }
}
