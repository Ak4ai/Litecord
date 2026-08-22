#![allow(dead_code)]

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(not(windows))]
use std::sync::atomic::AtomicU8;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use log::{info, warn};
use slint::{Rgba8Pixel, SharedPixelBuffer};

const P2P_VIDEO_PORT: u16 = 50005;
const MAGIC: &[u8; 4] = b"LTPV"; // Litecord P2P Video
const CHUNK_SIZE: usize = 1200; // Safe MTU for local & VPN UDP transmission

// Protocol Opcodes
const OP_ANNOUNCE: u8 = 1;
const OP_VIDEO_CHUNK: u8 = 2;
const OP_STOP: u8 = 3;
const OP_HEARTBEAT: u8 = 4;

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

    pub fn shared_buffer(&self) -> Arc<SharedFrameBuffer> {
        Arc::clone(&self.shared_buffer)
    }

    pub fn stop(&self) {
        if self.is_running.swap(false, Ordering::SeqCst) {
            info!("🛑 Parando captura e transmissão de tela P2P...");
            let cid = self.channel_id.load(Ordering::Relaxed);
            let uid = self.my_user_id.load(Ordering::Relaxed);
            let bcast_targets = get_broadcast_addresses();

            if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
                let _ = socket.set_broadcast(true);
                let mut stop_pkt = Vec::with_capacity(21);
                stop_pkt.extend_from_slice(MAGIC);
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
    pub fn start<F>(&self, target_hwnd: isize, res: i32, fps: i32, on_local_frame: F)
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

        info!("🖥️ Iniciando transmissão P2P ({}x{} @ {} FPS, hwnd={})...", target_w, target_h, target_fps, target_hwnd);

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
                    let mut ann_pkt = Vec::with_capacity(24 + uname_bytes.len());
                    ann_pkt.extend_from_slice(MAGIC);
                    ann_pkt.push(OP_ANNOUNCE);
                    ann_pkt.extend_from_slice(&cid.to_be_bytes());
                    ann_pkt.extend_from_slice(&uid.to_be_bytes());
                    ann_pkt.push(1); // is_streaming = true
                    ann_pkt.push(match res {
                        1080 => 0,
                        720 => 3,
                        _ => 4,
                    });
                    ann_pkt.push(uname_bytes.len() as u8);
                    ann_pkt.extend_from_slice(uname_bytes);

                    for _ in 0..3 {
                        for target in &bcast_targets {
                            let _ = socket.send_to(&ann_pkt, target);
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
                        let mut ann_pkt = Vec::with_capacity(24 + uname_bytes.len());
                        ann_pkt.extend_from_slice(MAGIC);
                        ann_pkt.push(OP_ANNOUNCE);
                        ann_pkt.extend_from_slice(&cid.to_be_bytes());
                        ann_pkt.extend_from_slice(&uid.to_be_bytes());
                        ann_pkt.push(1);
                        ann_pkt.push(match res {
                            1080 => 0,
                            720 => 3,
                            _ => 4,
                        });
                        ann_pkt.push(uname_bytes.len() as u8);
                        ann_pkt.extend_from_slice(uname_bytes);

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

                                let mut pkt = Vec::with_capacity(29 + chunk_slice.len());
                                pkt.extend_from_slice(MAGIC);
                                pkt.push(OP_VIDEO_CHUNK);
                                pkt.extend_from_slice(&cid.to_be_bytes());
                                pkt.extend_from_slice(&uid.to_be_bytes());
                                pkt.extend_from_slice(&frame_seq.to_be_bytes());
                                pkt.extend_from_slice(&total_chunks.to_be_bytes());
                                pkt.extend_from_slice(&chunk_idx.to_be_bytes());
                                pkt.extend_from_slice(chunk_slice);

                                let mut sent_direct = false;
                                if let Ok(peers) = peers_store.lock() {
                                    for (&_, &(addr, _)) in peers.iter() {
                                        let _ = socket.send_to(&pkt, addr);
                                        sent_direct = true;
                                    }
                                }
                                if !sent_direct {
                                    for target in &bcast_targets {
                                        let _ = socket.send_to(&pkt, target);
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
        let channel_id_atomic = Arc::clone(&self.channel_id);
        let my_user_id_atomic = Arc::clone(&self.my_user_id);
        let my_username_arc = Arc::clone(&self.my_username);
        let peers_store = Arc::clone(&self.known_peers);

        std::thread::Builder::new()
            .name("screen-capture-rx".to_string())
            .spawn(move || {
                let socket = match UdpSocket::bind(format!("0.0.0.0:{}", P2P_VIDEO_PORT)) {
                    Ok(s) => {
                        let _ = s.set_broadcast(true);
                        let _ = s.set_read_timeout(Some(Duration::from_millis(100)));
                        s
                    }
                    Err(e) => {
                        warn!("Porta fixa {} em uso: {:?}. Usando porta dinâmica...", P2P_VIDEO_PORT, e);
                        match UdpSocket::bind("0.0.0.0:0") {
                            Ok(s) => {
                                let _ = s.set_broadcast(true);
                                let _ = s.set_read_timeout(Some(Duration::from_millis(100)));
                                s
                            }
                            Err(e2) => {
                                warn!("Falha ao inicializar socket UDP RX: {:?}", e2);
                                return;
                            }
                        }
                    }
                };

                let mut recv_buf = vec![0u8; 65535];
                struct InFlightFrame {
                    seq: u32,
                    total_chunks: u16,
                    received: HashMap<u16, Vec<u8>>,
                    first_seen: Instant,
                }
                let mut in_flight: HashMap<u64, InFlightFrame> = HashMap::new();
                let mut peer_names: HashMap<u64, String> = HashMap::new();
                let mut active_streaming_users: HashMap<u64, bool> = HashMap::new();
                let mut last_stream_activity: HashMap<u64, Instant> = HashMap::new();

                info!("📡 Receptor UDP P2P de vídeo escutando na porta {}...", P2P_VIDEO_PORT);

                let local_socket_addr = socket.local_addr().ok();

                while is_running.load(Ordering::Relaxed) {
                    let current_cid = channel_id_atomic.load(Ordering::Relaxed);
                    let my_uid = my_user_id_atomic.load(Ordering::Relaxed);

                    match socket.recv_from(&mut recv_buf) {
                        Ok((len, src_addr)) => {
                            if len < 5 || &recv_buf[0..4] != MAGIC {
                                continue;
                            }
                            if let Some(l_addr) = local_socket_addr {
                                if src_addr == l_addr {
                                    continue;
                                }
                            }
                            let op = recv_buf[4];

                            match op {
                                OP_ANNOUNCE => {
                                    if len >= 24 {
                                        let pkt_uid = u64::from_be_bytes(recv_buf[13..21].try_into().unwrap());

                                        let is_streaming = recv_buf[21] == 1;
                                        let name_len = recv_buf[23] as usize;
                                        if len >= 24 + name_len {
                                            if let Ok(uname) = std::str::from_utf8(&recv_buf[24..24 + name_len]) {
                                                peer_names.insert(pkt_uid, uname.to_string());
                                            }
                                        }

                                        if let Ok(mut peers) = peers_store.lock() {
                                            peers.insert(pkt_uid, (src_addr, Instant::now()));
                                        }

                                        // Instant reciprocal heartbeat so transmitter gets our direct IP
                                        if is_streaming {
                                            let uname = my_username_arc.lock().unwrap().clone();
                                            let uname_bytes = uname.as_bytes();
                                            let mut ack_pkt = Vec::with_capacity(24 + uname_bytes.len());
                                            ack_pkt.extend_from_slice(MAGIC);
                                            ack_pkt.push(OP_HEARTBEAT);
                                            ack_pkt.extend_from_slice(&current_cid.to_be_bytes());
                                            ack_pkt.extend_from_slice(&my_uid.to_be_bytes());
                                            ack_pkt.push(0);
                                            ack_pkt.push(2);
                                            ack_pkt.push(uname_bytes.len() as u8);
                                            ack_pkt.extend_from_slice(uname_bytes);
                                            let _ = socket.send_to(&ack_pkt, src_addr);
                                        }

                                        let prev_state = active_streaming_users.insert(pkt_uid, is_streaming);
                                        if prev_state != Some(is_streaming) {
                                            info!("📡 Usuário {} ({}) alterou estado de stream: {}", pkt_uid, src_addr, is_streaming);
                                            on_state(pkt_uid, is_streaming);
                                        }
                                    }
                                }
                                OP_VIDEO_CHUNK => {
                                    if len > 29 {
                                        let pkt_uid = u64::from_be_bytes(recv_buf[13..21].try_into().unwrap());

                                        let seq = u32::from_be_bytes(recv_buf[21..25].try_into().unwrap());
                                        let total = u16::from_be_bytes(recv_buf[25..27].try_into().unwrap());
                                        let idx = u16::from_be_bytes(recv_buf[27..29].try_into().unwrap());
                                        let chunk_data = recv_buf[29..len].to_vec();

                                        if let Ok(mut peers) = peers_store.lock() {
                                            peers.insert(pkt_uid, (src_addr, Instant::now()));
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

                                            if let Some((pixel_buffer, w, h)) = decode_jpeg(&complete_jpeg) {
                                                if let Ok(mut f_map) = get_active_stream_frames().lock() {
                                                    f_map.insert(pkt_uid, pixel_buffer.clone());
                                                }
                                                let uname = peer_names.get(&pkt_uid).cloned().unwrap_or_else(|| format!("Usuário {}", pkt_uid));
                                                let quality_label = format!("{}x{}", w, h);
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
    addrs.push(SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(255, 255, 255, 255)), P2P_VIDEO_PORT));
    addrs.push(SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)), P2P_VIDEO_PORT));

    // Query active adapter IPv4 address via routing table probe
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(SocketAddr::V4(local)) = socket.local_addr() {
                let octets = local.ip().octets();
                addrs.push(SocketAddr::new(
                    std::net::IpAddr::V4(std::net::Ipv4Addr::new(octets[0], octets[1], octets[2], 255)),
                    P2P_VIDEO_PORT,
                ));
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
            if !title.is_empty() && title != "Program Manager" && title != "Settings" {
                data.windows.push(CapturableWindowItem {
                    id: (hwnd as isize).to_string(),
                    title: title.clone(),
                    app_name: title,
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
            app_name: "Browser".to_string(),
        },
        CapturableWindowItem {
            id: "2".to_string(),
            title: "Terminal de Linha de Comando".to_string(),
            app_name: "Terminal".to_string(),
        },
        CapturableWindowItem {
            id: "3".to_string(),
            title: "Editor de Código".to_string(),
            app_name: "Code Editor".to_string(),
        },
    ]
}

#[cfg(windows)]
fn capture_screen_rgb(target_hwnd: isize, target_w: u32, target_h: u32) -> Option<(SharedPixelBuffer<Rgba8Pixel>, Vec<u8>)> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
        ReleaseDC, SelectObject, SetBrushOrgEx, SetStretchBltMode, StretchBlt, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        HALFTONE, SRCCOPY,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetDesktopWindow, GetSystemMetrics, GetWindowRect, SM_CXSCREEN, SM_CYSCREEN,
    };

    unsafe {
        let hwnd_desktop = GetDesktopWindow();
        let hdc_desktop = GetDC(hwnd_desktop);
        if hdc_desktop.is_null() {
            return None;
        }

        let (src_x, src_y, src_w, src_h) = if target_hwnd != 0 {
            let hwnd = target_hwnd as windows_sys::Win32::Foundation::HWND;
            let mut rc: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut rc);
            let w = (rc.right - rc.left).max(1);
            let h = (rc.bottom - rc.top).max(1);
            (rc.left.max(0), rc.top.max(0), w, h)
        } else {
            (0, 0, GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
        };

        if src_w <= 0 || src_h <= 0 {
            ReleaseDC(hwnd_desktop, hdc_desktop);
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

        SetStretchBltMode(hdc_mem, HALFTONE);
        SetBrushOrgEx(hdc_mem, 0, 0, std::ptr::null_mut());

        StretchBlt(
            hdc_mem,
            0,
            0,
            target_w as i32,
            target_h as i32,
            hdc_desktop,
            src_x,
            src_y,
            src_w,
            src_h,
            SRCCOPY,
        );

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

    Some((pixel_buffer, rgb_bytes))
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
