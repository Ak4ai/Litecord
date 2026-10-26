//! Screen and window capture on a GPU the app chooses (Windows).
//!
//! The capture library creates its device on whatever GPU Windows gives the process by
//! default, which on a laptop with two GPUs depends on driver profiles and even on the
//! order things start in: desktop duplication then fails when that is not the GPU wired
//! to the screen, and window capture hands frames to the other GPU. Here the device is
//! created on the GPU the stream should run on:
//!
//! - Automatic: the GPU that drives the monitor. Desktop duplication (DXGI) works only
//!   there, and nothing crosses between GPUs; on a laptop that is the integrated GPU,
//!   which leaves the dedicated one to the game.
//! - A GPU chosen in the settings: desktop duplication when it drives the monitor,
//!   Windows Graphics Capture otherwise (Windows handles the transfer between GPUs).
//!
//! Each frame goes to the GPU send path (`gpu_tx`); when that path is off or gave up,
//! the frame is copied to memory for the CPU path, as the capture library did.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use log::{info, warn};
use windows::core::{Interface, PCWSTR};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, HMONITOR, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

type StatusCallback = Arc<dyn Fn() + Send + Sync>;
static STATUS_CALLBACK: Mutex<Option<StatusCallback>> = Mutex::new(None);
static STATUS: Mutex<Option<(String, &'static str)>> = Mutex::new(None);
static ACTIVE_CAPTURE: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

pub fn set_status_callback(callback: StatusCallback) {
    *STATUS_CALLBACK.lock().unwrap_or_else(|e| e.into_inner()) = Some(callback);
}

pub fn status() -> Option<(String, &'static str)> {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn status_changed() {
    let callback = STATUS_CALLBACK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if let Some(callback) = callback {
        callback();
    }
}

pub fn clear_status() {
    *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    status_changed();
}

pub fn stop() {
    if let Some(cancelled) = ACTIVE_CAPTURE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    {
        cancelled.store(true, Ordering::Release);
    }
    clear_status();
}

fn report(gpu: &GpuInfo, backend: &'static str, cancelled: &AtomicBool) {
    let mut status = STATUS.lock().unwrap_or_else(|e| e.into_inner());
    if cancelled.load(Ordering::Acquire) {
        return;
    }
    *status = Some((gpu.name.clone(), backend));
    drop(status);
    status_changed();
}

/// A GPU of this machine.
#[derive(Clone, Debug)]
pub struct GpuInfo {
    pub name: String,
    pub vendor: u32,
    pub device: u32,
    /// It drives at least one monitor.
    pub drives_display: bool,
}

impl GpuInfo {
    /// The settings id of this GPU ("gpu:VVVV:DDDD"), stable across restarts.
    pub fn id(&self) -> String {
        format!("gpu:{:04x}:{:04x}", self.vendor, self.device)
    }
}

/// What the stream should run on, from the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Auto,
    /// The CPU path (OpenH264), whatever the GPUs.
    Cpu,
    /// A GPU by vendor id, and device id when known (0: the first GPU of that vendor).
    Gpu {
        vendor: u32,
        device: u32,
    },
}

/// The choice stored in the settings. Values of older versions map to the GPU of their
/// encoder: "nvenc"/"ffmpeg" NVIDIA, "amf" AMD, "wmf" Intel.
pub fn parse_choice(value: &str) -> Choice {
    match value {
        "openh264" => Choice::Cpu,
        "nvenc" | "ffmpeg" => Choice::Gpu {
            vendor: 0x10de,
            device: 0,
        },
        "amf" => Choice::Gpu {
            vendor: 0x1002,
            device: 0,
        },
        "wmf" => Choice::Gpu {
            vendor: 0x8086,
            device: 0,
        },
        v => {
            let mut parts = v
                .strip_prefix("gpu:")
                .map(|r| r.split(':'))
                .into_iter()
                .flatten();
            match (
                parts.next().and_then(|p| u16::from_str_radix(p, 16).ok()),
                parts.next().and_then(|p| u16::from_str_radix(p, 16).ok()),
                parts.next(),
            ) {
                (Some(vendor), Some(device), None) => Choice::Gpu {
                    vendor: vendor.into(),
                    device: device.into(),
                },
                _ => Choice::Auto,
            }
        }
    }
}

pub fn choice() -> Choice {
    parse_choice(&crate::video_settings::get_video_encoder())
}

/// The hardware GPUs of this machine (software rasterizers left out).
pub fn gpus() -> Vec<GpuInfo> {
    adapters().into_iter().map(|(_, g)| g).collect()
}

fn adapters() -> Vec<(IDXGIAdapter1, GpuInfo)> {
    let mut out = Vec::new();
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return out;
        };
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let Ok(d) = adapter.GetDesc1() else { continue };
            if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 || d.VendorId == 0x1414 {
                continue;
            }
            let name = String::from_utf16_lossy(&d.Description)
                .trim_end_matches('\0')
                .trim()
                .to_string();
            let drives_display = adapter.EnumOutputs(0).is_ok();
            // The same GPU can be listed twice (e.g. with several outputs): keep one.
            if out.iter().any(|(_, g): &(IDXGIAdapter1, GpuInfo)| {
                g.vendor == d.VendorId && g.device == d.DeviceId
            }) {
                continue;
            }
            out.push((
                adapter,
                GpuInfo {
                    name,
                    vendor: d.VendorId,
                    device: d.DeviceId,
                    drives_display,
                },
            ));
        }
    }
    out
}

/// The GPU and output that drive `monitor`.
fn display_of(monitor: HMONITOR) -> Option<(IDXGIAdapter1, GpuInfo, IDXGIOutput1)> {
    for (adapter, info) in adapters() {
        let mut j = 0;
        while let Ok(output) = unsafe { adapter.EnumOutputs(j) } {
            j += 1;
            if unsafe { output.GetDesc() }
                .map(|d| d.Monitor == monitor)
                .unwrap_or(false)
            {
                return Some((adapter, info, output.cast().ok()?));
            }
        }
    }
    None
}

/// The GPU the stream of `monitor` runs on, and whether it drives that monitor.
pub fn target_gpu(monitor: isize) -> Option<(GpuInfo, bool)> {
    let display = display_of(HMONITOR(monitor as _)).map(|(_, g, _)| g);
    match parse_choice(&super::gpu_tx::encoder_choice()) {
        Choice::Cpu => None,
        Choice::Auto => display.map(|g| (g, true)),
        Choice::Gpu { vendor, device } => {
            let chosen = gpus()
                .into_iter()
                .find(|g| g.vendor == vendor && (device == 0 || g.device == device));
            match chosen {
                Some(g) => {
                    let same = display
                        .as_ref()
                        .map(|d| d.vendor == g.vendor && d.device == g.device)
                        .unwrap_or(false);
                    Some((g, same))
                }
                // The chosen GPU is gone (another machine, a removed eGPU): automatic.
                None => display.map(|g| (g, true)),
            }
        }
    }
}

fn create_device(adapter: &IDXGIAdapter1) -> windows::core::Result<ID3D11Device> {
    let mut device = None;
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )?;
    }
    device.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))
}

/// What the capture delivers to, and how often.
pub struct Request {
    /// A window to capture (0: the monitor).
    pub hwnd: isize,
    pub monitor: isize,
    pub width: u32,
    pub height: u32,
    pub fps: u64,
    pub is_running: Arc<AtomicBool>,
    /// The CPU path: frames copied to memory, and buffers coming back for reuse.
    pub tx_frame: SyncSender<(Vec<u8>, u32, u32, u128, u128)>,
    pub rx_recycle: Arc<Mutex<Receiver<Vec<u8>>>>,
}

/// Starts capturing on the chosen GPU. False when this capture cannot run here (the caller
/// then uses the capture library, as before).
pub fn start(req: Request) -> bool {
    let monitor = if req.hwnd != 0 {
        unsafe { MonitorFromWindow(HWND(req.hwnd as _), MONITOR_DEFAULTTONEAREST) }.0 as isize
    } else {
        req.monitor
    };
    let Some((gpu, drives_monitor)) = target_gpu(monitor) else {
        return false;
    };
    let Some((adapter, _)) = adapters()
        .into_iter()
        .find(|(_, g)| g.vendor == gpu.vendor && g.device == gpu.device)
    else {
        return false;
    };
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<bool>();
    let cancelled = Arc::new(AtomicBool::new(false));
    if let Some(previous) = ACTIVE_CAPTURE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .replace(Arc::clone(&cancelled))
    {
        previous.store(true, Ordering::Release);
    }
    let worker_cancelled = Arc::clone(&cancelled);
    let spawned = std::thread::Builder::new()
        .name("gpu-capture".into())
        .spawn(move || {
            crate::cpu_profiler::set_current_thread_name("gpu-capture");
            crate::utils::thread_priority::raise_current_thread();
            let device = match create_device(&adapter) {
                Ok(d) => d,
                Err(e) => {
                    warn!("⚠️ [CAPTURA GPU] Não foi possível criar o dispositivo na {}: {}", gpu.name, e);
                    let _ = ready_tx.send(false);
                    return;
                }
            };
            if let Ok(mt) = device.cast::<ID3D11Multithread>() {
                unsafe {
                    let _ = mt.SetMultithreadProtected(true);
                }
            }
            raise_gpu_priority(&device, &gpu.name);
            if worker_cancelled.load(Ordering::Acquire) || !req.is_running.load(Ordering::Relaxed) { return; }
            let mut sink = Sink::new(&req, &device, worker_cancelled);
            if req.hwnd == 0 && drives_monitor {
                match display_of(HMONITOR(monitor as _)).map(|(_, _, o)| o) {
                    Some(output) => match duplicate(&device, &output) {
                        Ok(dup) => {
                            info!("🖥️ [CAPTURA GPU] Tela capturada por DXGI na {} (placa ligada ao monitor)", gpu.name);
                            if ready_tx.send(true).is_err() { return; }
                            report(&gpu, "DXGI", &sink.cancelled);
                            run_duplication(dup, &device, &output, &mut sink, &req);
                            return;
                        }
                        Err(e) => warn!("⚠️ [CAPTURA GPU] DXGI indisponível na {} ({}); usando WGC", gpu.name, e),
                    },
                    None => warn!("⚠️ [CAPTURA GPU] Monitor não encontrado entre as saídas da {}; usando WGC", gpu.name),
                }
            }
            match run_wgc(&device, req.hwnd, monitor, sink, &req, &gpu, &ready_tx) {
                Ok(()) => {}
                Err(e) => {
                    warn!("⚠️ [CAPTURA GPU] WGC indisponível na {}: {}", gpu.name, e);
                    let _ = ready_tx.send(false);
                }
            }
        })
        .is_ok();
    let ready = spawned
        && ready_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or(false);
    if !ready {
        cancelled.store(true, Ordering::Release);
    }
    ready
}

/// Asks Windows to schedule this device's GPU work (the capture, the conversion and the
/// encoder all run on it) ahead of other processes', as Sunshine does: while a game keeps
/// the GPUs busy, work queued at normal priority waits behind it. The highest levels need
/// the "increase scheduling priority" privilege, so lower ones are tried in turn.
fn raise_gpu_priority(device: &ID3D11Device, gpu: &str) {
    let Ok(dxgi) = device.cast::<IDXGIDevice>() else { return };
    for level in [7, 4, 1] {
        if unsafe { dxgi.SetGPUThreadPriority(level) }.is_ok() {
            info!("⚡ [CAPTURA GPU] Prioridade de agendamento da {} elevada para {}", gpu, level);
            return;
        }
    }
    info!("ℹ️ [CAPTURA GPU] Windows não aceitou elevar a prioridade de agendamento da {}", gpu);
}

/// After this long without a reason to capture, the capture stops asking for frames.
const IDLE_CLOSE_AFTER: Duration = Duration::from_secs(1);

/// Someone watches the stream, or a window shows its local preview.
fn frames_wanted() -> bool {
    super::manager::tx_watched() || super::video_pipeline::streams_watched()
}

fn duplicate(
    device: &ID3D11Device,
    output: &IDXGIOutput1,
) -> windows::core::Result<IDXGIOutputDuplication> {
    unsafe { output.DuplicateOutput(device) }
}

/// How long the capture waits for the screen to change before the send thread repeats the
/// last picture.
const ACQUIRE_WAIT: Duration = Duration::from_millis(20);
/// Between two asks for a frame. Each ask costs a call into the driver, so asking every
/// millisecond kept a core 5% busy on a still screen; a frame lasts 16.7 ms at 60 FPS,
/// and one found up to 4 ms late still goes out with the next one sent.
const ACQUIRE_POLL: Duration = Duration::from_millis(4);

/// `AcquireNextFrame`, waiting up to `wait` for a new frame without sitting inside the call.
///
/// The call holds the lock of the multithread-protected device for as long as it waits
/// (Sunshine documents the same), and the send thread's encoder uses this device: a wait
/// inside the call made its copies and `ProcessInput` wait up to the whole timeout, which
/// showed as 12-20 ms "slow frames" whenever the screen changed less often than the
/// stream's pace. Asking without a timeout and sleeping between asks leaves the lock free.
fn acquire_frame(
    dup: &IDXGIOutputDuplication,
    info: &mut DXGI_OUTDUPL_FRAME_INFO,
    resource: &mut Option<IDXGIResource>,
    wait: Duration,
) -> windows::core::Result<()> {
    let start = Instant::now();
    loop {
        match unsafe { dup.AcquireNextFrame(0, info, resource) } {
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT && start.elapsed() < wait => {
                std::thread::sleep(ACQUIRE_POLL);
            }
            other => return other,
        }
    }
}

fn run_duplication(
    dup: IDXGIOutputDuplication,
    device: &ID3D11Device,
    output: &IDXGIOutput1,
    sink: &mut Sink,
    req: &Request,
) {
    // The waits between asks for a frame are a few ms, not a 15.6 ms timer tick.
    let _fine_timer = crate::utils::timer_resolution::FineTimerGuard::new();
    let mut duplication = Some(dup);
    let frame_interval = Duration::from_nanos(1_000_000_000 / req.fps.max(1));
    let mut was_watched = false;
    while req.is_running.load(Ordering::Relaxed) && !sink.cancelled.load(Ordering::Acquire) {
        let start = Instant::now();
        // Someone started watching: duplicate again, since a new duplication's first frame
        // is the whole screen. The open one only reports changes, and a still screen (a
        // paused game, a document) would leave the new viewer without any picture.
        let watched = super::manager::tx_watched();
        if watched && !was_watched {
            duplication = None;
        }
        was_watched = watched;
        // Nobody watching and no preview shown: no frames asked for (the duplication stays
        // open, so the next one comes at once when they are wanted again).
        if !frames_wanted() {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        if duplication.is_none() {
            duplication = duplicate(device, output).ok();
            if duplication.is_none() {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
        }
        let dup = duplication.as_ref().unwrap();
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource = None;
        match acquire_frame(dup, &mut info, &mut resource, ACQUIRE_WAIT) {
            Ok(()) => {
                if let Some(tex) = resource.and_then(|r| r.cast::<ID3D11Texture2D>().ok()) {
                    sink.deliver(&tex);
                }
                unsafe {
                    let _ = dup.ReleaseFrame();
                }
            }
            // The screen did not change: the send thread repeats the last picture.
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {}
            Err(e) => {
                // Mode change, UAC prompt, full-screen switch: duplicate again.
                warn!("⚠️ [CAPTURA GPU] Duplicação perdida ({}); recriando", e);
                // DuplicateOutput rejects a second duplication of this output in the
                // same process. Release the lost instance before trying again.
                duplication = None;
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let pace = super::manager::capture_pace(frame_interval);
        let el = start.elapsed();
        if el < pace {
            std::thread::sleep(pace - el);
        }
    }
}

fn run_wgc(
    device: &ID3D11Device,
    hwnd: isize,
    monitor: isize,
    sink: Sink,
    req: &Request,
    gpu: &GpuInfo,
    ready: &std::sync::mpsc::Sender<bool>,
) -> windows::core::Result<()> {
    struct Runtime;
    impl Drop for Runtime {
        fn drop(&mut self) {
            unsafe {
                windows::Win32::System::WinRT::RoUninitialize();
            }
        }
    }
    unsafe {
        windows::Win32::System::WinRT::RoInitialize(
            windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
        )?;
        let _runtime = Runtime;
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = if hwnd != 0 {
            interop.CreateForWindow(HWND(hwnd as _))?
        } else {
            interop.CreateForMonitor(HMONITOR(monitor as _))?
        };
        let dxgi: IDXGIDevice = device.cast()?;
        let d3d: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;
        let cancelled = Arc::clone(&sink.cancelled);
        let sink = Arc::new(Mutex::new(sink));
        // Opens a capture session; frames go to the sink until it is closed.
        let open = || -> windows::core::Result<(Direct3D11CaptureFramePool, GraphicsCaptureSession)> {
            let size = item.Size()?;
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size)?;
            let session: GraphicsCaptureSession = pool.CreateCaptureSession(&item)?;
            let _ = session.SetIsBorderRequired(false);
            let _ = session.SetIsCursorCaptureEnabled(true);
            let last_size = Arc::new(Mutex::new(size));
            let (sink_cb, size_cb) = (sink.clone(), last_size.clone());
            pool.FrameArrived(&TypedEventHandler::<Direct3D11CaptureFramePool, windows::core::IInspectable>::new(move |pool, _| {
                let Some(pool) = pool.as_ref() else {
                    return Ok(());
                };
                let frame = pool.TryGetNextFrame()?;
                let content = frame.ContentSize()?;
                if content.Width <= 0 || content.Height <= 0 {
                    return Ok(());
                }
                let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
                let tex: ID3D11Texture2D = access.GetInterface()?;
                let mut sink = sink_cb.lock().unwrap_or_else(|e| e.into_inner());
                sink.deliver(&tex);
                drop(frame);
                // A window that changed size: new buffers of its size.
                let mut last = size_cb.lock().unwrap_or_else(|e| e.into_inner());
                if content.Width != last.Width || content.Height != last.Height {
                    let dxgi: IDXGIDevice = sink.device.cast()?;
                    let d3d: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;
                    pool.Recreate(&d3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, content)?;
                    *last = content;
                }
                Ok(())
            }))?;
            session.StartCapture()?;
            Ok((pool, session))
        };
        let mut live = Some(open()?);
        info!(
            "🖥️ [CAPTURA GPU] {} capturada por WGC na {}",
            if hwnd != 0 { "Janela" } else { "Tela" },
            gpu.name
        );
        if ready.send(true).is_err() {
            cancelled.store(true, Ordering::Release);
        }
        report(gpu, "WGC", &cancelled);
        // Windows Graphics Capture delivers every display refresh, and Windows 11 23H2 and
        // older cannot slow it down: with nobody watching and no window showing the local
        // preview (Litecord hidden behind a game), the session is closed, and opened again
        // when someone watches or the preview shows.
        let mut unwanted_since: Option<Instant> = None;
        let mut was_watched = false;
        while req.is_running.load(Ordering::Relaxed) && !cancelled.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(100));
            // Someone started watching: a new session sends the current picture at once,
            // while the open one only sends changes (a still screen sends nothing).
            let watched = super::manager::tx_watched();
            if watched && !was_watched {
                if let Some((pool, session)) = live.take() {
                    let _ = session.Close();
                    let _ = pool.Close();
                }
            }
            was_watched = watched;
            if frames_wanted() {
                unwanted_since = None;
                if live.is_none() {
                    match open() {
                        Ok(l) => {
                            info!("🖥️ [CAPTURA GPU] Captura WGC retomada");
                            live = Some(l);
                        }
                        Err(e) => warn!("⚠️ [CAPTURA GPU] Não foi possível retomar o WGC: {}", e),
                    }
                }
            } else if live.is_some() && unwanted_since.get_or_insert_with(Instant::now).elapsed() >= IDLE_CLOSE_AFTER {
                if let Some((pool, session)) = live.take() {
                    let _ = session.Close();
                    let _ = pool.Close();
                    info!("🖥️ [CAPTURA GPU] Captura WGC pausada: ninguém assistindo e a prévia escondida");
                }
            }
        }
        if let Some((pool, session)) = live.take() {
            let _ = session.Close();
            let _ = pool.Close();
        }
    }
    Ok(())
}

/// Takes captured textures: to the GPU send path, or copied to memory for the CPU path.
struct Sink {
    cancelled: Arc<AtomicBool>,
    side: super::gpu_tx::CaptureSide,
    device: ID3D11Device,
    width: u32,
    height: u32,
    tx_frame: SyncSender<(Vec<u8>, u32, u32, u128, u128)>,
    rx_recycle: Arc<Mutex<Receiver<Vec<u8>>>>,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    last_copy: Option<Instant>,
    last_delivery: Option<Instant>,
    frame_interval: Duration,
}

// The device is multithread-protected; the sink is used by one capture callback at a time.
unsafe impl Send for Sink {}

impl Sink {
    fn new(req: &Request, device: &ID3D11Device, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            cancelled,
            side: super::gpu_tx::CaptureSide::new(req.width, req.height),
            device: device.clone(),
            width: req.width,
            height: req.height,
            tx_frame: req.tx_frame.clone(),
            rx_recycle: req.rx_recycle.clone(),
            staging: None,
            last_copy: None,
            last_delivery: None,
            frame_interval: Duration::from_nanos(1_000_000_000 / req.fps.max(1)),
        }
    }

    fn deliver(&mut self, tex: &ID3D11Texture2D) {
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        // WGC can notify at the monitor's refresh rate. Bound processing by the
        // requested stream rate (or preview rate while nobody watches).
        let pace = super::manager::capture_pace(self.frame_interval)
            .saturating_sub(Duration::from_millis(1));
        if self.last_delivery.is_some_and(|t| t.elapsed() < pace) {
            return;
        }
        self.last_delivery = Some(Instant::now());
        let watched = super::manager::tx_watched();
        if self.side.handle_typed(&self.device, tex, watched) {
            return;
        }
        // CPU path: while nobody watches, only as often as the local preview needs.
        if !watched
            && self
                .last_copy
                .is_some_and(|t| t.elapsed() < super::manager::IDLE_FRAME_INTERVAL)
        {
            return;
        }
        self.last_copy = Some(Instant::now());
        if let Err(e) = unsafe { self.copy_to_memory(tex) } {
            log::debug!("captura: cópia para a memória falhou: {e}");
        }
    }

    unsafe fn copy_to_memory(&mut self, tex: &ID3D11Texture2D) -> windows::core::Result<()> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        tex.GetDesc(&mut desc);
        if self
            .staging
            .as_ref()
            .map(|s| (s.1, s.2) != (desc.Width, desc.Height))
            .unwrap_or(true)
        {
            let sd = D3D11_TEXTURE2D_DESC {
                Width: desc.Width,
                Height: desc.Height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut t = None;
            self.device.CreateTexture2D(&sd, None, Some(&mut t))?;
            self.staging = t.map(|t| (t, desc.Width, desc.Height));
        }
        let (staging, w, h) = self.staging.as_ref().unwrap();
        let ctx = self.device.GetImmediateContext()?;
        ctx.CopyResource(staging, tex);
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
        let src =
            std::slice::from_raw_parts(m.pData as *const u8, m.RowPitch as usize * *h as usize);
        let total = (self.width * self.height * 4) as usize;
        let mut buf = self
            .rx_recycle
            .lock()
            .ok()
            .and_then(|r| r.try_recv().ok())
            .unwrap_or_else(|| Vec::with_capacity(total));
        buf.resize(total, 0);
        super::video_pipeline::fit_bgra_to_canvas(src, *w, *h, self.width, self.height, &mut buf);
        ctx.Unmap(staging, 0);
        let _ = self.tx_frame.try_send((buf, self.width, self.height, 0, 0));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_values_map_to_choices() {
        assert_eq!(parse_choice("auto"), Choice::Auto);
        assert_eq!(parse_choice("openh264"), Choice::Cpu);
        assert_eq!(
            parse_choice("gpu:10de:25a2"),
            Choice::Gpu {
                vendor: 0x10de,
                device: 0x25a2
            }
        );
        // Older versions stored an encoder name: its GPU.
        assert_eq!(
            parse_choice("nvenc"),
            Choice::Gpu {
                vendor: 0x10de,
                device: 0
            }
        );
        assert_eq!(
            parse_choice("amf"),
            Choice::Gpu {
                vendor: 0x1002,
                device: 0
            }
        );
        assert_eq!(
            parse_choice("wmf"),
            Choice::Gpu {
                vendor: 0x8086,
                device: 0
            }
        );
        assert_eq!(
            parse_choice("ffmpeg"),
            Choice::Gpu {
                vendor: 0x10de,
                device: 0
            }
        );
        assert_eq!(parse_choice("gpu:10de:25a2:extra"), Choice::Auto);
        assert_eq!(parse_choice("gpu:10000:25a2"), Choice::Auto);
        assert_eq!(parse_choice("gpu:zz"), Choice::Auto);
        assert_eq!(parse_choice(""), Choice::Auto);
    }
}
