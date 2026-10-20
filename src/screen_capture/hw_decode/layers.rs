//! Showing GPU-decoded pictures without bringing them back to the CPU (Windows).
//!
//! While a window shows a stream decoded on the GPU, DirectComposition layers cover it:
//! a video layer (a swap chain the video processor converts and scales each decoded
//! picture into, on the decoder thread) under a UI layer (what Slint's software renderer
//! drew, uploaded only where it changed). The stream area is painted in the hole colour,
//! which the renderer turns transparent, so the video shows through it while the controls
//! drawn over the stream still blend over the video. The window keeps drawing to GDI
//! underneath as usual, so turning the layers off needs no special care.
//!
//! The layers hold GPU resources only while on.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use windows::core::Interface;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

use super::device::{shared_device, SharedDevice};
use super::dxva::Frame;
use super::HwError;

/// Where a window wants a stream drawn: shared by the UI thread (size, stream) and the
/// decoder thread (drawing).
pub struct VideoOutput {
    peer: u64,
    swapchain: IDXGISwapChain1,
    size: (u32, u32),
    wanted: (u32, u32),
    processor: Option<Processor>,
    out_view: Option<ID3D11VideoProcessorOutputView>,
    in_views: HashMap<(usize, u32), ID3D11VideoProcessorInputView>,
}

// Used under its mutex only; the device is multithread-protected.
unsafe impl Send for VideoOutput {}

struct Processor {
    key: (u32, u32, u32, u32),
    enumerator: ID3D11VideoProcessorEnumerator,
    vp: ID3D11VideoProcessor,
    colour: Option<(bool, bool)>,
}

static OUTPUTS: Mutex<Vec<Arc<Mutex<VideoOutput>>>> = Mutex::new(Vec::new());

/// Decoder thread: draws `frame` into every window that shows `peer` on the GPU. Returns
/// false when none does, so the caller copies the picture back for the CPU path.
pub fn present(peer: u64, frame: &Frame, device: &SharedDevice) -> bool {
    let outputs: Vec<_> = OUTPUTS.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect();
    let mut shown = false;
    for out in outputs {
        let mut o = out.lock().unwrap_or_else(|e| e.into_inner());
        if o.peer != peer {
            continue;
        }
        match unsafe { draw(&mut o, frame, device) } {
            Ok(()) => shown = true,
            Err(e) => {
                static FAILED: crate::utils::logger::LogThrottle = crate::utils::logger::LogThrottle::new(5000);
                if FAILED.hit().is_some() {
                    log::warn!("🎞️ [GPU VIDEO] Falha ao desenhar o quadro na janela: {:?}", e);
                }
            }
        }
    }
    shown
}

unsafe fn draw(o: &mut VideoOutput, frame: &Frame, device: &SharedDevice) -> Result<(), HwError> {
    let vdev: ID3D11VideoDevice = device.device.cast()?;
    let vctx: ID3D11VideoContext = device.context.cast()?;
    if o.wanted != o.size && o.wanted.0 > 0 && o.wanted.1 > 0 {
        o.out_view = None;
        o.swapchain.ResizeBuffers(0, o.wanted.0, o.wanted.1, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
        o.size = o.wanted;
    }
    let (out_w, out_h) = o.size;
    let (in_w, in_h) = (frame.info.mb_width * 16, frame.info.mb_height * 16);
    let key = (in_w, in_h, out_w, out_h);
    if o.processor.as_ref().map(|p| p.key != key).unwrap_or(true) {
        o.out_view = None;
        o.in_views.clear();
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
        let enumerator = vdev.CreateVideoProcessorEnumerator(&desc)?;
        let vp = vdev.CreateVideoProcessor(&enumerator, 0)?;
        // Only convert and scale: no driver "enhancements" (they cost GPU time).
        vctx.VideoProcessorSetStreamAutoProcessingMode(&vp, 0, false);
        vctx.VideoProcessorSetStreamFrameFormat(&vp, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
        vctx.VideoProcessorSetStreamOutputRate(&vp, 0, D3D11_VIDEO_PROCESSOR_OUTPUT_RATE_NORMAL, false, None);
        vctx.VideoProcessorSetOutputBackgroundColor(&vp, false, &D3D11_VIDEO_COLOR { Anonymous: D3D11_VIDEO_COLOR_0 { RGBA: D3D11_VIDEO_COLOR_RGBA { R: 0.0, G: 0.0, B: 0.0, A: 1.0 } } });
        o.processor = Some(Processor { key, enumerator, vp, colour: None });
    }
    let p = o.processor.as_mut().unwrap();
    let colour = (frame.info.full_range, frame.info.bt709);
    if p.colour != Some(colour) {
        // YCbCr_Matrix (bit 2) = BT.709; Nominal_Range (bits 4-5): 1 = 16-235, 2 = 0-255.
        let bits = (colour.1 as u32) << 2 | if colour.0 { 2 } else { 1 } << 4;
        vctx.VideoProcessorSetStreamColorSpace(&p.vp, 0, &D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: bits });
        p.colour = Some(colour);
    }
    // Fit the picture inside the area, centred (black bars around it).
    let (fw, fh) = (frame.info.width, frame.info.height);
    let scale = (out_w as f64 / fw as f64).min(out_h as f64 / fh as f64);
    let (dw, dh) = ((fw as f64 * scale).round() as i32, (fh as f64 * scale).round() as i32);
    let (dx, dy) = ((out_w as i32 - dw) / 2, (out_h as i32 - dh) / 2);
    vctx.VideoProcessorSetStreamSourceRect(&p.vp, 0, true, Some(&RECT { left: 0, top: 0, right: fw as i32, bottom: fh as i32 }));
    vctx.VideoProcessorSetStreamDestRect(&p.vp, 0, true, Some(&RECT { left: dx, top: dy, right: dx + dw, bottom: dy + dh }));
    if o.out_view.is_none() {
        let back: ID3D11Texture2D = o.swapchain.GetBuffer(0)?;
        let od = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC { ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D, ..Default::default() };
        let mut v = None;
        vdev.CreateVideoProcessorOutputView(&back, &p.enumerator, &od, Some(&mut v))?;
        o.out_view = v;
    }
    let key = (frame.texture.as_raw() as usize, frame.index);
    if !o.in_views.contains_key(&key) {
        // A new decoder (the stream was watched again) brings new textures: the views of
        // the old ones would keep its whole picture array (~50 MB at 1080p) alive.
        o.in_views.retain(|(texture, _), _| *texture == key.0);
        let mut idesc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC { FourCC: 0, ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D, ..Default::default() };
        idesc.Anonymous.Texture2D = D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: frame.index };
        let mut v = None;
        vdev.CreateVideoProcessorInputView(&frame.texture, &p.enumerator, &idesc, Some(&mut v))?;
        o.in_views.insert(key, v.unwrap());
    }
    let p = o.processor.as_ref().unwrap();
    let streams = [D3D11_VIDEO_PROCESSOR_STREAM { Enable: true.into(), pInputSurface: std::mem::ManuallyDrop::new(Some(o.in_views[&key].clone())), ..Default::default() }];
    let result = vctx.VideoProcessorBlt(&p.vp, o.out_view.as_ref().unwrap(), 0, &streams);
    let [mut s] = streams;
    std::mem::ManuallyDrop::drop(&mut s.pInputSurface);
    result?;
    o.swapchain.Present(0, DXGI_PRESENT(0)).ok()?;
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    Off,
    /// Shown with the next UI frame (the one that paints the hole).
    Starting,
    On,
    /// Removed with the next UI frame (the one drawn without the hole).
    Stopping,
}

struct Live {
    device: Arc<SharedDevice>,
    dcomp: IDCompositionDevice,
    target: IDCompositionTarget,
    root: IDCompositionVisual,
    video_visual: IDCompositionVisual,
    ui_visual: IDCompositionVisual,
    ui_surface: Option<(IDCompositionSurface, u32, u32)>,
    output: Arc<Mutex<VideoOutput>>,
}

/// One window's layers (UI thread). Install it once as the window's layer sink.
pub struct WindowLayers {
    hwnd: isize,
    /// The layers could not be created here: this window stays on CPU pictures.
    failed: Cell<bool>,
    state: Cell<State>,
    live: RefCell<Option<Live>>,
    rect: Cell<(i32, i32, u32, u32)>,
}

impl WindowLayers {
    pub fn new(hwnd: isize) -> Self {
        Self { hwnd, failed: Cell::new(false), state: Cell::new(State::Off), live: RefCell::new(None), rect: Cell::new((0, 0, 0, 0)) }
    }

    /// Stream `peer` is shown on the GPU in this window, in `rect` (physical pixels).
    pub fn start(&self, peer: u64) -> Result<(), HwError> {
        if self.failed.get() {
            return Err(HwError::Unsupported("layers failed earlier in this window".into()));
        }
        if self.live.borrow().is_none() {
            let live = unsafe { self.create(peer) }.inspect_err(|_| self.failed.set(true))?;
            *self.live.borrow_mut() = Some(live);
        }
        if let Some(l) = self.live.borrow().as_ref() {
            l.output.lock().unwrap_or_else(|e| e.into_inner()).peer = peer;
            let mut outs = OUTPUTS.lock().unwrap_or_else(|e| e.into_inner());
            if !outs.iter().any(|o| Arc::ptr_eq(o, &l.output)) {
                outs.push(l.output.clone());
            }
        }
        if self.state.get() != State::On {
            self.state.set(State::Starting);
        }
        Ok(())
    }

    /// The window goes back to showing the stream through the CPU.
    pub fn stop(&self) {
        if let Some(l) = self.live.borrow().as_ref() {
            OUTPUTS.lock().unwrap_or_else(|e| e.into_inner()).retain(|o| !Arc::ptr_eq(o, &l.output));
        }
        match self.state.get() {
            State::On => self.state.set(State::Stopping),
            State::Starting => {
                self.state.set(State::Off);
                self.park();
            }
            _ => {}
        }
    }

    /// Like `stop`, for a window that is not drawn any more (hidden, minimized): the layers
    /// come off now instead of with the next frame.
    pub fn stop_now(&self) {
        self.stop();
        self.park();
        self.state.set(State::Off);
    }

    /// Takes the layers off the window but keeps them for the next stream shown here.
    /// Creating them again every time a stream was watched (composition device, swap
    /// chain, video processor) left about 30-50 MB behind each time, inside the driver.
    /// The views of the decoder's pictures go now, so its textures can be freed.
    fn park(&self) {
        if let Some(l) = self.live.borrow().as_ref() {
            unsafe {
                let _ = l.target.SetRoot(None);
                let _ = l.dcomp.Commit();
            }
            l.output.lock().unwrap_or_else(|e| e.into_inner()).in_views.clear();
        }
    }

    /// Whether this window can use layers (they did not fail to start here).
    pub fn usable(&self) -> bool {
        !self.failed.get()
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state.get(), State::Starting | State::On)
    }

    pub fn peer(&self) -> Option<u64> {
        self.is_active().then(|| self.live.borrow().as_ref().map(|l| l.output.lock().unwrap_or_else(|e| e.into_inner()).peer)).flatten()
    }

    /// Where the stream area is in the window (physical pixels).
    pub fn set_video_rect(&self, x: i32, y: i32, w: u32, h: u32) {
        self.rect.set((x, y, w, h));
        if let Some(l) = self.live.borrow().as_ref() {
            unsafe {
                let _ = l.video_visual.SetOffsetX2(x as f32);
                let _ = l.video_visual.SetOffsetY2(y as f32);
                let _ = l.dcomp.Commit();
            }
            if w > 0 && h > 0 {
                l.output.lock().unwrap_or_else(|e| e.into_inner()).wanted = (w, h);
            }
        }
    }

    unsafe fn create(&self, peer: u64) -> Result<Live, HwError> {
        let device = shared_device()?;
        let dxgi: IDXGIDevice = device.device.cast()?;
        let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
        let target = dcomp.CreateTargetForHwnd(HWND(self.hwnd as *mut _), true)?;
        let root = dcomp.CreateVisual()?;
        let video_visual = dcomp.CreateVisual()?;
        let ui_visual = dcomp.CreateVisual()?;
        let (x, y, w, h) = self.rect.get();
        let (w, h) = (w.max(2), h.max(2));
        let adapter: IDXGIAdapter = dxgi.GetAdapter()?;
        let factory: IDXGIFactory2 = adapter.GetParent()?;
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: w,
            Height: h,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: 0,
        };
        let swapchain = factory.CreateSwapChainForComposition(&device.device, &desc, None)?;
        video_visual.SetContent(&swapchain)?;
        video_visual.SetTransform2(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 })?;
        video_visual.SetOffsetX2(x as f32)?;
        video_visual.SetOffsetY2(y as f32)?;
        root.AddVisual(&video_visual, false, None)?;
        root.AddVisual(&ui_visual, true, &video_visual)?;
        let output = Arc::new(Mutex::new(VideoOutput { peer, swapchain, size: (w, h), wanted: (w, h), processor: None, out_view: None, in_views: HashMap::new() }));
        // The UI visual gets its surface with the first frame (it has the window's size).
        Ok(Live { device, dcomp, target, root, video_visual, ui_visual, ui_surface: None, output })
    }

    unsafe fn upload(&self, l: &mut Live, pixels: &[u32], width: u32, height: u32, dirty: [u32; 4]) -> Result<(), HwError> {
        let mut full = false;
        if l.ui_surface.as_ref().map(|s| (s.1, s.2) != (width, height)).unwrap_or(true) {
            let surface = l.dcomp.CreateSurface(width, height, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_ALPHA_MODE_PREMULTIPLIED)?;
            l.ui_visual.SetContent(&surface)?;
            l.ui_surface = Some((surface, width, height));
            full = true;
        }
        let [mut x, mut y, mut w, mut h] = dirty;
        if full || self.state.get() == State::Starting {
            (x, y, w, h) = (0, 0, width, height);
        }
        x = x.min(width);
        y = y.min(height);
        w = w.min(width - x);
        h = h.min(height - y);
        if w == 0 || h == 0 {
            return Ok(());
        }
        let (surface, _, _) = l.ui_surface.as_ref().unwrap();
        let rect = RECT { left: x as i32, top: y as i32, right: (x + w) as i32, bottom: (y + h) as i32 };
        let mut offset = POINT::default();
        let tex: ID3D11Texture2D = surface.BeginDraw(Some(&rect), &mut offset)?;
        let b = D3D11_BOX { left: offset.x as u32, top: offset.y as u32, front: 0, right: offset.x as u32 + w, bottom: offset.y as u32 + h, back: 1 };
        let src = pixels.as_ptr().add((y * width + x) as usize) as *const core::ffi::c_void;
        l.device.context.UpdateSubresource(&tex, 0, Some(&b), src, width * 4, 0);
        surface.EndDraw()?;
        Ok(())
    }
}

impl i_slint_backend_winit::LayerSink for WindowLayers {
    fn present(&self, pixels: &[u32], width: u32, height: u32, dirty: [u32; 4]) {
        let state = self.state.get();
        if state == State::Off {
            return;
        }
        if state == State::Stopping {
            // This frame was drawn without the hole: GDI shows it all, the layers can go.
            self.park();
            self.state.set(State::Off);
            return;
        }
        let mut live = self.live.borrow_mut();
        let Some(l) = live.as_mut() else { return };
        let result = unsafe { self.upload(l, pixels, width, height, dirty) };
        if let Err(e) = result {
            log::warn!("🎞️ [GPU VIDEO] Falha ao enviar a interface para a camada: {:?}", e);
            return;
        }
        unsafe {
            if state == State::Starting {
                let _ = l.target.SetRoot(&l.root);
                self.state.set(State::On);
            }
            let _ = l.dcomp.Commit();
        }
    }
}
