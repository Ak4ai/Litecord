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
static ACTIVE: AtomicBool = AtomicBool::new(false);
static VIEWERS: AtomicUsize = AtomicUsize::new(0);
static VIDEO_EPOCH: AtomicU64 = AtomicU64::new(0);
static FRAMES: Mutex<Option<mpsc::Sender<MediaFrame>>> = Mutex::new(None);
static COMMANDS: Mutex<Option<mpsc::Sender<Command>>> = Mutex::new(None);
static SESSION: Mutex<Option<WebShare>> = Mutex::new(None);
static LAST_ERROR: Mutex<String> = Mutex::new(String::new());
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
    stop: watch::Sender<bool>,
    thread: std::thread::JoinHandle<()>,
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
        if is_active() { return Ok(s.url.clone()); }
        if !s.thread.is_finished() { return Err(io::Error::new(io::ErrorKind::WouldBlock, "Web sharing is connecting")); }
    }
    if let Some(old) = session.take() {
        let _ = old.stop.send(true);
        let _ = old.thread.join();
    }
    install_crypto_provider();
    LAST_ERROR.lock().unwrap().clear();
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
    let ip = super::get_local_lan_addresses(0).into_iter().map(|a| a.ip())
        .find(|ip| ip.is_ipv4() && !ip.is_loopback()).unwrap_or(IpAddr::from([127, 0, 0, 1]));
    let local = SocketAddr::new(ip, socket.local_addr()?.port());
    let (stop, stop_rx) = watch::channel(false);
    let (frames_tx, frames_rx) = mpsc::channel(32);
    let (commands_tx, commands_rx) = mpsc::channel(8);
    *COMMANDS.lock().unwrap() = Some(commands_tx.clone());
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let thread = std::thread::Builder::new().name("web-share".into()).spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(r) => r,
            Err(e) => { let _ = ready_tx.send(Err(e.to_string())); return; }
        };
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
                ACTIVE.store(true, Ordering::Release);
                status_changed();
                let _ = ready_tx.send(Ok(()));
                run_rtc(socket, local, public, commands_rx, frames_rx, stop_rx).await;
                return;
            }
            let mut startup_stop = stop_rx.clone();
            let connected = tokio::select! {
                result = signal::connect(&base, &room, &host_token) => result,
                _ = startup_stop.changed() => Err("Web sharing cancelled".into()),
            };
            let ws = match connected {
                Ok(ws) => ws,
                Err(e) => { let _ = ready_tx.send(Err(e)); return; }
            };
            ACTIVE.store(true, Ordering::Release);
            status_changed();
            let _ = ready_tx.send(Ok(()));
            tokio::select! {
                result = signal::run(ws, key, commands_tx, stop_rx.clone()) => {
                    if let Err(e) = result { warn!("[WEB SHARE] {e}"); *LAST_ERROR.lock().unwrap() = e; }
                },
                _ = run_rtc(socket, local, public, commands_rx, frames_rx, stop_rx) => {},
            }
        });
        ACTIVE.store(false, Ordering::Release);
        VIEWERS.store(0, Ordering::Relaxed);
        status_changed();
    })?;
    *FRAMES.lock().unwrap() = Some(frames_tx);
    *session = Some(WebShare { url: url.clone(), stop, thread });
    drop(session);
    match ready_rx.recv_timeout(Duration::from_secs(20)) {
        Ok(Ok(())) => {},
        result => {
            self::stop();
            return Err(io::Error::other(match result {
                Ok(Err(e)) => e,
                _ => "The web sharing server did not respond in time".into(),
            }));
        }
    }
    if !is_active() { return Err(io::Error::other("Web sharing was cancelled")); }
    info!("[WEB SHARE] Browser sharing enabled (up to {MAX_WEB_VIEWERS} viewers).");
    Ok(url)
}
/// Joining releases the old socket/encoder before a new session can start.
pub fn stop() {
    let mut session = SESSION.lock().unwrap();
    ACTIVE.store(false, Ordering::Release);
    *FRAMES.lock().unwrap() = None;
    *COMMANDS.lock().unwrap() = None;
    if let Some(s) = session.take() {
        let _ = s.stop.send(true);
        let _ = s.thread.join();
        info!("[WEB SHARE] Browser sharing stopped.");
    }
    VIEWERS.store(0, Ordering::Relaxed);
    status_changed();
}
pub fn is_active() -> bool { ACTIVE.load(Ordering::Acquire) }
pub fn viewer_count() -> usize { VIEWERS.load(Ordering::Relaxed) }
pub fn last_error() -> String { LAST_ERROR.lock().unwrap().clone() }
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
}
async fn run_rtc(
    socket: tokio::net::UdpSocket, local: SocketAddr, mut public: Option<SocketAddr>,
    mut commands: mpsc::Receiver<Command>, mut frames: mpsc::Receiver<MediaFrame>,
    mut stop: watch::Receiver<bool>,
) {
    let started = Instant::now();
    let mut mapped_at = started;
    let mut viewers: Vec<Viewer> = Vec::new();
    let mut audio: Option<audio::AudioEncoder> = None;
    let mut buf = vec![0u8; 65536];
    loop {
        let now = Instant::now();
        let mut next = now + Duration::from_secs(3600);
        viewers.retain_mut(|v| {
            if !v.connected && now.duration_since(v.created) > Duration::from_secs(30) { return false; }
            loop {
                match v.rtc.poll_output() {
                    Ok(Output::Transmit(t)) => { let _ = socket.try_send_to(&t.contents, t.destination); },
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
                            v.connected = true;
                            super::request_intra_keyframe();
                        },
                        Event::KeyframeRequest(_) => { v.waiting_keyframe = true; super::request_intra_keyframe(); },
                        Event::IceConnectionStateChange(IceConnectionState::Disconnected) => break false,
                        _ => {},
                    },
                    Err(e) => { warn!("[WEB SHARE] Connection closed: {e}"); break false; },
                }
            }
        });
        let count = viewers.iter().filter(|v| v.connected).count();
        if VIEWERS.swap(count, Ordering::Relaxed) != count { status_changed(); }
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
                                waiting_keyframe: true, video_epoch: VIDEO_EPOCH.load(Ordering::Relaxed), created: Instant::now() });
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
                    let stale = at.elapsed() > Duration::from_millis(150);
                    let idr = contains_idr(&frame);
                    for v in &mut viewers {
                        if !v.connected { continue; }
                        if v.video_epoch != epoch {
                            v.video_epoch = epoch;
                            v.waiting_keyframe = true;
                            super::request_intra_keyframe();
                        }
                        if stale { v.waiting_keyframe = true; super::request_intra_keyframe(); continue; }
                        if v.waiting_keyframe && !idr { continue; }
                        v.waiting_keyframe = false;
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
                                if let Some((mid, pt)) = v.audio.filter(|_| v.connected) {
                                    if let Some(writer) = v.rtc.writer(mid) { let _ = writer.write(pt, at, time, packet.clone()); }
                                }
                            }
                        }
                    }
                },
                None => break,
            },
            _ = tokio::time::sleep_until(next.into()) => {
                let now = Instant::now();
                for v in &mut viewers { let _ = v.rtc.handle_input(Input::Timeout(now)); }
            },
        }
    }
    VIEWERS.store(0, Ordering::Relaxed);
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
