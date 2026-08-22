#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::net::{SocketAddr, UdpSocket};
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
const CHUNK_SIZE: usize = 1200; // Safe MTU for local & VPN UDP transmission

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

    pub fn shared_buffer(&self) -> Arc<SharedFrameBuffer> {
        Arc::clone(&self.shared_buffer)
    }

    pub fn stop(&self) {
        stop_window_border_overlay();
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
    pub fn start<F>(&self, target_hwnd: isize, res: i32, fps: i32, include_audio: bool, on_local_frame: F)
    where
        F: Fn(SharedPixelBuffer<Rgba8Pixel>) + Send + 'static,
    {
        if self.is_running.swap(true, Ordering::SeqCst) {
            warn!("Transmissão de tela já está em execução.");
            return;
        }

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

        if target_hwnd != 0 {
            start_window_border_overlay(target_hwnd);
        }

        if include_audio {
            start_audio_loopback_tx(
                Arc::clone(&self.is_running),
                Arc::clone(&self.channel_id),
                Arc::clone(&self.my_user_id),
                Arc::clone(&self.known_peers),
            );
        }

        info!("🖥️ Iniciando transmissão P2P ({}x{} @ {} FPS, hwnd={}, audio={})...", target_w, target_h, target_fps, target_hwnd, include_audio);

        std::thread::Builder::new()
            .name("screen-capture-tx".to_string())
            .spawn(move || {
                let socket = match UdpSocket::bind("0.0.0.0:0") {
                    Ok(s) => {
                        let _ = s.set_broadcast(true);
                        s
                    }
                    Err(e) => {
                        warn!("Falha ao criar socket UDP TX: {:?}", e);
                        return;
                    }
                };

                let bcast_targets = get_broadcast_addresses();
                let frame_interval = Duration::from_millis(1000 / target_fps);
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

                    if let Some((pixel_buf, rgb_data)) = capture_screen_rgb(target_hwnd, target_w, target_h) {
                        buffer.publish(pixel_buf.clone());
                        on_local_frame(pixel_buf);

                        // Compress frame to JPEG
                        if let Some(jpeg_bytes) = encode_jpeg(&rgb_data, target_w, target_h, 68) {
                            frame_seq = frame_seq.wrapping_add(1);
                            let total_len = jpeg_bytes.len();
                            let total_chunks = ((total_len + CHUNK_SIZE - 1) / CHUNK_SIZE) as u16;

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
                    }

                    let elapsed = start_time.elapsed();
                    if elapsed < frame_interval {
                        std::thread::sleep(frame_interval - elapsed);
                    }
                }
                stop_window_border_overlay();
                info!("🖥️ Thread emissora de tela finalizada.");
            })
            .expect("Falha ao iniciar thread TX de tela");
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

                while is_running.load(Ordering::Relaxed) {
                    let current_cid = channel_id_atomic.load(Ordering::Relaxed);
                    let my_uid = my_user_id_atomic.load(Ordering::Relaxed);

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

                                        let peer_p2p_addr = SocketAddr::new(src_addr.ip(), peer_port);
                                        if let Ok(mut peers) = peers_store.lock() {
                                            peers.insert(pkt_uid, (peer_p2p_addr, Instant::now()));
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

                                            let _ = socket.send_to(&ack_pkt, peer_p2p_addr);
                                            for target in get_broadcast_addresses() {
                                                let _ = socket.send_to(&ack_pkt, target);
                                            }
                                        }

                                        let prev_state = active_streaming_users.insert(pkt_uid, is_streaming);
                                        if prev_state != Some(is_streaming) {
                                            info!("📡 Usuário {} ({}) alterou estado de stream: {}", pkt_uid, peer_p2p_addr, is_streaming);
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

                                        let peer_p2p_addr = SocketAddr::new(src_addr.ip(), peer_port);
                                        if let Ok(mut peers) = peers_store.lock() {
                                            peers.insert(pkt_uid, (peer_p2p_addr, Instant::now()));
                                        }
                                        info!("📡 Heartbeat P2P recebido do peer {} ({})", pkt_uid, peer_p2p_addr);
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

fn get_broadcast_addresses() -> Vec<SocketAddr> {
    let mut addrs = Vec::new();
    
    // Broadcast to local port cluster (50005..=50010) on loopback & 255.255.255.255
    for port in 50005..=50010 {
        addrs.push(SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)), port));
        addrs.push(SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(255, 255, 255, 255)), port));
    }

    // Query active adapter IPv4 address via routing table probe
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(SocketAddr::V4(local)) = socket.local_addr() {
                let octets = local.ip().octets();
                for port in 50005..=50008 {
                    addrs.push(SocketAddr::new(
                        std::net::IpAddr::V4(std::net::Ipv4Addr::new(octets[0], octets[1], octets[2], 255)),
                        port,
                    ));
                    addrs.push(SocketAddr::new(
                        std::net::IpAddr::V4(std::net::Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3])),
                        port,
                    ));
                }
            }
        }
    }

    // Common local network subnets (Vivo 192.168.15.x, Claro/Net 192.168.0/1.x, etc.)
    for sub in [15, 1, 0, 100, 2] {
        addrs.push(SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, sub, 255)),
            P2P_VIDEO_PORT,
        ));
    }

    // Direct LAN peer targets
    addrs.push(SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 15, 5)), P2P_VIDEO_PORT));
    addrs.push(SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 15, 2)), P2P_VIDEO_PORT));

    addrs
}

fn encode_jpeg(rgb_data: &[u8], width: u32, height: u32, quality: u8) -> Option<Vec<u8>> {
    use image::codecs::jpeg::JpegEncoder;
    use image::ColorType;
    let mut dest = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut dest, quality);
    if encoder.encode(rgb_data, width, height, ColorType::Rgb8.into()).is_ok() {
        Some(dest)
    } else {
        None
    }
}

fn decode_jpeg(jpeg_data: &[u8]) -> Option<(SharedPixelBuffer<Rgba8Pixel>, u32, u32)> {
    let img = image::load_from_memory_with_format(jpeg_data, image::ImageFormat::Jpeg).ok()?;
    let rgba = img.into_rgba8();
    let (width, height) = rgba.dimensions();

    let mut pixel_buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    let slice = pixel_buffer.make_mut_slice();
    let raw = rgba.into_raw();

    for (i, pixel) in slice.iter_mut().enumerate() {
        let offset = i * 4;
        let r = raw[offset];
        let g = raw[offset + 1];
        let b = raw[offset + 2];
        let a = raw[offset + 3];
        *pixel = Rgba8Pixel::new(r, g, b, a);
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
    vec![
        MonitorItemInfo {
            id: 0,
            name: "Tela 1 (Principal)".to_string(),
            resolution: "1920 × 1080".to_string(),
            is_primary: true,
            hwnd: 0,
        }
    ]
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
                // Clean app label based on title
                let app_type = if lower.contains("chrome") {
                    "Google Chrome"
                } else if lower.contains("firefox") {
                    "Mozilla Firefox"
                } else if lower.contains("edge") {
                    "Microsoft Edge"
                } else if lower.contains("visual studio code") || lower.contains("code") {
                    "Visual Studio Code"
                } else if lower.contains("discord") {
                    "Discord"
                } else if lower.contains("spotify") {
                    "Spotify"
                } else if lower.contains("telegram") {
                    "Telegram"
                } else if lower.contains("litecord") {
                    "Litecord"
                } else if lower.contains("terminal") || lower.contains("powershell") || lower.contains("cmd") {
                    "Terminal"
                } else if lower.contains("notepad") || lower.contains("bloco de notas") {
                    "Bloco de Notas"
                } else {
                    "Janela"
                };

                data.windows.push(CapturableWindowItem {
                    id: (hwnd as isize).to_string(),
                    title: title.clone(),
                    app_name: app_type.to_string(),
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

#[cfg(not(windows))]
pub fn list_capturable_windows() -> Vec<CapturableWindowItem> {
    vec![
        CapturableWindowItem {
            id: "1".to_string(),
            title: "Navegador Web (Firefox / Chrome)".to_string(),
            app_name: "Google Chrome".to_string(),
        },
        CapturableWindowItem {
            id: "2".to_string(),
            title: "Terminal de Linha de Comando".to_string(),
            app_name: "Terminal".to_string(),
        },
        CapturableWindowItem {
            id: "3".to_string(),
            title: "Editor de Código".to_string(),
            app_name: "Visual Studio Code".to_string(),
        },
    ]
}

#[cfg(windows)]
fn capture_screen_rgb(target_hwnd: isize, target_w: u32, target_h: u32) -> Option<(SharedPixelBuffer<Rgba8Pixel>, Vec<u8>)> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateSolidBrush, DeleteDC, DeleteObject,
        FillRect, GetDC, GetDIBits, ReleaseDC, SelectObject, SetBrushOrgEx, SetStretchBltMode, StretchBlt,
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HALFTONE, SRCCOPY,
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

            SetStretchBltMode(hdc_mem, HALFTONE);
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

            SetStretchBltMode(hdc_mem, HALFTONE);
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

        let mut bgra_buf = vec![0u8; total_pixels * 4];
        GetDIBits(
            hdc_mem,
            hbm_screen,
            0,
            target_h,
            bgra_buf.as_mut_ptr() as _,
            &mut bmi,
            DIB_RGB_COLORS,
        );

        let mut rgb_bytes = vec![0u8; total_pixels * 3];

        for (i, pixel) in slice.iter_mut().enumerate() {
            let offset_bgra = i * 4;
            let offset_rgb = i * 3;
            let b = bgra_buf[offset_bgra];
            let g = bgra_buf[offset_bgra + 1];
            let r = bgra_buf[offset_bgra + 2];

            *pixel = Rgba8Pixel::new(r, g, b, 255);
            rgb_bytes[offset_rgb] = r;
            rgb_bytes[offset_rgb + 1] = g;
            rgb_bytes[offset_rgb + 2] = b;
        }

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
fn capture_screen_rgb(_target_hwnd: isize, target_w: u32, target_h: u32) -> Option<(SharedPixelBuffer<Rgba8Pixel>, Vec<u8>)> {
    static FRAME_COUNTER: AtomicU8 = AtomicU8::new(0);

    let total_pixels = (target_w * target_h) as usize;
    let mut pixel_buffer = SharedPixelBuffer::<Rgba8Pixel>::new(target_w, target_h);
    let slice = pixel_buffer.make_mut_slice();
    let mut rgb_bytes = vec![0u8; total_pixels * 3];

    let counter = FRAME_COUNTER.fetch_add(2, Ordering::Relaxed);

    for (i, pixel) in slice.iter_mut().enumerate() {
        let x = (i % target_w as usize) as u8;
        let y = (i / target_w as usize) as u8;
        let r = x.wrapping_add(counter);
        let g = y.wrapping_add(counter / 2);
        let b = 180u8;

        *pixel = Rgba8Pixel::new(r, g, b, 255);
        let offset_rgb = i * 3;
        rgb_bytes[offset_rgb] = r;
        rgb_bytes[offset_rgb + 1] = g;
        rgb_bytes[offset_rgb + 2] = b;
    }

    if let Some(digit) = get_test_watermark_digit() {
        draw_test_watermark(slice, &mut rgb_bytes, target_w, target_h, digit);
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
