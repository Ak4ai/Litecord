#![allow(dead_code)]

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use log::{info, warn};
use slint::{Rgba8Pixel, SharedPixelBuffer};

use super::types::*;
use super::crypto::*;
use super::signaling::*;
use super::video_pipeline::*;
use super::sources::*;
use super::audio_loopback::*;

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
        let mut uname = self.my_username.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if uname.is_empty() {
            uname = crate::gateway::get_my_username();
            if !uname.is_empty() {
                *self.my_username.lock().unwrap_or_else(|e| e.into_inner()) = uname.clone();
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

            if let Ok(peers) = self.known_peers.lock() {
                for (&_, &(addr, _)) in peers.iter() {
                    let _ = socket.send_to(&ack_pkt, addr);
                }
            }
            if let Ok(last_guard) = LAST_SEEN_PEER_ADDR.lock() {
                if let Some(map) = last_guard.as_ref() {
                    for (&p_uid, &addr) in map.iter() {
                        if p_uid != my_uid {
                            let _ = socket.send_to(&ack_pkt, addr);
                        }
                    }
                }
            }
        }
    }

    pub fn register_remote_peer(&self, uid: u64, addr: SocketAddr) {
        if let Ok(mut peers) = self.known_peers.lock() {
            peers.insert(uid, (addr, Instant::now()));
            info!("🌐 Peer remoto P2P registrado: User ID {} -> {}", uid, addr);
        }
        request_intra_keyframe();
        let current_cid = self.channel_id.load(Ordering::Relaxed);
        let my_uid = self.my_user_id.load(Ordering::Relaxed);
        let my_instance_id = get_process_instance_id();
        let my_rx = get_my_rx_port();
        let uname = self.my_username.lock().unwrap_or_else(|e| e.into_inner()).clone();
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

            // 1. Notifica imediatamente o canal de sinalização Cloudflare para fechar o stream em <50ms
            notify_signaling_stream_state(cid, uid, false);

            let known_peers_clone = Arc::clone(&self.known_peers);
            // 2. Dispara burst robusto em background thread para não travar a UI e garantir entrega sobre redes lentas/WAN
            std::thread::Builder::new()
                .name("p2p-stop-burst".to_string())
                .spawn(move || {
                    let socket_opt = get_shared_p2p_socket();
                    let temp_sock;
                    let sock_ref = if let Some(ref s) = socket_opt {
                        s.as_ref()
                    } else if let Ok(s) = UdpSocket::bind("0.0.0.0:0") {
                        temp_sock = s;
                        &temp_sock
                    } else {
                        return;
                    };

                    let inst = get_process_instance_id();
                    let mut stop_pkt = Vec::with_capacity(25);
                    stop_pkt.extend_from_slice(MAGIC);
                    stop_pkt.extend_from_slice(&inst.to_be_bytes());
                    stop_pkt.push(OP_STOP);
                    stop_pkt.extend_from_slice(&cid.to_be_bytes());
                    stop_pkt.extend_from_slice(&uid.to_be_bytes());

                    for _ in 0..10 {
                        if let Ok(peers) = known_peers_clone.lock() {
                            for (&_, &(addr, _)) in peers.iter() {
                                let _ = sock_ref.send_to(&stop_pkt, addr);
                            }
                        }
                        if let Ok(last_guard) = LAST_SEEN_PEER_ADDR.lock() {
                            if let Some(map) = last_guard.as_ref() {
                                for (&_p_uid, &addr) in map.iter() {
                                    let _ = sock_ref.send_to(&stop_pkt, addr);
                                }
                            }
                        }
                        std::thread::sleep(Duration::from_millis(60));
                    }
                })
                .ok();
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
        force_signaling_broadcast();

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
                crate::cpu_profiler::set_current_thread_name("screen-capture-tx");

                let mut h264_encoder: Option<Box<dyn crate::encoder::VideoEncoder>> = Some(
                    crate::encoder::create_best_encoder(target_fps as u32, camera_index.is_none())
                );

                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    crate::trim_process_memory();
                });

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
                                cameras::PixelFormat::Bgra8,
                                cameras::PixelFormat::Rgba8,
                                cameras::PixelFormat::Yuyv,
                                cameras::PixelFormat::Rgb8,
                            ];
                            let resolutions = [
                                cameras::Resolution { width: cam_w, height: cam_h },
                                cameras::Resolution { width: 1280, height: 720 },
                                cameras::Resolution { width: 640, height: 480 },
                                cameras::Resolution { width: 640, height: 360 },
                                cameras::Resolution { width: 1920, height: 1080 },
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
                                    crate::cpu_profiler::set_current_thread_name("dxgi-duplication-thread");
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
                                        if let Ok(frame) = cameras::next_frame(cam, Duration::from_millis(200)) {
                                            let (w, h) = (frame.width, frame.height);
                                            let converted = std::panic::catch_unwind(|| {
                                                cameras::to_rgba8(&frame)
                                            });
                                            if let Ok(Ok(mut rgba_bytes)) = converted {
                                                for chunk in rgba_bytes.chunks_exact_mut(4) {
                                                    chunk.swap(0, 2); // RGBA -> BGRA
                                                }
                                                let total_canvas_bytes = (target_w * target_h * 4) as usize;
                                                cur_buf.resize(total_canvas_bytes, 0);
                                                fit_bgra_to_canvas(&rgba_bytes, w, h, target_w, target_h, &mut cur_buf);
                                                Some((target_w, target_h, 0u128, 0u128))
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
                let mut total_preview_us = 0u128;
                let mut total_crypt_us = 0u128;
                let mut total_fec_us = 0u128;
                let mut total_net_us = 0u128;
                let mut total_blt_us = 0u128;
                let mut total_pix_us = 0u128;
                let mut last_local_preview = Instant::now() - Duration::from_secs(1);
                let mut last_announce = Instant::now() - Duration::from_secs(10);
                let mut last_idr = Instant::now() - Duration::from_secs(10);
                let mut frame_seq: u32 = 0;

                let frame_target_interval = Duration::from_micros(1_000_000 / target_fps.max(1));
                let mut next_tick = Instant::now();
                let mut latest_cached_frame: Option<(Vec<u8>, u32, u32, u128, u128)> = None;

                while is_running.load(Ordering::Relaxed) {
                    let mut cid = channel_id_atomic.load(Ordering::Relaxed);
                    if cid == 0 {
                        cid = crate::gateway::get_my_voice_channel_id();
                        if cid > 0 {
                            channel_id_atomic.store(cid, Ordering::Relaxed);
                        }
                    }
                    let mut uid = my_user_id_atomic.load(Ordering::Relaxed);
                    if uid == 0 {
                        uid = crate::gateway::get_my_user_id();
                        if uid > 0 {
                            my_user_id_atomic.store(uid, Ordering::Relaxed);
                        }
                    }

                    // Announce presence periodically
                    if last_announce.elapsed() > Duration::from_millis(1500) {
                        let uname = my_username_arc.lock().unwrap_or_else(|e| e.into_inner()).clone();
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

                        if let Ok(peers) = peers_store.lock() {
                            for (&_, &(addr, _)) in peers.iter() {
                                let _ = socket.send_to(&ann_pkt, addr);
                            }
                        }
                        if let Ok(last_guard) = LAST_SEEN_PEER_ADDR.lock() {
                            if let Some(map) = last_guard.as_ref() {
                                for (&p_uid, &addr) in map.iter() {
                                    if p_uid != uid {
                                        let _ = socket.send_to(&ann_pkt, addr);
                                    }
                                }
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

                    // Se nenhum quadro chegou ainda, aguarda até 20ms ou usa fallback GDI imediato para captura de tela (vital para notebooks com GPUs híbridas)
                    if latest_cached_frame.is_none() {
                        match rx_frame.recv_timeout(Duration::from_millis(20)) {
                            Ok(first_frame) => {
                                latest_cached_frame = Some(first_frame);
                            }
                            Err(_) => {
                                if camera_index.is_none() {
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
                                } else {
                                    continue;
                                }
                            }
                        }
                    }

                    // Pacer CFR de precisão (Sem busy spinloop para 0% CPU overhead)
                    let now = Instant::now();
                    if now < next_tick {
                        let sleep_dur = (next_tick - now).min(frame_target_interval);
                        std::thread::sleep(sleep_dur);
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

                    let t_preview_start = Instant::now();
                    // Decouple UI local preview with fast downsampler (480w) to eliminate UI thread lag and memory overhead
                    if last_local_preview.elapsed() >= Duration::from_millis(250) {
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
                    let t_preview_us = t_preview_start.elapsed().as_micros();
                    total_preview_us += t_preview_us;

                    let enc_start = Instant::now();
                    let frame_bytes_opt = if let Some(ref mut enc) = h264_encoder {
                        let target_bitrate = REQUESTED_BITRATE_BPS.load(Ordering::Relaxed);
                        if enc.get_bitrate_bps() != target_bitrate {
                            enc.set_bitrate_bps(target_bitrate);
                        }
                        if KEYFRAME_REQUESTED.swap(false, Ordering::Relaxed) || last_idr.elapsed() >= Duration::from_millis(2000) {
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
                        let t_crypt_start = Instant::now();
                        let sec_key = get_voice_encryption_key(cid);
                        let frame_bytes = encrypt_signaling_payload(&sec_key, &raw_frame_bytes).unwrap_or(raw_frame_bytes);
                        let t_crypt_us = t_crypt_start.elapsed().as_micros();
                        total_crypt_us += t_crypt_us;

                        frame_seq = frame_seq.wrapping_add(1);
                        let total_len = frame_bytes.len();
                        let total_chunks = ((total_len + CHUNK_SIZE - 1) / CHUNK_SIZE) as u16;

                        // Send directly to verified active peers in current voice channel
                        let mut target_addrs: Vec<SocketAddr> = Vec::with_capacity(8);

                        if let Ok(guard) = LAST_SEEN_PEER_ADDR.lock() {
                            if let Some(map) = guard.as_ref() {
                                for (&p_uid, &active_addr) in map.iter() {
                                    if p_uid != uid && !is_tailscale_or_forbidden(&active_addr) && !target_addrs.contains(&active_addr) {
                                        target_addrs.push(active_addr);
                                    }
                                }
                            }
                        }

                        if let Ok(mut peers) = peers_store.lock() {
                            let now = Instant::now();
                            peers.retain(|_, (_, seen)| {
                                now.duration_since(*seen) < Duration::from_secs(60)
                            });
                            for (&_p_key, &(addr, _)) in peers.iter() {
                                if !is_tailscale_or_forbidden(&addr) && !target_addrs.contains(&addr) {
                                    target_addrs.push(addr);
                                }
                            }
                        }
                        if let Ok(last_guard) = LAST_SEEN_PEER_ADDR.lock() {
                            if let Some(map) = last_guard.as_ref() {
                                for (&p_uid, &addr) in map.iter() {
                                    if p_uid != uid && !is_tailscale_or_forbidden(&addr) && !target_addrs.contains(&addr) {
                                        target_addrs.push(addr);
                                    }
                                }
                            }
                        }

                        let t_fec_start = Instant::now();
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
                        let t_fec_us = t_fec_start.elapsed().as_micros();
                        total_fec_us += t_fec_us;

                        let pts_ms = get_tx_pts_ms();
                        let t_net_start = Instant::now();

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

                            // Sunshine micro-pacing: pausa microscópica a cada 4 pacotes para evitar bufferbloat
                            if (chunk_idx + 1) % 4 == 0 && (chunk_idx + 1) < total_chunks {
                                std::thread::yield_now();
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
                        let t_net_us = t_net_start.elapsed().as_micros();
                        total_net_us += t_net_us;
                    }

                    tx_frame_count += 1;
                    if last_tx_stats.elapsed() >= Duration::from_secs(1) {
                        let elapsed_s = last_tx_stats.elapsed().as_secs_f64();
                        let fps = (tx_frame_count as f64) / elapsed_s;
                        CURRENT_TX_FPS.store((fps * 10.0).round() as u32, Ordering::Relaxed);
                        let n = tx_frame_count.max(1) as f64;
                        let avg_enc_ms = (total_encode_us as f64) / n / 1000.0;
                        let avg_preview_ms = (total_preview_us as f64) / n / 1000.0;
                        let avg_crypt_ms = (total_crypt_us as f64) / n / 1000.0;
                        let avg_fec_ms = (total_fec_us as f64) / n / 1000.0;
                        let avg_net_ms = (total_net_us as f64) / n / 1000.0;
                        let _avg_blt_ms = (total_blt_us as f64) / n / 1000.0;
                        let _avg_pix_ms = (total_pix_us as f64) / n / 1000.0;
                        let cur_mbps = (REQUESTED_BITRATE_BPS.load(Ordering::Relaxed) as f64) / 1_000_000.0;

                        info!("📊 [DETALHAMENTO DE CPU & GPU (TX)] FPS: {:.1}/{} | Bitrate: {:.2} Mbps | GPU Encode: {:.2}ms | UI Preview: {:.2}ms | Cripto E2EE: {:.2}ms | FEC: {:.2}ms | Rede UDP: {:.2}ms",
                            fps, target_fps, cur_mbps, avg_enc_ms, avg_preview_ms, avg_crypt_ms, avg_fec_ms, avg_net_ms);

                        tx_frame_count = 0;
                        total_encode_us = 0;
                        total_preview_us = 0;
                        total_crypt_us = 0;
                        total_fec_us = 0;
                        total_net_us = 0;
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

        let on_state_arc: Arc<dyn Fn(u64, bool) + Send + Sync + 'static> = Arc::new(on_state);
        if let Ok(mut guard) = ON_STREAM_STATE_CB.lock() {
            *guard = Some(Arc::clone(&on_state_arc));
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

        let (tx_decode, rx_decode) = std::sync::mpsc::sync_channel::<QueuedVideoFrame>(120);

        let is_running_decoder = Arc::clone(&self.is_receiver_running);
        let channel_id_decoder = Arc::clone(&self.channel_id);
        let my_user_id_decoder = Arc::clone(&self.my_user_id);
        let peers_store_decoder = Arc::clone(&self.known_peers);

        // Dedicated Video Decoder Worker Thread (Sunshine / Moonlight Architecture)
        std::thread::Builder::new()
            .name("video-decoder-worker".to_string())
            .spawn(move || {
                crate::cpu_profiler::set_current_thread_name("video-decoder-worker");
                info!("🎬 [VIDEO DECODER WORKER] Thread dedicada de decodificação H.264 iniciada com buffer elástico de 120 quadros!");
                let mut h264_decoders: HashMap<u64, openh264::decoder::Decoder> = HashMap::new();
                let mut last_pli_req: HashMap<u64, Instant> = HashMap::new();
                let mut rx_frame_count: HashMap<u64, u64> = HashMap::new();
                let mut rx_last_stats: HashMap<u64, Instant> = HashMap::new();
                let mut rx_error_streak: HashMap<u64, u32> = HashMap::new();
                // Aguardando IDR por peer: quando true, dropa frames até receber SPS/IDR
                let mut waiting_for_idr: HashMap<u64, bool> = HashMap::new();

                while is_running_decoder.load(Ordering::Relaxed) {
                    match rx_decode.recv_timeout(Duration::from_millis(100)) {
                        Ok(item) => {
                            let QueuedVideoFrame { peer_uid, peer_name, peer_fps, seq, pts_ms, frame_data } = item;
                            let t_dec_start = Instant::now();

                            let mut send_pli = |target_uid: u64| {
                                let should_req_pli = match last_pli_req.get(&target_uid) {
                                    Some(&t) => t.elapsed() >= Duration::from_millis(300),
                                    None => true,
                                };
                                if should_req_pli {
                                    last_pli_req.insert(target_uid, Instant::now());
                                    let current_cid = channel_id_decoder.load(Ordering::Relaxed);
                                    let my_uid = my_user_id_decoder.load(Ordering::Relaxed);
                                    let inst = get_process_instance_id();
                                    let mut req_pkt = Vec::with_capacity(33);
                                    req_pkt.extend_from_slice(MAGIC);
                                    req_pkt.extend_from_slice(&inst.to_be_bytes());
                                    req_pkt.push(OP_KEYFRAME_REQ);
                                    req_pkt.extend_from_slice(&current_cid.to_be_bytes());
                                    req_pkt.extend_from_slice(&my_uid.to_be_bytes());
                                    req_pkt.extend_from_slice(&target_uid.to_be_bytes());
                                    if let Ok(guard) = SHARED_P2P_SOCKET.lock() {
                                        if let Some(sock) = guard.as_ref() {
                                            if let Ok(peers) = peers_store_decoder.lock() {
                                                let mut sent_targets = Vec::new();
                                                if let Ok(last_guard) = LAST_SEEN_PEER_ADDR.lock() {
                                                    if let Some(map) = last_guard.as_ref() {
                                                        if let Some(&direct_addr) = map.get(&target_uid) {
                                                            if !is_tailscale_or_forbidden(&direct_addr) {
                                                                let _ = sock.send_to(&req_pkt, direct_addr);
                                                                sent_targets.push(direct_addr);
                                                            }
                                                        }
                                                    }
                                                }
                                                for (&k, &(p_addr, _)) in peers.iter() {
                                                    if is_tailscale_or_forbidden(&p_addr) {
                                                        continue;
                                                    }
                                                    let base_low = (k & 0xFFFF_FFFF) as u32;
                                                    let target_low = (target_uid & 0xFFFF_FFFF) as u32;
                                                    let is_match = k == target_uid
                                                        || base_low.abs_diff(target_low) < 16;
                                                    if is_match && !sent_targets.contains(&p_addr) {
                                                        let _ = sock.send_to(&req_pkt, p_addr);
                                                        sent_targets.push(p_addr);
                                                    }
                                                }
                                                info!("🔄 [PLI RECOVERY] Keyframe solicitado ao peer {} (rotas acionadas: {:?})", target_uid, sent_targets);
                                            }
                                            for bcast in get_broadcast_addresses() {
                                                let _ = sock.send_to(&req_pkt, bcast);
                                            }
                                        }
                                    }
                                }
                            };

                            // Se estiver aguardando IDR, verificar se o frame atual contém SPS ou IDR antes de decodificar
                            if *waiting_for_idr.entry(peer_uid).or_insert(false) {
                                let is_idr_or_sps = frame_data.windows(5).any(|w| {
                                    (w[..4] == [0, 0, 0, 1] && (w[4] & 0x1F == 7 || w[4] & 0x1F == 5)) ||
                                    (w[..3] == [0, 0, 1] && (w[3] & 0x1F == 7 || w[3] & 0x1F == 5))
                                });
                                if !is_idr_or_sps {
                                    // Continua aguardando IDR: solicita PLI periodicamente para destravar o transmissor
                                    send_pli(peer_uid);
                                    continue;
                                }
                                // Recebeu IDR/SPS — pode decodificar normalmente
                                waiting_for_idr.insert(peer_uid, false);
                                log::info!("✅ [DECODER RECOVERY] IDR/SPS recebido para peer {} — retomando decodificação!", peer_uid);
                            }

                            if let Some((pixel_buffer, _w, h)) = decode_video_frame(&mut h264_decoders, peer_uid, &frame_data) {
                                let dec_us = t_dec_start.elapsed().as_micros();
                                let count = rx_frame_count.entry(peer_uid).or_insert(0);
                                *count += 1;
                                let last_stats = rx_last_stats.entry(peer_uid).or_insert_with(Instant::now);
                                if last_stats.elapsed() >= Duration::from_secs(1) {
                                    let elapsed_s = last_stats.elapsed().as_secs_f64();
                                    let rx_fps = (*count as f64) / elapsed_s;
                                    CURRENT_RX_FPS.store((rx_fps * 10.0).round() as u32, Ordering::Relaxed);
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

                                rx_error_streak.remove(&peer_uid);
                                on_frame(peer_uid, peer_name, quality_label, pixel_buffer);
                            } else {
                                let streak = rx_error_streak.entry(peer_uid).or_insert(0);
                                *streak += 1;
                                if *streak >= 60 {
                                    log::info!("🔄 [DECODER RECOVERY] Resetando instância do OpenH264 para peer {} após {} falhas consecutivas — aguardando IDR", peer_uid, *streak);
                                    h264_decoders.remove(&peer_uid);
                                    waiting_for_idr.insert(peer_uid, true);
                                    *streak = 0;
                                }

                                send_pli(peer_uid);
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            let mut has_recent = false;
                            for (_, t) in rx_last_stats.iter() {
                                if t.elapsed() < Duration::from_millis(2000) {
                                    has_recent = true;
                                    break;
                                }
                            }
                            if !has_recent && CURRENT_RX_FPS.load(Ordering::Relaxed) > 0 {
                                CURRENT_RX_FPS.store(0, Ordering::Relaxed);
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                CURRENT_RX_FPS.store(0, Ordering::Relaxed);
                info!("🎬 [VIDEO DECODER WORKER] Thread de decodificação finalizada.");
            })
            .ok();

        std::thread::Builder::new()
            .name("screen-capture-rx".to_string())
            .spawn(move || {
                crate::cpu_profiler::set_current_thread_name("screen-capture-rx");
                let (socket, bound_port) = {
                    let mut bound = None;
                    for port in P2P_VIDEO_PORT..=(P2P_VIDEO_PORT + 10) {
                        if let Ok(addr) = format!("0.0.0.0:{}", port).parse::<SocketAddr>() {
                            if let Ok(sock) = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP)) {
                                let _ = sock.set_send_buffer_size(16 * 1024 * 1024);
                                let _ = sock.set_recv_buffer_size(16 * 1024 * 1024);
                                let _ = sock.set_broadcast(true);
                                let _ = sock.set_read_timeout(Some(Duration::from_millis(2)));
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
                                    let _ = sock.set_send_buffer_size(16 * 1024 * 1024);
                                    let _ = sock.set_recv_buffer_size(16 * 1024 * 1024);
                                    let _ = sock.set_broadcast(true);
                                    let _ = sock.set_read_timeout(Some(Duration::from_millis(2)));
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
                ensure_stream_audio_playback_started();
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
                        let mut uname = my_username_arc.lock().unwrap_or_else(|e| e.into_inner()).clone();
                        if uname.is_empty() {
                            uname = crate::gateway::get_my_username();
                            if !uname.is_empty() {
                                *my_username_arc.lock().unwrap_or_else(|e| e.into_inner()) = uname.clone();
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

                        let bcasts = get_broadcast_addresses();
                        for target in &bcasts {
                            let _ = socket.send_to(&hb_pkt, target);
                        }
                        
                        let mut sent_peers = Vec::new();
                        if let Ok(peers) = peers_store.lock() {
                            for (&_p_uid, &(p_addr, _)) in peers.iter() {
                                if !is_tailscale_or_forbidden(&p_addr) {
                                    let _ = socket.send_to(&hb_pkt, p_addr);
                                    sent_peers.push(p_addr);
                                }
                            }
                        }
                        // Enviar também para LAST_SEEN_PEER_ADDR para manter NAT pinhole aberto
                        let mut sent_last = Vec::new();
                        if let Ok(last_guard) = LAST_SEEN_PEER_ADDR.lock() {
                            if let Some(map) = last_guard.as_ref() {
                                for (&_p_uid, &addr) in map.iter() {
                                    if !is_tailscale_or_forbidden(&addr) {
                                        let _ = socket.send_to(&hb_pkt, addr);
                                        sent_last.push(addr);
                                    }
                                }
                            }
                        }
                        info!("💓 [HEARTBEAT PROATIVO] Enviado para {} bcasts. PeersStore: {:?}, LastSeen: {:?}", bcasts.len(), sent_peers, sent_last);
                    }

                    let mut packets_drained = 0;
                    for _ in 0..128 {
                        match socket.recv_from(&mut recv_buf) {
                            Ok((len, src_addr)) => {
                                packets_drained += 1;
                                // 1. STUN Binding Success Response (0x01, 0x01 with Magic Cookie 0x2112A442)
                                if len >= 20 && recv_buf[0] == 0x01 && recv_buf[1] == 0x01 && &recv_buf[4..8] == &[0x21, 0x12, 0xa4, 0x42] {
                                    let mut i = 20;
                                    while i + 4 <= len {
                                        let attr_type = u16::from_be_bytes([recv_buf[i], recv_buf[i + 1]]);
                                        let attr_len = u16::from_be_bytes([recv_buf[i + 2], recv_buf[i + 3]]) as usize;
                                        if i + 4 + attr_len > len { break; }
                                        if (attr_type == 0x0020 || attr_type == 0x0001) && attr_len >= 8 && recv_buf[i + 5] == 0x01 {
                                            let port = if attr_type == 0x0020 {
                                                u16::from_be_bytes([recv_buf[i + 6], recv_buf[i + 7]]) ^ 0x2112
                                            } else {
                                                u16::from_be_bytes([recv_buf[i + 6], recv_buf[i + 7]])
                                            };
                                            let ip = if attr_type == 0x0020 {
                                                std::net::Ipv4Addr::new(
                                                    recv_buf[i + 8] ^ 0x21,
                                                    recv_buf[i + 9] ^ 0x12,
                                                    recv_buf[i + 10] ^ 0xa4,
                                                    recv_buf[i + 11] ^ 0x42,
                                                )
                                            } else {
                                                std::net::Ipv4Addr::new(recv_buf[i + 8], recv_buf[i + 9], recv_buf[i + 10], recv_buf[i + 11])
                                            };
                                            let public_stun_addr = SocketAddr::new(std::net::IpAddr::V4(ip), port);
                                            if let Ok(mut guard) = CACHED_STUN_ADDR.lock() {
                                                *guard = Some((public_stun_addr, Instant::now()));
                                            }
                                            info!("🌐 [NAT TRAVERSAL] Porta pública mapeada com sucesso no socket P2P: {}", public_stun_addr);
                                            break;
                                        }
                                        i += 4 + ((attr_len + 3) & !3);
                                    }
                                    continue;
                                }

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

                                info!("📥 [TELEMETRIA RX P2P] Pacote MÁGICO recebido: OP={}, de UID={}, de IP={}", op, pkt_uid, src_addr);

                                if (pkt_uid == my_uid && my_uid > 0) || pkt_inst == my_instance_id {
                                    info!("🚫 [TELEMETRIA RX P2P] Pacote ignorado (self echo): UID={}, IP={}", pkt_uid, src_addr);
                                    continue;
                                }

                                let my_current_cid = crate::gateway::get_my_voice_channel_id();
                                // Regra Estrita de Segurança & Privacidade:
                                // Descarta imediatamente qualquer pacote de áudio/vídeo se não estivermos na mesma sala de voz
                                if my_current_cid == 0 || pkt_cid == 0 || pkt_cid != my_current_cid {
                                    continue;
                                }

                                if !is_tailscale_or_forbidden(&src_addr) {
                                    if let Ok(mut last_guard) = LAST_SEEN_PEER_ADDR.lock() {
                                        last_guard.get_or_insert_with(HashMap::new).insert(pkt_uid, src_addr);
                                    }
                                }

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

                                        let is_private_ip = match src_addr.ip() {
                                            std::net::IpAddr::V4(ipv4) => ipv4.is_private() || ipv4.is_loopback(),
                                            std::net::IpAddr::V6(ipv6) => ipv6.is_loopback(),
                                        };

                                        let peer_rx_port = if is_private_ip && len >= 29 + name_len + 2 {
                                            u16::from_be_bytes(recv_buf[29 + name_len..31 + name_len].try_into().unwrap())
                                        } else {
                                            src_addr.port()
                                        };

                                        let explicit_addr = SocketAddr::new(src_addr.ip(), peer_rx_port);

                                        if !is_tailscale_or_forbidden(&src_addr) {
                                            if let Ok(mut peers) = peers_store.lock() {
                                                peers.insert(pkt_uid, (src_addr, Instant::now()));
                                                if is_private_ip && explicit_addr != src_addr && !is_tailscale_or_forbidden(&explicit_addr) {
                                                    peers.insert(pkt_uid.wrapping_add(0x8000_0000_0000_0000), (explicit_addr, Instant::now()));
                                                }
                                            }
                                        }

                                        // Instant reciprocal heartbeat so transmitter gets our direct IP and RX port
                                        if is_streaming {
                                            let uname = my_username_arc.lock().unwrap_or_else(|e| e.into_inner()).clone();
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
                                        }

                                        if pkt_inst == my_instance_id {
                                            continue;
                                        }
                                        if is_streaming {
                                            last_stream_activity.insert(pkt_uid, Instant::now());
                                        }
                                        let prev_state = active_streaming_users.insert(pkt_uid, is_streaming);
                                        if prev_state != Some(is_streaming) {
                                            info!("📡 Usuário {} alterou estado de stream: {}", pkt_uid, is_streaming);
                                            if is_streaming {
                                                on_state_arc(pkt_uid, true);
                                            } else {
                                                trigger_stream_stopped(pkt_uid);
                                            }
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

                                        let is_private_ip = match src_addr.ip() {
                                            std::net::IpAddr::V4(ipv4) => ipv4.is_private() || ipv4.is_loopback(),
                                            std::net::IpAddr::V6(ipv6) => ipv6.is_loopback(),
                                        };

                                        let peer_port = if is_private_ip && len >= 28 + name_len + 2 {
                                            u16::from_be_bytes(recv_buf[28 + name_len..30 + name_len].try_into().unwrap())
                                        } else {
                                            src_addr.port()
                                        };

                                        let explicit_addr = SocketAddr::new(src_addr.ip(), peer_port);
                                        if !is_tailscale_or_forbidden(&src_addr) {
                                            let peer_key = (pkt_uid as u64) ^ ((pkt_inst as u64) << 32);
                                            if let Ok(mut peers) = peers_store.lock() {
                                                let is_new = !peers.contains_key(&peer_key);
                                                peers.insert(peer_key, (src_addr, Instant::now()));
                                                if is_private_ip && explicit_addr != src_addr && !is_tailscale_or_forbidden(&explicit_addr) {
                                                    peers.insert(peer_key.wrapping_add(0x8000_0000_0000_0000), (explicit_addr, Instant::now()));
                                                }
                                                if is_new {
                                                    info!("📡 [P2P HEARTBEAT RECEBIDO] Novo peer UID={} conectado em {}", pkt_uid, src_addr);
                                                    if is_tx_running.load(Ordering::Relaxed) {
                                                        KEYFRAME_REQUESTED.store(true, Ordering::Relaxed);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                OP_VIDEO_CHUNK => {
                                    if current_cid == 0 {
                                        current_cid = crate::gateway::get_my_voice_channel_id();
                                    }
                                    if current_cid != 0 && pkt_cid != 0 && pkt_cid != current_cid {
                                        continue;
                                    }
                                    if pkt_inst == my_instance_id {
                                        continue;
                                    }
                                    last_stream_activity.insert(pkt_uid, Instant::now());
                                    if len >= 37 {
                                        let seq = u32::from_be_bytes(recv_buf[25..29].try_into().unwrap());
                                        let pts_ms = u32::from_be_bytes(recv_buf[29..33].try_into().unwrap());
                                        let total = u16::from_be_bytes(recv_buf[33..35].try_into().unwrap());
                                        let idx = u16::from_be_bytes(recv_buf[35..37].try_into().unwrap());
                                        let chunk_data = recv_buf[37..len].to_vec();

                                        let user_frames = in_flight.entry(pkt_uid).or_insert_with(HashMap::new);
                                        if user_frames.len() > 30 {
                                            let now = Instant::now();
                                            user_frames.retain(|_, f| now.duration_since(f.first_seen) < Duration::from_millis(500));
                                        }
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
                                                    if m_idx == total.saturating_sub(1) && frame_entry.total_len > 0 {
                                                        let expected_last_len = frame_entry.total_len.saturating_sub((total as usize - 1) * CHUNK_SIZE);
                                                        if expected_last_len > 0 && expected_last_len < recovered.len() {
                                                            recovered.truncate(expected_last_len);
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
                                            if frame_entry.total_len > 0 && complete_frame.len() > frame_entry.total_len {
                                                complete_frame.truncate(frame_entry.total_len);
                                            }
                                            user_frames.remove(&seq);

                                            let sec_key = get_voice_encryption_key(my_current_cid);
                                            let final_frame_opt = if let Some(decrypted) = decrypt_signaling_payload(&sec_key, &complete_frame) {
                                                Some(decrypted)
                                            } else if complete_frame.starts_with(&[0, 0, 0, 1]) || complete_frame.starts_with(&[0, 0, 1]) || (complete_frame.len() >= 2 && complete_frame[0] == 0xFF && complete_frame[1] == 0xD8) {
                                                Some(complete_frame)
                                            } else {
                                                None
                                            };

                                            if let Some(valid_frame) = final_frame_opt {
                                                let last_seq = last_rendered_seq.get(&pkt_uid).copied().unwrap_or(0);
                                                let is_newer = seq > last_seq || (last_seq.wrapping_sub(seq) > 0x8000_0000);

                                                if is_newer || last_rendered_seq.get(&pkt_uid).is_none() {
                                                    last_rendered_seq.insert(pkt_uid, seq);

                                                    let prev_state = active_streaming_users.insert(pkt_uid, true);
                                                    if prev_state != Some(true) {
                                                        info!("📡 [AUTO-ATIVAÇÃO STREAM] Usuário {} iniciou transmissão de tela (quadro recebido)!", pkt_uid);
                                                        on_state_arc(pkt_uid, true);
                                                    }

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
                                                        frame_data: valid_frame,
                                                    };

                                                    if let Err(std::sync::mpsc::TrySendError::Full(_)) = tx_decode.try_send(q_item) {
                                                        log::warn!("⚠️ [JITTER BUFFER] Fila de decodificação cheia (>120 quadros), solicitando Keyframe para ressincronização imediata");
                                                        request_intra_keyframe();
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                OP_FEC_PARITY => {
                                    if current_cid == 0 {
                                        current_cid = crate::gateway::get_my_voice_channel_id();
                                    }
                                    if current_cid != 0 && pkt_cid != 0 && pkt_cid != current_cid {
                                        continue;
                                    }
                                    if pkt_inst == my_instance_id {
                                        continue;
                                    }
                                    last_stream_activity.insert(pkt_uid, Instant::now());
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
                                                    if m_idx == total.saturating_sub(1) && frame_entry.total_len > 0 {
                                                        let expected_last_len = frame_entry.total_len.saturating_sub((total as usize - 1) * CHUNK_SIZE);
                                                        if expected_last_len > 0 && expected_last_len < recovered.len() {
                                                            recovered.truncate(expected_last_len);
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
                                            if frame_entry.total_len > 0 && complete_frame.len() > frame_entry.total_len {
                                                complete_frame.truncate(frame_entry.total_len);
                                            }
                                            user_frames.remove(&seq);

                                            let sec_key = get_voice_encryption_key(my_current_cid);
                                            let final_frame_opt = if let Some(decrypted) = decrypt_signaling_payload(&sec_key, &complete_frame) {
                                                Some(decrypted)
                                            } else if complete_frame.starts_with(&[0, 0, 0, 1]) || complete_frame.starts_with(&[0, 0, 1]) || (complete_frame.len() >= 2 && complete_frame[0] == 0xFF && complete_frame[1] == 0xD8) {
                                                Some(complete_frame)
                                            } else {
                                                None
                                            };

                                            if let Some(valid_frame) = final_frame_opt {
                                                let last_seq = last_rendered_seq.get(&pkt_uid).copied().unwrap_or(0);
                                                let is_newer = seq > last_seq || (last_seq.wrapping_sub(seq) > 0x8000_0000);

                                                if is_newer || last_rendered_seq.get(&pkt_uid).is_none() {
                                                    last_rendered_seq.insert(pkt_uid, seq);

                                                    let prev_state = active_streaming_users.insert(pkt_uid, true);
                                                    if prev_state != Some(true) {
                                                        info!("📡 [AUTO-ATIVAÇÃO STREAM FEC] Usuário {} iniciou transmissão de tela (quadro recuperado via FEC)!", pkt_uid);
                                                        on_state_arc(pkt_uid, true);
                                                    }

                                                    let qos = qos_windows.entry(pkt_uid).or_default();
                                                    qos.frames_received += 1;
                                                    qos.fec_recovered += 1;
                                                    last_stream_activity.insert(pkt_uid, Instant::now());

                                                    let uname = peer_names.get(&pkt_uid).cloned().unwrap_or_else(|| format!("Usuário {}", pkt_uid));
                                                    let fps_val = peer_fps.get(&pkt_uid).copied().unwrap_or(60);

                                                    let q_item = QueuedVideoFrame {
                                                        peer_uid: pkt_uid,
                                                        peer_name: uname,
                                                        peer_fps: fps_val,
                                                        seq,
                                                        pts_ms,
                                                        frame_data: valid_frame,
                                                    };

                                                    if let Err(std::sync::mpsc::TrySendError::Full(_)) = tx_decode.try_send(q_item) {
                                                        log::warn!("⚠️ [JITTER BUFFER] Fila de decodificação cheia, descartando frame {} para manter latência ultra-baixa em tempo real", seq);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                OP_KEYFRAME_REQ => {
                                    if is_tx_running.load(Ordering::Relaxed) {
                                        let is_for_me = if len >= 33 {
                                            let target_uid = u64::from_be_bytes(recv_buf[25..33].try_into().unwrap());
                                            target_uid == my_uid || target_uid == 0
                                        } else {
                                            true
                                        };
                                        if is_for_me {
                                            info!("⚡ [KEYFRAME REQ RECEBIDO] Receptor UID={} solicitou IDR Keyframe imediato!", pkt_uid);
                                            KEYFRAME_REQUESTED.store(true, Ordering::Relaxed);
                                        }
                                    }
                                }
                                OP_QOS_FEEDBACK => {
                                    if len >= 35 && is_tx_running.load(Ordering::Relaxed) {
                                        let target_peer = u64::from_be_bytes(recv_buf[25..33].try_into().unwrap());
                                        if target_peer == my_uid || target_peer == 0 {
                                            let loss_permille = u16::from_be_bytes(recv_buf[33..35].try_into().unwrap());
                                            let current_bps = REQUESTED_BITRATE_BPS.load(Ordering::Relaxed);
                                            if loss_permille > 80 {
                                                // Congestion / Loss > 8%: Multiplicative Decrease (-20% to relieve queue buffers)
                                                let new_bps = ((current_bps as f64) * 0.80).round() as u32;
                                                let clamped = new_bps.clamp(3_000_000, 8_000_000);
                                                if (current_bps as i32 - clamped as i32).abs() >= 1_000_000 {
                                                    info!("📉 [DYNAMIC ABR] Perda de {:.1}% reportada pelo receptor. Reduzindo bitrate: {:.2} Mbps -> {:.2} Mbps",
                                                        (loss_permille as f64) / 10.0,
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
                                        if pkt_inst != my_instance_id {
                                            info!("📡 Usuário {} encerrou a transmissão de tela via UDP OP_STOP.", pkt_uid);
                                            active_streaming_users.insert(pkt_uid, false);
                                            in_flight.remove(&pkt_uid);
                                            last_rendered_seq.remove(&pkt_uid);
                                            last_stream_activity.remove(&pkt_uid);
                                            trigger_stream_stopped(pkt_uid);
                                        }
                                    }
                                }
                                OP_AUDIO_FRAME => {
                                    if len >= 40 {
                                        if pkt_inst == my_instance_id {
                                            continue;
                                        }
                                        let sample_rate = u32::from_be_bytes(recv_buf[34..38].try_into().unwrap_or([0, 0, 0xbb, 0x80]));
                                        let sample_count = u16::from_be_bytes(recv_buf[38..40].try_into().unwrap()) as usize;
                                        let pcm_payload = &recv_buf[40..len];

                                        let sec_key = get_voice_encryption_key(my_current_cid);
                                        let raw_pcm = if let Some(decrypted) = decrypt_signaling_payload(&sec_key, pcm_payload) {
                                            decrypted
                                        } else if pcm_payload.len() == sample_count * 2 {
                                            pcm_payload.to_vec()
                                        } else {
                                            continue;
                                        };

                                        let samples_i16: Vec<i16> = (0..sample_count)
                                            .filter_map(|i| {
                                                if i * 2 + 1 < raw_pcm.len() {
                                                    Some(i16::from_le_bytes([raw_pcm[i * 2], raw_pcm[i * 2 + 1]]))
                                                } else {
                                                    None
                                                }
                                            })
                                            .collect();

                                        let mut peak_rx: i16 = 0;
                                        for &s in &samples_i16 {
                                            let abs = s.saturating_abs();
                                            if abs > peak_rx { peak_rx = abs; }
                                        }

                                        static RX_AUDIO_LOG_COUNT: AtomicU64 = AtomicU64::new(0);
                                        let c = RX_AUDIO_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
                                        if c < 10 || c % 50 == 0 {
                                            info!("🔊 [STREAM AUDIO RX] Pkt #{} de UID={} | Amostras={} @ {}Hz | Peak Amplitude: {}/32767 | Vol={:.2}", c, pkt_uid, sample_count, sample_rate, peak_rx, get_stream_volume(pkt_uid));
                                        }

                                        let pts_ms = u32::from_be_bytes(recv_buf[29..33].try_into().unwrap());
                                        update_audio_clock_pts(pts_ms);
                                        let vol = get_stream_volume(pkt_uid);
                                        if vol > 0.001 && !samples_i16.is_empty() {
                                            let is_voice_active = crate::gateway::CURRENT_VOICE_SESSION_ID.load(Ordering::Relaxed) != 0 || crate::gateway::get_my_voice_channel_id() != 0;

                                            if is_voice_active {
                                                // 1. Direct feed EXCLUSIVELY to Voice Gateway Master Speaker Mixer (SSRC 0x8000_5354)
                                                let stream_ssrc: u32 = 0x8000_5354;
                                                if let Ok(mut queues) = crate::gateway::get_speaker_pcm_queues().lock() {
                                                    let q = queues.entry(stream_ssrc).or_insert_with(|| std::collections::VecDeque::with_capacity(48000));
                                                    if q.len() > 9600 {
                                                        let excess = q.len() - 4800;
                                                        q.drain(0..excess);
                                                        let fade_len = 32.min(q.len());
                                                        for i in 0..fade_len {
                                                            let factor = i as f32 / fade_len as f32;
                                                            let (l, r) = q[i];
                                                            q[i] = (l * factor, r * factor);
                                                        }
                                                    }
                                                    if sample_rate == 48000 || sample_rate == 0 {
                                                        for s in &samples_i16 {
                                                            let f = (*s as f32 / 32768.0) * vol;
                                                            q.push_back((f, f));
                                                        }
                                                    } else {
                                                        let ratio = 48000.0 / (sample_rate as f32);
                                                        let out_count = ((samples_i16.len() as f32) * ratio).round() as usize;
                                                        for j in 0..out_count {
                                                            let src_idx = (j as f32) / ratio;
                                                            let i0 = (src_idx.floor() as usize).min(samples_i16.len() - 1);
                                                            let i1 = (i0 + 1).min(samples_i16.len() - 1);
                                                            let frac = src_idx - (i0 as f32);
                                                            let s = (samples_i16[i0] as f32) * (1.0 - frac) + (samples_i16[i1] as f32) * frac;
                                                            let f = (s / 32768.0) * vol;
                                                            q.push_back((f, f));
                                                        }
                                                    }
                                                }
                                                // Clear standalone queue so it never contains leftover echoes
                                                if let Ok(mut q_guard) = get_stream_audio_queue().lock() {
                                                    if !q_guard.is_empty() {
                                                        q_guard.clear();
                                                    }
                                                }
                                            } else {
                                                // 2. Direct feed to Standalone Stream Audio Queue ONLY when not in a voice call
                                                let queue = get_stream_audio_queue();
                                                let mut q_guard = queue.lock().unwrap_or_else(|e| e.into_inner());
                                                if q_guard.len() > 4800 {
                                                    let excess = q_guard.len() - 2400;
                                                    q_guard.drain(0..excess);
                                                    let fade_len = 32.min(q_guard.len());
                                                    for i in 0..fade_len {
                                                        let factor = i as f32 / fade_len as f32;
                                                        q_guard[i] *= factor;
                                                    }
                                                }

                                                if sample_rate == 48000 || sample_rate == 0 {
                                                    for s in &samples_i16 {
                                                        q_guard.push_back((*s as f32 / 32768.0) * vol);
                                                    }
                                                } else {
                                                    let ratio = 48000.0 / (sample_rate as f32);
                                                    let out_count = ((samples_i16.len() as f32) * ratio).round() as usize;
                                                    for j in 0..out_count {
                                                        let src_idx = (j as f32) / ratio;
                                                        let i0 = (src_idx.floor() as usize).min(samples_i16.len() - 1);
                                                        let i1 = (i0 + 1).min(samples_i16.len() - 1);
                                                        let frac = src_idx - (i0 as f32);
                                                        let s = (samples_i16[i0] as f32) * (1.0 - frac) + (samples_i16[i1] as f32) * frac;
                                                        q_guard.push_back((s / 32768.0) * vol);
                                                    }
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
                        #[cfg(target_os = "windows")]
                        Err(ref e) if e.kind() == std::io::ErrorKind::ConnectionReset => {
                            // Ignora ICMP Port/Host Unreachable (WSAECONNRESET 10054) no socket UDP
                            continue;
                        }
                        Err(_) => break,
                    }
                }

                if packets_drained == 0 {
                    std::thread::sleep(Duration::from_millis(4));
                }

                    // Check stream timeouts (> 2500ms sem quadros)
                    let now = Instant::now();
                    let mut expired = Vec::new();
                    for (&uid, &last_act) in last_stream_activity.iter() {
                        if now.duration_since(last_act) > Duration::from_millis(2500) {
                            expired.push(uid);
                        }
                    }
                    for uid in expired {
                        last_stream_activity.remove(&uid);
                        if active_streaming_users.insert(uid, false) != Some(false) {
                            info!("📡 Stream do usuário {} expirou por inatividade (2.5s sem quadros).", uid);
                            last_rendered_seq.remove(&uid);
                            in_flight.remove(&uid);
                            trigger_stream_stopped(uid);
                        }
                    }
                }
            })
            .expect("Falha ao iniciar thread RX");
    }
}
