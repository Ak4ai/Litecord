#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::net::{SocketAddr, UdpSocket, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(not(windows))]
use std::sync::atomic::AtomicU8;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use log::{info, warn};
use slint::{Rgba8Pixel, SharedPixelBuffer};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const P2P_VIDEO_PORT: u16 = 50005;
const MAGIC: &[u8; 4] = b"LTPV";

static PROCESS_INSTANCE_ID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

pub fn get_process_instance_id() -> u32 {
    *PROCESS_INSTANCE_ID.get_or_init(|| {
        use std::time::SystemTime;
        let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
        let pid = std::process::id();
        ((nanos as u32) ^ (pid << 16) ^ (nanos >> 32) as u32) | 1
    })
}

static MY_RX_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(P2P_VIDEO_PORT);

pub fn get_my_rx_port() -> u16 {
    MY_RX_PORT.load(Ordering::Relaxed)
}

pub fn set_my_rx_port(port: u16) {
    MY_RX_PORT.store(port, Ordering::Relaxed);
}

static CURRENT_TX_FPS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static KEYFRAME_REQUESTED: AtomicBool = AtomicBool::new(false);
static REQUESTED_BITRATE_BPS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(6_000_000);

pub fn request_intra_keyframe() {
    KEYFRAME_REQUESTED.store(true, Ordering::Relaxed);
}

pub fn request_bitrate_adjustment(bitrate_bps: u32) {
    REQUESTED_BITRATE_BPS.store(bitrate_bps.clamp(1_500_000, 12_000_000), Ordering::Relaxed);
}

pub fn get_current_bitrate_bps() -> u32 {
    REQUESTED_BITRATE_BPS.load(Ordering::Relaxed)
}

pub fn get_tx_fps() -> f32 {
    let raw = CURRENT_TX_FPS.load(Ordering::Relaxed);
    (raw as f32) / 10.0
}

static TX_EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
static STREAM_AUDIO_CLOCK_PTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static STREAM_AUDIO_CLOCK_UPDATE: std::sync::OnceLock<Arc<Mutex<Option<Instant>>>> = std::sync::OnceLock::new();

pub fn get_tx_pts_ms() -> u32 {
    let epoch = TX_EPOCH.get_or_init(Instant::now);
    epoch.elapsed().as_millis() as u32
}

pub fn get_estimated_audio_pts() -> u32 {
    let base_pts = STREAM_AUDIO_CLOCK_PTS.load(Ordering::Relaxed);
    let cell = STREAM_AUDIO_CLOCK_UPDATE.get_or_init(|| Arc::new(Mutex::new(None)));
    if let Ok(guard) = cell.lock() {
        if let Some(last_time) = *guard {
            let elapsed = last_time.elapsed().as_millis() as u32;
            return base_pts.wrapping_add(elapsed);
        }
    }
    base_pts
}

pub fn update_audio_clock_pts(pts_ms: u32) {
    STREAM_AUDIO_CLOCK_PTS.store(pts_ms, Ordering::Relaxed);
    let cell = STREAM_AUDIO_CLOCK_UPDATE.get_or_init(|| Arc::new(Mutex::new(None)));
    if let Ok(mut guard) = cell.lock() {
        *guard = Some(Instant::now());
    }
}

// Litecord P2P Video
const CHUNK_SIZE: usize = 1350; // Optimized MTU for local & VPN UDP transmission without fragmentation

// Protocol Opcodes
const OP_ANNOUNCE: u8 = 1;
const OP_VIDEO_CHUNK: u8 = 2;
const OP_STOP: u8 = 3;
const OP_HEARTBEAT: u8 = 4;
pub const OP_AUDIO_FRAME: u8 = 5;
pub const OP_KEYFRAME_REQ: u8 = 6;
pub const OP_FEC_PARITY: u8 = 7;
pub const OP_QOS_FEEDBACK: u8 = 8;

#[derive(Debug, Clone)]
pub struct MonitorItemInfo {
    pub id: i32,
    pub name: String,
    pub resolution: String,
    pub is_primary: bool,
    pub hwnd: isize,
}

#[derive(Debug, Clone)]
pub struct CapturableWindowItem {
    pub id: String,
    pub title: String,
    pub app_name: String,
    pub icon_rgba: Option<(u32, u32, Vec<u8>)>,
}

#[derive(Debug, Clone)]
pub struct CameraItemInfo {
    pub id: String,
    pub name: String,
    pub index: u32,
}

pub fn list_cameras() -> Vec<CameraItemInfo> {
    let mut result = Vec::new();
    if let Ok(devs) = cameras::devices() {
        for (i, dev) in devs.into_iter().enumerate() {
            let test_config_mjpeg = cameras::StreamConfig {
                resolution: cameras::Resolution { width: 640, height: 480 },
                framerate: 30,
                pixel_format: cameras::PixelFormat::Mjpeg,
            };
            let can_open = cameras::open(&dev, test_config_mjpeg).is_ok() || {
                let test_config_yuyv = cameras::StreamConfig {
                    resolution: cameras::Resolution { width: 640, height: 480 },
                    framerate: 30,
                    pixel_format: cameras::PixelFormat::Yuyv,
                };
                cameras::open(&dev, test_config_yuyv).is_ok()
            };

            if can_open || cfg!(windows) {
                let name = dev.name.clone();
                result.push(CameraItemInfo {
                    id: format!("{}", i),
                    name,
                    index: i as u32,
                });
            }
        }
    }
    result
}

/// Thread-safe shared frame buffer for double-buffered local stream rendering.
pub struct SharedFrameBuffer {
    frame: Mutex<Option<SharedPixelBuffer<Rgba8Pixel>>>,
    dirty: AtomicBool,
}

impl SharedFrameBuffer {
    pub fn new() -> Self {
        Self {
            frame: Mutex::new(None),
            dirty: AtomicBool::new(false),
        }
    }

    pub fn publish(&self, buffer: SharedPixelBuffer<Rgba8Pixel>) {
        if let Ok(mut guard) = self.frame.lock() {
            *guard = Some(buffer);
            self.dirty.store(true, Ordering::Release);
        }
    }

    pub fn consume(&self) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        if !self.dirty.swap(false, Ordering::AcqRel) {
            return None;
        }
        if let Ok(guard) = self.frame.lock() {
            guard.clone()
        } else {
            None
        }
    }
}

pub struct ScreenCaptureManager {
    is_running: Arc<AtomicBool>,
    is_receiver_running: Arc<AtomicBool>,
    channel_id: Arc<AtomicU64>,
    my_user_id: Arc<AtomicU64>,
    my_username: Arc<Mutex<String>>,
    known_peers: Arc<Mutex<HashMap<u64, (SocketAddr, Instant)>>>,
    shared_buffer: Arc<SharedFrameBuffer>,
}

impl ScreenCaptureManager {
    pub fn new() -> Self {
        Self {
            is_running: Arc::new(AtomicBool::new(false)),
            is_receiver_running: Arc::new(AtomicBool::new(false)),
            channel_id: Arc::new(AtomicU64::new(0)),
            my_user_id: Arc::new(AtomicU64::new(0)),
            my_username: Arc::new(Mutex::new(String::new())),
            known_peers: Arc::new(Mutex::new(HashMap::new())),
            shared_buffer: Arc::new(SharedFrameBuffer::new()),
        }
    }

    pub fn set_context(&self, channel_id: u64, my_user_id: u64, my_username: &str) {
        self.channel_id.store(channel_id, Ordering::Relaxed);
        self.my_user_id.store(my_user_id, Ordering::Relaxed);
        if let Ok(mut uname) = self.my_username.lock() {
            *uname = my_username.to_string();
        }
    }

    pub fn is_active(&self) -> bool {
        self.is_running.load(Ordering::Relaxed)
    }

    pub fn announce_presence(&self) {
        let current_cid = self.channel_id.load(Ordering::Relaxed);
        if current_cid == 0 { return; }
        let mut my_uid = self.my_user_id.load(Ordering::Relaxed);
        if my_uid == 0 {
            my_uid = crate::gateway::get_my_user_id();
            if my_uid > 0 {
                self.my_user_id.store(my_uid, Ordering::Relaxed);
            }
        }
        let my_instance_id = get_process_instance_id();
        let my_rx = get_my_rx_port();
        let mut uname = self.my_username.lock().unwrap().clone();
        if uname.is_empty() {
            uname = crate::gateway::get_my_username();
            if !uname.is_empty() {
                *self.my_username.lock().unwrap() = uname.clone();
            }
        }
        let uname_bytes = uname.as_bytes();

        let socket_opt = get_shared_p2p_socket().or_else(|| {
            UdpSocket::bind("0.0.0.0:0").ok().map(|s| {
                let _ = s.set_broadcast(true);
                Arc::new(s)
            })
        });

        if let Some(socket) = socket_opt {
            let mut ack_pkt = Vec::with_capacity(30 + uname_bytes.len());
            ack_pkt.extend_from_slice(MAGIC);
            ack_pkt.extend_from_slice(&my_instance_id.to_be_bytes());
            ack_pkt.push(OP_HEARTBEAT);
            ack_pkt.extend_from_slice(&current_cid.to_be_bytes());
            ack_pkt.extend_from_slice(&my_uid.to_be_bytes());
            ack_pkt.push(0);
            ack_pkt.push(2);
            ack_pkt.push(uname_bytes.len() as u8);
            ack_pkt.extend_from_slice(uname_bytes);
            ack_pkt.extend_from_slice(&my_rx.to_be_bytes());

            for target in get_broadcast_addresses() {
                let _ = socket.send_to(&ack_pkt, target);
            }
            if let Ok(peers) = self.known_peers.lock() {
                for (&_, &(addr, _)) in peers.iter() {
                    let _ = socket.send_to(&ack_pkt, addr);
                }
            }
        }
    }

    pub fn register_remote_peer(&self, uid: u64, addr: SocketAddr) {
        if let Ok(mut peers) = self.known_peers.lock() {
            peers.insert(uid, (addr, Instant::now()));
            info!("🌐 Peer remoto P2P registrado: User ID {} -> {}", uid, addr);
        }
        let current_cid = self.channel_id.load(Ordering::Relaxed);
        let my_uid = self.my_user_id.load(Ordering::Relaxed);
        let my_instance_id = get_process_instance_id();
        let my_rx = get_my_rx_port();
        let uname = self.my_username.lock().unwrap().clone();
        let uname_bytes = uname.as_bytes();

        let socket_opt = get_shared_p2p_socket().or_else(|| {
            UdpSocket::bind("0.0.0.0:0").ok().map(|s| {
                let _ = s.set_broadcast(true);
                Arc::new(s)
            })
        });

        if let Some(socket) = socket_opt {
            let mut ack_pkt = Vec::with_capacity(30 + uname_bytes.len());
            ack_pkt.extend_from_slice(MAGIC);
            ack_pkt.extend_from_slice(&my_instance_id.to_be_bytes());
            ack_pkt.push(OP_HEARTBEAT);
            ack_pkt.extend_from_slice(&current_cid.to_be_bytes());
            ack_pkt.extend_from_slice(&my_uid.to_be_bytes());
            ack_pkt.push(0);
            ack_pkt.push(2);
            ack_pkt.push(uname_bytes.len() as u8);
            ack_pkt.extend_from_slice(uname_bytes);
            ack_pkt.extend_from_slice(&my_rx.to_be_bytes());

            for _ in 0..3 {
                let _ = socket.send_to(&ack_pkt, addr);
            }
        }
    }

    pub fn shared_buffer(&self) -> Arc<SharedFrameBuffer> {
        Arc::clone(&self.shared_buffer)
    }

    pub fn stop(&self) {
        stop_window_border_overlay();
        #[cfg(not(windows))]
        {
            PORTAL_INITIALIZED.store(false, Ordering::SeqCst);
            if let Ok(mut slot) = PORTAL_FRAME.lock() {
                *slot = None;
            }
            #[cfg(target_os = "linux")]
            {
                if let Ok(mut lock) = PORTAL_CHILD.lock() {
                    if let Some(mut child) = lock.take() {
                        let _ = child.kill();
                    }
                }
                if let Ok(mut cb_slot) = PORTAL_LOCAL_CB.lock() {
                    *cb_slot = None;
                }
            }
        }
        if self.is_running.swap(false, Ordering::SeqCst) {
            CURRENT_TX_FPS.store(0, Ordering::Relaxed);
            info!("🛑 Parando captura e transmissão de tela P2P...");
            let mut cid = self.channel_id.load(Ordering::Relaxed);
            if cid == 0 {
                cid = crate::gateway::get_my_voice_channel_id();
            }
            let mut uid = self.my_user_id.load(Ordering::Relaxed);
            if uid == 0 {
                uid = crate::gateway::get_my_user_id();
            }
            let bcast_targets = get_broadcast_addresses();

            if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
                let _ = socket.set_broadcast(true);
                let inst = get_process_instance_id();
                let mut stop_pkt = Vec::with_capacity(25);
                stop_pkt.extend_from_slice(MAGIC);
                stop_pkt.extend_from_slice(&inst.to_be_bytes());
                stop_pkt.push(OP_STOP);
                stop_pkt.extend_from_slice(&cid.to_be_bytes());
                stop_pkt.extend_from_slice(&uid.to_be_bytes());

                for _ in 0..5 {
                    for target in &bcast_targets {
                        let _ = socket.send_to(&stop_pkt, target);
                    }
                    if let Ok(peers) = self.known_peers.lock() {
                        for (&_, &(addr, _)) in peers.iter() {
                            let _ = socket.send_to(&stop_pkt, addr);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }

    /// Starts the capture and UDP transmitter thread
    pub fn start<F>(&self, target_hwnd: isize, screen_index: usize, camera_index: Option<u32>, res: i32, fps: i32, include_audio: bool, on_local_frame: F)
    where
        F: Fn(SharedPixelBuffer<Rgba8Pixel>) + Send + Sync + 'static,
    {
        if self.is_running.swap(true, Ordering::SeqCst) {
            warn!("Transmissão de tela já está em execução.");
            return;
        }

        let on_local_arc: Arc<dyn Fn(SharedPixelBuffer<Rgba8Pixel>) + Send + Sync + 'static> = Arc::new(on_local_frame);
        #[cfg(target_os = "linux")]
        {
            if let Ok(mut cb_slot) = PORTAL_LOCAL_CB.lock() {
                *cb_slot = Some(Arc::clone(&on_local_arc));
            }
        }
        let on_local_tx = Arc::clone(&on_local_arc);

        let is_running = Arc::clone(&self.is_running);
        let channel_id_atomic = Arc::clone(&self.channel_id);
        let my_user_id_atomic = Arc::clone(&self.my_user_id);
        let my_username_arc = Arc::clone(&self.my_username);
        let peers_store = Arc::clone(&self.known_peers);
        let buffer = Arc::clone(&self.shared_buffer);

        let (target_w, target_h) = match res {
            1080 => (1920u32, 1080u32),
            480 => (854u32, 480u32),
            _ => (1280u32, 720u32),
        };
        // Uncapped 60 FPS across all resolutions thanks to lightweight H.264 temporal compression
        let target_fps = (fps.clamp(15, 60)) as u64;

        if camera_index.is_none() && target_hwnd != 0 {
            start_window_border_overlay(target_hwnd);
        }

        // Camera transmission is strictly without system loopback audio
        if camera_index.is_none() && include_audio {
            start_audio_loopback_tx(
                Arc::clone(&self.is_running),
                Arc::clone(&self.channel_id),
                Arc::clone(&self.my_user_id),
                Arc::clone(&self.known_peers),
            );
        }

        info!("🖥️ Iniciando transmissão P2P H.264 ({}x{} @ {} FPS, screen_idx={}, hwnd={}, cam={:?}, audio={})...", target_w, target_h, target_fps, screen_index, target_hwnd, camera_index, camera_index.is_none() && include_audio);

        std::thread::Builder::new()
            .name("screen-capture-tx".to_string())
            .spawn(move || {
                #[cfg(windows)]
                unsafe {
                    windows_sys::Win32::Media::timeBeginPeriod(1);
                }

                let mut h264_encoder: Option<Box<dyn crate::gpu_encoder::VideoEncoder>> = Some(
                    crate::gpu_encoder::create_best_encoder(target_fps as u32, camera_index.is_none())
                );

                let socket = get_shared_p2p_socket().unwrap_or_else(|| {
                    let s = UdpSocket::bind("0.0.0.0:0").unwrap();
                    let _ = s.set_broadcast(true);
                    let _ = s.set_nonblocking(true);
                    Arc::new(s)
                });

                let camera_handle = if let Some(cam_idx) = camera_index {
                    if let Ok(devs) = cameras::devices() {
                        if let Some(dev) = devs.into_iter().nth(cam_idx as usize) {
                            let cam_w = target_w.min(1280);
                            let cam_h = target_h.min(720);
                            let cam_fps = (target_fps as u32).min(60);
                            let mut opened = None;
                            let formats = [
                                cameras::PixelFormat::Mjpeg,
                                cameras::PixelFormat::Yuyv,
                                cameras::PixelFormat::Nv12,
                                cameras::PixelFormat::Bgra8,
                                cameras::PixelFormat::Rgb8,
                                cameras::PixelFormat::Rgba8,
                            ];
                            let resolutions = [
                                cameras::Resolution { width: cam_w, height: cam_h },
                                cameras::Resolution { width: 1280, height: 720 },
                                cameras::Resolution { width: 640, height: 480 },
                                cameras::Resolution { width: 640, height: 360 },
                            ];
                            let framerates = [cam_fps, 30, 15, 10];
                            'outer: for fmt in formats {
                                for res_test in resolutions {
                                    for fps_test in framerates {
                                        let config = cameras::StreamConfig {
                                            resolution: res_test,
                                            framerate: fps_test,
                                            pixel_format: fmt,
                                        };
                                        if let Ok(cam) = cameras::open(&dev, config) {
                                            info!("📷 Câmera '{}' aberta com sucesso ({}x{} @ {} FPS, {:?})!", dev.name, res_test.width, res_test.height, fps_test, fmt);
                                            opened = Some(cam);
                                            break 'outer;
                                        }
                                    }
                                }
                            }
                            if opened.is_none() {
                                warn!("Falha ao abrir câmera '{}' em todos os formatos de pixel testados.", dev.name);
                            }
                            opened
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                let bcast_targets = get_broadcast_addresses();
                let (tx_frame, rx_frame) = std::sync::mpsc::sync_channel::<(Vec<u8>, u32, u32, u128, u128)>(6);
                let (tx_recycle, rx_recycle) = std::sync::mpsc::sync_channel::<Vec<u8>>(6);
                let is_running_cap = Arc::clone(&is_running);

                // =========================================================================
                // ESTÁGIO 1: Captura Windows Graphics Capture (WGC - GPU Direct 60-100 FPS)
                // =========================================================================
                #[cfg(windows)]
                let wgc_started = if camera_index.is_none() {
                    use windows_capture::{
                        capture::{Context, GraphicsCaptureApiHandler},
                        frame::Frame,
                        graphics_capture_api::InternalCaptureControl,
                        monitor::Monitor,
                        window::Window,
                        settings::{
                            ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
                            MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
                        },
                    };

                    struct WgcHandler {
                        tx_frame: std::sync::mpsc::SyncSender<(Vec<u8>, u32, u32, u128, u128)>,
                        rx_recycle: Arc<std::sync::Mutex<std::sync::mpsc::Receiver<Vec<u8>>>>,
                        is_running: Arc<AtomicBool>,
                        target_w: u32,
                        target_h: u32,
                    }

                    struct WgcFlags {
                        tx_frame: std::sync::mpsc::SyncSender<(Vec<u8>, u32, u32, u128, u128)>,
                        rx_recycle: Arc<std::sync::Mutex<std::sync::mpsc::Receiver<Vec<u8>>>>,
                        is_running: Arc<AtomicBool>,
                        target_fps: u64,
                        target_w: u32,
                        target_h: u32,
                    }

                    impl GraphicsCaptureApiHandler for WgcHandler {
                        type Flags = WgcFlags;
                        type Error = Box<dyn std::error::Error + Send + Sync>;

                        fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
                            Ok(Self {
                                tx_frame: ctx.flags.tx_frame,
                                rx_recycle: ctx.flags.rx_recycle,
                                is_running: ctx.flags.is_running,
                                target_w: ctx.flags.target_w,
                                target_h: ctx.flags.target_h,
                            })
                        }

                        fn on_frame_arrived(
                            &mut self,
                            frame: &mut Frame,
                            capture_control: InternalCaptureControl,
                        ) -> Result<(), Self::Error> {
                            if !self.is_running.load(Ordering::Relaxed) {
                                capture_control.stop();
                                return Ok(());
                            }

                            let (w, h) = (frame.width(), frame.height());
                            if let Ok(mut fb) = frame.buffer() {
                                let slice = fb.as_raw_buffer();
                                let canvas_w = self.target_w;
                                let canvas_h = self.target_h;
                                let total_canvas_bytes = (canvas_w * canvas_h * 4) as usize;

                                let mut cur_buf = if let Ok(rx) = self.rx_recycle.lock() {
                                    rx.try_recv().unwrap_or_else(|_| Vec::with_capacity(total_canvas_bytes))
                                } else {
                                    Vec::with_capacity(total_canvas_bytes)
                                };
                                if cur_buf.len() != total_canvas_bytes {
                                    cur_buf.resize(total_canvas_bytes, 0);
                                }

                                fit_bgra_to_canvas(slice, w, h, canvas_w, canvas_h, &mut cur_buf);
                                let _ = self.tx_frame.try_send((cur_buf, canvas_w, canvas_h, 0, 0));
                            }
                            Ok(())
                        }

                        fn on_closed(&mut self) -> Result<(), Self::Error> {
                            Ok(())
                        }
                    }

                    let rx_recycle_arc = Arc::new(std::sync::Mutex::new(rx_recycle));
                    let flags = WgcFlags {
                        tx_frame: tx_frame.clone(),
                        rx_recycle: Arc::clone(&rx_recycle_arc),
                        is_running: Arc::clone(&is_running),
                        target_fps,
                        target_w,
                        target_h,
                    };

                    let tx_frame_fallback = tx_frame.clone();
                    let is_running_fallback = Arc::clone(&is_running);
                    let mut started = false;

                    if target_hwnd != 0 {
                        let win = Window::from_raw_hwnd(target_hwnd as *mut std::ffi::c_void);
                        let settings = Settings::new(
                            win,
                            CursorCaptureSettings::Default,
                            DrawBorderSettings::WithoutBorder,
                            SecondaryWindowSettings::Default,
                            MinimumUpdateIntervalSettings::Default,
                            DirtyRegionSettings::Default,
                            ColorFormat::Bgra8,
                            flags,
                        );
                        std::thread::Builder::new()
                            .name("wgc-capture-thread".to_string())
                            .spawn(move || {
                                info!("⚡ Iniciando Windows Graphics Capture (WGC) para Janela HWND={}", target_hwnd);
                                if let Err(e) = WgcHandler::start(settings) {
                                    warn!("⚠️ WGC falhou ({:?}), acionando fallback GDI...", e);
                                    let mut cur_buf = Vec::with_capacity((target_w * target_h * 4) as usize);
                                    let frame_interval = Duration::from_nanos(1_000_000_000 / target_fps.max(1));
                                    while is_running_fallback.load(Ordering::Relaxed) {
                                        let t_start = Instant::now();
                                        if let Some((blt, pix)) = capture_screen_rgb(target_hwnd, target_w, target_h, target_fps, &mut cur_buf) {
                                            let _ = tx_frame_fallback.try_send((cur_buf.clone(), target_w, target_h, blt, pix));
                                        }
                                        let el = t_start.elapsed();
                                        if el < frame_interval {
                                            std::thread::sleep(frame_interval - el);
                                        }
                                    }
                                }
                            })
                            .ok();
                        started = true;
                    } else {
                        let monitor_opt = if let Ok(monitors) = Monitor::enumerate() {
                            monitors.into_iter().nth(screen_index).or_else(|| Monitor::primary().ok())
                        } else {
                            Monitor::primary().ok()
                        };

                        if let Some(monitor) = monitor_opt {
                            let mon_name = monitor.name().unwrap_or_else(|_| format!("Monitor #{}", screen_index + 1));
                            use windows_capture::dxgi_duplication_api::DxgiDuplicationApi;

                            let tx_frame_dxgi = tx_frame.clone();
                            let rx_recycle_dxgi = Arc::clone(&rx_recycle_arc);
                            let is_running_dxgi = Arc::clone(&is_running);
                            let tx_frame_fallback_inner = tx_frame_fallback.clone();
                            let is_running_fallback_inner = Arc::clone(&is_running_fallback);

                            std::thread::Builder::new()
                                .name("dxgi-duplication-thread".to_string())
                                .spawn(move || {
                                    info!("⚡ [SUNSHINE GPU ENGINE] Tentando DXGI Desktop Duplication Direto na GPU para {}...", mon_name);
                                    let mut dup_opt = DxgiDuplicationApi::new(monitor).ok();
                                    let dxgi_ok = dup_opt.is_some();

                                    if dxgi_ok {
                                        info!("🚀 [SUNSHINE GPU ENGINE] DXGI Desktop Duplication ATIVADO com sucesso na GPU! (Zero-Copy VRAM 60 FPS)");
                                        let canvas_w = target_w;
                                        let canvas_h = target_h;
                                        let total_canvas_bytes = (canvas_w * canvas_h * 4) as usize;
                                        let frame_interval = Duration::from_nanos(1_000_000_000 / target_fps.max(1));

                                        while is_running_dxgi.load(Ordering::Relaxed) {
                                            let t_start = Instant::now();
                                            if let Some(mut dup) = dup_opt.take() {
                                                match dup.acquire_next_frame(20) {
                                                    Ok(mut frame) => {
                                                        let (w, h) = (frame.width(), frame.height());
                                                        if let Ok(mut fb) = frame.buffer() {
                                                            let slice = fb.as_raw_buffer();
                                                            let mut cur_buf = if let Ok(rx) = rx_recycle_dxgi.lock() {
                                                                rx.try_recv().unwrap_or_else(|_| Vec::with_capacity(total_canvas_bytes))
                                                            } else {
                                                                Vec::with_capacity(total_canvas_bytes)
                                                            };
                                                            if cur_buf.len() != total_canvas_bytes {
                                                                cur_buf.resize(total_canvas_bytes, 0);
                                                            }

                                                            fit_bgra_to_canvas(slice, w, h, canvas_w, canvas_h, &mut cur_buf);
                                                            let _ = tx_frame_dxgi.try_send((cur_buf, canvas_w, canvas_h, 0, 0));
                                                        }
                                                        dup_opt = Some(dup);
                                                    }
                                                    Err(windows_capture::dxgi_duplication_api::Error::Timeout) => {
                                                        // Screen static: nothing to do, CFR pacer will handle frame rate
                                                        dup_opt = Some(dup);
                                                    }
                                                    Err(windows_capture::dxgi_duplication_api::Error::AccessLost) => {
                                                        warn!("⚠️ [DXGI GPU] Access Lost (troca de modo D3D11 / UAC), recriando sessão...");
                                                        dup_opt = dup.recreate().ok().or_else(|| DxgiDuplicationApi::new(monitor).ok());
                                                        if dup_opt.is_none() {
                                                            std::thread::sleep(Duration::from_millis(50));
                                                        }
                                                    }
                                                    Err(e) => {
                                                        warn!("⚠️ [DXGI GPU] Erro na duplicação DXGI ({:?}), tentando recriar...", e);
                                                        dup_opt = dup.recreate().ok().or_else(|| DxgiDuplicationApi::new(monitor).ok());
                                                        if dup_opt.is_none() {
                                                            std::thread::sleep(Duration::from_millis(50));
                                                        }
                                                    }
                                                }
                                            } else {
                                                dup_opt = DxgiDuplicationApi::new(monitor).ok();
                                                if dup_opt.is_none() {
                                                    std::thread::sleep(Duration::from_millis(100));
                                                }
                                            }

                                            let el = t_start.elapsed();
                                            if el < frame_interval {
                                                std::thread::sleep(frame_interval - el);
                                            }
                                        }
                                    }

                                    if !dxgi_ok {
                                        warn!("⚠️ [DXGI GPU] DXGI Duplication indisponível para este monitor, iniciando Windows Graphics Capture (WGC)...");
                                        let settings = Settings::new(
                                            monitor,
                                            CursorCaptureSettings::Default,
                                            DrawBorderSettings::WithoutBorder,
                                            SecondaryWindowSettings::Default,
                                            MinimumUpdateIntervalSettings::Default,
                                            DirtyRegionSettings::Default,
                                            ColorFormat::Bgra8,
                                            flags,
                                        );
                                        if let Err(e) = WgcHandler::start(settings) {
                                            warn!("⚠️ WGC falhou ({:?}), acionando fallback GDI...", e);
                                            let mut cur_buf = Vec::with_capacity((target_w * target_h * 4) as usize);
                                            let frame_interval = Duration::from_nanos(1_000_000_000 / target_fps.max(1));
                                            while is_running_fallback_inner.load(Ordering::Relaxed) {
                                                let t_start = Instant::now();
                                                if let Some((blt, pix)) = capture_screen_rgb(0, target_w, target_h, target_fps, &mut cur_buf) {
                                                    let _ = tx_frame_fallback_inner.try_send((cur_buf.clone(), target_w, target_h, blt, pix));
                                                }
                                                let el = t_start.elapsed();
                                                if el < frame_interval {
                                                    std::thread::sleep(frame_interval - el);
                                                }
                                            }
                                        }
                                    }
                                })
                                .ok();
                            started = true;
                        }
                    }
                    started
                } else {
                    false
                };

                #[cfg(not(windows))]
                let wgc_started = false;

                if !wgc_started {
                    // Fallback para Câmera ou Captura Tradicional
                    std::thread::Builder::new()
                        .name("screen-capture-worker".to_string())
                        .spawn(move || {
                            #[cfg(windows)]
                            unsafe {
                                windows_sys::Win32::Media::timeBeginPeriod(1);
                            }

                            let frame_interval = Duration::from_nanos(1_000_000_000 / (target_fps as u64).max(1));
                            let mut next_cap_time = Instant::now() + frame_interval;

                            while is_running_cap.load(Ordering::Relaxed) {
                                let mut cur_buf = Vec::with_capacity((target_w * target_h * 4) as usize);

                                let cap_res = if camera_index.is_some() {
                                    if let Some(ref cam) = camera_handle {
                                        if let Ok(frame) = cameras::next_frame(cam, Duration::from_millis(50)) {
                                            let (w, h) = (frame.width, frame.height);
                                            if let Ok(mut rgba_bytes) = cameras::to_rgba8(&frame) {
                                                for chunk in rgba_bytes.chunks_exact_mut(4) {
                                                    chunk.swap(0, 2); // RGBA -> BGRA
                                                }
                                                cur_buf = rgba_bytes;
                                                Some((w, h, 0u128, 0u128))
                                            } else {
                                                None
                                            }
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    }
                                } else {
                                    capture_screen_rgb(target_hwnd, target_w, target_h, target_fps, &mut cur_buf)
                                        .map(|(blt, pix)| (target_w, target_h, blt, pix))
                                };

                                if let Some((w, h, blt, pix)) = cap_res {
                                    let _ = tx_frame.try_send((cur_buf, w, h, blt, pix));
                                }

                                let now = Instant::now();
                                if now < next_cap_time {
                                    let rem = next_cap_time - now;
                                    if rem > Duration::from_millis(3) {
                                        std::thread::sleep(rem - Duration::from_millis(2));
                                    }
                                    while Instant::now() < next_cap_time {
                                        std::hint::spin_loop();
                                    }
                                    next_cap_time += frame_interval;
                                } else {
                                    if now - next_cap_time > Duration::from_millis(100) {
                                        next_cap_time = now + frame_interval;
                                    } else {
                                        next_cap_time += frame_interval;
                                    }
                                }
                            }

                            #[cfg(windows)]
                            unsafe {
                                windows_sys::Win32::Media::timeEndPeriod(1);
                            }
                        })
                        .ok();
                }

                // =========================================================================
                // ESTÁGIO 2: Worker de Encode OpenH264 e Emissão UDP (Em Paralelo)
                // =========================================================================
                let mut tx_frame_count = 0u64;
                let mut last_tx_stats = std::time::Instant::now();
                let mut total_encode_us = 0u128;
                let mut total_blt_us = 0u128;
                let mut total_pix_us = 0u128;
                let mut last_local_preview = Instant::now() - Duration::from_secs(1);
                let mut last_announce = Instant::now() - Duration::from_secs(10);
                let mut last_idr = Instant::now();
                let mut frame_seq: u32 = 0;

                let frame_target_interval = Duration::from_micros(1_000_000 / target_fps.max(1));
                let mut next_tick = Instant::now();
                let mut latest_cached_frame: Option<(Vec<u8>, u32, u32, u128, u128)> = None;

                while is_running.load(Ordering::Relaxed) {
                    let cid = channel_id_atomic.load(Ordering::Relaxed);
                    let uid = my_user_id_atomic.load(Ordering::Relaxed);

                    // Announce presence periodically
                    if last_announce.elapsed() > Duration::from_millis(1500) {
                        let uname = my_username_arc.lock().unwrap().clone();
                        let uname_bytes = uname.as_bytes();
                        let my_rx = get_my_rx_port();
                        let inst = get_process_instance_id();
                        let mut ann_pkt = Vec::with_capacity(32 + uname_bytes.len());
                        ann_pkt.extend_from_slice(MAGIC);
                        ann_pkt.extend_from_slice(&inst.to_be_bytes());
                        ann_pkt.push(OP_ANNOUNCE);
                        ann_pkt.extend_from_slice(&cid.to_be_bytes());
                        ann_pkt.extend_from_slice(&uid.to_be_bytes());
                        ann_pkt.push(1);
                        ann_pkt.push(match res {
                            1080 => 108,
                            480 => 48,
                            _ => 72,
                        });
                        ann_pkt.push(target_fps as u8);
                        ann_pkt.push(uname_bytes.len() as u8);
                        ann_pkt.extend_from_slice(uname_bytes);
                        ann_pkt.extend_from_slice(&my_rx.to_be_bytes());

                        for target in &bcast_targets {
                            let _ = socket.send_to(&ann_pkt, target);
                        }
                        if let Ok(peers) = peers_store.lock() {
                            for (&_, &(addr, _)) in peers.iter() {
                                let _ = socket.send_to(&ann_pkt, addr);
                            }
                        }
                        last_announce = Instant::now();
                    }

                    // Drena quadros novos da GPU para sempre ter o frame mais atualizado
                    while let Ok(new_frame) = rx_frame.try_recv() {
                        if let Some((old_buf, _, _, _, _)) = latest_cached_frame.replace(new_frame) {
                            let _ = tx_recycle.try_send(old_buf);
                        }
                    }

                    // Se nenhum quadro chegou ainda, aguarda até 20ms ou usa fallback GDI imediato (vital para notebooks com GPUs híbridas)
                    if latest_cached_frame.is_none() {
                        match rx_frame.recv_timeout(Duration::from_millis(20)) {
                            Ok(first_frame) => {
                                latest_cached_frame = Some(first_frame);
                            }
                            Err(_) => {
                                #[cfg(windows)]
                                {
                                    let mut cur_buf = Vec::with_capacity((target_w * target_h * 4) as usize);
                                    if let Some((blt, pix)) = capture_screen_rgb(target_hwnd, target_w, target_h, target_fps, &mut cur_buf) {
                                        latest_cached_frame = Some((cur_buf, target_w, target_h, blt, pix));
                                    } else {
                                        continue;
                                    }
                                }
                                #[cfg(not(windows))]
                                continue;
                            }
                        }
                    }

                    // Pacer CFR de 60.0 FPS rígido (Relógio de precisão sem drift de encode para 60 FPS cravados)
                    let now = Instant::now();
                    if now < next_tick {
                        let sleep_dur = (next_tick - now).min(frame_target_interval);
                        if sleep_dur > Duration::from_millis(1) {
                            std::thread::sleep(sleep_dur - Duration::from_millis(1));
                        }
                        while Instant::now() < next_tick {
                            std::hint::spin_loop();
                        }
                    }
                    next_tick += frame_target_interval;
                    if now > next_tick + frame_target_interval * 2 {
                        next_tick = now + frame_target_interval;
                    }

                    let (bgra_slice, cur_w, cur_h, blt_us, pix_us) = match latest_cached_frame {
                        Some(ref f) => (f.0.as_slice(), f.1, f.2, f.3, f.4),
                        None => continue,
                    };

                    total_blt_us += blt_us;
                    total_pix_us += pix_us;

                    // Decouple UI local preview with fast downsampler (480w) to eliminate UI thread lag and memory overhead
                    if last_local_preview.elapsed() >= Duration::from_millis(50) {
                        if (camera_index.is_some() || cfg!(windows)) && cur_w > 0 && cur_h > 0 {
                            let prev_w = 480u32.min(cur_w);
                            let prev_h = (((prev_w as f32 / cur_w as f32) * (cur_h as f32)).round() as u32).max(1);

                            let mut pixel_buffer = SharedPixelBuffer::<Rgba8Pixel>::new(prev_w, prev_h);
                            let slice = pixel_buffer.make_mut_slice();

                            let x_step = ((cur_w as u64) << 16) / (prev_w as u64);
                            let y_step = ((cur_h as u64) << 16) / (prev_h as u64);

                            let src_u32: &[u32] = unsafe {
                                std::slice::from_raw_parts(bgra_slice.as_ptr() as *const u32, (cur_w * cur_h) as usize)
                            };

                            let mut src_y_accum = 0u64;
                            for dy in 0..prev_h {
                                let sy = ((src_y_accum >> 16) as usize).min((cur_h as usize).saturating_sub(1));
                                let src_row_start = sy * (cur_w as usize);
                                let src_row_end = (src_row_start + (cur_w as usize)).min(src_u32.len());
                                if src_row_start >= src_u32.len() {
                                    break;
                                }
                                let src_row = &src_u32[src_row_start..src_row_end];
                                let dst_row_start = (dy * prev_w) as usize;

                                let mut src_x_accum = 0u64;
                                for dx in 0..(prev_w as usize) {
                                    let sx = ((src_x_accum >> 16) as usize).min(src_row.len().saturating_sub(1));
                                    if sx < src_row.len() && dst_row_start + dx < slice.len() {
                                        let bgra_val = src_row[sx];
                                        let b = (bgra_val & 0xFF) as u8;
                                        let g = ((bgra_val >> 8) & 0xFF) as u8;
                                        let r = ((bgra_val >> 16) & 0xFF) as u8;
                                        slice[dst_row_start + dx] = Rgba8Pixel::new(r, g, b, 255);
                                    }
                                    src_x_accum += x_step;
                                }
                                src_y_accum += y_step;
                            }

                            buffer.publish(pixel_buffer.clone());
                            on_local_tx(pixel_buffer);
                        }
                        last_local_preview = Instant::now();
                    }

                    let enc_start = Instant::now();
                    let frame_bytes_opt = if let Some(ref mut enc) = h264_encoder {
                        let target_bitrate = REQUESTED_BITRATE_BPS.load(Ordering::Relaxed);
                        if enc.get_bitrate_bps() != target_bitrate {
                            enc.set_bitrate_bps(target_bitrate);
                        }
                        if KEYFRAME_REQUESTED.swap(false, Ordering::Relaxed) || last_idr.elapsed() >= Duration::from_millis(1000) {
                            last_idr = Instant::now();
                            enc.force_intra_frame();
                        }
                        enc.encode(bgra_slice, cur_w, cur_h)
                    } else {
                        let mut rgb = Vec::with_capacity((cur_w * cur_h * 3) as usize);
                        for p in bgra_slice.chunks_exact(4) {
                            rgb.push(p[2]);
                            rgb.push(p[1]);
                            rgb.push(p[0]);
                        }
                        encode_jpeg(&rgb, cur_w, cur_h, 55)
                    };
                    let enc_dur = enc_start.elapsed().as_micros();
                    total_encode_us += enc_dur;

                    if let Some(raw_frame_bytes) = frame_bytes_opt {
                        let sec_key = get_voice_encryption_key(cid);
                        let frame_bytes = encrypt_signaling_payload(&sec_key, &raw_frame_bytes).unwrap_or(raw_frame_bytes);
                        frame_seq = frame_seq.wrapping_add(1);
                        let total_len = frame_bytes.len();
                        let total_chunks = ((total_len + CHUNK_SIZE - 1) / CHUNK_SIZE) as u16;

                        // Send directly to active peers (or fallback to LAN broadcast if no peers found yet)
                        let mut target_addrs: Vec<SocketAddr> = Vec::with_capacity(4);
                        target_addrs.push(SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)), 50005));
                        let mut has_remote_peers = false;
                        if let Ok(mut peers) = peers_store.lock() {
                            let now = Instant::now();
                            peers.retain(|_, (addr, seen)| {
                                let is_private = match addr.ip() {
                                    std::net::IpAddr::V4(v4) => v4.is_private() || v4.is_loopback(),
                                    _ => false,
                                };
                                is_private || now.duration_since(*seen) < Duration::from_secs(60)
                            });
                            for (&_, &(addr, _)) in peers.iter() {
                                if !target_addrs.contains(&addr) {
                                    target_addrs.push(addr);
                                    has_remote_peers = true;
                                }
                            }
                        }
                        if !has_remote_peers {
                            for bcast in &bcast_targets {
                                if !target_addrs.contains(bcast) {
                                    target_addrs.push(*bcast);
                                }
                            }
                        }

                        // Compute Sunshine-grade Forward Error Correction (FEC) XOR parity
                        let fec_parity = if total_chunks > 1 {
                            let mut p = vec![0u8; CHUNK_SIZE];
                            for c_idx in 0..total_chunks {
                                let start = (c_idx as usize) * CHUNK_SIZE;
                                let end = (start + CHUNK_SIZE).min(total_len);
                                let slice = &frame_bytes[start..end];
                                for (i, &b) in slice.iter().enumerate() {
                                    p[i] ^= b;
                                }
                            }
                            Some(p)
                        } else {
                            None
                        };

                        let pts_ms = get_tx_pts_ms();

                        for chunk_idx in 0..total_chunks {
                            let start = (chunk_idx as usize) * CHUNK_SIZE;
                            let end = (start + CHUNK_SIZE).min(total_len);
                            let chunk_slice = &frame_bytes[start..end];

                            let inst = get_process_instance_id();
                            let mut pkt = Vec::with_capacity(37 + chunk_slice.len());
                            pkt.extend_from_slice(MAGIC);
                            pkt.extend_from_slice(&inst.to_be_bytes());
                            pkt.push(OP_VIDEO_CHUNK);
                            pkt.extend_from_slice(&cid.to_be_bytes());
                            pkt.extend_from_slice(&uid.to_be_bytes());
                            pkt.extend_from_slice(&frame_seq.to_be_bytes());
                            pkt.extend_from_slice(&pts_ms.to_be_bytes());
                            pkt.extend_from_slice(&total_chunks.to_be_bytes());
                            pkt.extend_from_slice(&chunk_idx.to_be_bytes());
                            pkt.extend_from_slice(chunk_slice);

                            for target in &target_addrs {
                                let _ = socket.send_to(&pkt, target);
                            }

                            // Micro-pace multi-chunk bursts using CPU spin loop (12µs, zero OS sleep) to prevent Wi-Fi FIFO drops
                            if total_chunks > 1 {
                                let spin_start = Instant::now();
                                while spin_start.elapsed() < Duration::from_micros(12) {
                                    std::hint::spin_loop();
                                }
                            }
                        }

                        // Emit FEC Parity packet for zero-latency recovery of Wi-Fi single-packet drops
                        if let Some(parity) = fec_parity {
                            let inst = get_process_instance_id();
                            let mut fec_pkt = Vec::with_capacity(41 + parity.len());
                            fec_pkt.extend_from_slice(MAGIC);
                            fec_pkt.extend_from_slice(&inst.to_be_bytes());
                            fec_pkt.push(OP_FEC_PARITY);
                            fec_pkt.extend_from_slice(&cid.to_be_bytes());
                            fec_pkt.extend_from_slice(&uid.to_be_bytes());
                            fec_pkt.extend_from_slice(&frame_seq.to_be_bytes());
                            fec_pkt.extend_from_slice(&pts_ms.to_be_bytes());
                            fec_pkt.extend_from_slice(&total_chunks.to_be_bytes());
                            fec_pkt.extend_from_slice(&(total_len as u32).to_be_bytes());
                            fec_pkt.extend_from_slice(&parity);

                            for target in &target_addrs {
                                let _ = socket.send_to(&fec_pkt, target);
                            }
                        }
                    }

                    tx_frame_count += 1;
                    if last_tx_stats.elapsed() >= Duration::from_secs(1) {
                        let elapsed_s = last_tx_stats.elapsed().as_secs_f64();
                        let fps = (tx_frame_count as f64) / elapsed_s;
                        CURRENT_TX_FPS.store((fps * 10.0).round() as u32, Ordering::Relaxed);
                        let n = tx_frame_count.max(1) as f64;
                        let avg_enc_ms = (total_encode_us as f64) / n / 1000.0;
                        let avg_blt_ms = (total_blt_us as f64) / n / 1000.0;
                        let avg_pix_ms = (total_pix_us as f64) / n / 1000.0;
                        let cur_mbps = (REQUESTED_BITRATE_BPS.load(Ordering::Relaxed) as f64) / 1_000_000.0;
                        info!("📊 [STREAM TELEMETRIA TX] FPS: {:.1}/{} | Bitrate: {:.2} Mbps | Encode: {:.2}ms | Captura: {:.2}ms/{:.2}ms", fps, target_fps, cur_mbps, avg_enc_ms, avg_blt_ms, avg_pix_ms);
                        tx_frame_count = 0;
                        total_encode_us = 0;
                        total_blt_us = 0;
                        total_pix_us = 0;
                        last_tx_stats = Instant::now();
                    }
                }

                CURRENT_TX_FPS.store(0, Ordering::Relaxed);
                stop_window_border_overlay();
                #[cfg(windows)]
                unsafe {
                    windows_sys::Win32::Media::timeEndPeriod(1);
                }
                info!("🖥️ Thread emissora de tela/vídeo finalizada.");
            })
            .expect("Falha ao iniciar thread TX");
    }

    /// Starts the background UDP receiver thread for incoming streams from peers
    pub fn start_receiver<F, S>(&self, on_frame: F, on_state: S)
    where
        F: Fn(u64, String, String, SharedPixelBuffer<Rgba8Pixel>) + Send + Sync + 'static,
        S: Fn(u64, bool) + Send + Sync + 'static,
    {
        if self.is_receiver_running.swap(true, Ordering::SeqCst) {
            return;
        }

        start_global_signaling(
            Arc::clone(&self.channel_id),
            Arc::clone(&self.my_user_id),
            Arc::clone(&self.my_username),
            Arc::clone(&self.is_running),
            Arc::clone(&self.known_peers),
        );
        let is_running = Arc::clone(&self.is_receiver_running);
        let is_tx_running = Arc::clone(&self.is_running);
        let channel_id_atomic = Arc::clone(&self.channel_id);
        let my_user_id_atomic = Arc::clone(&self.my_user_id);
        let my_username_arc = Arc::clone(&self.my_username);
        let peers_store = Arc::clone(&self.known_peers);

        struct QueuedVideoFrame {
            peer_uid: u64,
            peer_name: String,
            peer_fps: u8,
            seq: u32,
            pts_ms: u32,
            frame_data: Vec<u8>,
        }

        let (tx_decode, rx_decode) = std::sync::mpsc::sync_channel::<QueuedVideoFrame>(8);

        let is_running_decoder = Arc::clone(&self.is_receiver_running);
        let channel_id_decoder = Arc::clone(&self.channel_id);
        let peers_store_decoder = Arc::clone(&self.known_peers);

        // Dedicated Video Decoder Worker Thread (Sunshine / Moonlight Architecture)
        std::thread::Builder::new()
            .name("video-decoder-worker".to_string())
            .spawn(move || {
                info!("🎬 [VIDEO DECODER WORKER] Thread dedicada de decodificação H.264 iniciada!");
                let mut h264_decoders: HashMap<u64, openh264::decoder::Decoder> = HashMap::new();
                let mut last_pli_req: HashMap<u64, Instant> = HashMap::new();
                let mut rx_frame_count: HashMap<u64, u64> = HashMap::new();
                let mut rx_last_stats: HashMap<u64, Instant> = HashMap::new();

                while is_running_decoder.load(Ordering::Relaxed) {
                    match rx_decode.recv_timeout(Duration::from_millis(100)) {
                        Ok(item) => {
                            let QueuedVideoFrame { peer_uid, peer_name, peer_fps, seq, pts_ms, frame_data } = item;
                            let t_dec_start = Instant::now();

                            if let Some((pixel_buffer, _w, h)) = decode_video_frame(&mut h264_decoders, peer_uid, &frame_data) {
                                let dec_us = t_dec_start.elapsed().as_micros();
                                let count = rx_frame_count.entry(peer_uid).or_insert(0);
                                *count += 1;
                                let last_stats = rx_last_stats.entry(peer_uid).or_insert_with(Instant::now);
                                if last_stats.elapsed() >= Duration::from_secs(1) {
                                    let elapsed_s = last_stats.elapsed().as_secs_f64();
                                    let rx_fps = (*count as f64) / elapsed_s;
                                    info!("📥 [TELEMETRIA RX WORKER] Peer {}: {:.1} FPS ({} quadros em {:.2}s) | {}p {}fps | Decode: {:.2}ms",
                                        peer_uid, rx_fps, *count, elapsed_s, h, peer_fps, dec_us as f64 / 1000.0);
                                    *count = 0;
                                    *last_stats = Instant::now();
                                }

                                if let Ok(mut f_map) = get_active_stream_frames().lock() {
                                    f_map.insert(peer_uid, pixel_buffer.clone());
                                }
                                let quality_label = format!("{}p {}fps (H.264)", h, peer_fps);

                                // Non-blocking Fine Audio-Video Lip Sync Telemetry
                                let audio_pts = get_estimated_audio_pts();
                                if audio_pts > 0 && pts_ms > 0 {
                                    let delta = (pts_ms as i64) - (audio_pts as i64);
                                    log::trace!("⏱️ [A/V SYNC] Frame {} | Video PTS: {}ms | Audio PTS: {}ms | Delta: {}ms", seq, pts_ms, audio_pts, delta);
                                }

                                on_frame(peer_uid, peer_name, quality_label, pixel_buffer);
                            } else {
                                // Decode failed on P-frame -> Request immediate IDR Keyframe (WebRTC PLI mechanism)
                                let last_req = last_pli_req.entry(peer_uid).or_insert_with(|| Instant::now() - Duration::from_secs(10));
                                if last_req.elapsed() >= Duration::from_millis(250) {
                                    *last_req = Instant::now();
                                    let current_cid = channel_id_decoder.load(Ordering::Relaxed);
                                    let inst = get_process_instance_id();
                                    let mut req_pkt = Vec::with_capacity(25);
                                    req_pkt.extend_from_slice(MAGIC);
                                    req_pkt.extend_from_slice(&inst.to_be_bytes());
                                    req_pkt.push(OP_KEYFRAME_REQ);
                                    req_pkt.extend_from_slice(&current_cid.to_be_bytes());
                                    req_pkt.extend_from_slice(&peer_uid.to_be_bytes());
                                    if let Ok(guard) = SHARED_P2P_SOCKET.lock() {
                                        if let Some(sock) = guard.as_ref() {
                                            if let Ok(peers) = peers_store_decoder.lock() {
                                                if let Some(&(p_addr, _)) = peers.get(&peer_uid) {
                                                    let _ = sock.send_to(&req_pkt, p_addr);
                                                }
                                            }
                                            for bcast in get_broadcast_addresses() {
                                                let _ = sock.send_to(&req_pkt, bcast);
                                            }
                                        }
                                    }
                                    info!("🔄 [PLI RECOVERY] Keyframe solicitado ao peer {} após falha de decode", peer_uid);
                                }
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                info!("🎬 [VIDEO DECODER WORKER] Thread de decodificação finalizada.");
            })
            .ok();

        std::thread::Builder::new()
            .name("screen-capture-rx".to_string())
            .spawn(move || {
                let (socket, bound_port) = {
                    let mut bound = None;
                    for port in P2P_VIDEO_PORT..=(P2P_VIDEO_PORT + 10) {
                        if let Ok(addr) = format!("0.0.0.0:{}", port).parse::<SocketAddr>() {
                            if let Ok(sock) = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP)) {
                                let _ = sock.set_reuse_address(true);
                                let _ = sock.set_send_buffer_size(4 * 1024 * 1024);
                                let _ = sock.set_recv_buffer_size(4 * 1024 * 1024);
                                let _ = sock.set_broadcast(true);
                                let _ = sock.set_read_timeout(Some(Duration::from_millis(5)));
                                if sock.bind(&addr.into()).is_ok() {
                                    let std_sock: UdpSocket = sock.into();
                                    bound = Some((std_sock, port));
                                    break;
                                }
                            }
                        }
                    }
                    match bound {
                        Some(pair) => pair,
                        None => {
                            if let Ok(addr) = "0.0.0.0:0".parse::<SocketAddr>() {
                                if let Ok(sock) = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP)) {
                                    let _ = sock.set_reuse_address(true);
                                    let _ = sock.set_send_buffer_size(4 * 1024 * 1024);
                                    let _ = sock.set_recv_buffer_size(4 * 1024 * 1024);
                                    let _ = sock.set_broadcast(true);
                                    let _ = sock.set_read_timeout(Some(Duration::from_millis(5)));
                                    if sock.bind(&addr.into()).is_ok() {
                                        let std_sock: UdpSocket = sock.into();
                                        let p = std_sock.local_addr().map(|a| a.port()).unwrap_or(P2P_VIDEO_PORT);
                                        (std_sock, p)
                                    } else {
                                        warn!("Falha ao bind socket UDP RX");
                                        return;
                                    }
                                } else {
                                    warn!("Falha ao criar socket UDP RX");
                                    return;
                                }
                            } else {
                                warn!("Falha ao parsear endereço UDP RX");
                                return;
                            }
                        }
                    }
                };

                set_my_rx_port(bound_port);
                let socket = Arc::new(socket);
                if let Ok(mut lock) = SHARED_P2P_SOCKET.lock() {
                    *lock = Some(Arc::clone(&socket));
                }
                let mut recv_buf = vec![0u8; 65535];
                struct InFlightFrame {
                    total_chunks: u16,
                    total_len: usize,
                    pts_ms: u32,
                    received: HashMap<u16, Vec<u8>>,
                    parity: Option<Vec<u8>>,
                    first_seen: Instant,
                }
                let mut in_flight: HashMap<u64, HashMap<u32, InFlightFrame>> = HashMap::new();
                let mut last_rendered_seq: HashMap<u64, u32> = HashMap::new();
                let mut peer_names: HashMap<u64, String> = HashMap::new();
                let mut peer_fps: HashMap<u64, u8> = HashMap::new();
                #[derive(Default, Debug)]
                struct PeerQosWindow {
                    frames_received: u32,
                    frames_lost: u32,
                    fec_recovered: u32,
                }
                let mut qos_windows: HashMap<u64, PeerQosWindow> = HashMap::new();
                let mut active_streaming_users: HashMap<u64, bool> = HashMap::new();
                let mut last_stream_activity: HashMap<u64, Instant> = HashMap::new();
                let mut last_qos_feedback_time = Instant::now();

                let my_instance_id = get_process_instance_id();
                let mut last_outbound_heartbeat = Instant::now() - Duration::from_secs(10);

                while is_running.load(Ordering::Relaxed) {
                    let mut current_cid = channel_id_atomic.load(Ordering::Relaxed);
                    if current_cid == 0 {
                        current_cid = crate::gateway::get_my_voice_channel_id();
                        if current_cid > 0 {
                            channel_id_atomic.store(current_cid, Ordering::Relaxed);
                        }
                    }
                    let mut my_uid = my_user_id_atomic.load(Ordering::Relaxed);
                    if my_uid == 0 {
                        my_uid = crate::gateway::get_my_user_id();
                        if my_uid > 0 {
                            my_user_id_atomic.store(my_uid, Ordering::Relaxed);
                        }
                    }

                    // Periodic Sunshine-grade RTCP QoS / Bitrate Feedback to active senders every 1.0s
                    if last_qos_feedback_time.elapsed() >= Duration::from_millis(1000) {
                        last_qos_feedback_time = Instant::now();
                        for (&peer_uid, qos) in qos_windows.iter_mut() {
                            let total = qos.frames_received + qos.frames_lost;
                            if total > 0 {
                                let loss_permille = ((qos.frames_lost as u64 * 1000) / (total as u64)) as u16;

                                let inst = get_process_instance_id();
                                let mut qos_pkt = Vec::with_capacity(31);
                                qos_pkt.extend_from_slice(MAGIC);
                                qos_pkt.extend_from_slice(&inst.to_be_bytes());
                                qos_pkt.push(OP_QOS_FEEDBACK);
                                qos_pkt.extend_from_slice(&current_cid.to_be_bytes());
                                qos_pkt.extend_from_slice(&my_uid.to_be_bytes());
                                qos_pkt.extend_from_slice(&peer_uid.to_be_bytes());
                                qos_pkt.extend_from_slice(&loss_permille.to_be_bytes());

                                if let Ok(peers) = peers_store.lock() {
                                    if let Some(&(p_addr, _)) = peers.get(&peer_uid) {
                                        let _ = socket.send_to(&qos_pkt, p_addr);
                                    }
                                }

                                if qos.fec_recovered > 0 || qos.frames_lost > 0 {
                                    info!("🛡️ [QOS TELEMETRIA RX] Peer {}: {} frames OK, {} recuperados via FEC, {} perdidos (Perda real: {:.1}%)",
                                        peer_uid, qos.frames_received, qos.fec_recovered, qos.frames_lost, (loss_permille as f64) / 10.0
                                    );
                                }
                            }
                            qos.frames_received = 0;
                            qos.frames_lost = 0;
                            qos.fec_recovered = 0;
                        }
                    }

                    // Proactive presence heartbeat every 1.5s to maintain direct peer routes and NAT pinholes
                    if last_outbound_heartbeat.elapsed() >= Duration::from_millis(1500) {
                        last_outbound_heartbeat = Instant::now();
                        let mut uname = my_username_arc.lock().unwrap().clone();
                        if uname.is_empty() {
                            uname = crate::gateway::get_my_username();
                            if !uname.is_empty() {
                                *my_username_arc.lock().unwrap() = uname.clone();
                            }
                        }
                        let uname_bytes = uname.as_bytes();
                        let my_rx = get_my_rx_port();
                        let mut hb_pkt = Vec::with_capacity(30 + uname_bytes.len());
                        hb_pkt.extend_from_slice(MAGIC);
                        hb_pkt.extend_from_slice(&my_instance_id.to_be_bytes());
                        hb_pkt.push(OP_HEARTBEAT);
                        hb_pkt.extend_from_slice(&current_cid.to_be_bytes());
                        hb_pkt.extend_from_slice(&my_uid.to_be_bytes());
                        hb_pkt.push(0);
                        hb_pkt.push(2);
                        hb_pkt.push(uname_bytes.len() as u8);
                        hb_pkt.extend_from_slice(uname_bytes);
                        hb_pkt.extend_from_slice(&my_rx.to_be_bytes());

                        for target in get_broadcast_addresses() {
                            let _ = socket.send_to(&hb_pkt, target);
                        }
                        if let Ok(peers) = peers_store.lock() {
                            for (&_p_uid, &(p_addr, _)) in peers.iter() {
                                let _ = socket.send_to(&hb_pkt, p_addr);
                            }
                        }
                    }

                    for _ in 0..128 {
                        match socket.recv_from(&mut recv_buf) {
                            Ok((len, src_addr)) => {
                                if len < 25 || &recv_buf[0..4] != MAGIC {
                                    continue;
                                }
                                let pkt_inst = u32::from_be_bytes(recv_buf[4..8].try_into().unwrap());
                                if pkt_inst == my_instance_id {
                                    continue;
                                }

                                let op = recv_buf[8];
                                let pkt_cid = u64::from_be_bytes(recv_buf[9..17].try_into().unwrap());
                                let pkt_uid = u64::from_be_bytes(recv_buf[17..25].try_into().unwrap());

                            match op {
                                OP_ANNOUNCE => {
                                    if len >= 29 {
                                        let is_streaming = recv_buf[25] == 1;
                                        let fps_val = recv_buf[27];
                                        if fps_val > 0 {
                                            peer_fps.insert(pkt_uid, fps_val);
                                        }
                                        let name_len = recv_buf[28] as usize;
                                        if len >= 29 + name_len {
                                            if let Ok(uname) = std::str::from_utf8(&recv_buf[29..29 + name_len]) {
                                                peer_names.insert(pkt_uid, uname.to_string());
                                            }
                                        }

                                        let peer_rx_port = if len >= 29 + name_len + 2 {
                                            u16::from_be_bytes(recv_buf[29 + name_len..31 + name_len].try_into().unwrap())
                                        } else {
                                            src_addr.port()
                                        };

                                        let explicit_addr = SocketAddr::new(src_addr.ip(), peer_rx_port);

                                        if let Ok(mut peers) = peers_store.lock() {
                                            peers.insert(pkt_uid, (src_addr, Instant::now()));
                                            if explicit_addr != src_addr {
                                                peers.insert(pkt_uid.wrapping_add(0x8000_0000_0000_0000), (explicit_addr, Instant::now()));
                                            }
                                        }

                                        // Instant reciprocal heartbeat so transmitter gets our direct IP and RX port
                                        if is_streaming {
                                            let uname = my_username_arc.lock().unwrap().clone();
                                            let uname_bytes = uname.as_bytes();
                                            let my_rx = get_my_rx_port();
                                            let mut ack_pkt = Vec::with_capacity(30 + uname_bytes.len());
                                            ack_pkt.extend_from_slice(MAGIC);
                                            ack_pkt.extend_from_slice(&my_instance_id.to_be_bytes());
                                            ack_pkt.push(OP_HEARTBEAT);
                                            ack_pkt.extend_from_slice(&current_cid.to_be_bytes());
                                            ack_pkt.extend_from_slice(&my_uid.to_be_bytes());
                                            ack_pkt.push(0);
                                            ack_pkt.push(2);
                                            ack_pkt.push(uname_bytes.len() as u8);
                                            ack_pkt.extend_from_slice(uname_bytes);
                                            ack_pkt.extend_from_slice(&my_rx.to_be_bytes());

                                            let _ = socket.send_to(&ack_pkt, src_addr);
                                            if explicit_addr != src_addr {
                                                let _ = socket.send_to(&ack_pkt, explicit_addr);
                                            }
                                            for target in get_broadcast_addresses() {
                                                let _ = socket.send_to(&ack_pkt, target);
                                            }
                                        }

                                        if is_tx_running.load(Ordering::Relaxed) && pkt_uid == my_uid {
                                            continue;
                                        }
                                        if is_streaming {
                                            last_stream_activity.insert(pkt_uid, Instant::now());
                                        }
                                        let prev_state = active_streaming_users.insert(pkt_uid, is_streaming);
                                        if prev_state != Some(is_streaming) {
                                            info!("📡 Usuário {} alterou estado de stream: {}", pkt_uid, is_streaming);
                                            on_state(pkt_uid, is_streaming);
                                        }
                                    }
                                }
                                OP_HEARTBEAT => {
                                    if len >= 28 {
                                        let name_len = recv_buf[27] as usize;
                                        if len >= 28 + name_len {
                                            if let Ok(uname) = std::str::from_utf8(&recv_buf[28..28 + name_len]) {
                                                peer_names.insert(pkt_uid, uname.to_string());
                                            }
                                        }

                                        let peer_port = if len >= 28 + name_len + 2 {
                                            u16::from_be_bytes(recv_buf[28 + name_len..30 + name_len].try_into().unwrap())
                                        } else {
                                            src_addr.port()
                                        };

                                        let explicit_addr = SocketAddr::new(src_addr.ip(), peer_port);
                                        if let Ok(mut peers) = peers_store.lock() {
                                            peers.insert(pkt_uid, (src_addr, Instant::now()));
                                            if explicit_addr != src_addr {
                                                peers.insert(pkt_uid.wrapping_add(0x8000_0000_0000_0000), (explicit_addr, Instant::now()));
                                            }
                                        }
                                        log::debug!("📡 Heartbeat P2P recebido do peer {} ({})", pkt_uid, src_addr);
                                    }
                                }
                                OP_VIDEO_CHUNK => {
                                    if current_cid == 0 {
                                        current_cid = crate::gateway::get_my_voice_channel_id();
                                    }
                                    if current_cid == 0 || (pkt_cid != 0 && pkt_cid != current_cid) {
                                        continue;
                                    }
                                    if is_tx_running.load(Ordering::Relaxed) && pkt_uid == my_uid {
                                        continue;
                                    }
                                    if len >= 37 {
                                        let seq = u32::from_be_bytes(recv_buf[25..29].try_into().unwrap());
                                        let pts_ms = u32::from_be_bytes(recv_buf[29..33].try_into().unwrap());
                                        let total = u16::from_be_bytes(recv_buf[33..35].try_into().unwrap());
                                        let idx = u16::from_be_bytes(recv_buf[35..37].try_into().unwrap());
                                        let chunk_data = recv_buf[37..len].to_vec();

                                        let user_frames = in_flight.entry(pkt_uid).or_insert_with(HashMap::new);
                                        let frame_entry = user_frames.entry(seq).or_insert_with(|| InFlightFrame {
                                            total_chunks: total,
                                            total_len: 0,
                                            pts_ms,
                                            received: HashMap::with_capacity(total as usize),
                                            parity: None,
                                            first_seen: Instant::now(),
                                        });
                                        frame_entry.pts_ms = pts_ms;
                                        frame_entry.total_chunks = total;
                                        frame_entry.received.insert(idx, chunk_data);

                                        // Bidirectional FEC Parity Recovery (If parity packet arrived before this chunk)
                                        if frame_entry.received.len() == (total as usize).saturating_sub(1) && frame_entry.parity.is_some() {
                                            let mut missing_idx = None;
                                            for i in 0..total {
                                                if !frame_entry.received.contains_key(&i) { missing_idx = Some(i); break; }
                                            }
                                            if let Some(m_idx) = missing_idx {
                                                if let Some(parity) = frame_entry.parity.as_ref() {
                                                    let mut recovered = parity.clone();
                                                    for (&_c_i, chunk) in &frame_entry.received {
                                                        for (i, &b) in chunk.iter().enumerate() {
                                                            if i < recovered.len() { recovered[i] ^= b; }
                                                        }
                                                    }
                                                    frame_entry.received.insert(m_idx, recovered);
                                                }
                                            }
                                        }

                                        if frame_entry.received.len() == (total as usize) {
                                            let mut complete_frame = Vec::new();
                                            for i in 0..total {
                                                if let Some(c) = frame_entry.received.get(&i) {
                                                    complete_frame.extend_from_slice(c);
                                                }
                                            }
                                            user_frames.remove(&seq);

                                            let sec_key = get_voice_encryption_key(current_cid);
                                            let final_frame = decrypt_signaling_payload(&sec_key, &complete_frame).unwrap_or(complete_frame);

                                            let last_seq = last_rendered_seq.get(&pkt_uid).copied().unwrap_or(0);
                                            let is_newer = seq > last_seq || (last_seq.wrapping_sub(seq) > 0x8000_0000);

                                            if is_newer || last_rendered_seq.get(&pkt_uid).is_none() {
                                                last_rendered_seq.insert(pkt_uid, seq);

                                                let qos = qos_windows.entry(pkt_uid).or_default();
                                                qos.frames_received += 1;
                                                last_stream_activity.insert(pkt_uid, Instant::now());

                                                let uname = peer_names.get(&pkt_uid).cloned().unwrap_or_else(|| format!("Usuário {}", pkt_uid));
                                                let fps_val = peer_fps.get(&pkt_uid).copied().unwrap_or(60);

                                                let q_item = QueuedVideoFrame {
                                                    peer_uid: pkt_uid,
                                                    peer_name: uname,
                                                    peer_fps: fps_val,
                                                    seq,
                                                    pts_ms,
                                                    frame_data: final_frame,
                                                };

                                                if let Err(std::sync::mpsc::TrySendError::Full(_)) = tx_decode.try_send(q_item) {
                                                    log::warn!("⚠️ [JITTER BUFFER] Fila de decodificação cheia, descartando frame {} para manter latência ultra-baixa em tempo real", seq);
                                                }
                                            }
                                        }
                                    }
                                }
                                OP_FEC_PARITY => {
                                    if current_cid == 0 {
                                        current_cid = crate::gateway::get_my_voice_channel_id();
                                    }
                                    if current_cid == 0 || (pkt_cid != 0 && pkt_cid != current_cid) {
                                        continue;
                                    }
                                    if is_tx_running.load(Ordering::Relaxed) && pkt_uid == my_uid {
                                        continue;
                                    }
                                    if len >= 39 {
                                        let seq = u32::from_be_bytes(recv_buf[25..29].try_into().unwrap());
                                        let pts_ms = u32::from_be_bytes(recv_buf[29..33].try_into().unwrap());
                                        let total = u16::from_be_bytes(recv_buf[33..35].try_into().unwrap());
                                        let total_frame_len = u32::from_be_bytes(recv_buf[35..39].try_into().unwrap_or([0; 4])) as usize;
                                        let parity_data = recv_buf[39..len].to_vec();

                                        let user_frames = in_flight.entry(pkt_uid).or_insert_with(HashMap::new);
                                        let frame_entry = user_frames.entry(seq).or_insert_with(|| InFlightFrame {
                                            total_chunks: total,
                                            total_len: total_frame_len,
                                            pts_ms,
                                            received: HashMap::with_capacity(total as usize),
                                            parity: None,
                                            first_seen: Instant::now(),
                                        });
                                        frame_entry.total_chunks = total;
                                        frame_entry.total_len = total_frame_len;
                                        frame_entry.parity = Some(parity_data);

                                        if frame_entry.received.len() == (total as usize).saturating_sub(1) {
                                            let mut complete_frame = Vec::new();
                                            let mut missing_idx = None;
                                            for i in 0..total {
                                                if !frame_entry.received.contains_key(&i) { missing_idx = Some(i); break; }
                                            }
                                            
                                            if let Some(m_idx) = missing_idx {
                                                if let Some(parity) = frame_entry.parity.as_ref() {
                                                    let mut recovered = parity.clone();
                                                    for (&_c_i, chunk) in &frame_entry.received {
                                                        for (i, &b) in chunk.iter().enumerate() {
                                                            if i < recovered.len() { recovered[i] ^= b; }
                                                        }
                                                    }
                                                    frame_entry.received.insert(m_idx, recovered);
                                                }
                                            }

                                            for i in 0..total {
                                                if let Some(c) = frame_entry.received.get(&i) {
                                                    complete_frame.extend_from_slice(c);
                                                }
                                            }
                                            user_frames.remove(&seq);

                                            let sec_key = get_voice_encryption_key(current_cid);
                                            let final_frame = decrypt_signaling_payload(&sec_key, &complete_frame).unwrap_or(complete_frame);

                                            let last_seq = last_rendered_seq.get(&pkt_uid).copied().unwrap_or(0);
                                            let is_newer = seq > last_seq || (last_seq.wrapping_sub(seq) > 0x8000_0000);

                                            if is_newer || last_rendered_seq.get(&pkt_uid).is_none() {
                                                last_rendered_seq.insert(pkt_uid, seq);

                                                let qos = qos_windows.entry(pkt_uid).or_default();
                                                qos.frames_received += 1;
                                                last_stream_activity.insert(pkt_uid, Instant::now());

                                                let uname = peer_names.get(&pkt_uid).cloned().unwrap_or_else(|| format!("Usuário {}", pkt_uid));
                                                let fps_val = peer_fps.get(&pkt_uid).copied().unwrap_or(60);

                                                let q_item = QueuedVideoFrame {
                                                    peer_uid: pkt_uid,
                                                    peer_name: uname,
                                                    peer_fps: fps_val,
                                                    seq,
                                                    pts_ms,
                                                    frame_data: final_frame,
                                                };

                                                if let Err(std::sync::mpsc::TrySendError::Full(_)) = tx_decode.try_send(q_item) {
                                                    log::warn!("⚠️ [JITTER BUFFER] Fila de decodificação cheia, descartando frame {} para manter latência ultra-baixa em tempo real", seq);
                                                }
                                            }
                                        }
                                    }
                                }
                                OP_KEYFRAME_REQ => {
                                    if is_tx_running.load(Ordering::Relaxed) {
                                        KEYFRAME_REQUESTED.store(true, Ordering::Relaxed);
                                    }
                                }
                                OP_QOS_FEEDBACK => {
                                    if len >= 35 && is_tx_running.load(Ordering::Relaxed) {
                                        let target_peer = u64::from_be_bytes(recv_buf[25..33].try_into().unwrap());
                                        if target_peer == my_uid || target_peer == 0 {
                                            let loss_permille = u16::from_be_bytes(recv_buf[33..35].try_into().unwrap());
                                            let current_bps = REQUESTED_BITRATE_BPS.load(Ordering::Relaxed);
                                            if loss_permille > 50 {
                                                // Congestion / Loss > 5%: Multiplicative Decrease (-15% to relieve queue buffers)
                                                let new_bps = ((current_bps as f64) * 0.85).round() as u32;
                                                let clamped = new_bps.clamp(2_000_000, 10_000_000);
                                                if (current_bps as i32 - clamped as i32).abs() >= 400_000 {
                                                    info!("📉 [DYNAMIC ABR] Perda de {:.1}% reportada pelo receptor. Reduzindo bitrate: {:.2} Mbps -> {:.2} Mbps",
                                                        (loss_permille as f64) / 10.0,
                                                        (current_bps as f64) / 1_000_000.0,
                                                        (clamped as f64) / 1_000_000.0
                                                    );
                                                    REQUESTED_BITRATE_BPS.store(clamped, Ordering::Relaxed);
                                                }
                                            } else if loss_permille == 0 && current_bps < 8_500_000 {
                                                // Stable link / 0% Loss: Additive Increase (+250 kbps probing)
                                                let new_bps = current_bps.saturating_add(250_000);
                                                let clamped = new_bps.clamp(2_000_000, 8_500_000);
                                                if (clamped as i32 - current_bps as i32) >= 200_000 {
                                                    info!("📈 [DYNAMIC ABR] Conexão 100% estável (0% perda). Elevando bitrate: {:.2} Mbps -> {:.2} Mbps",
                                                        (current_bps as f64) / 1_000_000.0,
                                                        (clamped as f64) / 1_000_000.0
                                                    );
                                                    REQUESTED_BITRATE_BPS.store(clamped, Ordering::Relaxed);
                                                }
                                            }
                                        }
                                    }
                                }
                                OP_STOP => {
                                    if len >= 25 {
                                        let pkt_uid = u64::from_be_bytes(recv_buf[17..25].try_into().unwrap());
                                        if !is_tx_running.load(Ordering::Relaxed) || pkt_uid != my_uid {
                                            info!("📡 Usuário {} encerrou a transmissão de tela.", pkt_uid);
                                            active_streaming_users.insert(pkt_uid, false);
                                            in_flight.remove(&pkt_uid);
                                            last_rendered_seq.remove(&pkt_uid);
                                            last_stream_activity.remove(&pkt_uid);
                                            if let Ok(mut frames) = get_active_stream_frames().lock() {
                                                frames.remove(&pkt_uid);
                                            }
                                            on_state(pkt_uid, false);
                                        }
                                    }
                                }
                                OP_AUDIO_FRAME => {
                                    if len >= 40 {
                                        // 1. If we are not connected to any voice room, do not play stream audio
                                        if current_cid == 0 {
                                            continue;
                                        }
                                        // 2. If packet is from a different voice room, ignore
                                        if pkt_cid != 0 && pkt_cid != current_cid {
                                            continue;
                                        }
                                        // 3. If this instance is actively transmitting screen/audio, do not play back stream audio to avoid acoustic feedback
                                        if is_tx_running.load(Ordering::Relaxed) {
                                            continue;
                                        }
                                        ensure_stream_audio_playback_started();

                                        let pts_ms = u32::from_be_bytes(recv_buf[29..33].try_into().unwrap());
                                        update_audio_clock_pts(pts_ms);

                                        let vol = get_stream_volume(pkt_uid);
                                        if vol > 0.001 {
                                            let sample_count = u16::from_be_bytes(recv_buf[38..40].try_into().unwrap()) as usize;
                                            let enc_pcm_bytes = &recv_buf[40..len];
                                            let sec_key = get_voice_encryption_key(current_cid);
                                            let raw_pcm = decrypt_signaling_payload(&sec_key, enc_pcm_bytes).unwrap_or_else(|| enc_pcm_bytes.to_vec());
                                            let expected_bytes = sample_count * 2;
                                            if raw_pcm.len() >= expected_bytes && sample_count > 0 {
                                                let queue = get_stream_audio_queue();
                                                let mut q_guard = queue.lock().unwrap();
                                                // Limit queue size to 100ms (4800 samples) to eliminate lag & stutter
                                                if q_guard.len() > 4800 {
                                                    let excess = q_guard.len() - 2400;
                                                    q_guard.drain(0..excess);
                                                }
                                                for i in 0..sample_count {
                                                    let s_i16 = i16::from_le_bytes([raw_pcm[i*2], raw_pcm[i*2 + 1]]);
                                                    let s_f32 = (s_i16 as f32 / 32768.0) * vol;
                                                    q_guard.push_back(s_f32);
                                                }
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
                            break;
                        }
                        Err(_) => break,
                    }
                }

                    // Check stream timeouts (> 800ms sem quadros)
                    let now = Instant::now();
                    let mut expired = Vec::new();
                    for (&uid, &last_act) in last_stream_activity.iter() {
                        if now.duration_since(last_act) > Duration::from_millis(800) {
                            expired.push(uid);
                        }
                    }
                    for uid in expired {
                        last_stream_activity.remove(&uid);
                        if active_streaming_users.insert(uid, false) != Some(false) {
                            info!("📡 Stream do usuário {} expirou por inatividade.", uid);
                            last_rendered_seq.remove(&uid);
                            in_flight.remove(&uid);
                            if let Ok(mut frames) = get_active_stream_frames().lock() {
                                frames.remove(&uid);
                            }
                            on_state(uid, false);
                        }
                    }
                }
            })
            .expect("Falha ao iniciar thread RX");
    }
}

static SHARED_P2P_SOCKET: Mutex<Option<Arc<UdpSocket>>> = Mutex::new(None);

pub fn get_shared_p2p_socket() -> Option<Arc<UdpSocket>> {
    SHARED_P2P_SOCKET.lock().ok()?.clone()
}

fn encode_mqtt_string(s: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(2 + s.len());
    let len = s.len() as u16;
    v.extend_from_slice(&len.to_be_bytes());
    v.extend_from_slice(s.as_bytes());
    v
}

fn build_mqtt_connect_pkt(client_id: &str) -> Vec<u8> {
    let mut payload = encode_mqtt_string(client_id);
    let mut var_header = vec![
        0x00, 0x04, b'M', b'Q', b'T', b'T', // Protocol Name
        0x04, // Protocol Level (3.1.1)
        0x02, // Connect Flags (Clean Session)
        0x00, 0x3C, // Keep Alive (60s)
    ];
    let mut body = Vec::new();
    body.append(&mut var_header);
    body.append(&mut payload);

    let mut pkt = vec![0x10]; // CONNECT packet type
    let len = body.len();
    pkt.push(len as u8);
    pkt.extend_from_slice(&body);
    pkt
}

fn build_mqtt_subscribe_pkt(pkt_id: u16, topic: &str) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&pkt_id.to_be_bytes());
    body.extend_from_slice(&encode_mqtt_string(topic));
    body.push(0); // QoS 0

    let mut pkt = vec![0x82]; // SUBSCRIBE packet type
    pkt.push(body.len() as u8);
    pkt.extend_from_slice(&body);
    pkt
}

fn build_mqtt_publish_pkt(topic: &str, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&encode_mqtt_string(topic));
    body.extend_from_slice(payload);

    let mut pkt = vec![0x30]; // PUBLISH packet type (QoS 0)
    let len = body.len();
    if len < 128 {
        pkt.push(len as u8);
    } else {
        pkt.push(((len & 0x7F) | 0x80) as u8);
        pkt.push((len >> 7) as u8);
    }
    pkt.extend_from_slice(&body);
    pkt
}

fn get_local_lan_addresses(port: u16) -> Vec<SocketAddr> {
    let mut addrs = Vec::new();
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(SocketAddr::V4(local)) = socket.local_addr() {
                addrs.push(SocketAddr::new(std::net::IpAddr::V4(*local.ip()), port));
            }
        }
    }
    addrs.push(SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)), port));
    addrs
}

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use sha2::{Sha256, Digest};
use rand::RngCore;

/// Deriva uma chave AES de 32 bytes exclusiva e sincronizada para todos os participantes do canal de voz
fn get_voice_encryption_key(cid: u64) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"litecord_e2ee_voice_p2p_channel_salt_v3_2026");
    hasher.update(&cid.to_be_bytes());
    if let Some(secret) = crate::gateway::get_voice_secret_key() {
        hasher.update(&secret);
    }
    let res = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&res);
    key
}

/// Deriva um tópico MQTT anônimo e indecifrável para observadores externos
fn get_anonymous_signaling_topic(cid: u64, key: &[u8; 32]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key);
    hasher.update(b"litecord_anon_topic_v1");
    hasher.update(&cid.to_be_bytes());
    let hash = format!("{:x}", hasher.finalize());
    format!("litecord/sig/{}", &hash[..24])
}

/// Criptografa o payload usando AES-256-GCM com Nonce aleatório de 12 bytes
fn encrypt_signaling_payload(key_bytes: &[u8; 32], plaintext: &[u8]) -> Option<Vec<u8>> {
    let key = Key::<Aes256Gcm>::from_slice(key_bytes);
    let cipher = Aes256Gcm::new(key);
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    match cipher.encrypt(nonce, plaintext) {
        Ok(ciphertext) => {
            let mut out = Vec::with_capacity(12 + ciphertext.len());
            out.extend_from_slice(&nonce_bytes);
            out.extend_from_slice(&ciphertext);
            Some(out)
        }
        Err(_) => None,
    }
}

/// Descriptografa o payload via AES-256-GCM. Rejeita pacotes forjados ou sem autenticação válida.
fn decrypt_signaling_payload(key_bytes: &[u8; 32], encrypted_data: &[u8]) -> Option<Vec<u8>> {
    if encrypted_data.len() < 12 + 16 {
        return None;
    }
    let key = Key::<Aes256Gcm>::from_slice(key_bytes);
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(&encrypted_data[..12]);
    let ciphertext = &encrypted_data[12..];

    cipher.decrypt(nonce, ciphertext).ok()
}

fn parse_and_handle_mqtt_messages(
    buf: &[u8],
    current_cid: u64,
    my_inst: u32,
    peers_store: &Arc<Mutex<HashMap<u64, (SocketAddr, Instant)>>>,
) {
    if buf.is_empty() { return; }
    let key = get_voice_encryption_key(current_cid);

    let process_json = |json_bytes: &[u8]| {
        if let Ok(val) = serde_json::from_slice::<serde_json::Value>(json_bytes) {
            let pkt_cid = val["cid"].as_u64().unwrap_or(0);
            let pkt_uid = val["uid"].as_u64().unwrap_or(0);
            let pkt_inst = val["inst"].as_u64().unwrap_or(0) as u32;

            if pkt_cid == current_cid && pkt_inst != my_inst && pkt_inst != 0 {
                let mut addrs_to_punch = Vec::new();

                // 1. LAN IPs
                if let Some(lan_arr) = val["lan_ips"].as_array() {
                    for item in lan_arr {
                        if let Some(s) = item.as_str() {
                            if let Ok(addr) = s.parse::<SocketAddr>() {
                                addrs_to_punch.push(addr);
                            }
                        }
                    }
                }

                // 2. WAN IP
                if let Some(wan_str) = val["wan_ip"].as_str() {
                    if let Ok(addr) = wan_str.parse::<SocketAddr>() {
                        if !addrs_to_punch.contains(&addr) {
                            addrs_to_punch.push(addr);
                        }
                    }
                }

                if !addrs_to_punch.is_empty() {
                    if let Ok(mut peers) = peers_store.lock() {
                        for (i, &addr) in addrs_to_punch.iter().enumerate() {
                            let key = if i == 0 { pkt_uid } else { pkt_uid.wrapping_add((i as u64) << 32) };
                            peers.insert(key, (addr, Instant::now()));
                        }
                    }

                    // Send UDP Hole Punch packet immediately through shared socket
                    if let Some(socket) = get_shared_p2p_socket() {
                        let mut punch_pkt = Vec::with_capacity(32);
                        punch_pkt.extend_from_slice(MAGIC);
                        punch_pkt.extend_from_slice(&my_inst.to_be_bytes());
                        punch_pkt.push(OP_HEARTBEAT);
                        punch_pkt.extend_from_slice(&current_cid.to_be_bytes());
                        punch_pkt.extend_from_slice(&crate::gateway::get_my_user_id().to_be_bytes());
                        punch_pkt.push(0);
                        punch_pkt.push(2);
                        punch_pkt.push(0);
                        punch_pkt.extend_from_slice(&get_my_rx_port().to_be_bytes());

                        for target in addrs_to_punch {
                            let _ = socket.send_to(&punch_pkt, target);
                        }
                    }
                }
            }
        }
    };

    let mut pos = 0;
    while pos < buf.len() {
        if (buf[pos] & 0xF0) == 0x30 { // MQTT PUBLISH
            pos += 1;
            if pos >= buf.len() { break; }
            let mut rem_len: usize = (buf[pos] & 0x7F) as usize;
            let has_next = (buf[pos] & 0x80) != 0;
            pos += 1;
            if has_next && pos < buf.len() {
                rem_len += ((buf[pos] & 0x7F) as usize) << 7;
                pos += 1;
            }
            if pos + rem_len > buf.len() { break; }
            let frame_body = &buf[pos..pos + rem_len];
            pos += rem_len;

            if frame_body.len() >= 2 {
                let topic_len = u16::from_be_bytes([frame_body[0], frame_body[1]]) as usize;
                let payload_offset = 2 + topic_len;
                if frame_body.len() >= payload_offset {
                    let payload = &frame_body[payload_offset..];
                    if let Some(decrypted) = decrypt_signaling_payload(&key, payload) {
                        process_json(&decrypted);
                    } else if let Some(json_start) = payload.iter().position(|&b| b == b'{') {
                        if let Some(json_end) = payload.iter().rposition(|&b| b == b'}') {
                            if json_end >= json_start {
                                process_json(&payload[json_start..=json_end]);
                            }
                        }
                    }
                }
            }
        } else {
            pos += 1;
        }
    }
}

pub fn start_global_signaling(
    channel_id: Arc<AtomicU64>,
    my_user_id: Arc<AtomicU64>,
    my_username: Arc<Mutex<String>>,
    is_tx_running: Arc<AtomicBool>,
    peers_store: Arc<Mutex<HashMap<u64, (SocketAddr, Instant)>>>,
) {
    std::thread::Builder::new()
        .name("global-p2p-signaling".to_string())
        .spawn(move || {
            let my_inst = get_process_instance_id();
            let brokers = ["broker.emqx.io:1883", "test.mosquitto.org:1883"];
            let mut broker_idx = 0;

            loop {
                let mut cid = channel_id.load(Ordering::Relaxed);
                if cid == 0 {
                    cid = crate::gateway::get_my_voice_channel_id();
                    if cid > 0 {
                        channel_id.store(cid, Ordering::Relaxed);
                    }
                }
                if cid == 0 {
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }

                let key = get_voice_encryption_key(cid);
                let topic = get_anonymous_signaling_topic(cid, &key);

                let broker = brokers[broker_idx % brokers.len()];
                broker_idx = broker_idx.wrapping_add(1);

                use std::io::{Read, Write};
                let mut stream = match std::net::TcpStream::connect_timeout(&match broker.to_socket_addrs() {
                    Ok(mut addrs) => match addrs.next() {
                        Some(a) => a,
                        None => { std::thread::sleep(Duration::from_secs(1)); continue; }
                    },
                    Err(_) => { std::thread::sleep(Duration::from_secs(1)); continue; }
                }, Duration::from_secs(4)) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("⚠️ Falha ao conectar ao broker de sinalização P2P {}: {:?}. Tentando próximo...", broker, e);
                        std::thread::sleep(Duration::from_secs(2));
                        continue;
                    }
                };

                let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                let client_id = format!("litecord_{}_{}", &topic[13..], my_inst);
                let conn_pkt = build_mqtt_connect_pkt(&client_id);
                if stream.write_all(&conn_pkt).is_err() {
                    continue;
                }

                let mut connack = [0u8; 4];
                if stream.read_exact(&mut connack).is_err() || connack[0] != 0x20 || connack[3] != 0x00 {
                    continue;
                }

                let sub_pkt = build_mqtt_subscribe_pkt(1, &topic);
                if stream.write_all(&sub_pkt).is_err() {
                    continue;
                }

                info!("🛡️ Conectado à rede de sinalização P2P criptografada E2EE (AES-256-GCM) para a sala!");

                let mut last_pub = Instant::now() - Duration::from_secs(10);
                let mut last_tx_state = false;
                let mut read_buf = vec![0u8; 4096];

                while channel_id.load(Ordering::Relaxed) == cid {
                    let cur_tx = is_tx_running.load(Ordering::Relaxed);
                    let mut my_uid = my_user_id.load(Ordering::Relaxed);
                    if my_uid == 0 {
                        my_uid = crate::gateway::get_my_user_id();
                    }

                    if last_pub.elapsed() >= Duration::from_secs(2) || cur_tx != last_tx_state {
                        last_pub = Instant::now();
                        last_tx_state = cur_tx;

                        let my_rx_port = get_my_rx_port();
                        let wan_addr = resolve_public_stun_address().map(|a| SocketAddr::new(a.ip(), my_rx_port));
                        let lan_addrs = get_local_lan_addresses(my_rx_port);

                        let uname = my_username.lock().unwrap().clone();
                        let payload = serde_json::json!({
                            "op": "presence",
                            "cid": cid,
                            "uid": my_uid,
                            "inst": my_inst,
                            "uname": uname,
                            "streaming": cur_tx,
                            "rx_port": my_rx_port,
                            "lan_ips": lan_addrs.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                            "wan_ip": wan_addr.map(|a| a.to_string()),
                        });

                        let current_key = get_voice_encryption_key(cid);
                        if let Some(enc_payload) = encrypt_signaling_payload(&current_key, payload.to_string().as_bytes()) {
                            let pub_pkt = build_mqtt_publish_pkt(&topic, &enc_payload);
                            if stream.write_all(&pub_pkt).is_err() {
                                break;
                            }
                        }
                    }

                    match stream.read(&mut read_buf) {
                        Ok(n) if n > 0 => {
                            parse_and_handle_mqtt_messages(&read_buf[..n], cid, my_inst, &peers_store);
                        }
                        Ok(_) => {
                            break;
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
                            // continue
                        }
                        Err(_) => {
                            break;
                        }
                    }
                }

                info!("🛑 Desconectado da sala de sinalização P2P para o canal {}", cid);
                std::thread::sleep(Duration::from_millis(500));
            }
        })
        .expect("Falha ao iniciar thread de sinalização global P2P");
}

static CACHED_STUN_ADDR: Mutex<Option<(SocketAddr, Instant)>> = Mutex::new(None);

pub fn resolve_public_stun_address() -> Option<SocketAddr> {
    if let Ok(guard) = CACHED_STUN_ADDR.lock() {
        if let Some((addr, time)) = *guard {
            if time.elapsed() < Duration::from_secs(60) {
                return Some(addr);
            }
        }
    }

    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.set_read_timeout(Some(Duration::from_millis(500))).ok()?;
    
    let stun_servers = [
        "stun.l.google.com:19302",
        "stun1.l.google.com:19302",
        "stun2.l.google.com:19302",
        "stun.cloudflare.com:3478",
    ];

    let stun_req: [u8; 20] = [
        0x00, 0x01, // Binding Request
        0x00, 0x00, // Length
        0x21, 0x12, 0xa4, 0x42, // Magic Cookie
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
    ];

    for s_host in stun_servers {
        if let Ok(mut addrs) = s_host.to_socket_addrs() {
            if let Some(stun_server) = addrs.next() {
                if socket.send_to(&stun_req, stun_server).is_ok() {
                    let mut buf = [0u8; 256];
                    if let Ok((amt, _)) = socket.recv_from(&mut buf) {
                        if amt >= 32 && buf[0] == 0x01 && buf[1] == 0x01 {
                            let mut i = 20;
                            while i + 4 <= amt {
                                let attr_type = u16::from_be_bytes([buf[i], buf[i + 1]]);
                                let attr_len = u16::from_be_bytes([buf[i + 2], buf[i + 3]]) as usize;
                                if i + 4 + attr_len > amt {
                                    break;
                                }
                                let res_addr = if attr_type == 0x0020 && attr_len >= 8 { // XOR-MAPPED-ADDRESS
                                    let port = u16::from_be_bytes([buf[i + 6], buf[i + 7]]) ^ 0x2112;
                                    let ip = std::net::Ipv4Addr::new(
                                        buf[i + 8] ^ 0x21,
                                        buf[i + 9] ^ 0x12,
                                        buf[i + 10] ^ 0xa4,
                                        buf[i + 11] ^ 0x42,
                                    );
                                    Some(SocketAddr::new(std::net::IpAddr::V4(ip), port))
                                } else if attr_type == 0x0001 && attr_len >= 8 { // MAPPED-ADDRESS
                                    let port = u16::from_be_bytes([buf[i + 6], buf[i + 7]]);
                                    let ip = std::net::Ipv4Addr::new(buf[i + 8], buf[i + 9], buf[i + 10], buf[i + 11]);
                                    Some(SocketAddr::new(std::net::IpAddr::V4(ip), port))
                                } else {
                                    None
                                };

                                if let Some(addr) = res_addr {
                                    if let Ok(mut guard) = CACHED_STUN_ADDR.lock() {
                                        *guard = Some((addr, Instant::now()));
                                    }
                                    return Some(addr);
                                }
                                i += 4 + ((attr_len + 3) & !3);
                            }
                        }
                    }
                }
            }
        }
    }

    // HTTPS Fallback for firewalls/ISPs that block UDP STUN ports
    let https_endpoints = [
        "https://api.ipify.org",
        "https://checkip.amazonaws.com",
        "https://icanhazip.com",
    ];
    for ep in https_endpoints {
        if let Ok(resp) = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(1500))
            .build()
            .and_then(|c| c.get(ep).send())
        {
            if let Ok(text) = resp.text() {
                if let Ok(ip) = text.trim().parse::<std::net::IpAddr>() {
                    let rx_port = get_my_rx_port();
                    let addr = SocketAddr::new(ip, rx_port);
                    if let Ok(mut guard) = CACHED_STUN_ADDR.lock() {
                        *guard = Some((addr, Instant::now()));
                    }
                    return Some(addr);
                }
            }
        }
    }

    None
}

fn get_broadcast_addresses() -> Vec<SocketAddr> {
    let mut addrs = Vec::new();
    
    // 1. Broadcast to local port cluster (50005..=50007) on loopback & 255.255.255.255
    for port in 50005..=50007 {
        addrs.push(SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)), port));
        addrs.push(SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(255, 255, 255, 255)), port));
    }

    // 2. Query active adapter IPv4 subnet broadcast via routing table probe
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(SocketAddr::V4(local)) = socket.local_addr() {
                let octets = local.ip().octets();
                for port in 50005..=50007 {
                    addrs.push(SocketAddr::new(
                        std::net::IpAddr::V4(std::net::Ipv4Addr::new(octets[0], octets[1], octets[2], 255)),
                        port,
                    ));
                }
            }
        }
    }

    // 3. Resolve STUN Public Internet Address for global P2P hole punching
    if let Some(pub_addr) = resolve_public_stun_address() {
        for port in 50005..=50007 {
            addrs.push(SocketAddr::new(pub_addr.ip(), port));
        }
    }

    addrs
}

fn fit_bgra_to_canvas(
    src_bgra: &[u8],
    src_w: u32,
    src_h: u32,
    canvas_w: u32,
    canvas_h: u32,
    out_bgra: &mut [u8],
) {
    if src_w == 0 || src_h == 0 || canvas_w == 0 || canvas_h == 0 {
        return;
    }

    let total_pixels = (canvas_w as usize) * (canvas_h as usize);
    let total_bytes = total_pixels * 4;
    if out_bgra.len() < total_bytes {
        return;
    }

    // Calcula o stride real por linha da textura (GPU D3D11 Texture2D pitch)
    let src_stride_bytes = if src_h > 0 && src_bgra.len() >= (src_h as usize) {
        src_bgra.len() / (src_h as usize)
    } else {
        (src_w as usize) * 4
    };

    // Fast direct copy if dimensions match (even with GPU row pitch padding)
    if src_w == canvas_w && src_h == canvas_h {
        let row_bytes = (canvas_w as usize) * 4;
        if src_stride_bytes == row_bytes && src_bgra.len() >= total_bytes {
            out_bgra[..total_bytes].copy_from_slice(&src_bgra[..total_bytes]);
            return;
        }
        for y in 0..(canvas_h as usize) {
            let src_off = y * src_stride_bytes;
            let dst_off = y * row_bytes;
            if src_off + row_bytes <= src_bgra.len() && dst_off + row_bytes <= out_bgra.len() {
                out_bgra[dst_off..dst_off + row_bytes].copy_from_slice(&src_bgra[src_off..src_off + row_bytes]);
            }
        }
        return;
    }

    let canvas_u32: &mut [u32] = unsafe {
        std::slice::from_raw_parts_mut(out_bgra.as_mut_ptr() as *mut u32, total_pixels)
    };
    canvas_u32.fill(0xFF111214); // Fundo escuro (#111214) estilo Discord/YouTube

    let scale_w = canvas_w as f32 / src_w as f32;
    let scale_h = canvas_h as f32 / src_h as f32;
    let scale = scale_w.min(scale_h);

    let mut dest_w = ((src_w as f32 * scale).round() as u32).clamp(2, canvas_w) & !1;
    let mut dest_h = ((src_h as f32 * scale).round() as u32).clamp(2, canvas_h) & !1;
    let dest_x = ((canvas_w.saturating_sub(dest_w)) / 2) & !1;
    let dest_y = ((canvas_h.saturating_sub(dest_h)) / 2) & !1;

    // Bounds safety clamp: ensure dest_x + dest_w <= canvas_w and dest_y + dest_h <= canvas_h
    if dest_x + dest_w > canvas_w {
        dest_w = canvas_w.saturating_sub(dest_x) & !1;
    }
    if dest_y + dest_h > canvas_h {
        dest_h = canvas_h.saturating_sub(dest_y) & !1;
    }

    if dest_w == 0 || dest_h == 0 {
        return;
    }

    let x_step = ((src_w as u64) << 16) / (dest_w as u64);
    let y_step = ((src_h as u64) << 16) / (dest_h as u64);
    let src_stride_u32 = src_stride_bytes / 4;

    let src_u32: &[u32] = unsafe {
        std::slice::from_raw_parts(src_bgra.as_ptr() as *const u32, src_bgra.len() / 4)
    };

    let mut src_y_accum = 0u64;
    for dy in 0..dest_h {
        let sy = ((src_y_accum >> 16) as usize).min((src_h as usize).saturating_sub(1));
        let src_row_start = sy * src_stride_u32;
        if src_row_start >= src_u32.len() {
            break;
        }
        let src_row_end = (src_row_start + (src_w as usize)).min(src_u32.len());
        let src_row = &src_u32[src_row_start..src_row_end];

        let dst_y_idx = (dest_y + dy) as usize;
        if dst_y_idx >= (canvas_h as usize) {
            break;
        }
        let dst_row_start = dst_y_idx * (canvas_w as usize) + (dest_x as usize);
        let dst_row_end = (dst_row_start + (dest_w as usize)).min(total_pixels);
        if dst_row_start >= total_pixels || dst_row_start >= dst_row_end {
            break;
        }
        let dst_row = &mut canvas_u32[dst_row_start..dst_row_end];

        let mut src_x_accum = 0u64;
        for dx in 0..dst_row.len() {
            let sx = ((src_x_accum >> 16) as usize).min(src_row.len().saturating_sub(1));
            if sx < src_row.len() {
                dst_row[dx] = src_row[sx];
            }
            src_x_accum += x_step;
        }
        src_y_accum += y_step;
    }
}



fn decode_video_frame(
    decoders: &mut HashMap<u64, openh264::decoder::Decoder>,
    peer_uid: u64,
    frame_data: &[u8],
) -> Option<(SharedPixelBuffer<Rgba8Pixel>, u32, u32)> {
    if frame_data.is_empty() { return None; }
    
    // Check if legacy JPEG header (0xFF, 0xD8)
    if frame_data.len() >= 2 && frame_data[0] == 0xFF && frame_data[1] == 0xD8 {
        return decode_jpeg(frame_data);
    }

    use openh264::formats::YUVSource;
    let decoder = decoders.entry(peer_uid).or_insert_with(|| {
        openh264::decoder::Decoder::new().expect("Falha ao criar Decoder H.264")
    });

    match decoder.decode(frame_data) {
        Ok(Some(decoded_yuv)) => {
            let (w, h) = decoded_yuv.dimensions();
            if w > 0 && h > 0 {
                use rayon::prelude::*;
                let mut pixel_buffer = SharedPixelBuffer::<Rgba8Pixel>::new(w as u32, h as u32);
                let (ys, us, _vs) = decoded_yuv.strides();
                let y_raw = decoded_yuv.y().as_ptr() as usize;
                let u_raw = decoded_yuv.u().as_ptr() as usize;
                let v_raw = decoded_yuv.v().as_ptr() as usize;
                let rgba_ptr = pixel_buffer.make_mut_bytes().as_mut_ptr() as usize;

                (0..h).into_par_iter().with_min_len(32).for_each(|j| {
                    let y_p = y_raw as *const u8;
                    let u_p = u_raw as *const u8;
                    let v_p = v_raw as *const u8;
                    let rgba_p_u32 = rgba_ptr as *mut u32;

                    let y_row = j * ys;
                    let uv_row = (j / 2) * us;
                    let dst_row = j * w;

                    let mut i = 0;
                    while i + 1 < w {
                        unsafe {
                            let u_val = *u_p.add(uv_row + (i / 2)) as i32;
                            let v_val = *v_p.add(uv_row + (i / 2)) as i32;

                            let d = u_val - 128;
                            let e = v_val - 128;

                            let r_add = 409 * e + 128;
                            let g_add = -100 * d - 208 * e + 128;
                            let b_add = 516 * d + 128;

                            // Pixel 0
                            let y0 = *y_p.add(y_row + i) as i32;
                            let c0 = 298 * (y0 - 16);
                            let r0 = ((c0 + r_add) >> 8).clamp(0, 255) as u8;
                            let g0 = ((c0 + g_add) >> 8).clamp(0, 255) as u8;
                            let b0 = ((c0 + b_add) >> 8).clamp(0, 255) as u8;
                            *rgba_p_u32.add(dst_row + i) = u32::from_le_bytes([r0, g0, b0, 255]);

                            // Pixel 1
                            let y1 = *y_p.add(y_row + i + 1) as i32;
                            let c1 = 298 * (y1 - 16);
                            let r1 = ((c1 + r_add) >> 8).clamp(0, 255) as u8;
                            let g1 = ((c1 + g_add) >> 8).clamp(0, 255) as u8;
                            let b1 = ((c1 + b_add) >> 8).clamp(0, 255) as u8;
                            *rgba_p_u32.add(dst_row + i + 1) = u32::from_le_bytes([r1, g1, b1, 255]);
                        }
                        i += 2;
                    }

                    if i < w {
                        unsafe {
                            let y = *y_p.add(y_row + i) as i32;
                            let u = *u_p.add(uv_row + (i / 2)) as i32;
                            let v = *v_p.add(uv_row + (i / 2)) as i32;

                            let c = 298 * (y - 16);
                            let d = u - 128;
                            let e = v - 128;

                            let r = ((c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
                            let g = ((c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
                            let b = ((c + 516 * d + 128) >> 8).clamp(0, 255) as u8;

                            *rgba_p_u32.add(dst_row + i) = u32::from_le_bytes([r, g, b, 255]);
                        }
                    }
                });

                return Some((pixel_buffer, w as u32, h as u32));
            }
        }
        Ok(None) => {}
        Err(_e) => {
            if let Ok(fresh) = openh264::decoder::Decoder::new() {
                *decoder = fresh;
            }
        }
    }
    None
}

fn encode_jpeg(rgb_data: &[u8], width: u32, height: u32, quality: u8) -> Option<Vec<u8>> {
    let mut dest = Vec::with_capacity((width * height / 4) as usize);
    let encoder = jpeg_encoder::Encoder::new(&mut dest, quality);
    if encoder.encode(rgb_data, width as u16, height as u16, jpeg_encoder::ColorType::Rgb).is_ok() {
        Some(dest)
    } else {
        None
    }
}

fn decode_jpeg(jpeg_data: &[u8]) -> Option<(SharedPixelBuffer<Rgba8Pixel>, u32, u32)> {
    let mut decoder = zune_jpeg::JpegDecoder::new(jpeg_data);
    let rgb_pixels = decoder.decode().ok()?;
    let info = decoder.info()?;
    let (width, height) = (info.width as u32, info.height as u32);
    let total_pixels = (width * height) as usize;

    if rgb_pixels.len() < total_pixels * 3 {
        return None;
    }

    let mut pixel_buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    let dest_bytes = pixel_buffer.make_mut_bytes();

    let mut src_idx = 0;
    let mut dst_idx = 0;
    while src_idx + 2 < total_pixels * 3 && dst_idx + 3 < dest_bytes.len() {
        dest_bytes[dst_idx] = rgb_pixels[src_idx];
        dest_bytes[dst_idx + 1] = rgb_pixels[src_idx + 1];
        dest_bytes[dst_idx + 2] = rgb_pixels[src_idx + 2];
        dest_bytes[dst_idx + 3] = 255;
        src_idx += 3;
        dst_idx += 4;
    }

    Some((pixel_buffer, width, height))
}

#[cfg(windows)]
pub fn list_screens() -> Vec<MonitorItemInfo> {
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
    };
    use windows_sys::Win32::Foundation::{BOOL, LPARAM, RECT, TRUE};

    struct MonitorEnumData {
        screens: Vec<MonitorItemInfo>,
        count: i32,
    }

    unsafe extern "system" fn monitor_proc(
        hmon: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let data = &mut *(lparam as *mut MonitorEnumData);
        data.count += 1;

        let mut info: MONITORINFOEXW = std::mem::zeroed();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;

        let is_primary = if GetMonitorInfoW(hmon, &mut info.monitorInfo as *mut _ as _) != 0 {
            (info.monitorInfo.dwFlags & 1) != 0 // MONITORINFOF_PRIMARY = 1
        } else {
            data.count == 1
        };

        let width = (info.monitorInfo.rcMonitor.right - info.monitorInfo.rcMonitor.left).abs();
        let height = (info.monitorInfo.rcMonitor.bottom - info.monitorInfo.rcMonitor.top).abs();
        let (w, h) = if width > 0 && height > 0 { (width, height) } else { (1920, 1080) };

        let name = if is_primary {
            format!("Tela {} (Principal)", data.count)
        } else {
            format!("Tela {}", data.count)
        };

        data.screens.push(MonitorItemInfo {
            id: data.count - 1,
            name,
            resolution: format!("{} × {}", w, h),
            is_primary,
            hwnd: 0,
        });

        TRUE
    }

    let mut data = MonitorEnumData { screens: Vec::new(), count: 0 };
    unsafe {
        EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(monitor_proc), &mut data as *mut _ as LPARAM);
    }

    if data.screens.is_empty() {
        data.screens.push(MonitorItemInfo {
            id: 0,
            name: "Tela 1 (Principal)".to_string(),
            resolution: "1920 × 1080".to_string(),
            is_primary: true,
            hwnd: 0,
        });
    }

    data.screens
}

#[cfg(not(windows))]
pub fn list_screens() -> Vec<MonitorItemInfo> {
    let mut screens = Vec::new();
    #[cfg(target_os = "linux")]
    {
        use x11rb::connection::Connection;
        if let Ok((conn, screen_num)) = x11rb::connect(None) {
            let root = &conn.setup().roots[screen_num];
            screens.push(MonitorItemInfo {
                id: 0,
                name: "Tela Principal".to_string(),
                resolution: format!("{} × {}", root.width_in_pixels, root.height_in_pixels),
                is_primary: true,
                hwnd: root.root as isize,
            });
        }
    }

    if screens.is_empty() {
        screens.push(MonitorItemInfo {
            id: 0,
            name: "Tela 1 (Principal)".to_string(),
            resolution: "1920 × 1080".to_string(),
            is_primary: true,
            hwnd: 0,
        });
    }
    screens
}

#[cfg(windows)]
pub fn list_capturable_windows() -> Vec<CapturableWindowItem> {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, TRUE};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowLongPtrW, GetWindowTextW, IsWindowVisible, GWL_EXSTYLE, GWL_STYLE,
        WS_EX_TOOLWINDOW, WS_VISIBLE,
    };

    struct EnumData {
        windows: Vec<CapturableWindowItem>,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let data = &mut *(lparam as *mut EnumData);
        if IsWindowVisible(hwnd) == 0 {
            return TRUE;
        }

        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;

        if (style & WS_VISIBLE) == 0 || (ex_style & WS_EX_TOOLWINDOW) != 0 {
            return TRUE;
        }

        let mut title_buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), 512);
        if len > 0 {
            let title = String::from_utf16_lossy(&title_buf[..len as usize]).trim().to_string();
            let lower = title.to_lowercase();
            // Filter out internal and background overlay windows
            if !title.is_empty() 
                && !lower.starts_with("program manager") 
                && !lower.starts_with("settings") 
                && !lower.starts_with("configurações")
                && !lower.starts_with("windows input")
                && !lower.starts_with("msctfime ui")
                && !lower.starts_with("default ime")
                && !lower.starts_with("litecord - transmissão") 
            {
                use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
                use windows_sys::Win32::System::ProcessStatus::GetModuleFileNameExW;
                use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

                let mut pid: u32 = 0;
                GetWindowThreadProcessId(hwnd, &mut pid);
                let is_own_process = pid == std::process::id();

                let mut exe_name = String::new();
                if pid != 0 {
                    let hproc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                    if !hproc.is_null() {
                        let mut path_buf = [0u16; 1024];
                        let len = GetModuleFileNameExW(hproc, std::ptr::null_mut(), path_buf.as_mut_ptr(), 1024);
                        windows_sys::Win32::Foundation::CloseHandle(hproc);
                        if len > 0 {
                            let full_path = String::from_utf16_lossy(&path_buf[..len as usize]);
                            if let Some(filename) = full_path.split('\\').last() {
                                exe_name = filename.to_lowercase();
                            }
                        }
                    }
                }

                // Strictly detect Litecord first (own PID or litecord exe/title)
                let app_type = if is_own_process || exe_name.contains("litecord") || lower.contains("litecord") {
                    "Litecord".to_string()
                } else if exe_name.contains("chrome") || lower.contains("chrome") {
                    "Google Chrome".to_string()
                } else if exe_name.contains("firefox") || lower.contains("firefox") {
                    "Mozilla Firefox".to_string()
                } else if exe_name.contains("msedge") || lower.contains("edge") {
                    "Microsoft Edge".to_string()
                } else if exe_name.contains("code") || lower.contains("visual studio code") {
                    "Visual Studio Code".to_string()
                } else if exe_name.contains("discord") {
                    "Discord".to_string()
                } else if exe_name.contains("spotify") || lower.contains("spotify") {
                    "Spotify".to_string()
                } else if exe_name.contains("telegram") || lower.contains("telegram") {
                    "Telegram".to_string()
                } else if exe_name.contains("terminal") || exe_name.contains("powershell") || exe_name.contains("cmd") || lower.contains("terminal") {
                    "Terminal".to_string()
                } else if exe_name.contains("notepad") || lower.contains("bloco de notas") {
                    "Bloco de Notas".to_string()
                } else if !exe_name.is_empty() {
                    let clean = exe_name.trim_end_matches(".exe");
                    let mut chars = clean.chars();
                    match chars.next() {
                        None => "Janela".to_string(),
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    }
                } else {
                    "Janela".to_string()
                };

                let icon_rgba = extract_window_icon_rgba(hwnd);

                data.windows.push(CapturableWindowItem {
                    id: (hwnd as isize).to_string(),
                    title: title.clone(),
                    app_name: app_type,
                    icon_rgba,
                });
            }
        }
        TRUE
    }

    let mut data = EnumData { windows: Vec::new() };
    unsafe {
        EnumWindows(Some(enum_proc), &mut data as *mut _ as LPARAM);
    }
    data.windows
}

#[cfg(windows)]
unsafe fn extract_window_icon_rgba(hwnd: windows_sys::Win32::Foundation::HWND) -> Option<(u32, u32, Vec<u8>)> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, GetClassLongPtrW, DestroyIcon, GetIconInfo,
        WM_GETICON, ICON_BIG, ICON_SMALL2, ICON_SMALL, GCLP_HICON, GCLP_HICONSM,
        SMTO_ABORTIFHUNG, HICON, ICONINFO,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        GetDC, ReleaseDC, CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits,
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };

    let mut hicon: HICON = std::ptr::null_mut();
    let mut need_destroy = false;

    // 1. Try WM_GETICON (BIG, then SMALL2, then SMALL)
    let mut res_icon: usize = 0;
    for &icon_type in &[ICON_BIG, ICON_SMALL2, ICON_SMALL] {
        if SendMessageTimeoutW(
            hwnd,
            WM_GETICON,
            icon_type as usize,
            0,
            SMTO_ABORTIFHUNG,
            50,
            &mut res_icon,
        ) != 0 && res_icon != 0 {
            hicon = res_icon as HICON;
            break;
        }
    }

    // 2. If null, try class icons
    if hicon.is_null() {
        let cls_icon = GetClassLongPtrW(hwnd, GCLP_HICON);
        if cls_icon != 0 {
            hicon = cls_icon as HICON;
        } else {
            let cls_icon_sm = GetClassLongPtrW(hwnd, GCLP_HICONSM);
            if cls_icon_sm != 0 {
                hicon = cls_icon_sm as HICON;
            }
        }
    }

    // 3. If null, try ExtractIconW from process executable path
    if hicon.is_null() {
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        use windows_sys::Win32::System::ProcessStatus::GetModuleFileNameExW;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

        #[link(name = "shell32")]
        extern "system" {
            fn ExtractIconW(
                hInst: isize,
                pszExeFileName: *const u16,
                nIconIndex: u32,
            ) -> HICON;
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != 0 {
            let hproc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if !hproc.is_null() {
                let mut path_buf = [0u16; 1024];
                let len = GetModuleFileNameExW(hproc, std::ptr::null_mut(), path_buf.as_mut_ptr(), 1024);
                windows_sys::Win32::Foundation::CloseHandle(hproc);
                if len > 0 {
                    let extracted = ExtractIconW(0, path_buf.as_ptr(), 0);
                    if !extracted.is_null() && (extracted as usize) > 1 {
                        hicon = extracted;
                        need_destroy = true;
                    }
                }
            }
        }
    }

    if hicon.is_null() || (hicon as usize) <= 1 {
        return None;
    }

    let mut icon_info: ICONINFO = std::mem::zeroed();
    if GetIconInfo(hicon, &mut icon_info) == 0 {
        if need_destroy { DestroyIcon(hicon); }
        return None;
    }

    let hbm = if !icon_info.hbmColor.is_null() {
        icon_info.hbmColor
    } else {
        icon_info.hbmMask
    };

    if hbm.is_null() {
        if !icon_info.hbmColor.is_null() { DeleteObject(icon_info.hbmColor); }
        if !icon_info.hbmMask.is_null() { DeleteObject(icon_info.hbmMask); }
        if need_destroy { DestroyIcon(hicon); }
        return None;
    }

    let hdc_screen = GetDC(std::ptr::null_mut());
    let hdc_mem = CreateCompatibleDC(hdc_screen);

    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;

    if GetDIBits(hdc_mem, hbm, 0, 0, std::ptr::null_mut(), &mut bmi, DIB_RGB_COLORS) == 0 {
        DeleteDC(hdc_mem);
        ReleaseDC(std::ptr::null_mut(), hdc_screen);
        if !icon_info.hbmColor.is_null() { DeleteObject(icon_info.hbmColor); }
        if !icon_info.hbmMask.is_null() { DeleteObject(icon_info.hbmMask); }
        if need_destroy { DestroyIcon(hicon); }
        return None;
    }

    let width = bmi.bmiHeader.biWidth.abs() as u32;
    let mut height = bmi.bmiHeader.biHeight.abs() as u32;
    if icon_info.hbmColor.is_null() {
        height /= 2;
    }

    if width == 0 || height == 0 || width > 256 || height > 256 {
        DeleteDC(hdc_mem);
        ReleaseDC(std::ptr::null_mut(), hdc_screen);
        if !icon_info.hbmColor.is_null() { DeleteObject(icon_info.hbmColor); }
        if !icon_info.hbmMask.is_null() { DeleteObject(icon_info.hbmMask); }
        if need_destroy { DestroyIcon(hicon); }
        return None;
    }

    bmi.bmiHeader.biWidth = width as i32;
    bmi.bmiHeader.biHeight = -(height as i32); // Top-down DIB
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB as u32;

    let total_pixels = (width * height) as usize;
    let mut bgra_buf = vec![0u8; total_pixels * 4];

    GetDIBits(
        hdc_mem,
        hbm,
        0,
        height,
        bgra_buf.as_mut_ptr() as _,
        &mut bmi,
        DIB_RGB_COLORS,
    );

    DeleteDC(hdc_mem);
    ReleaseDC(std::ptr::null_mut(), hdc_screen);

    if !icon_info.hbmColor.is_null() { DeleteObject(icon_info.hbmColor); }
    if !icon_info.hbmMask.is_null() { DeleteObject(icon_info.hbmMask); }
    if need_destroy { DestroyIcon(hicon); }

    let mut rgba_buf = vec![0u8; total_pixels * 4];
    let mut has_non_zero_alpha = false;

    for i in 0..total_pixels {
        let b = bgra_buf[i * 4];
        let g = bgra_buf[i * 4 + 1];
        let r = bgra_buf[i * 4 + 2];
        let a = bgra_buf[i * 4 + 3];

        if a > 0 {
            has_non_zero_alpha = true;
        }

        rgba_buf[i * 4] = r;
        rgba_buf[i * 4 + 1] = g;
        rgba_buf[i * 4 + 2] = b;
        rgba_buf[i * 4 + 3] = a;
    }

    if !has_non_zero_alpha {
        for i in 0..total_pixels {
            rgba_buf[i * 4 + 3] = 255;
        }
    }

    Some((width, height, rgba_buf))
}

#[cfg(not(windows))]
pub fn list_capturable_windows() -> Vec<CapturableWindowItem> {
    let mut windows = Vec::new();
    #[cfg(target_os = "linux")]
    {
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::ConnectionExt;

        if std::env::var("DISPLAY").is_err() {
            std::env::set_var("DISPLAY", ":0");
        }
        if std::env::var("XAUTHORITY").is_err() {
            if let Ok(entries) = std::fs::read_dir("/run/user/1000") {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with("xauth_") {
                        std::env::set_var("XAUTHORITY", entry.path());
                        break;
                    }
                }
            }
        }

        if let Ok((conn, screen_num)) = x11rb::connect(None) {
            let root = conn.setup().roots[screen_num].root;
            if let Ok(tree) = conn.query_tree(root) {
                if let Ok(tree_reply) = tree.reply() {
                    for &win in tree_reply.children.iter().rev() {
                        if let Ok(attrs) = conn.get_window_attributes(win) {
                            if let Ok(attr_reply) = attrs.reply() {
                                if attr_reply.map_state == x11rb::protocol::xproto::MapState::VIEWABLE {
                                    if let Ok(name_prop) = conn.get_property(
                                        false,
                                        win,
                                        x11rb::protocol::xproto::AtomEnum::WM_NAME,
                                        x11rb::protocol::xproto::AtomEnum::STRING,
                                        0,
                                        1024,
                                    ) {
                                        if let Ok(prop_reply) = name_prop.reply() {
                                            if !prop_reply.value.is_empty() {
                                                if let Ok(title) = String::from_utf8(prop_reply.value) {
                                                    let clean_title = title.trim().to_string();
                                                    if !clean_title.is_empty() && clean_title != "Desktop" {
                                                        windows.push(CapturableWindowItem {
                                                            id: format!("{}", win),
                                                            title: clean_title.clone(),
                                                            app_name: clean_title,
                                                            icon_rgba: None,
                                                        });
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if windows.is_empty() {
        windows.push(CapturableWindowItem {
            id: "0".to_string(),
            title: "Área de Trabalho".to_string(),
            app_name: "Desktop".to_string(),
            icon_rgba: None,
        });
    }
    windows
}

#[cfg(windows)]
fn capture_screen_rgb(target_hwnd: isize, target_w: u32, target_h: u32, _target_fps: u64, out_bgra: &mut Vec<u8>) -> Option<(u128, u128)> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, CreateSolidBrush, DeleteDC, DeleteObject,
        FillRect, GetDC, ReleaseDC, SelectObject, SetBrushOrgEx, SetStretchBltMode, StretchBlt,
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, COLORONCOLOR, DIB_RGB_COLORS, SRCCOPY,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, GetWindowRect, IsIconic, IsWindow,
        SM_CXSCREEN, SM_CYSCREEN,
    };

    #[link(name = "user32")]
    extern "system" {
        fn PrintWindow(
            hwnd: windows_sys::Win32::Foundation::HWND,
            hdcBlt: windows_sys::Win32::Graphics::Gdi::HDC,
            nFlags: u32,
        ) -> windows_sys::Win32::Foundation::BOOL;
    }

    struct GdiCaptureContext {
        hdc_desktop: windows_sys::Win32::Graphics::Gdi::HDC,
        hdc_mem: windows_sys::Win32::Graphics::Gdi::HDC,
        hbm_dib: windows_sys::Win32::Graphics::Gdi::HBITMAP,
        p_bits: *mut u8,
        hdc_win_mem: windows_sys::Win32::Graphics::Gdi::HDC,
        hbm_win: windows_sys::Win32::Graphics::Gdi::HBITMAP,
        win_w: i32,
        win_h: i32,
        dark_brush: windows_sys::Win32::Graphics::Gdi::HBRUSH,
        target_w: u32,
        target_h: u32,
    }

    thread_local! {
        static GDI_CAPTURE_CACHE: std::cell::RefCell<Option<GdiCaptureContext>> = std::cell::RefCell::new(None);
    }

    unsafe {
        GDI_CAPTURE_CACHE.with(|cell| {
            let mut cache_opt = cell.borrow_mut();
            if let Some(ref cache) = *cache_opt {
                if cache.target_w != target_w || cache.target_h != target_h {
                    // Clean up old handles if dimensions changed
                    DeleteObject(cache.dark_brush);
                    if !cache.hbm_win.is_null() {
                        DeleteObject(cache.hbm_win);
                    }
                    if !cache.hdc_win_mem.is_null() {
                        DeleteDC(cache.hdc_win_mem);
                    }
                    DeleteObject(cache.hbm_dib);
                    DeleteDC(cache.hdc_mem);
                    ReleaseDC(std::ptr::null_mut(), cache.hdc_desktop);
                    *cache_opt = None;
                }
            }

            if cache_opt.is_none() {
                let hdc_desktop = GetDC(std::ptr::null_mut());
                if hdc_desktop.is_null() {
                    return None;
                }

                let hdc_mem = CreateCompatibleDC(hdc_desktop);
                if hdc_mem.is_null() {
                    ReleaseDC(std::ptr::null_mut(), hdc_desktop);
                    return None;
                }

                let mut bmi: BITMAPINFO = std::mem::zeroed();
                bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                bmi.bmiHeader.biWidth = target_w as i32;
                bmi.bmiHeader.biHeight = -(target_h as i32); // Top-down
                bmi.bmiHeader.biPlanes = 1;
                bmi.bmiHeader.biBitCount = 32;
                bmi.bmiHeader.biCompression = BI_RGB as u32;

                let mut p_bits: *mut std::ffi::c_void = std::ptr::null_mut();
                let hbm_dib = CreateDIBSection(
                    hdc_desktop,
                    &bmi,
                    DIB_RGB_COLORS,
                    &mut p_bits,
                    std::ptr::null_mut(),
                    0,
                );
                if hbm_dib.is_null() || p_bits.is_null() {
                    DeleteDC(hdc_mem);
                    ReleaseDC(std::ptr::null_mut(), hdc_desktop);
                    return None;
                }

                SelectObject(hdc_mem, hbm_dib);
                let hdc_win_mem = CreateCompatibleDC(hdc_desktop);
                let dark_brush = CreateSolidBrush(0x00141211); // COLORREF: 0x00BBGGRR

                *cache_opt = Some(GdiCaptureContext {
                    hdc_desktop,
                    hdc_mem,
                    hbm_dib,
                    p_bits: p_bits as *mut u8,
                    hdc_win_mem,
                    hbm_win: std::ptr::null_mut(),
                    win_w: 0,
                    win_h: 0,
                    dark_brush,
                    target_w,
                    target_h,
                });
            }

            let cache = cache_opt.as_mut().unwrap();
            let hdc_desktop = cache.hdc_desktop;
            let hdc_mem = cache.hdc_mem;
            let hdc_win_mem = cache.hdc_win_mem;
            let dark_brush = cache.dark_brush;
            let p_bits = cache.p_bits;

            let t_blt_start = Instant::now();
            if target_hwnd != 0 {
                let hwnd = target_hwnd as windows_sys::Win32::Foundation::HWND;
                if IsWindow(hwnd) == 0 {
                    return None;
                }

                let mut rc: RECT = std::mem::zeroed();
                GetWindowRect(hwnd, &mut rc);
                let win_w = (rc.right - rc.left).max(1);
                let win_h = (rc.bottom - rc.top).max(1);

                if cache.win_w != win_w || cache.win_h != win_h || cache.hbm_win.is_null() {
                    if !cache.hbm_win.is_null() {
                        DeleteObject(cache.hbm_win);
                    }
                    cache.hbm_win = CreateCompatibleBitmap(hdc_desktop, win_w, win_h);
                    cache.win_w = win_w;
                    cache.win_h = win_h;
                    SelectObject(hdc_win_mem, cache.hbm_win);
                }

                let mut captured_ok = false;
                if IsIconic(hwnd) == 0 {
                    // 1. Tenta PrintWindow com PW_RENDERFULLCONTENT (2) para janelas DirectX/DWM
                    if PrintWindow(hwnd, hdc_win_mem, 2) != 0 {
                        captured_ok = true;
                    }
                }

                // 2. Fallback para crop direto do desktop (garante fidelidade visual e zero rabiscos)
                if !captured_ok && IsIconic(hwnd) == 0 {
                    let src_x = rc.left.max(0);
                    let src_y = rc.top.max(0);
                    BitBlt(hdc_win_mem, 0, 0, win_w, win_h, hdc_desktop, src_x, src_y, SRCCOPY);
                }

                // Fill background with dark letterbox/pillarbox color (#111214)
                let target_rc = RECT { left: 0, top: 0, right: target_w as i32, bottom: target_h as i32 };
                FillRect(hdc_mem, &target_rc, dark_brush);

                // Compute aspect-ratio preserving dimensions
                let scale_w = target_w as f32 / win_w as f32;
                let scale_h = target_h as f32 / win_h as f32;
                let scale = scale_w.min(scale_h);

                let dest_w = ((win_w as f32 * scale).round() as i32).max(1);
                let dest_h = ((win_h as f32 * scale).round() as i32).max(1);
                let dest_x = ((target_w as i32 - dest_w) / 2).max(0);
                let dest_y = ((target_h as i32 - dest_h) / 2).max(0);

                SetStretchBltMode(hdc_mem, COLORONCOLOR);
                SetBrushOrgEx(hdc_mem, 0, 0, std::ptr::null_mut());
                StretchBlt(
                    hdc_mem,
                    dest_x,
                    dest_y,
                    dest_w,
                    dest_h,
                    hdc_win_mem,
                    0,
                    0,
                    win_w,
                    win_h,
                    SRCCOPY,
                );
            } else {
                // Full screen capture
                let screen_w = GetSystemMetrics(SM_CXSCREEN);
                let screen_h = GetSystemMetrics(SM_CYSCREEN);

                if screen_w == target_w as i32 && screen_h == target_h as i32 {
                    BitBlt(
                        hdc_mem,
                        0,
                        0,
                        target_w as i32,
                        target_h as i32,
                        hdc_desktop,
                        0,
                        0,
                        SRCCOPY,
                    );
                } else {
                    SetStretchBltMode(hdc_mem, COLORONCOLOR);
                    SetBrushOrgEx(hdc_mem, 0, 0, std::ptr::null_mut());
                    StretchBlt(
                        hdc_mem,
                        0,
                        0,
                        target_w as i32,
                        target_h as i32,
                        hdc_desktop,
                        0,
                        0,
                        screen_w,
                        screen_h,
                        SRCCOPY,
                    );
                }
            }
            let blt_dur_us = t_blt_start.elapsed().as_micros();

            let t_pix_start = Instant::now();
            let total_bytes = (target_w * target_h * 4) as usize;
            if out_bgra.len() != total_bytes {
                out_bgra.resize(total_bytes, 0);
            }
            std::ptr::copy_nonoverlapping(p_bits, out_bgra.as_mut_ptr(), total_bytes);
            let pix_dur_us = t_pix_start.elapsed().as_micros();

            Some((blt_dur_us, pix_dur_us))
        })
    }
}

#[cfg(not(windows))]
static PORTAL_FRAME: std::sync::Mutex<Option<(SharedPixelBuffer<Rgba8Pixel>, Vec<u8>)>> = std::sync::Mutex::new(None);
#[cfg(not(windows))]
static PORTAL_INITIALIZED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(not(windows))]
static PORTAL_CANCELLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(target_os = "linux")]
static PORTAL_LAST_ATTEMPT: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
#[cfg(target_os = "linux")]
static PORTAL_LOCAL_CB: std::sync::Mutex<Option<std::sync::Arc<dyn Fn(SharedPixelBuffer<Rgba8Pixel>) + Send + Sync + 'static>>> = std::sync::Mutex::new(None);
#[cfg(target_os = "linux")]
static PORTAL_CHILD: std::sync::Mutex<Option<std::process::Child>> = std::sync::Mutex::new(None);

#[cfg(target_os = "linux")]
pub fn reset_wayland_portal_cancelled() {
    PORTAL_CANCELLED.store(false, std::sync::atomic::Ordering::SeqCst);
    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(target_os = "linux")]
pub fn kill_portal_child() {
    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
    PORTAL_CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Ok(mut lock) = PORTAL_CHILD.lock() {
        if let Some(mut child) = lock.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(target_os = "linux")]
fn init_wayland_portal_screencast(target_w: u32, target_h: u32, target_fps: u64) {
    if PORTAL_CANCELLED.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }

    if let Ok(guard) = PORTAL_LAST_ATTEMPT.lock() {
        if let Some(last) = *guard {
            if last.elapsed() < std::time::Duration::from_secs(3) {
                return;
            }
        }
    }

    if PORTAL_INITIALIZED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }

    if let Ok(mut guard) = PORTAL_LAST_ATTEMPT.lock() {
        *guard = Some(std::time::Instant::now());
    }

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
            Ok(r) => r,
            Err(e) => {
                log::error!("Falha ao criar tokio runtime para Portal ScreenCast: {:?}", e);
                PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                return;
            }
        };

        let _guard = rt.enter();

        rt.block_on(async move {
            use ashpd::desktop::{
                PersistMode,
                screencast::{
                    CursorMode, OpenPipeWireRemoteOptions, Screencast,
                    SelectSourcesOptions, SourceType, StartCastOptions,
                },
                CreateSessionOptions,
            };
            use std::os::fd::AsRawFd;
            use std::process::{Command, Stdio};
            use std::io::Read;

            log::info!("📡 Solicitando sessão nativa do XDG Desktop Portal ScreenCast via conexão D-Bus dedicada...");
            let conn = match ashpd::zbus::connection::Builder::session() {
                Ok(b) => match b.build().await {
                    Ok(c) => c,
                    Err(e) => {
                        log::error!("Falha ao estabelecer conexão D-Bus privada para Portal: {:?}", e);
                        PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                        return;
                    }
                },
                Err(e) => {
                    log::error!("Falha ao construir builder D-Bus para Portal: {:?}", e);
                    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };

            let proxy = match Screencast::with_connection(conn).await {
                Ok(p) => p,
                Err(e) => {
                    log::error!("Falha ao conectar no Screencast Portal: {:?}", e);
                    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };

            let session = match proxy.create_session(CreateSessionOptions::default()).await {
                Ok(s) => s,
                Err(e) => {
                    log::error!("Falha ao criar sessão do ScreenCast: {:?}", e);
                    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };

            let select_opts = SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Embedded)
                .set_sources(SourceType::Monitor | SourceType::Window)
                .set_multiple(false)
                .set_restore_token(None)
                .set_persist_mode(PersistMode::DoNot);

            if let Err(e) = proxy.select_sources(&session, select_opts).await {
                log::error!("Falha ao selecionar fontes de captura no Portal: {:?}", e);
                let _ = session.close().await;
                PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                return;
            }

            let response = match proxy.start(&session, None, StartCastOptions::default()).await {
                Ok(r) => match r.response() {
                    Ok(resp) => resp,
                    Err(e) => {
                        log::error!("Resposta de erro do Portal ScreenCast: {:?}", e);
                        let _ = session.close().await;
                        PORTAL_CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
                        PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                        return;
                    }
                },
                Err(e) => {
                    log::error!("Falha ao iniciar captura no Portal ScreenCast: {:?}", e);
                    let _ = session.close().await;
                    PORTAL_CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
                    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };

            let stream = match response.streams().first() {
                Some(s) => s,
                None => {
                    log::error!("Nenhum stream retornado pelo Portal ScreenCast");
                    let _ = session.close().await;
                    PORTAL_CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
                    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };

            let node_id = stream.pipe_wire_node_id();
            let pw_fd = match proxy.open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default()).await {
                Ok(fd) => fd,
                Err(e) => {
                    log::error!("Falha ao abrir PipeWire Remote: {:?}", e);
                    let _ = session.close().await;
                    PORTAL_CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
                    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };

            let raw_fd = pw_fd.as_raw_fd();
            unsafe {
                let flags = libc::fcntl(raw_fd, libc::F_GETFD);
                if flags >= 0 {
                    libc::fcntl(raw_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
                }
            }

            log::info!("🎉 Conectando GStreamer ao PipeWire Node ID={}, FD={} ({}x{} @ {} FPS)...", node_id, raw_fd, target_w, target_h, target_fps);

            let (dst_w, dst_h) = (target_w, target_h);
            let frame_size = (dst_w * dst_h * 4) as usize;
            let total_pixels = (dst_w * dst_h) as usize;

            let mut gst_args = vec![
                "-q".to_string(),
                "pipewiresrc".to_string(),
                format!("fd={}", raw_fd),
                format!("path={}", node_id),
                "do-timestamp=true".to_string(),
                "!".to_string(),
                "videoconvert".to_string(),
                "n-threads=4".to_string(),
                "!".to_string(),
                "videoscale".to_string(),
                "!".to_string(),
            ];

            if target_fps > 0 && target_fps < 120 {
                gst_args.push("videorate".to_string());
                gst_args.push("!".to_string());
                gst_args.push(format!("video/x-raw,format=RGBA,width={},height={},framerate={}/1", dst_w, dst_h, target_fps));
            } else {
                gst_args.push(format!("video/x-raw,format=RGBA,width={},height={}", dst_w, dst_h));
            }

            gst_args.extend_from_slice(&[
                "!".to_string(),
                "queue".to_string(),
                "max-size-buffers=1".to_string(),
                "max-size-bytes=0".to_string(),
                "max-size-time=0".to_string(),
                "leaky=downstream".to_string(),
                "!".to_string(),
                "fdsink".to_string(),
                "fd=1".to_string(),
                "sync=false".to_string(),
            ]);

            log::info!("🔧 GStreamer pipeline args: {:?}", gst_args);

            let mut child = match Command::new("gst-launch-1.0")
                .args(&gst_args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    log::error!("Falha ao iniciar pipeline GStreamer PipeWire: {:?}", e);
                    let _ = session.close().await;
                    PORTAL_CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
                    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };

            // Spawn a thread to log GStreamer stderr for debugging
            if let Some(stderr) = child.stderr.take() {
                std::thread::Builder::new()
                    .name("gst-stderr-logger".to_string())
                    .spawn(move || {
                        use std::io::BufRead;
                        let reader = std::io::BufReader::new(stderr);
                        for line in reader.lines() {
                            match line {
                                Ok(l) => log::warn!("🎬 [GStreamer STDERR]: {}", l),
                                Err(_) => break,
                            }
                        }
                    })
                    .ok();
            }

            let mut stdout = match child.stdout.take() {
                Some(s) => s,
                None => {
                    let _ = session.close().await;
                    PORTAL_CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
                    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };

            if let Ok(mut lock) = PORTAL_CHILD.lock() {
                if let Some(mut old) = lock.take() {
                    let _ = old.kill();
                }
                *lock = Some(child);
            }

            let mut raw_buf = vec![0u8; frame_size];
            let mut pw_frame_count = 0u64;
            let mut last_pw_stats = std::time::Instant::now();
            let first_frame_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let mut got_first_frame = false;

            log::info!("⏳ Aguardando primeiro quadro do GStreamer PipeWire (timeout 10s)...");

            while PORTAL_INITIALIZED.load(std::sync::atomic::Ordering::Relaxed) && stdout.read_exact(&mut raw_buf).is_ok() {
                if !got_first_frame {
                    got_first_frame = true;
                    log::info!("✅ Primeiro quadro do GStreamer PipeWire recebido com sucesso!");
                }
                pw_frame_count += 1;
                let now = std::time::Instant::now();
                if now.duration_since(last_pw_stats) >= std::time::Duration::from_secs(1) {
                    let elapsed_s = now.duration_since(last_pw_stats).as_secs_f64();
                    let fps = (pw_frame_count as f64) / elapsed_s;
                    log::info!("📊 [TELEMETRIA] PipeWire GStreamer Source: {:.1} FPS ({} frames em {:.2}s)", fps, pw_frame_count, elapsed_s);
                    pw_frame_count = 0;
                    last_pw_stats = now;
                }

                let mut pixel_buffer = SharedPixelBuffer::<Rgba8Pixel>::new(dst_w, dst_h);
                pixel_buffer.make_mut_bytes().copy_from_slice(&raw_buf);

                if let Ok(cb_guard) = PORTAL_LOCAL_CB.lock() {
                    if let Some(ref local_cb) = *cb_guard {
                        local_cb(pixel_buffer.clone());
                    }
                }

                let mut rgb_bytes = vec![0u8; total_pixels * 3];
                for (chunk, rgb_chunk) in raw_buf.chunks_exact(4).zip(rgb_bytes.chunks_exact_mut(3)) {
                    rgb_chunk[0] = chunk[0];
                    rgb_chunk[1] = chunk[1];
                    rgb_chunk[2] = chunk[2];
                }

                if let Some(digit) = get_test_watermark_digit() {
                    draw_test_watermark(pixel_buffer.make_mut_slice(), &mut rgb_bytes, dst_w, dst_h, digit);
                }

                if let Ok(mut slot) = PORTAL_FRAME.lock() {
                    *slot = Some((pixel_buffer, rgb_bytes));
                }
            }

            if !got_first_frame {
                log::error!("❌ GStreamer PipeWire encerrou sem produzir nenhum quadro! Pipeline falhou.");
                // Check if child exited with an error
                if let Ok(mut lock) = PORTAL_CHILD.lock() {
                    if let Some(ref mut child) = *lock {
                        match child.try_wait() {
                            Ok(Some(status)) => log::error!("❌ GStreamer exit status: {}", status),
                            Ok(None) => log::warn!("⚠️ GStreamer ainda está rodando mas stdout fechou"),
                            Err(e) => log::error!("❌ Erro ao verificar status do GStreamer: {:?}", e),
                        }
                    }
                }
                PORTAL_CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
            }

            if let Ok(mut lock) = PORTAL_CHILD.lock() {
                if let Some(mut child) = lock.take() {
                    let _ = child.kill();
                }
            }
            let _ = session.close().await;
            log::info!("🛑 Sessão do XDG Desktop Portal ScreenCast fechada com sucesso via D-Bus!");
            PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    });
}

#[cfg(not(windows))]
fn capture_screen_rgb(target_hwnd: isize, target_w: u32, target_h: u32, target_fps: u64, out_bgra: &mut Vec<u8>) -> Option<(u128, u128)> {
    #[cfg(target_os = "linux")]
    {
        init_wayland_portal_screencast(target_w, target_h, target_fps);

        if let Ok(slot) = PORTAL_FRAME.lock() {
            if let Some(ref frame) = *slot {
                let total = (target_w * target_h * 4) as usize;
                if out_bgra.len() != total {
                    out_bgra.resize(total, 0);
                }
                return Some((0, 0));
            }
        }
    }

    let total = (target_w * target_h * 4) as usize;
    if out_bgra.len() != total {
        out_bgra.resize(total, 24);
    }
    Some((0, 0))
}

const GLYPH_1: [u8; 12] = [
    0b00011000,
    0b00111000,
    0b01111000,
    0b00011000,
    0b00011000,
    0b00011000,
    0b00011000,
    0b00011000,
    0b00011000,
    0b00011000,
    0b01111110,
    0b01111110,
];

const GLYPH_2: [u8; 12] = [
    0b00111100,
    0b01100110,
    0b01100110,
    0b00000110,
    0b00001100,
    0b00011000,
    0b00110000,
    0b01100000,
    0b01100000,
    0b01100000,
    0b01111110,
    0b01111110,
];

fn get_test_watermark_digit() -> Option<char> {
    if let Ok(val) = std::env::var("LITECORD_INSTANCE_ID") {
        let v = val.trim();
        if v == "1" { return Some('1'); }
        if v == "2" { return Some('2'); }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_str = cwd.to_string_lossy().to_lowercase();
        if cwd_str.contains("instance2") || cwd_str.contains("profile2") || cwd_str.ends_with("_2") {
            return Some('2');
        }
        if cwd_str.contains("instance1") || cwd_str.contains("profile1") || cwd_str.ends_with("_1") {
            return Some('1');
        }
    }
    None
}

fn draw_test_watermark(
    slice: &mut [Rgba8Pixel],
    rgb_bytes: &mut [u8],
    width: u32,
    height: u32,
    digit: char,
) {
    let glyph = match digit {
        '1' => &GLYPH_1,
        '2' => &GLYPH_2,
        _ => return,
    };

    let cx = (width / 2) as i32;
    let cy = (height / 2) as i32;

    // Badge Dimensions
    let half_w = 90i32;
    let half_h = 90i32;
    let border_thick = 4i32;

    // Accent color: Instance 1 = Blurple (#5865F2), Instance 2 = Pink/Magenta (#EB459E)
    let (border_r, border_g, border_b) = if digit == '1' {
        (88u8, 101u8, 242u8)
    } else {
        (235u8, 69u8, 158u8)
    };

    // Draw background badge (dark semi-transparent box with solid border)
    for dy in -half_h..=half_h {
        let y = cy + dy;
        if y < 0 || y >= height as i32 {
            continue;
        }
        for dx in -half_w..=half_w {
            let x = cx + dx;
            if x < 0 || x >= width as i32 {
                continue;
            }

            let idx = (y as usize) * (width as usize) + (x as usize);
            let offset_rgb = idx * 3;

            let is_border = dx.abs() >= half_w - border_thick || dy.abs() >= half_h - border_thick;

            let (r, g, b) = if is_border {
                (border_r, border_g, border_b)
            } else {
                let cur_r = rgb_bytes[offset_rgb];
                let cur_g = rgb_bytes[offset_rgb + 1];
                let cur_b = rgb_bytes[offset_rgb + 2];
                let bg_r = 17u8;
                let bg_g = 18u8;
                let bg_b = 20u8;
                let br = ((cur_r as u16 * 2 + bg_r as u16 * 8) / 10) as u8;
                let bg = ((cur_g as u16 * 2 + bg_g as u16 * 8) / 10) as u8;
                let bb = ((cur_b as u16 * 2 + bg_b as u16 * 8) / 10) as u8;
                (br, bg, bb)
            };

            slice[idx] = Rgba8Pixel::new(r, g, b, 255);
            rgb_bytes[offset_rgb] = r;
            rgb_bytes[offset_rgb + 1] = g;
            rgb_bytes[offset_rgb + 2] = b;
        }
    }

    // Draw the big scaled digit in the center of the badge
    let scale = 9i32; // 8 cols * 9 = 72px width, 12 rows * 9 = 108px height
    let glyph_w = 8 * scale;
    let glyph_h = 12 * scale;
    let start_x = cx - glyph_w / 2;
    let start_y = cy - glyph_h / 2;

    for (row_idx, &row_bits) in glyph.iter().enumerate() {
        for col_idx in 0..8 {
            if (row_bits & (1 << (7 - col_idx))) != 0 {
                for sy in 0..scale {
                    let y = start_y + (row_idx as i32) * scale + sy;
                    if y < 0 || y >= height as i32 {
                        continue;
                    }
                    for sx in 0..scale {
                        let x = start_x + (col_idx as i32) * scale + sx;
                        if x < 0 || x >= width as i32 {
                            continue;
                        }

                        let idx = (y as usize) * (width as usize) + (x as usize);
                        let offset_rgb = idx * 3;

                        let r = 255u8;
                        let g = 255u8;
                        let b = 255u8;

                        slice[idx] = Rgba8Pixel::new(r, g, b, 255);
                        rgb_bytes[offset_rgb] = r;
                        rgb_bytes[offset_rgb + 1] = g;
                        rgb_bytes[offset_rgb + 2] = b;
                    }
                }
            }
        }
    }
}

static ACTIVE_STREAM_FRAMES: std::sync::OnceLock<Arc<Mutex<HashMap<u64, SharedPixelBuffer<Rgba8Pixel>>>>> = std::sync::OnceLock::new();
static ACTIVE_STREAM_USERS: std::sync::OnceLock<Arc<Mutex<HashMap<u64, bool>>>> = std::sync::OnceLock::new();

pub fn get_active_stream_frames() -> Arc<Mutex<HashMap<u64, SharedPixelBuffer<Rgba8Pixel>>>> {
    ACTIVE_STREAM_FRAMES.get_or_init(|| Arc::new(Mutex::new(HashMap::new()))).clone()
}

pub fn get_active_stream_users() -> Arc<Mutex<HashMap<u64, bool>>> {
    ACTIVE_STREAM_USERS.get_or_init(|| Arc::new(Mutex::new(HashMap::new()))).clone()
}

pub fn is_user_streaming(uid: u64) -> bool {
    if let Ok(users) = get_active_stream_users().lock() {
        users.get(&uid).copied().unwrap_or(false)
    } else {
        false
    }
}

pub fn get_user_stream_frame(uid: u64) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    if let Ok(frames) = get_active_stream_frames().lock() {
        frames.get(&uid).cloned()
    } else {
        None
    }
}

// ==========================================
// PURPLE WINDOW CAPTURE BORDER OVERLAY (DWM)
// ==========================================
static OVERLAY_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static OVERLAY_HWND: std::sync::Mutex<Option<isize>> = std::sync::Mutex::new(None);

#[cfg(windows)]
pub fn start_window_border_overlay(target_hwnd: isize) {
    if target_hwnd == 0 {
        return;
    }
    stop_window_border_overlay();
    OVERLAY_ACTIVE.store(true, Ordering::SeqCst);

    std::thread::Builder::new()
        .name("window-border-overlay".to_string())
        .spawn(move || {
            use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
            use windows_sys::Win32::Graphics::Gdi::{
                BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, FrameRect,
                InvalidateRect, PAINTSTRUCT,
            };
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetWindowRect,
                IsIconic, IsWindow, PeekMessageW, PostQuitMessage, RegisterClassW,
                SetLayeredWindowAttributes, SetWindowPos, ShowWindow, TranslateMessage, CS_HREDRAW,
                CS_VREDRAW, HWND_TOPMOST, LWA_COLORKEY, MSG, PM_REMOVE, SWP_NOACTIVATE,
                SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE, WM_DESTROY, WM_ERASEBKGND, WM_PAINT,
                WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
                WS_EX_TRANSPARENT, WS_POPUP,
            };

            unsafe extern "system" fn overlay_wndproc(
                hwnd: HWND,
                msg: u32,
                wparam: WPARAM,
                lparam: LPARAM,
            ) -> LRESULT {
                match msg {
                    WM_PAINT => {
                        let mut ps: PAINTSTRUCT = std::mem::zeroed();
                        let hdc = BeginPaint(hwnd, &mut ps);
                        if !hdc.is_null() {
                            let mut rc: RECT = std::mem::zeroed();
                            GetWindowRect(hwnd, &mut rc);
                            let width = rc.right - rc.left;
                            let height = rc.bottom - rc.top;

                            // Fill entire area with black color key (transparent & click-through)
                            let black_brush = CreateSolidBrush(0x00000000);
                            let client_rc = RECT { left: 0, top: 0, right: width, bottom: height };
                            FillRect(hdc, &client_rc, black_brush);
                            DeleteObject(black_brush);

                            // Draw a vibrant 3px purple border (Discord-style Blurple / Purple #7C5CFC: 0x00FC5C7C)
                            let purple_brush = CreateSolidBrush(0x00FC5C7C); // COLORREF: 0x00BBGGRR -> R:124(0x7C), G:92(0x5C), B:252(0xFC)
                            for thickness in 0..3 {
                                let border_rc = RECT {
                                    left: thickness,
                                    top: thickness,
                                    right: width - thickness,
                                    bottom: height - thickness,
                                };
                                FrameRect(hdc, &border_rc, purple_brush);
                            }
                            DeleteObject(purple_brush);

                            EndPaint(hwnd, &mut ps);
                        }
                        0
                    }
                    WM_ERASEBKGND => 1,
                    WM_DESTROY => {
                        PostQuitMessage(0);
                        0
                    }
                    _ => DefWindowProcW(hwnd, msg, wparam, lparam),
                }
            }

            unsafe {
                let class_name: Vec<u16> = "LitecordCaptureBorder\0".encode_utf16().collect();
                let mut wc: WNDCLASSW = std::mem::zeroed();
                wc.style = CS_HREDRAW | CS_VREDRAW;
                wc.lpfnWndProc = Some(overlay_wndproc);
                wc.lpszClassName = class_name.as_ptr();
                RegisterClassW(&wc);

                let hwnd_target = target_hwnd as HWND;
                if IsWindow(hwnd_target) == 0 {
                    return;
                }

                #[link(name = "dwmapi")]
                extern "system" {
                    fn DwmGetWindowAttribute(
                        hwnd: HWND,
                        dwAttribute: u32,
                        pvAttribute: *mut std::ffi::c_void,
                        cbAttribute: u32,
                    ) -> i32;
                }

                let get_window_bounds = |h: HWND| -> RECT {
                    let mut rc: RECT = std::mem::zeroed();
                    // DWMWA_EXTENDED_FRAME_BOUNDS = 9: Obtém as dimensões visíveis exatas da janela, excluindo sombras invisíveis do DWM
                    let hr = DwmGetWindowAttribute(
                        h,
                        9,
                        &mut rc as *mut _ as *mut std::ffi::c_void,
                        std::mem::size_of::<RECT>() as u32,
                    );
                    if hr != 0 || (rc.right - rc.left) <= 0 || (rc.bottom - rc.top) <= 0 {
                        GetWindowRect(h, &mut rc);
                    }
                    rc
                };

                let target_rc = get_window_bounds(hwnd_target);
                let x = target_rc.left;
                let y = target_rc.top;
                let w = (target_rc.right - target_rc.left).max(1);
                let h = (target_rc.bottom - target_rc.top).max(1);

                let overlay_hwnd = CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                    class_name.as_ptr(),
                    std::ptr::null(),
                    WS_POPUP,
                    x,
                    y,
                    w,
                    h,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );

                if overlay_hwnd.is_null() {
                    return;
                }

                // Set colorkey transparency (0x00000000 black is transparent)
                SetLayeredWindowAttributes(overlay_hwnd, 0x00000000, 255, LWA_COLORKEY);

                // Exclude overlay from all screen captures (WDA_EXCLUDEFROMCAPTURE = 0x00000011)
                #[link(name = "user32")]
                extern "system" {
                    fn SetWindowDisplayAffinity(hwnd: HWND, dwAffinity: u32) -> windows_sys::Win32::Foundation::BOOL;
                }
                SetWindowDisplayAffinity(overlay_hwnd, 0x00000011);

                if let Ok(mut g) = OVERLAY_HWND.lock() {
                    *g = Some(overlay_hwnd as isize);
                }

                ShowWindow(overlay_hwnd, SW_SHOWNOACTIVATE);

                let mut last_rc: RECT = std::mem::zeroed();

                while OVERLAY_ACTIVE.load(Ordering::Relaxed) {
                    if IsWindow(hwnd_target) == 0 || IsIconic(hwnd_target) != 0 {
                        ShowWindow(overlay_hwnd, SW_HIDE);
                    } else {
                        let current_rc = get_window_bounds(hwnd_target);

                        if current_rc.left != last_rc.left
                            || current_rc.top != last_rc.top
                            || current_rc.right != last_rc.right
                            || current_rc.bottom != last_rc.bottom
                        {
                            last_rc = current_rc;
                            let nx = current_rc.left;
                            let ny = current_rc.top;
                            let nw = (current_rc.right - current_rc.left).max(1);
                            let nh = (current_rc.bottom - current_rc.top).max(1);

                            SetWindowPos(
                                overlay_hwnd,
                                HWND_TOPMOST,
                                nx,
                                ny,
                                nw,
                                nh,
                                SWP_NOACTIVATE | SWP_SHOWWINDOW,
                            );
                            InvalidateRect(overlay_hwnd, std::ptr::null(), 0);
                        } else {
                            ShowWindow(overlay_hwnd, SW_SHOWNOACTIVATE);
                        }
                    }

                    // Process pending overlay window messages
                    let mut msg: MSG = std::mem::zeroed();
                    while PeekMessageW(&mut msg, overlay_hwnd, 0, 0, PM_REMOVE) != 0 {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }

                    std::thread::sleep(Duration::from_millis(30));
                }

                ShowWindow(overlay_hwnd, SW_HIDE);
                DestroyWindow(overlay_hwnd);
                if let Ok(mut g) = OVERLAY_HWND.lock() {
                    *g = None;
                }
            }
        })
        .expect("Falha ao criar thread de overlay da borda de janela");
}

#[cfg(not(windows))]
pub fn start_window_border_overlay(_target_hwnd: isize) {}

pub fn stop_window_border_overlay() {
    OVERLAY_ACTIVE.store(false, Ordering::SeqCst);
}

// ==========================================
// STREAM AUDIO SUBSYSTEM (LOOPBACK & PLAYBACK)
// ==========================================
static STREAM_AUDIO_PLAYBACK_INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
static STREAM_AUDIO_QUEUE: std::sync::OnceLock<Arc<Mutex<VecDeque<f32>>>> = std::sync::OnceLock::new();
static STREAM_VOLUMES: std::sync::OnceLock<Arc<Mutex<HashMap<u64, f32>>>> = std::sync::OnceLock::new();

pub fn get_stream_audio_queue() -> Arc<Mutex<VecDeque<f32>>> {
    STREAM_AUDIO_QUEUE.get_or_init(|| Arc::new(Mutex::new(VecDeque::with_capacity(48000 * 2)))).clone()
}

pub fn clear_stream_audio_queue() {
    if let Ok(mut q) = get_stream_audio_queue().lock() {
        q.clear();
    }
}

pub fn get_stream_volumes() -> Arc<Mutex<HashMap<u64, f32>>> {
    STREAM_VOLUMES.get_or_init(|| Arc::new(Mutex::new(HashMap::new()))).clone()
}

pub fn set_stream_volume(uid: u64, vol: f32) {
    if let Ok(mut map) = get_stream_volumes().lock() {
        map.insert(uid, vol.clamp(0.0, 2.0));
    }
}

pub fn get_stream_volume(uid: u64) -> f32 {
    if let Ok(map) = get_stream_volumes().lock() {
        map.get(&uid).copied().unwrap_or(1.0)
    } else {
        1.0
    }
}

pub fn ensure_stream_audio_playback_started() {
    STREAM_AUDIO_PLAYBACK_INIT.get_or_init(|| {
        std::thread::Builder::new()
            .name("stream-audio-playback".to_string())
            .spawn(|| {
                let host = cpal::default_host();
                let device = match host.default_output_device() {
                    Some(d) => d,
                    None => return,
                };
                let config = match device.default_output_config() {
                    Ok(c) => c,
                    Err(_) => return,
                };
                let channels = config.channels() as usize;
                let queue = get_stream_audio_queue();

                let last_err_time = Arc::new(Mutex::new(Option::<Instant>::None));
                let last_err_c = Arc::clone(&last_err_time);
                let err_fn = move |err| {
                    if let Ok(mut guard) = last_err_c.lock() {
                        if guard.map_or(true, |t| t.elapsed() >= Duration::from_secs(5)) {
                            *guard = Some(Instant::now());
                            warn!("Erro no playback de áudio do stream: {}", err);
                        }
                    }
                };

                let stream_res = match config.sample_format() {
                    cpal::SampleFormat::F32 => {
                        let q = Arc::clone(&queue);
                        device.build_output_stream(
                            &config.into(),
                            move |data: &mut [f32], _| {
                                let mut queue_guard = q.lock().unwrap();
                                let q_len = queue_guard.len();
                                if q_len > 4800 {
                                    let excess = q_len - 2400;
                                    queue_guard.drain(0..excess);
                                }
                                for chunk in data.chunks_mut(channels) {
                                    let sample = queue_guard.pop_front().unwrap_or(0.0);
                                    for channel_sample in chunk.iter_mut() {
                                        *channel_sample = sample;
                                    }
                                }
                            },
                            err_fn,
                            None,
                        )
                    }
                    cpal::SampleFormat::I16 => {
                        let q = Arc::clone(&queue);
                        device.build_output_stream(
                            &config.into(),
                            move |data: &mut [i16], _| {
                                let mut queue_guard = q.lock().unwrap();
                                let q_len = queue_guard.len();
                                if q_len > 4800 {
                                    let excess = q_len - 2400;
                                    queue_guard.drain(0..excess);
                                }
                                for chunk in data.chunks_mut(channels) {
                                    let sample_f = queue_guard.pop_front().unwrap_or(0.0);
                                    let sample_i = (sample_f.clamp(-1.0, 1.0) * 32767.0) as i16;
                                    for channel_sample in chunk.iter_mut() {
                                        *channel_sample = sample_i;
                                    }
                                }
                            },
                            err_fn,
                            None,
                        )
                    }
                    _ => return,
                };

                if let Ok(stream) = stream_res {
                    let _ = stream.play();
                    info!("🔊 Saída de áudio da transmissão inicializada.");
                    loop {
                        std::thread::sleep(Duration::from_secs(3600));
                    }
                }
            })
            .expect("Falha ao iniciar thread de playback de áudio do stream");
    });
}

pub fn start_audio_loopback_tx(
    is_running: Arc<AtomicBool>,
    channel_id: Arc<AtomicU64>,
    my_user_id: Arc<AtomicU64>,
    peers_store: Arc<Mutex<HashMap<u64, (SocketAddr, Instant)>>>,
) {
    std::thread::Builder::new()
        .name("audio-loopback-tx".to_string())
        .spawn(move || {
            let host = cpal::default_host();
            let device = match host.default_output_device() {
                Some(d) => d,
                None => {
                    warn!("⚠️ Dispositivo de áudio de saída padrão não encontrado para loopback");
                    return;
                }
            };

            let config_res = device.default_output_config();
            if let Err(e) = config_res {
                warn!("⚠️ Falha ao obter configuração de áudio loopback: {:?}", e);
                return;
            }
            let config = config_res.unwrap();
            let sample_rate = config.sample_rate().0;
            let channels = config.channels() as usize;

            let socket = get_shared_p2p_socket().unwrap_or_else(|| {
                let s = UdpSocket::bind("0.0.0.0:0").unwrap();
                let _ = s.set_broadcast(true);
                Arc::new(s)
            });

            let bcast_targets = get_broadcast_addresses();
            let mut seq: u32 = 0;
            let pcm_buffer: Arc<Mutex<Vec<i16>>> = Arc::new(Mutex::new(Vec::with_capacity(4800)));
            let pcm_buffer_cb = Arc::clone(&pcm_buffer);

            let target_chunk_samples = ((sample_rate as usize) * 20) / 1000; // 20ms of audio

            let err_fn = |err| {
                warn!("Erro na captura de áudio loopback: {}", err);
            };

            let stream_res = match config.sample_format() {
                cpal::SampleFormat::F32 => {
                    let pcm_buf = Arc::clone(&pcm_buffer_cb);
                    device.build_input_stream(
                        &config.into(),
                        move |data: &[f32], _| {
                            let mut buf = pcm_buf.lock().unwrap();
                            if channels == 1 {
                                for &s in data {
                                    let sample_i16 = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                                    buf.push(sample_i16);
                                }
                            } else if channels >= 2 {
                                for chunk in data.chunks_exact(channels) {
                                    let mono_f = (chunk[0] + chunk[1]) * 0.5;
                                    let sample_i16 = (mono_f.clamp(-1.0, 1.0) * 32767.0) as i16;
                                    buf.push(sample_i16);
                                }
                            }
                        },
                        err_fn,
                        None,
                    )
                }
                cpal::SampleFormat::I16 => {
                    let pcm_buf = Arc::clone(&pcm_buffer_cb);
                    device.build_input_stream(
                        &config.into(),
                        move |data: &[i16], _| {
                            let mut buf = pcm_buf.lock().unwrap();
                            if channels == 1 {
                                buf.extend_from_slice(data);
                            } else if channels >= 2 {
                                for chunk in data.chunks_exact(channels) {
                                    let mono = ((chunk[0] as i32 + chunk[1] as i32) / 2) as i16;
                                    buf.push(mono);
                                }
                            }
                        },
                        err_fn,
                        None,
                    )
                }
                _ => {
                    warn!("Formato de áudio não suportado para loopback");
                    return;
                }
            };

            let stream = match stream_res {
                Ok(s) => s,
                Err(e) => {
                    warn!("⚠️ Falha ao inicializar stream de captura de áudio loopback: {:?}", e);
                    return;
                }
            };

            if let Err(e) = stream.play() {
                warn!("⚠️ Falha ao iniciar gravação do áudio loopback: {:?}", e);
                return;
            }

            info!("🔊 Captura de áudio da transmissão iniciada ({}Hz mono)...", sample_rate);

            while is_running.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(15));

                let chunks_to_send: Vec<Vec<i16>> = {
                    let mut buf = pcm_buffer.lock().unwrap();
                    let mut res = Vec::new();
                    // Prevent TX buffer accumulation
                    if buf.len() > target_chunk_samples * 6 {
                        let excess = buf.len() - target_chunk_samples * 2;
                        buf.drain(0..excess);
                    }
                    while buf.len() >= target_chunk_samples && target_chunk_samples > 0 {
                        let chunk: Vec<i16> = buf.drain(0..target_chunk_samples).collect();
                        res.push(chunk);
                    }
                    res
                };

                let cid = channel_id.load(Ordering::Relaxed);
                let uid = my_user_id.load(Ordering::Relaxed);
                let inst = get_process_instance_id();
                let sec_key = get_voice_encryption_key(cid);

                for chunk in chunks_to_send {
                    seq = seq.wrapping_add(1);
                    let pts_ms = get_tx_pts_ms();
                    let sample_count = chunk.len() as u16;
                    let mut raw_pcm = Vec::with_capacity(chunk.len() * 2);
                    for s in chunk {
                        raw_pcm.extend_from_slice(&s.to_le_bytes());
                    }
                    let encrypted_pcm = encrypt_signaling_payload(&sec_key, &raw_pcm).unwrap_or(raw_pcm);

                    let mut pkt = Vec::with_capacity(36 + encrypted_pcm.len());
                    pkt.extend_from_slice(MAGIC);
                    pkt.extend_from_slice(&inst.to_be_bytes());
                    pkt.push(OP_AUDIO_FRAME);
                    pkt.extend_from_slice(&cid.to_be_bytes());
                    pkt.extend_from_slice(&uid.to_be_bytes());
                    pkt.extend_from_slice(&seq.to_be_bytes());
                    pkt.extend_from_slice(&pts_ms.to_be_bytes());
                    pkt.push(1); // 1 channel
                    pkt.extend_from_slice(&sample_rate.to_be_bytes());
                    pkt.extend_from_slice(&sample_count.to_be_bytes());
                    pkt.extend_from_slice(&encrypted_pcm);

                    let mut target_addrs: Vec<SocketAddr> = Vec::with_capacity(4);
                    target_addrs.push(SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)), 50005));
                    let mut has_remote = false;
                    if let Ok(peers) = peers_store.lock() {
                        for (&_, &(addr, _)) in peers.iter() {
                            if !target_addrs.contains(&addr) {
                                target_addrs.push(addr);
                                has_remote = true;
                            }
                        }
                    }
                    if !has_remote {
                        for target in &bcast_targets {
                            if !target_addrs.contains(target) {
                                target_addrs.push(*target);
                            }
                        }
                    }
                    for target in &target_addrs {
                        let _ = socket.send_to(&pkt, target);
                    }
                }
            }

            drop(stream);
            info!("🔊 Captura de áudio da transmissão finalizada.");
        })
        .expect("Falha ao iniciar thread de captura de áudio da transmissão");
}
