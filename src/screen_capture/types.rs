#![allow(dead_code)]

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use log::info;
use slint::{Rgba8Pixel, SharedPixelBuffer};

pub const P2P_VIDEO_PORT: u16 = 50005;
pub const MAGIC: &[u8; 4] = b"LTPV";
pub const CHUNK_SIZE: usize = 1200; // Universal MTU for Open Internet (WAN), 4G/5G, IPv6 & LAN without fragmentation

// Protocol Opcodes
pub const OP_ANNOUNCE: u8 = 1;
pub const OP_VIDEO_CHUNK: u8 = 2;
pub const OP_STOP: u8 = 3;
pub const OP_HEARTBEAT: u8 = 4;
pub const OP_AUDIO_FRAME: u8 = 5;
pub const OP_KEYFRAME_REQ: u8 = 6;
pub const OP_FEC_PARITY: u8 = 7;
pub const OP_QOS_FEEDBACK: u8 = 8;
// Protocol 3 (watching on demand, see docs/WATCH_ON_DEMAND.md).
/// Viewer -> sender, every second while watching: target uid (8) + viewer X25519 public key (32).
pub const OP_WATCH: u8 = 9;
/// Viewer -> sender: target uid (8). Stops at once instead of waiting for the lease.
pub const OP_UNWATCH: u8 = 10;
/// Sender -> viewer: viewer uid (8) + sender public key (32) + stream key wrapped with the pair key (60).
pub const OP_WATCH_ACK: u8 = 11;

static PROCESS_INSTANCE_ID: OnceLock<u32> = OnceLock::new();

pub fn get_process_instance_id() -> u32 {
    *PROCESS_INSTANCE_ID.get_or_init(|| {
        use std::time::SystemTime;
        let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
        let pid = std::process::id();
        ((nanos as u32) ^ (pid << 16) ^ (nanos >> 32) as u32) | 1
    })
}

static MY_RX_PORT: AtomicU16 = AtomicU16::new(P2P_VIDEO_PORT);

/// Woken when the voice channel or connection changes, so the signaling loop can
/// wait for a call instead of polling for one.
pub static P2P_CONTEXT_WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Tells the P2P threads that the call context changed. The receive thread blocks in
/// `recv` outside a call, so it is woken with a 1-byte datagram to its own port
/// (shorter than any real packet, so the parser drops it).
pub fn notify_p2p_context_changed() {
    P2P_CONTEXT_WAKE.notify_waiters();
    SIGNALING_WAKE.notify_one();
    wake_p2p_receiver();
}

/// Wakes only the receive thread (it may be blocked in `recv`), with the same 1-byte
/// datagram: it then sends or stops watch requests without waiting for traffic.
pub fn wake_p2p_receiver() {
    let port = get_my_rx_port();
    if port != 0 {
        if let Ok(sock) = std::net::UdpSocket::bind("127.0.0.1:0") {
            let _ = sock.send_to(&[0u8], ("127.0.0.1", port));
        }
    }
}

/// The voice channel the P2P threads should use when none was set explicitly: the
/// last joined channel, but only while still connected (it is never reset on leave).
pub fn current_voice_channel_fallback() -> u64 {
    if crate::gateway::is_connected_to_voice() {
        crate::gateway::get_my_voice_channel_id()
    } else {
        0
    }
}

pub fn get_my_rx_port() -> u16 {
    MY_RX_PORT.load(Ordering::Relaxed)
}

pub fn set_my_rx_port(port: u16) {
    MY_RX_PORT.store(port, Ordering::Relaxed);
}

pub static CURRENT_TX_FPS: AtomicU32 = AtomicU32::new(0);
pub static CURRENT_RX_FPS: AtomicU32 = AtomicU32::new(0);
/// Received stream pictures handed to the UI, and pictures skipped because no window
/// could show them (read by tests).
pub static RX_FRAMES_SHOWN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static RX_FRAMES_UNWATCHED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static KEYFRAME_REQUESTED: AtomicBool = AtomicBool::new(false);
pub static REQUESTED_BITRATE_BPS: AtomicU32 = AtomicU32::new(8_000_000);

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

pub fn get_rx_fps() -> f32 {
    let raw = CURRENT_RX_FPS.load(Ordering::Relaxed);
    (raw as f32) / 10.0
}

static TX_EPOCH: OnceLock<Instant> = OnceLock::new();
static STREAM_AUDIO_CLOCK_PTS: AtomicU32 = AtomicU32::new(0);
static STREAM_AUDIO_CLOCK_UPDATE: OnceLock<Arc<Mutex<Option<Instant>>>> = OnceLock::new();

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

pub static LAST_SEEN_PEER_ADDR: Mutex<Option<HashMap<u64, SocketAddr>>> = Mutex::new(None);
pub static ON_STREAM_STATE_CB: Mutex<Option<Arc<dyn Fn(u64, bool) + Send + Sync + 'static>>> = Mutex::new(None);
pub static SIGNALING_FORCE_WAKE: AtomicBool = AtomicBool::new(false);
pub static SIGNALING_OUT_TX: Mutex<Option<tokio::sync::mpsc::UnboundedSender<tokio_tungstenite::tungstenite::Message>>> = Mutex::new(None);
pub static PEER_STREAMING_STATES: Mutex<Option<HashMap<u64, bool>>> = Mutex::new(None);
/// `user_id` -> (`self_stream`, `self_video`) from voice state events.  Cached so
/// joining a channel can subscribe to streams that were already running.
pub static VOICE_STREAM_FLAGS: Mutex<Option<HashMap<u64, (bool, bool)>>> = Mutex::new(None);

pub fn set_user_stream_flags(uid: u64, self_stream: bool, self_video: bool) {
    if let Ok(mut lock) = VOICE_STREAM_FLAGS.lock() {
        lock.get_or_insert_with(HashMap::new).insert(uid, (self_stream, self_video));
    }
}

pub fn get_user_stream_flags(uid: u64) -> (bool, bool) {
    VOICE_STREAM_FLAGS
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().and_then(|map| map.get(&uid).copied()))
        .unwrap_or((false, false))
}

/// The peer announced (over signaling or P2P) that it is streaming.
pub fn peer_is_streaming(uid: u64) -> bool {
    PEER_STREAMING_STATES.lock().map(|g| g.as_ref().and_then(|m| m.get(&uid).copied()).unwrap_or(false)).unwrap_or(false)
}

/// Local screen share (Go Live / stream) state. The UI sets it when the share
/// starts or stops so the sidebar can show our own LIVE badge, just like it does
/// for the peers in `peer_is_streaming`.
static SELF_STREAMING: AtomicBool = AtomicBool::new(false);

pub fn set_self_streaming(streaming: bool) {
    SELF_STREAMING.store(streaming, Ordering::Relaxed);
}

pub fn is_self_streaming() -> bool {
    SELF_STREAMING.load(Ordering::Relaxed)
}

pub fn set_peer_streaming(uid: u64, streaming: bool) {
    let changed = {
        let mut lock = PEER_STREAMING_STATES.lock().unwrap_or_else(|e| e.into_inner());
        let map = lock.get_or_insert_with(HashMap::new);
        let old = map.insert(uid, streaming);
        old != Some(streaming)
    };
    if changed {
        if let Some(cb) = ON_STREAM_STATE_CB.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            cb(uid, streaming);
        }
    }
}
pub static CACHED_STUN_ADDR: Mutex<Option<(SocketAddr, Instant)>> = Mutex::new(None);
pub static SHARED_P2P_SOCKET: Mutex<Option<Arc<UdpSocket>>> = Mutex::new(None);

/// Something that may have changed our addresses happened (gateway reconnect, new
/// voice session...). The signaling loop re-checks them right away and announces
/// only if they really changed.
pub fn force_signaling_broadcast() {
    SIGNALING_FORCE_WAKE.store(true, Ordering::Relaxed);
    SIGNALING_WAKE.notify_one();
}

/// Wakes the signaling loop to re-check what it has to send; it sleeps otherwise.
pub static SIGNALING_WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Set when the signaling server reports a new peer in the room: that peer has
/// never heard of us, so presence must be sent even though nothing changed.
pub static SIGNALING_PEER_JOINED: AtomicBool = AtomicBool::new(false);

pub fn announce_to_new_peer() {
    SIGNALING_PEER_JOINED.store(true, Ordering::Relaxed);
    SIGNALING_WAKE.notify_one();
}

/// Stream state already sent out-of-band by `notify_signaling_stream_state`
/// (0 = nothing, 1 = "not streaming", 2 = "streaming"), so the signaling loop does
/// not announce the same change a second time.
pub static SIGNALING_ANNOUNCED_TX_STATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// When each peer's stream was last stopped. Video that was already in flight (or
/// still queued in the decoder) keeps arriving for a moment after the stop; without
/// this it would re-open the viewer on a frozen last frame that nothing closes.
pub static STREAM_STOPPED_AT: Mutex<Option<HashMap<u64, Instant>>> = Mutex::new(None);

/// How long leftover video is ignored after a stop, unless the peer explicitly
/// announces a new stream first (see `mark_stream_started`).
const STREAM_STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// The peer explicitly said it is streaming (announce packet or presence).
pub fn mark_stream_started(uid: u64) {
    if let Ok(mut guard) = STREAM_STOPPED_AT.lock() {
        if let Some(map) = guard.as_mut() {
            map.remove(&uid);
        }
    }
}

/// True while video from `uid` must be treated as leftovers of a stopped stream.
pub fn is_stream_recently_stopped(uid: u64) -> bool {
    match STREAM_STOPPED_AT.lock() {
        Ok(guard) => guard
            .as_ref()
            .and_then(|map| map.get(&uid))
            .map(|at| at.elapsed() < STREAM_STOP_GRACE)
            .unwrap_or(false),
        Err(_) => false,
    }
}

pub fn trigger_stream_stopped(uid: u64) {
    info!("🛑 [STREAM STOPPED] Encerrando imediatamente estado do stream UID={}", uid);
    // The next stream of this peer comes with a new key.
    super::watch::forget_stream(uid);
    if let Ok(mut guard) = STREAM_STOPPED_AT.lock() {
        guard.get_or_insert_with(HashMap::new).insert(uid, Instant::now());
    }
    if let Ok(mut guard) = PEER_STREAMING_STATES.lock() {
        if let Some(map) = guard.as_mut() {
            map.insert(uid, false);
        }
    }
    if let Ok(mut frames) = get_active_stream_frames().lock() {
        frames.remove(&uid);
    }
    let stream_ssrc: u32 = ((uid as u32) ^ 0x5354524D) | 0x8000_0000;
    if let Ok(mut queues) = crate::gateway::get_speaker_pcm_queues().lock() {
        queues.remove(&stream_ssrc);
    }
    if let Ok(guard) = ON_STREAM_STATE_CB.lock() {
        if let Some(ref cb) = *guard {
            cb(uid, false);
        }
    }
}

pub fn notify_signaling_stream_state(cid: u64, uid: u64, streaming: bool) {
    if let Ok(guard) = SIGNALING_OUT_TX.lock() {
        if let Some(ref tx) = *guard {
            let my_inst = get_process_instance_id();
            let my_rx_port = get_my_rx_port();
            let wan_addr = super::signaling::resolve_public_stun_address();
            let lan_addrs = get_local_lan_addresses(my_rx_port);
            let my_ecdh_pub = super::crypto::get_or_create_local_ecdh_keypair();
            let ecdh_hex: String = my_ecdh_pub.iter().map(|b| format!("{:02x}", b)).collect();

            let payload = serde_json::json!({
                "op": "presence",
                "cid": cid,
                "uid": uid,
                "inst": my_inst,
                "uname": "",
                "ecdh_pub": ecdh_hex,
                "streaming": streaming,
                "rx_port": my_rx_port,
                "lan_ips": lan_addrs.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                "wan_ip": wan_addr.map(|a| a.to_string()),
            });

            let current_key = super::crypto::get_voice_encryption_key(cid);
            if let Some(enc_payload) = super::crypto::encrypt_signaling_payload(&current_key, payload.to_string().as_bytes()) {
                if tx.send(tokio_tungstenite::tungstenite::Message::Binary(enc_payload.into())).is_ok() {
                    SIGNALING_ANNOUNCED_TX_STATE.store(if streaming { 2 } else { 1 }, Ordering::Relaxed);
                    return;
                }
            }
        }
    }
    // Not connected (or the send failed): let the signaling loop announce it.
    force_signaling_broadcast();
}

#[inline]
pub fn is_tailscale_or_forbidden(addr: &SocketAddr) -> bool {
    match addr.ip() {
        std::net::IpAddr::V4(v4) => {
            let oct = v4.octets();
            // 100.64.0.0/10 (Tailscale / Carrier-Grade NAT)
            (oct[0] == 100 && (oct[1] & 0xC0) == 64)
                // 169.254.0.0/16 (Link-local)
                || (oct[0] == 169 && oct[1] == 254)
                // 198.18.0.0/15 (Benchmarking / Virtual)
                || (oct[0] == 198 && (oct[1] == 18 || oct[1] == 19))
        }
        _ => false,
    }
}

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
            let name = dev.name.clone();
            result.push(CameraItemInfo {
                id: format!("{}", i),
                name,
                index: i as u32,
            });
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

pub fn get_shared_p2p_socket() -> Option<Arc<UdpSocket>> {
    SHARED_P2P_SOCKET.lock().ok()?.clone()
}

pub fn get_local_lan_addresses(port: u16) -> Vec<SocketAddr> {
    let mut addrs = Vec::new();
    let mut seen_ips = std::collections::HashSet::new();

    // 1. Probes across physical LAN gateways and internet routing (Direct Internet / Wi-Fi / Ethernet only)
    let probe_targets = [
        "8.8.8.8:80",
        "1.1.1.1:80",
        "192.168.1.1:80",
        "192.168.0.1:80",
        "192.168.15.1:80",
        "192.168.10.1:80",
        "10.0.0.1:80",
        "172.16.0.1:80",
    ];

    for target in probe_targets {
        if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
            if socket.connect(target).is_ok() {
                if let Ok(SocketAddr::V4(local)) = socket.local_addr() {
                    let ip = *local.ip();
                    let octets = ip.octets();
                    // Ignora VPNs / Tailscale (100.64.0.0/10), Link-Local (169.254.x.x) e Virtual Adapters
                    if (octets[0] == 100 && (octets[1] & 0xC0) == 64)
                        || (octets[0] == 169 && octets[1] == 254)
                        || (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19))
                    {
                        continue;
                    }
                    if !ip.is_loopback() && !ip.is_unspecified() && seen_ips.insert(ip) {
                        addrs.push(SocketAddr::new(std::net::IpAddr::V4(ip), port));
                    }
                }
            }
        }
    }

    // 2. Query hostname for all locally bound physical interface IPs
    if let Ok(hostname) = std::env::var("COMPUTERNAME") {
        if let Ok(host_addrs) = format!("{}:0", hostname).to_socket_addrs() {
            for sa in host_addrs {
                if let SocketAddr::V4(v4) = sa {
                    let ip = *v4.ip();
                    let octets = ip.octets();
                    // Ignora VPNs / Tailscale (100.64.0.0/10), Link-Local (169.254.x.x) e Virtual Adapters
                    if (octets[0] == 100 && (octets[1] & 0xC0) == 64)
                        || (octets[0] == 169 && octets[1] == 254)
                        || (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19))
                    {
                        continue;
                    }
                    if !ip.is_loopback() && !ip.is_unspecified() && seen_ips.insert(ip) {
                        addrs.push(SocketAddr::new(std::net::IpAddr::V4(ip), port));
                    }
                }
            }
        }
    }

    addrs.push(SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)), port));
    addrs
}

pub fn get_broadcast_addresses() -> Vec<SocketAddr> {
    let mut addrs = Vec::new();
    for port in 50005..=50007 {
        addrs.push(SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(255, 255, 255, 255)), port));
    }
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(SocketAddr::V4(local)) = socket.local_addr() {
                let octets = local.ip().octets();
                if !(octets[0] == 100 && (octets[1] & 0xC0) == 64) && !(octets[0] == 169 && octets[1] == 254) {
                    for port in 50005..=50007 {
                        addrs.push(SocketAddr::new(
                            std::net::IpAddr::V4(std::net::Ipv4Addr::new(octets[0], octets[1], octets[2], 255)),
                            port,
                        ));
                    }
                }
            }
        }
    }
    addrs
}

static ACTIVE_STREAM_FRAMES: OnceLock<Arc<Mutex<HashMap<u64, SharedPixelBuffer<Rgba8Pixel>>>>> = OnceLock::new();
static ACTIVE_STREAM_USERS: OnceLock<Arc<Mutex<HashMap<u64, bool>>>> = OnceLock::new();

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
