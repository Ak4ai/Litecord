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

// Litecord P2P Video
const CHUNK_SIZE: usize = 1350; // Optimized MTU for local & VPN UDP transmission without fragmentation

// Protocol Opcodes
const OP_ANNOUNCE: u8 = 1;
const OP_VIDEO_CHUNK: u8 = 2;
const OP_STOP: u8 = 3;
const OP_HEARTBEAT: u8 = 4;
pub const OP_AUDIO_FRAME: u8 = 5;

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
        let my_uid = self.my_user_id.load(Ordering::Relaxed);
        let my_instance_id = get_process_instance_id();
        let my_rx = get_my_rx_port();
        let uname = self.my_username.lock().unwrap().clone();
        let uname_bytes = uname.as_bytes();

        if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
            let _ = socket.set_broadcast(true);
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

        if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
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
            info!("🛑 Parando captura e transmissão de tela P2P...");
            let cid = self.channel_id.load(Ordering::Relaxed);
            let uid = self.my_user_id.load(Ordering::Relaxed);
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

                for target in &bcast_targets {
                    let _ = socket.send_to(&stop_pkt, target);
                }
                if let Ok(peers) = self.known_peers.lock() {
                    for (&_, &(addr, _)) in peers.iter() {
                        let _ = socket.send_to(&stop_pkt, addr);
                    }
                }
            }
        }
    }

    /// Starts the capture and UDP transmitter thread
    pub fn start<F>(&self, target_hwnd: isize, camera_index: Option<u32>, res: i32, fps: i32, include_audio: bool, on_local_frame: F)
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

        info!("🖥️ Iniciando transmissão P2P ({}x{} @ {} FPS, hwnd={}, cam={:?}, audio={})...", target_w, target_h, target_fps, target_hwnd, camera_index, camera_index.is_none() && include_audio);

        std::thread::Builder::new()
            .name("screen-capture-tx".to_string())
            .spawn(move || {
                let socket = match UdpSocket::bind("0.0.0.0:0") {
                    Ok(s) => {
                        let _ = s.set_broadcast(true);
                        let _ = s.set_nonblocking(true);
                        s
                    }
                    Err(e) => {
                        warn!("Falha ao criar socket UDP TX: {:?}", e);
                        return;
                    }
                };

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
                let frame_interval = Duration::from_nanos(1_000_000_000 / (target_fps as u64).max(1));
                let mut frame_seq: u32 = 0;
                let mut last_announce = Instant::now() - Duration::from_secs(10);

                // Initial 3x announce burst for instant <10ms peer discovery
                {
                    let cid = channel_id_atomic.load(Ordering::Relaxed);
                    let uid = my_user_id_atomic.load(Ordering::Relaxed);
                    let uname = my_username_arc.lock().unwrap().clone();
                    let uname_bytes = uname.as_bytes();
                    let inst = get_process_instance_id();
                    let my_rx = get_my_rx_port();
                    let mut ann_pkt = Vec::with_capacity(32 + uname_bytes.len());
                    ann_pkt.extend_from_slice(MAGIC);
                    ann_pkt.extend_from_slice(&inst.to_be_bytes());
                    ann_pkt.push(OP_ANNOUNCE);
                    ann_pkt.extend_from_slice(&cid.to_be_bytes());
                    ann_pkt.extend_from_slice(&uid.to_be_bytes());
                    ann_pkt.push(1); // is_streaming = true
                    ann_pkt.push(match res {
                        1080 => 108,
                        480 => 48,
                        _ => 72,
                    });
                    ann_pkt.push(target_fps as u8);
                    ann_pkt.push(uname_bytes.len() as u8);
                    ann_pkt.extend_from_slice(uname_bytes);
                    ann_pkt.extend_from_slice(&my_rx.to_be_bytes());

                    for _ in 0..3 {
                        for target in &bcast_targets {
                            let _ = socket.send_to(&ann_pkt, target);
                        }
                        if let Ok(peers) = peers_store.lock() {
                            for (&_, &(addr, _)) in peers.iter() {
                                let _ = socket.send_to(&ann_pkt, addr);
                            }
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }

                let mut tx_frame_count = 0u64;
                let mut last_tx_stats = std::time::Instant::now();
                let mut total_encode_us = 0u128;

                while is_running.load(Ordering::Relaxed) {
                    let start_time = Instant::now();
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

                    let capture_res = if camera_index.is_some() {
                        if let Some(ref cam) = camera_handle {
                            if let Ok(frame) = cameras::next_frame(cam, Duration::from_millis(50)) {
                                let (w, h) = (frame.width, frame.height);
                                let total = (w * h) as usize;
                                if let Ok(rgba_bytes) = cameras::to_rgba8(&frame) {
                                    let mut pixel_buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
                                    pixel_buf.make_mut_bytes().copy_from_slice(&rgba_bytes);
                                    let mut rgb = Vec::with_capacity(total * 3);
                                    for p in rgba_bytes.chunks_exact(4) {
                                        rgb.push(p[0]);
                                        rgb.push(p[1]);
                                        rgb.push(p[2]);
                                    }
                                    Some((pixel_buf, rgb))
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
                        capture_screen_rgb(target_hwnd, target_w, target_h, target_fps)
                    };

                    if let Some((pixel_buf, rgb_data)) = capture_res {
                        let cur_w = pixel_buf.width();
                        let cur_h = pixel_buf.height();
                        buffer.publish(pixel_buf.clone());
                        if camera_index.is_some() || cfg!(windows) {
                            on_local_tx(pixel_buf);
                        }

                        // Compress frame to JPEG using fast SIMD encoder
                        let enc_start = Instant::now();
                        let jpeg_opt = encode_jpeg(&rgb_data, cur_w, cur_h, 65);
                        let enc_dur = enc_start.elapsed().as_micros();
                        total_encode_us += enc_dur;

                        if let Some(jpeg_bytes) = jpeg_opt {
                            frame_seq = frame_seq.wrapping_add(1);
                            let total_len = jpeg_bytes.len();
                            let total_chunks = ((total_len + CHUNK_SIZE - 1) / CHUNK_SIZE) as u16;

                            // Send directly to known peers; fallback to broadcast only if no peers discovered yet
                            let mut target_addrs: Vec<SocketAddr> = Vec::new();
                            if let Ok(peers) = peers_store.lock() {
                                for (&_, &(addr, _)) in peers.iter() {
                                    target_addrs.push(addr);
                                }
                            }
                            if target_addrs.is_empty() {
                                target_addrs.extend_from_slice(&bcast_targets);
                            }

                            for chunk_idx in 0..total_chunks {
                                let start = (chunk_idx as usize) * CHUNK_SIZE;
                                let end = (start + CHUNK_SIZE).min(total_len);
                                let chunk_slice = &jpeg_bytes[start..end];

                                let inst = get_process_instance_id();
                                let mut pkt = Vec::with_capacity(33 + chunk_slice.len());
                                pkt.extend_from_slice(MAGIC);
                                pkt.extend_from_slice(&inst.to_be_bytes());
                                pkt.push(OP_VIDEO_CHUNK);
                                pkt.extend_from_slice(&cid.to_be_bytes());
                                pkt.extend_from_slice(&uid.to_be_bytes());
                                pkt.extend_from_slice(&frame_seq.to_be_bytes());
                                pkt.extend_from_slice(&total_chunks.to_be_bytes());
                                pkt.extend_from_slice(&chunk_idx.to_be_bytes());
                                pkt.extend_from_slice(chunk_slice);

                                for target in &target_addrs {
                                    let _ = socket.send_to(&pkt, target);
                                }
                            }
                        }

                        tx_frame_count += 1;
                        if last_tx_stats.elapsed() >= Duration::from_secs(1) {
                            let elapsed_s = last_tx_stats.elapsed().as_secs_f64();
                            let fps = (tx_frame_count as f64) / elapsed_s;
                            let avg_enc_ms = (total_encode_us as f64) / (tx_frame_count.max(1) as f64) / 1000.0;
                            log::info!("📊 [TELEMETRIA] TX Loop: {:.1} FPS | Encode JPEG Médio: {:.2} ms/frame", fps, avg_enc_ms);
                            tx_frame_count = 0;
                            total_encode_us = 0;
                            last_tx_stats = Instant::now();
                        }
                    }

                    let elapsed = start_time.elapsed();
                    if elapsed < frame_interval {
                        std::thread::sleep(frame_interval - elapsed);
                    }
                }

                stop_window_border_overlay();
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

        let is_running = Arc::clone(&self.is_receiver_running);
        let is_tx_running = Arc::clone(&self.is_running);
        let channel_id_atomic = Arc::clone(&self.channel_id);
        let my_user_id_atomic = Arc::clone(&self.my_user_id);
        let my_username_arc = Arc::clone(&self.my_username);
        let peers_store = Arc::clone(&self.known_peers);

        std::thread::Builder::new()
            .name("screen-capture-rx".to_string())
            .spawn(move || {
                let (socket, bound_port) = {
                    let mut bound = None;
                    for port in P2P_VIDEO_PORT..=(P2P_VIDEO_PORT + 10) {
                        if let Ok(s) = UdpSocket::bind(format!("0.0.0.0:{}", port)) {
                            let _ = s.set_broadcast(true);
                            let _ = s.set_read_timeout(Some(Duration::from_millis(100)));
                            bound = Some((s, port));
                            break;
                        }
                    }
                    match bound {
                        Some(pair) => pair,
                        None => {
                            match UdpSocket::bind("0.0.0.0:0") {
                                Ok(s) => {
                                    let _ = s.set_broadcast(true);
                                    let _ = s.set_read_timeout(Some(Duration::from_millis(100)));
                                    let p = s.local_addr().map(|a| a.port()).unwrap_or(P2P_VIDEO_PORT);
                                    (s, p)
                                }
                                Err(e2) => {
                                    warn!("Falha ao inicializar socket UDP RX: {:?}", e2);
                                    return;
                                }
                            }
                        }
                    }
                };

                set_my_rx_port(bound_port);
                info!("📡 Receptor UDP P2P de vídeo escutando na porta {}...", bound_port);

                let mut recv_buf = vec![0u8; 65535];
                struct InFlightFrame {
                    seq: u32,
                    total_chunks: u16,
                    received: HashMap<u16, Vec<u8>>,
                    first_seen: Instant,
                }
                let mut in_flight: HashMap<u64, InFlightFrame> = HashMap::new();
                let mut peer_names: HashMap<u64, String> = HashMap::new();
                let mut peer_fps: HashMap<u64, u8> = HashMap::new();
                let mut active_streaming_users: HashMap<u64, bool> = HashMap::new();
                let mut last_stream_activity: HashMap<u64, Instant> = HashMap::new();

                let my_instance_id = get_process_instance_id();
                let mut last_outbound_heartbeat = Instant::now() - Duration::from_secs(10);

                while is_running.load(Ordering::Relaxed) {
                    let current_cid = channel_id_atomic.load(Ordering::Relaxed);
                    let my_uid = my_user_id_atomic.load(Ordering::Relaxed);

                    // Proactive presence heartbeat every 2s to punch through NAT routers and VirtualBox gateways
                    if last_outbound_heartbeat.elapsed() >= Duration::from_secs(2) && current_cid > 0 && my_uid > 0 {
                        last_outbound_heartbeat = Instant::now();
                        let uname = my_username_arc.lock().unwrap().clone();
                        let uname_bytes = uname.as_bytes();
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
                        hb_pkt.extend_from_slice(&bound_port.to_be_bytes());

                        for target in get_broadcast_addresses() {
                            let _ = socket.send_to(&hb_pkt, target);
                        }
                    }

                    match socket.recv_from(&mut recv_buf) {
                        Ok((len, src_addr)) => {
                            if len < 25 || &recv_buf[0..4] != MAGIC {
                                continue;
                            }
                            let pkt_inst = u32::from_be_bytes(recv_buf[4..8].try_into().unwrap());
                            if pkt_inst == my_instance_id {
                                // Ignore self-broadcast packets from our own process
                                continue;
                            }
                            let op = recv_buf[8];
                            let pkt_cid = u64::from_be_bytes(recv_buf[9..17].try_into().unwrap());
                            let pkt_uid = u64::from_be_bytes(recv_buf[17..25].try_into().unwrap());

                            match op {
                                OP_ANNOUNCE => {
                                    if len >= 28 {
                                        let is_streaming = recv_buf[25] == 1;
                                        
                                        // Robust parsing supporting format with explicit fps byte
                                        let (fps_val, name_offset, name_len) = if len >= 29 && recv_buf[27] >= 10 && recv_buf[27] <= 120 {
                                            (recv_buf[27], 29, recv_buf[28] as usize)
                                        } else {
                                            (30, 28, recv_buf[27] as usize)
                                        };

                                        if is_streaming {
                                            peer_fps.insert(pkt_uid, fps_val);
                                        }

                                        if len >= name_offset + name_len {
                                            if let Ok(uname) = std::str::from_utf8(&recv_buf[name_offset..name_offset + name_len]) {
                                                peer_names.insert(pkt_uid, uname.to_string());
                                            }
                                        }

                                        let peer_port = if len >= name_offset + name_len + 2 {
                                            u16::from_be_bytes(recv_buf[name_offset + name_len..name_offset + name_len + 2].try_into().unwrap())
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

                                        let prev_state = active_streaming_users.insert(pkt_uid, is_streaming);
                                        if prev_state != Some(is_streaming) {
                                            info!("📡 Usuário {} ({}) alterou estado de stream: {}", pkt_uid, src_addr, is_streaming);
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
                                        info!("📡 Heartbeat P2P recebido do peer {} ({})", pkt_uid, src_addr);
                                    }
                                }
                                OP_VIDEO_CHUNK => {
                                    // Only accept video chunks if we are in the same voice channel
                                    if current_cid == 0 || (pkt_cid != 0 && pkt_cid != current_cid) {
                                        continue;
                                    }
                                    if len > 33 {
                                        let seq = u32::from_be_bytes(recv_buf[25..29].try_into().unwrap());
                                        let total = u16::from_be_bytes(recv_buf[29..31].try_into().unwrap());
                                        let idx = u16::from_be_bytes(recv_buf[31..33].try_into().unwrap());
                                        let chunk_data = recv_buf[33..len].to_vec();

                                        if let Ok(mut peers) = peers_store.lock() {
                                            peers.entry(pkt_uid).or_insert((src_addr, Instant::now()));
                                        }

                                        last_stream_activity.insert(pkt_uid, Instant::now());
                                        if active_streaming_users.insert(pkt_uid, true) != Some(true) {
                                            info!("📡 Novo stream de vídeo recebido do usuário {}!", pkt_uid);
                                            on_state(pkt_uid, true);
                                        }

                                        let frame_entry = in_flight.entry(pkt_uid).or_insert_with(|| InFlightFrame {
                                            seq,
                                            total_chunks: total,
                                            received: HashMap::new(),
                                            first_seen: Instant::now(),
                                        });

                                        if frame_entry.seq != seq {
                                            frame_entry.seq = seq;
                                            frame_entry.total_chunks = total;
                                            frame_entry.received.clear();
                                            frame_entry.first_seen = Instant::now();
                                        }

                                        frame_entry.received.insert(idx, chunk_data);

                                        if frame_entry.received.len() == (total as usize) {
                                            let mut complete_jpeg = Vec::new();
                                            for i in 0..total {
                                                if let Some(c) = frame_entry.received.get(&i) {
                                                    complete_jpeg.extend_from_slice(c);
                                                }
                                            }
                                            frame_entry.received.clear();

                                            if let Some((pixel_buffer, _w, h)) = decode_jpeg(&complete_jpeg) {
                                                if let Ok(mut f_map) = get_active_stream_frames().lock() {
                                                    f_map.insert(pkt_uid, pixel_buffer.clone());
                                                }
                                                let uname = peer_names.get(&pkt_uid).cloned().unwrap_or_else(|| format!("Usuário {}", pkt_uid));
                                                let fps_val = peer_fps.get(&pkt_uid).copied().unwrap_or(30);
                                                let quality_label = format!("{}p {}fps", h, fps_val);
                                                on_frame(pkt_uid, uname, quality_label, pixel_buffer);
                                            }
                                        }
                                    }
                                }
                                OP_STOP => {
                                    if len >= 21 {
                                        let pkt_uid = u64::from_be_bytes(recv_buf[13..21].try_into().unwrap());
                                        if pkt_uid != my_uid || my_uid == 0 {
                                            info!("📡 Usuário {} encerrou a transmissão de tela.", pkt_uid);
                                            active_streaming_users.insert(pkt_uid, false);
                                            in_flight.remove(&pkt_uid);
                                            if let Ok(mut frames) = get_active_stream_frames().lock() {
                                                frames.remove(&pkt_uid);
                                            }
                                            on_state(pkt_uid, false);
                                        }
                                    }
                                }
                                OP_AUDIO_FRAME => {
                                    if len >= 36 {
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
                                        if pkt_uid != my_uid || my_uid == 0 {
                                            ensure_stream_audio_playback_started();
                                            let vol = get_stream_volume(pkt_uid);
                                            if vol > 0.001 {
                                                let sample_count = u16::from_be_bytes(recv_buf[34..36].try_into().unwrap()) as usize;
                                                let pcm_bytes = &recv_buf[36..len];
                                                let expected_bytes = sample_count * 2;
                                                if pcm_bytes.len() >= expected_bytes && sample_count > 0 {
                                                    let queue = get_stream_audio_queue();
                                                    let mut q_guard = queue.lock().unwrap();
                                                    // Limit queue size to 100ms (4800 samples) to eliminate lag & stutter
                                                    if q_guard.len() > 4800 {
                                                        let excess = q_guard.len() - 2400;
                                                        q_guard.drain(0..excess);
                                                    }
                                                    for i in 0..sample_count {
                                                        let s_i16 = i16::from_le_bytes([pcm_bytes[i*2], pcm_bytes[i*2 + 1]]);
                                                        let s_f32 = (s_i16 as f32 / 32768.0) * vol;
                                                        q_guard.push_back(s_f32);
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
                            // Read timeout
                        }
                        Err(_) => {}
                    }

                    // Check stream timeouts (> 2.5s sem quadros)
                    let now = Instant::now();
                    for (&uid, &last_act) in last_stream_activity.iter() {
                        if now.duration_since(last_act) > Duration::from_millis(2500) {
                            if active_streaming_users.insert(uid, false) == Some(true) {
                                info!("📡 Stream do usuário {} expirou por inatividade.", uid);
                                if let Ok(mut frames) = get_active_stream_frames().lock() {
                                    frames.remove(&uid);
                                }
                                on_state(uid, false);
                            }
                        }
                    }
                }
            })
            .expect("Falha ao iniciar thread RX");
    }
}

pub fn resolve_public_stun_address() -> Option<SocketAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.set_read_timeout(Some(Duration::from_millis(800))).ok()?;
    
    // Resolve STUN server hostname (stun.l.google.com:19302)
    let stun_server: SocketAddr = match "stun.l.google.com:19302".to_socket_addrs() {
        Ok(mut addrs) => addrs.next()?,
        Err(_) => "74.125.141.127:19302".parse().ok()?,
    };

    let stun_req: [u8; 20] = [
        0x00, 0x01, // Binding Request
        0x00, 0x00, // Length
        0x21, 0x12, 0xa4, 0x42, // Magic Cookie
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
    ];

    if socket.send_to(&stun_req, stun_server).is_err() {
        return None;
    }

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
                if attr_type == 0x0020 && attr_len >= 8 { // XOR-MAPPED-ADDRESS
                    let port = u16::from_be_bytes([buf[i + 6], buf[i + 7]]) ^ 0x2112;
                    let ip = std::net::Ipv4Addr::new(
                        buf[i + 8] ^ 0x21,
                        buf[i + 9] ^ 0x12,
                        buf[i + 10] ^ 0xa4,
                        buf[i + 11] ^ 0x42,
                    );
                    return Some(SocketAddr::new(std::net::IpAddr::V4(ip), port));
                } else if attr_type == 0x0001 && attr_len >= 8 { // MAPPED-ADDRESS
                    let port = u16::from_be_bytes([buf[i + 6], buf[i + 7]]);
                    let ip = std::net::Ipv4Addr::new(buf[i + 8], buf[i + 9], buf[i + 10], buf[i + 11]);
                    return Some(SocketAddr::new(std::net::IpAddr::V4(ip), port));
                }
                i += 4 + ((attr_len + 3) & !3);
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
        // Add VirtualBox NAT gateway & default subnets (10.0.2.2, 10.0.2.15, 10.0.2.255)
        addrs.push(SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 2, 2)), port));
        addrs.push(SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 2, 15)), port));
        addrs.push(SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 2, 255)), port));
    }

    // 2. Query active adapter IPv4 address via routing table probe
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(SocketAddr::V4(local)) = socket.local_addr() {
                let octets = local.ip().octets();
                for port in 50005..=50007 {
                    addrs.push(SocketAddr::new(
                        std::net::IpAddr::V4(std::net::Ipv4Addr::new(octets[0], octets[1], octets[2], 255)),
                        port,
                    ));
                    addrs.push(SocketAddr::new(
                        std::net::IpAddr::V4(std::net::Ipv4Addr::new(octets[0], octets[1], octets[2], 1)),
                        port,
                    ));
                    addrs.push(SocketAddr::new(
                        std::net::IpAddr::V4(std::net::Ipv4Addr::new(octets[0], octets[1], octets[2], 2)),
                        port,
                    ));
                }
            }
        }
    }

    // 3. Resolve STUN Public Internet Address for global P2P hole punching
    if let Some(pub_addr) = resolve_public_stun_address() {
        info!("🌐 IP Público STUN detectado para P2P Global: {}", pub_addr);
        addrs.push(pub_addr);
    }

    addrs
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
fn capture_screen_rgb(target_hwnd: isize, target_w: u32, target_h: u32, _target_fps: u64) -> Option<(SharedPixelBuffer<Rgba8Pixel>, Vec<u8>)> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateSolidBrush, DeleteDC, DeleteObject,
        FillRect, GetDC, GetDIBits, ReleaseDC, SelectObject, SetBrushOrgEx, SetStretchBltMode, StretchBlt,
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, COLORONCOLOR, DIB_RGB_COLORS, SRCCOPY,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetDesktopWindow, GetSystemMetrics, GetWindowRect, IsIconic, IsWindow,
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

    unsafe {
        let hwnd_desktop = GetDesktopWindow();
        let hdc_desktop = GetDC(hwnd_desktop);
        if hdc_desktop.is_null() {
            return None;
        }

        let hdc_mem = CreateCompatibleDC(hdc_desktop);
        if hdc_mem.is_null() {
            ReleaseDC(hwnd_desktop, hdc_desktop);
            return None;
        }

        let hbm_screen = CreateCompatibleBitmap(hdc_desktop, target_w as i32, target_h as i32);
        if hbm_screen.is_null() {
            DeleteDC(hdc_mem);
            ReleaseDC(hwnd_desktop, hdc_desktop);
            return None;
        }

        let old_obj = SelectObject(hdc_mem, hbm_screen);

        if target_hwnd != 0 {
            let hwnd = target_hwnd as windows_sys::Win32::Foundation::HWND;
            if IsWindow(hwnd) == 0 {
                SelectObject(hdc_mem, old_obj);
                DeleteObject(hbm_screen);
                DeleteDC(hdc_mem);
                ReleaseDC(hwnd_desktop, hdc_desktop);
                return None;
            }

            let mut rc: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut rc);
            let win_w = (rc.right - rc.left).max(1);
            let win_h = (rc.bottom - rc.top).max(1);

            let hdc_win_mem = CreateCompatibleDC(hdc_desktop);
            let hbm_win = CreateCompatibleBitmap(hdc_desktop, win_w, win_h);
            let old_win_obj = SelectObject(hdc_win_mem, hbm_win);

            let mut captured_ok = false;
            if IsIconic(hwnd) == 0 {
                // 1. Try PrintWindow with PW_RENDERFULLCONTENT (0x2) for direct compositor capture without occlusion
                if PrintWindow(hwnd, hdc_win_mem, 2) != 0 {
                    captured_ok = true;
                }
            }

            // 2. Fallback to direct window DC if PrintWindow failed
            if !captured_ok && IsIconic(hwnd) == 0 {
                let hdc_win = GetDC(hwnd);
                if !hdc_win.is_null() {
                    BitBlt(hdc_win_mem, 0, 0, win_w, win_h, hdc_win, 0, 0, SRCCOPY);
                    ReleaseDC(hwnd, hdc_win);
                    captured_ok = true;
                }
            }

            // 3. Fallback to desktop crop if window is minimized or special GDI overlay
            if !captured_ok {
                let src_x = rc.left.max(0);
                let src_y = rc.top.max(0);
                BitBlt(hdc_win_mem, 0, 0, win_w, win_h, hdc_desktop, src_x, src_y, SRCCOPY);
            }

            // Fill background with dark letterbox/pillarbox color (#111214)
            let dark_brush = CreateSolidBrush(0x00141211); // COLORREF: 0x00BBGGRR
            let target_rc = RECT { left: 0, top: 0, right: target_w as i32, bottom: target_h as i32 };
            FillRect(hdc_mem, &target_rc, dark_brush);
            DeleteObject(dark_brush);

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

            SelectObject(hdc_win_mem, old_win_obj);
            DeleteObject(hbm_win);
            DeleteDC(hdc_win_mem);
        } else {
            // Full screen capture
            let screen_w = GetSystemMetrics(SM_CXSCREEN);
            let screen_h = GetSystemMetrics(SM_CYSCREEN);

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

        let total_pixels = (target_w * target_h) as usize;
        let mut pixel_buffer = SharedPixelBuffer::<Rgba8Pixel>::new(target_w, target_h);
        let slice = pixel_buffer.make_mut_slice();

        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = target_w as i32;
        bmi.bmiHeader.biHeight = -(target_h as i32);
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB as u32;

        let mut rgb_bytes = vec![0u8; total_pixels * 3];

        thread_local! {
            static CAPTURE_BGRA_POOL: std::cell::RefCell<Vec<u8>> = std::cell::RefCell::new(Vec::new());
        }

        CAPTURE_BGRA_POOL.with(|cell| {
            let mut bgra_buf = cell.borrow_mut();
            if bgra_buf.len() < total_pixels * 4 {
                bgra_buf.resize(total_pixels * 4, 0);
            }

            GetDIBits(
                hdc_mem,
                hbm_screen,
                0,
                target_h,
                bgra_buf.as_mut_ptr() as _,
                &mut bmi,
                DIB_RGB_COLORS,
            );

            let bgra_slice = &bgra_buf[..total_pixels * 4];
            for (i, pixel) in slice.iter_mut().enumerate() {
                let offset_bgra = i * 4;
                let offset_rgb = i * 3;
                let b = bgra_slice[offset_bgra];
                let g = bgra_slice[offset_bgra + 1];
                let r = bgra_slice[offset_bgra + 2];

                *pixel = Rgba8Pixel::new(r, g, b, 255);
                rgb_bytes[offset_rgb] = r;
                rgb_bytes[offset_rgb + 1] = g;
                rgb_bytes[offset_rgb + 2] = b;
            }
        });

        SelectObject(hdc_mem, old_obj);
        DeleteObject(hbm_screen);
        DeleteDC(hdc_mem);
        ReleaseDC(hwnd_desktop, hdc_desktop);

        if let Some(digit) = get_test_watermark_digit() {
            draw_test_watermark(slice, &mut rgb_bytes, target_w, target_h, digit);
        }

        Some((pixel_buffer, rgb_bytes))
    }
}

#[cfg(not(windows))]
static PORTAL_FRAME: std::sync::Mutex<Option<(SharedPixelBuffer<Rgba8Pixel>, Vec<u8>)>> = std::sync::Mutex::new(None);
#[cfg(not(windows))]
static PORTAL_INITIALIZED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(target_os = "linux")]
static PORTAL_LOCAL_CB: std::sync::Mutex<Option<std::sync::Arc<dyn Fn(SharedPixelBuffer<Rgba8Pixel>) + Send + Sync + 'static>>> = std::sync::Mutex::new(None);
#[cfg(target_os = "linux")]
static PORTAL_CHILD: std::sync::Mutex<Option<std::process::Child>> = std::sync::Mutex::new(None);

#[cfg(target_os = "linux")]
fn init_wayland_portal_screencast(target_w: u32, target_h: u32, target_fps: u64) {
    if PORTAL_INITIALIZED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(r) => r,
            Err(e) => {
                log::error!("Falha ao criar tokio runtime para Portal ScreenCast: {:?}", e);
                PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                return;
            }
        };

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
                        PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                        return;
                    }
                },
                Err(e) => {
                    log::error!("Falha ao iniciar captura no Portal ScreenCast: {:?}", e);
                    let _ = session.close().await;
                    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };

            let stream = match response.streams().first() {
                Some(s) => s,
                None => {
                    log::error!("Nenhum stream retornado pelo Portal ScreenCast");
                    let _ = session.close().await;
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
                "keepalive-time=1000".to_string(),
                "!".to_string(),
            ];

            if target_fps < 60 {
                gst_args.push("videorate".to_string());
                gst_args.push("!".to_string());
            }

            gst_args.extend_from_slice(&[
                "videoconvert".to_string(),
                "n-threads=4".to_string(),
                "!".to_string(),
                "videoscale".to_string(),
                "n-threads=4".to_string(),
                "method=0".to_string(),
                "!".to_string(),
            ]);

            if target_fps < 60 {
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

            let mut child = match Command::new("gst-launch-1.0")
                .args(&gst_args)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    log::error!("Falha ao iniciar pipeline GStreamer PipeWire: {:?}", e);
                    let _ = session.close().await;
                    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };

            let mut stdout = match child.stdout.take() {
                Some(s) => s,
                None => {
                    let _ = session.close().await;
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

            while PORTAL_INITIALIZED.load(std::sync::atomic::Ordering::Relaxed) && stdout.read_exact(&mut raw_buf).is_ok() {
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
fn capture_screen_rgb(target_hwnd: isize, target_w: u32, target_h: u32, target_fps: u64) -> Option<(SharedPixelBuffer<Rgba8Pixel>, Vec<u8>)> {
    #[cfg(target_os = "linux")]
    {
        init_wayland_portal_screencast(target_w, target_h, target_fps);

        if let Ok(slot) = PORTAL_FRAME.lock() {
            if let Some(ref frame) = *slot {
                return Some(frame.clone());
            }
        }
    }

    let total_pixels = (target_w * target_h) as usize;
    let mut pixel_buffer = SharedPixelBuffer::<Rgba8Pixel>::new(target_w, target_h);
    let slice = pixel_buffer.make_mut_slice();
    let rgb_bytes = vec![24u8; total_pixels * 3];

    for pixel in slice.iter_mut() {
        *pixel = Rgba8Pixel::new(24, 24, 24, 255);
    }

    Some((pixel_buffer, rgb_bytes))
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

                let mut target_rc: RECT = std::mem::zeroed();
                GetWindowRect(hwnd_target, &mut target_rc);
                let border_pad = 3;
                let x = target_rc.left - border_pad;
                let y = target_rc.top - border_pad;
                let w = (target_rc.right - target_rc.left) + (border_pad * 2);
                let h = (target_rc.bottom - target_rc.top) + (border_pad * 2);

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
                        let mut current_rc: RECT = std::mem::zeroed();
                        GetWindowRect(hwnd_target, &mut current_rc);

                        if current_rc.left != last_rc.left
                            || current_rc.top != last_rc.top
                            || current_rc.right != last_rc.right
                            || current_rc.bottom != last_rc.bottom
                        {
                            last_rc = current_rc;
                            let nx = current_rc.left - border_pad;
                            let ny = current_rc.top - border_pad;
                            let nw = (current_rc.right - current_rc.left) + (border_pad * 2);
                            let nh = (current_rc.bottom - current_rc.top) + (border_pad * 2);

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

                let err_fn = |err| {
                    warn!("Erro no playback de áudio do stream: {}", err);
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

            let socket = match UdpSocket::bind("0.0.0.0:0") {
                Ok(s) => {
                    let _ = s.set_broadcast(true);
                    s
                }
                Err(e) => {
                    warn!("⚠️ Falha ao criar socket UDP para áudio do stream: {:?}", e);
                    return;
                }
            };

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

                for chunk in chunks_to_send {
                    seq = seq.wrapping_add(1);
                    let sample_count = chunk.len() as u16;
                    let mut pkt = Vec::with_capacity(32 + chunk.len() * 2);
                    pkt.extend_from_slice(MAGIC);
                    pkt.extend_from_slice(&inst.to_be_bytes());
                    pkt.push(OP_AUDIO_FRAME);
                    pkt.extend_from_slice(&cid.to_be_bytes());
                    pkt.extend_from_slice(&uid.to_be_bytes());
                    pkt.extend_from_slice(&seq.to_be_bytes());
                    pkt.push(1); // 1 channel
                    pkt.extend_from_slice(&sample_rate.to_be_bytes());
                    pkt.extend_from_slice(&sample_count.to_be_bytes());
                    for s in chunk {
                        pkt.extend_from_slice(&s.to_le_bytes());
                    }

                    for target in &bcast_targets {
                        let _ = socket.send_to(&pkt, target);
                    }
                    if let Ok(peers) = peers_store.lock() {
                        for (&_, &(addr, _)) in peers.iter() {
                            let _ = socket.send_to(&pkt, addr);
                        }
                    }
                }
            }

            drop(stream);
            info!("🔊 Captura de áudio da transmissão finalizada.");
        })
        .expect("Falha ao iniciar thread de captura de áudio da transmissão");
}
