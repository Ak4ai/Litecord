//! Optional browser viewers. Reuses H.264; only system/window audio is encoded
//! as Opus. SDP is encrypted with the link fragment key; media goes directly
//! between this PC and each browser. No microphone or Discord voice is forwarded.
use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use futures_util::{SinkExt, StreamExt};
use log::{info, warn};
use str0m::change::{SdpAnswer, SdpOffer};
use str0m::format::Codec;
use str0m::media::{Frequency, MediaKind, MediaTime, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcConfig};
use tokio::sync::{mpsc, oneshot, watch};
mod audio;
mod signal;
#[cfg(test)]
mod tests;
pub const MAX_WEB_VIEWERS: usize = 3;
// Only start/stop change this fast path. Retired workers never publish global state.
static HAS_SESSION: AtomicBool = AtomicBool::new(false);
static VIDEO_EPOCH: AtomicU64 = AtomicU64::new(0);
static FRAMES: Mutex<Option<mpsc::Sender<MediaFrame>>> = Mutex::new(None);
static COMMANDS: Mutex<Option<mpsc::Sender<Command>>> = Mutex::new(None);
static SESSION: Mutex<Option<WebShare>> = Mutex::new(None);
type StatusCallback = Arc<dyn Fn() + Send + Sync>;
static STATUS_CALLBACK: Mutex<Option<StatusCallback>> = Mutex::new(None);

pub fn on_status_change(callback: impl Fn() + Send + Sync + 'static) {
    *STATUS_CALLBACK.lock().unwrap() = Some(Arc::new(callback));
}
fn status_changed() {
    let callback = STATUS_CALLBACK.lock().unwrap().clone();
    if let Some(callback) = callback { callback(); }
}
struct WebShare {
    url: String,
    state: Arc<SessionState>,
    stop: watch::Sender<bool>,
    thread: std::thread::JoinHandle<()>,
}
#[derive(Default)]
struct SessionState {
    active: AtomicBool,
    viewers: AtomicUsize,
    error: Mutex<String>,
}
enum MediaFrame {
    Video(Arc<[u8]>, Instant, u64),
    Audio(Vec<i16>, u32, Instant),
}
enum Command {
    Offer { id: String, offer: SdpOffer, reply: oneshot::Sender<Result<SdpAnswer, u16>> },
    Leave(String),
}
/// Called on a background thread: setup must never block the UI.
pub fn start() -> io::Result<String> {
    let mut session = SESSION.lock().unwrap();
    if let Some(s) = session.as_ref() {
        if s.state.active.load(Ordering::Acquire) { return Ok(s.url.clone()); }
        if !s.thread.is_finished() { return Err(io::Error::new(io::ErrorKind::WouldBlock, "Web sharing is connecting")); }
    }
    if let Some(old) = session.take() {
        HAS_SESSION.store(false, Ordering::Release);
        *FRAMES.lock().unwrap() = None;
        *COMMANDS.lock().unwrap() = None;
        cancel_session(old);
    }
    install_crypto_provider();
    let state = Arc::new(SessionState::default());
    let key: [u8; 32] = random_bytes()?;
    let host_token = random_hex::<32>()?;
    // Ownership is verifiable without storing a record for every share forever.
    // Viewers know the room/key, but never receive this independent host token.
    use sha2::Digest;
    let room = hex(&sha2::Sha256::digest(host_token.as_bytes())[..16]);
    let base = std::env::var("LITECORD_WEB_SIGNALING_URL").ok()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(super::signaling::signaling_base_url);
    let manual = std::env::var("LITECORD_WEB_MANUAL").as_deref() == Ok("1");
    let url = if manual {
        let page = std::env::temp_dir().join("litecord-web-manual.html");
        std::fs::write(&page, include_str!("../../assets/web/watch.html"))?;
        format!("file:///{}#manual", page.to_string_lossy().replace('\\', "/"))
    } else { format!("{}/watch#room={room}&key={}", http_base(&base), hex(&key)) };
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    // A picture leaves the encoder as one burst of packets (a keyframe is hundreds of them);
    // with Windows' default send buffer the socket refused a third of a 1080p60 stream.
    let sock = socket2::SockRef::from(&socket);
    let _ = sock.set_send_buffer_size(SEND_BUFFER);
    let _ = sock.set_recv_buffer_size(RECV_BUFFER);
    let ip = super::get_local_lan_addresses(0).into_iter().map(|a| a.ip())
        .find(|ip| ip.is_ipv4() && !ip.is_loopback()).unwrap_or(IpAddr::from([127, 0, 0, 1]));
    let local = SocketAddr::new(ip, socket.local_addr()?.port());
    let (stop, stop_rx) = watch::channel(false);
    let (frames_tx, frames_rx) = mpsc::channel(32);
    let (commands_tx, commands_rx) = mpsc::channel(8);
    *COMMANDS.lock().unwrap() = Some(commands_tx.clone());
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let worker_state = Arc::clone(&state);
    let thread = std::thread::Builder::new().name("web-share".into()).spawn(move || {
        crate::utils::thread_priority::raise_current_thread();
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(r) => r,
            Err(e) => { let _ = ready_tx.send(Err(e.to_string())); return; }
        };
        let running_state = Arc::clone(&worker_state);
        runtime.block_on(async move {
            let public = signal::discover_public_address(&socket);
            if let Err(e) = socket.set_nonblocking(true) {
                let _ = ready_tx.send(Err(e.to_string())); return;
            }
            let socket = match tokio::net::UdpSocket::from_std(socket) {
                Ok(s) => s,
                Err(e) => { let _ = ready_tx.send(Err(e.to_string())); return; }
            };
            if manual {
                running_state.active.store(true, Ordering::Release);
                status_changed();
                let _ = ready_tx.send(Ok(()));
                run_rtc(socket, local, public, commands_rx, frames_rx, stop_rx, &running_state).await;
                return;
            }
            let mut startup_stop = stop_rx.clone();
            let connected = tokio::select! {
                result = signal::connect(&base, &room, &host_token, &key) => result,
                _ = startup_stop.changed() => Err("Web sharing cancelled".into()),
            };
            let ws = match connected {
                Ok(ws) => ws,
                Err(e) => { let _ = ready_tx.send(Err(e)); return; }
            };
            running_state.active.store(true, Ordering::Release);
            status_changed();
            let _ = ready_tx.send(Ok(()));
            tokio::select! {
                result = signal::serve(ws, &base, &room, &host_token, key, commands_tx, stop_rx.clone()) => {
                    if let Err(e) = result { warn!("[WEB SHARE] {e}"); *running_state.error.lock().unwrap() = e; }
                },
                _ = run_rtc(socket, local, public, commands_rx, frames_rx, stop_rx, &running_state) => {},
            }
        });
        worker_state.active.store(false, Ordering::Release);
        worker_state.viewers.store(0, Ordering::Relaxed);
        status_changed();
    })?;
    *FRAMES.lock().unwrap() = Some(frames_tx);
    *session = Some(WebShare { url: url.clone(), state: Arc::clone(&state), stop, thread });
    HAS_SESSION.store(true, Ordering::Release);
    drop(session);
    match ready_rx.recv_timeout(Duration::from_secs(20)) {
        Ok(Ok(())) => {},
        result => {
            stop_if_current(Some(&state));
            return Err(io::Error::other(match result {
                Ok(Err(e)) => e,
                _ => "The web sharing server did not respond in time".into(),
            }));
        }
    }
    let current = SESSION.lock().unwrap().as_ref().is_some_and(|s| Arc::ptr_eq(&s.state, &state));
    if !current || !state.active.load(Ordering::Acquire) { return Err(io::Error::other("Web sharing was cancelled")); }
    info!("[WEB SHARE] Browser sharing enabled (up to {MAX_WEB_VIEWERS} viewers).");
    Ok(url)
}
/// Cancel immediately; dropping the handle detaches cleanup from the caller/UI.
/// Every new session has fresh channels, socket, encoder and status storage.
pub fn stop() {
    stop_if_current(None);
}
fn stop_if_current(expected: Option<&Arc<SessionState>>) {
    let mut session = SESSION.lock().unwrap();
    if let Some(expected) = expected {
        if !session.as_ref().is_some_and(|s| Arc::ptr_eq(&s.state, expected)) { return; }
    }
    HAS_SESSION.store(false, Ordering::Release);
    *FRAMES.lock().unwrap() = None;
    *COMMANDS.lock().unwrap() = None;
    let retired = session.take();
    drop(session);
    if let Some(s) = retired {
        cancel_session(s);
        info!("[WEB SHARE] Browser sharing stopped.");
    }
    status_changed();
}
fn cancel_session(s: WebShare) {
    s.state.active.store(false, Ordering::Release);
    s.state.viewers.store(0, Ordering::Relaxed);
    let _ = s.stop.send(true);
    // No join: a peer or an OS network call may still be stalled.
}
pub fn is_active() -> bool {
    HAS_SESSION.load(Ordering::Acquire) && SESSION.lock().unwrap().as_ref()
        .is_some_and(|s| s.state.active.load(Ordering::Acquire))
}
pub fn viewer_count() -> usize {
    SESSION.lock().unwrap().as_ref().map_or(0, |s| s.state.viewers.load(Ordering::Relaxed))
}
pub fn last_error() -> String {
    SESSION.lock().unwrap().as_ref().map_or_else(String::new, |s| s.state.error.lock().unwrap().clone())
}
/// Test-only manual SDP exchange. Must be called off the UI thread.
pub fn answer_manual(offer: SdpOffer) -> Result<SdpAnswer, String> {
    if std::env::var("LITECORD_WEB_MANUAL").as_deref() != Ok("1") || !is_active() {
        return Err("Manual web sharing is not active".into());
    }
    let tx = COMMANDS.lock().unwrap().clone().ok_or("Web sharing stopped")?;
    let (reply, rx) = oneshot::channel();
    tx.blocking_send(Command::Offer { id: random_hex::<16>().map_err(|e| e.to_string())?, offer, reply })
        .map_err(|_| "Web sharing stopped")?;
    rx.blocking_recv().map_err(|_| "Web sharing stopped".to_string())?
        .map_err(|status| format!("Offer rejected ({status})"))
}
pub fn offer_frame(annex_b: &[u8]) {
    if !is_active() || viewer_count() == 0 || !looks_like_h264(annex_b) { return; }
    if let Some(tx) = FRAMES.lock().unwrap().as_ref() {
        let epoch = VIDEO_EPOCH.load(Ordering::Relaxed);
        if tx.try_send(MediaFrame::Video(Arc::from(annex_b), Instant::now(), epoch)).is_err() {
            VIDEO_EPOCH.fetch_add(1, Ordering::Relaxed);
            super::request_intra_keyframe();
        }
    }
}
/// The existing mono loopback stream, never the call/microphone stream.
pub fn offer_audio(pcm: &[i16], sample_rate: u32) {
    if !is_active() || viewer_count() == 0 { return; }
    if let Some(tx) = FRAMES.lock().unwrap().as_ref() {
        let _ = tx.try_send(MediaFrame::Audio(pcm.to_vec(), sample_rate, Instant::now()));
    }
}
fn looks_like_h264(data: &[u8]) -> bool {
    data.starts_with(&[0, 0, 0, 1]) || data.starts_with(&[0, 0, 1])
}
struct Viewer {
    id: String,
    rtc: Rtc,
    video: Option<(Mid, Pt)>,
    audio: Option<(Mid, Pt)>,
    connected: bool,
    waiting_keyframe: bool,
    video_epoch: u64,
    created: Instant,
    /// ICE lost the path (Wi-Fi roaming, a NAT rebinding, a busy network) at this moment;
    /// it often comes back within seconds.
    lost_since: Option<Instant>,
}
/// How long a viewer whose connection dropped may take to come back before it is removed.
const ICE_GRACE: Duration = Duration::from_secs(15);
/// Socket buffers: room for a few keyframes at once.
const SEND_BUFFER: usize = 4 << 20;
const RECV_BUFFER: usize = 1 << 20;
/// Packets the socket could not take yet, kept to send as soon as it can. Past this many
/// the oldest are dropped (a viewer that far behind needs a new picture, not old packets).
const BACKLOG_MAX: usize = 2048;

type Backlog = std::collections::VecDeque<(Vec<u8>, SocketAddr)>;

/// Sends `packet` now, or queues it behind the ones the socket refused so they keep their order.
fn send_or_queue(socket: &tokio::net::UdpSocket, backlog: &mut Backlog, packet: &[u8], to: SocketAddr, stats: &mut WebStats) {
    if backlog.is_empty() {
        match socket.try_send_to(packet, to) {
            Ok(_) => return,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {},
            Err(_) => { stats.send_errors += 1; return; },
        }
    }
    if backlog.len() >= BACKLOG_MAX {
        backlog.pop_front();
        stats.send_errors += 1;
    }
    stats.queued += 1;
    backlog.push_back((packet.to_vec(), to));
}

/// Sends what the socket accepts of the queue.
fn flush_backlog(socket: &tokio::net::UdpSocket, backlog: &mut Backlog, stats: &mut WebStats) {
    while let Some((packet, to)) = backlog.front() {
        match socket.try_send_to(packet, *to) {
            Ok(_) => { backlog.pop_front(); },
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => { stats.send_errors += 1; backlog.pop_front(); },
        }
    }
}
async fn run_rtc(
    socket: tokio::net::UdpSocket, local: SocketAddr, mut public: Option<SocketAddr>,
    mut commands: mpsc::Receiver<Command>, mut frames: mpsc::Receiver<MediaFrame>,
    mut stop: watch::Receiver<bool>,
    state: &SessionState,
) {
    let started = Instant::now();
    let mut mapped_at = started;
    let mut viewers: Vec<Viewer> = Vec::new();
    let mut audio: Option<audio::AudioEncoder> = None;
    let mut buf = vec![0u8; 65536];
    // Counted while somebody watches and logged every 5 s, to tell where a frozen picture
    // comes from: frames not arriving, frames held back for a keyframe, or packets the
    // socket refused.
    let mut stats = WebStats::default();
    let mut stats_since = Instant::now();
    let mut backlog = Backlog::new();
    loop {
        flush_backlog(&socket, &mut backlog, &mut stats);
        let now = Instant::now();
        let mut next = now + Duration::from_secs(3600);
        viewers.retain_mut(|v| {
            if !v.connected && now.duration_since(v.created) > Duration::from_secs(30) { return false; }
            if v.lost_since.is_some_and(|t| now.duration_since(t) > ICE_GRACE) {
                warn!("[WEB SHARE] Viewer did not come back within {} s: removed", ICE_GRACE.as_secs());
                return false;
            }
            loop {
                match v.rtc.poll_output() {
                    Ok(Output::Transmit(t)) => {
                        stats.packets += 1;
                        send_or_queue(&socket, &mut backlog, &t.contents, t.destination, &mut stats);
                    },
                    Ok(Output::Timeout(t)) => { next = next.min(t); break true; },
                    Ok(Output::Event(e)) => match e {
                        Event::MediaAdded(m) => {
                            if let Some(writer) = v.rtc.writer(m.mid) {
                                let codec = if m.kind == MediaKind::Video { Codec::H264 } else { Codec::Opus };
                                let pt = writer.payload_params().find(|p| p.spec().codec == codec).map(|p| p.pt());
                                if let Some(pt) = pt {
                                    if m.kind == MediaKind::Video { v.video = Some((m.mid, pt)); }
                                    else { v.audio = Some((m.mid, pt)); }
                                }
                            }
                        },
                        Event::IceConnectionStateChange(IceConnectionState::Connected | IceConnectionState::Completed) => {
                            if let Some(t) = v.lost_since.take() {
                                info!("[WEB SHARE] Viewer connection back after {:.1} s", t.elapsed().as_secs_f64());
                            }
                            v.connected = true;
                            v.waiting_keyframe = true;
                            super::request_intra_keyframe();
                        },
                        Event::KeyframeRequest(_) => { stats.keyframe_requests += 1; v.waiting_keyframe = true; super::request_intra_keyframe(); },
                        Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                            warn!("[WEB SHARE] Viewer connection lost (ICE disconnected) after {:.1} s; waiting up to {} s for it",
                                v.created.elapsed().as_secs_f64(), ICE_GRACE.as_secs());
                            if !v.connected { break false; }
                            v.lost_since.get_or_insert(now);
                        },
                        _ => {},
                    },
                    Err(e) => { warn!("[WEB SHARE] Connection closed: {e}"); break false; },
                }
            }
        });
        let count = viewers.iter().filter(|v| v.connected).count();
        if count > 0 && stats_since.elapsed() >= Duration::from_secs(5) {
            info!("[WEB SHARE] 5 s: {} frames in, {} sent, {} stale, {} held for a keyframe, {} keyframe requests, {} packets, {} queued by the socket, {} lost, {} viewer(s)",
                stats.frames_in, stats.frames_sent, stats.stale, stats.held, stats.keyframe_requests, stats.packets, stats.queued, stats.send_errors, count);
            stats = WebStats::default();
            stats_since = Instant::now();
        } else if count == 0 {
            stats_since = Instant::now();
        }
        if state.viewers.swap(count, Ordering::Relaxed) != count { status_changed(); }
        if viewers.is_empty() { audio = None; }
        tokio::select! {
            biased;
            _ = stop.changed() => break,
            command = commands.recv() => match command {
                Some(Command::Offer { id, offer, reply }) => {
                    viewers.retain(|v| v.id != id);
                    if viewers.len() >= MAX_WEB_VIEWERS { let _ = reply.send(Err(409)); continue; }
                    // An unused NAT mapping can expire. Refresh on demand, with no
                    // periodic STUN traffic or timer while nobody is watching.
                    if viewers.is_empty() && mapped_at.elapsed() > Duration::from_secs(20) {
                        public = signal::refresh_public_address(&socket).await.or(public);
                        mapped_at = Instant::now();
                    }
                    let mut rtc = RtcConfig::new().clear_codecs().enable_h264(true).enable_opus(true, false).build(Instant::now());
                    if let Ok(c) = Candidate::host(local, "udp") { rtc.add_local_candidate(c); }
                    if let Some(addr) = public {
                        if let Ok(c) = Candidate::server_reflexive(addr, local, "udp") { rtc.add_local_candidate(c); }
                    }
                    match rtc.sdp_api().accept_offer(offer) {
                        Ok(answer) => {
                            viewers.push(Viewer { id, rtc, video: None, audio: None, connected: false,
                                waiting_keyframe: true, video_epoch: VIDEO_EPOCH.load(Ordering::Relaxed), created: Instant::now(),
                                lost_since: None });
                            let _ = reply.send(Ok(answer));
                        },
                        Err(_) => { let _ = reply.send(Err(400)); },
                    }
                },
                Some(Command::Leave(id)) => viewers.retain(|v| v.id != id),
                None => break,
            },
            packet = socket.recv_from(&mut buf) => {
                if let Ok((n, source)) = packet {
                    if let Ok(contents) = buf[..n].try_into() {
                        let input = Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source, destination: local, contents });
                        if let Some(v) = viewers.iter_mut().find(|v| v.rtc.accepts(&input)) { let _ = v.rtc.handle_input(input); }
                    }
                }
            },
            frame = frames.recv() => match frame {
                Some(MediaFrame::Video(frame, at, epoch)) => {
                    stats.frames_in += 1;
                    let stale = at.elapsed() > Duration::from_millis(150);
                    let idr = contains_idr(&frame);
                    for v in &mut viewers {
                        if !v.connected || v.lost_since.is_some() { continue; }
                        if v.video_epoch != epoch {
                            v.video_epoch = epoch;
                            v.waiting_keyframe = true;
                            super::request_intra_keyframe();
                        }
                        if stale { stats.stale += 1; v.waiting_keyframe = true; super::request_intra_keyframe(); continue; }
                        if v.waiting_keyframe && !idr { stats.held += 1; continue; }
                        v.waiting_keyframe = false;
                        stats.frames_sent += 1;
                        if let Some((mid, pt)) = v.video {
                            if let Some(writer) = v.rtc.writer(mid) {
                                let time = MediaTime::new(at.duration_since(started).as_micros() as u64 * 9 / 100, Frequency::NINETY_KHZ);
                                let _ = writer.write(pt, at, time, Arc::clone(&frame));
                            }
                        }
                    }
                },
                Some(MediaFrame::Audio(pcm, rate, at)) => {
                    if at.elapsed() > Duration::from_millis(150) { audio = None; continue; }
                    if audio.is_none() { audio = audio::AudioEncoder::new(); }
                    if let Some(encoder) = audio.as_mut() {
                        for (packet, offset) in encoder.push(&pcm, rate) {
                            let time = MediaTime::new(offset, Frequency::FORTY_EIGHT_KHZ);
                            for v in &mut viewers {
                                if let Some((mid, pt)) = v.audio.filter(|_| v.connected && v.lost_since.is_none()) {
                                    if let Some(writer) = v.rtc.writer(mid) { let _ = writer.write(pt, at, time, packet.clone()); }
                                }
                            }
                        }
                    }
                },
                None => break,
            },
            _ = socket.writable(), if !backlog.is_empty() => {
                flush_backlog(&socket, &mut backlog, &mut stats);
            },
            _ = tokio::time::sleep_until(next.into()) => {
                let now = Instant::now();
                for v in &mut viewers { let _ = v.rtc.handle_input(Input::Timeout(now)); }
            },
        }
    }
    state.viewers.store(0, Ordering::Relaxed);
}
#[derive(Default)]
struct WebStats {
    frames_in: u32,
    frames_sent: u32,
    stale: u32,
    held: u32,
    keyframe_requests: u32,
    packets: u32,
    /// Packets the socket could not take at once (sent later from the queue).
    queued: u32,
    /// Packets lost for good: a send error, or the queue was full.
    send_errors: u32,
}
fn contains_idr(data: &[u8]) -> bool {
    data.windows(4).any(|w| w[..3] == [0, 0, 1] && w[3] & 0x1f == 5)
}
fn install_crypto_provider() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| str0m::crypto::from_feature_flags().install_process_default());
}
fn random_bytes<const N: usize>() -> io::Result<[u8; N]> {
    use rand::RngCore;
    let mut bytes = [0u8; N];
    rand::rngs::OsRng.try_fill_bytes(&mut bytes).map_err(io::Error::other)?;
    Ok(bytes)
}
fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }
fn random_hex<const N: usize>() -> io::Result<String> { Ok(hex(&random_bytes::<N>()?)) }
fn http_base(base: &str) -> String { base.trim_end_matches('/').replacen("wss://", "https://", 1).replacen("ws://", "http://", 1) }
