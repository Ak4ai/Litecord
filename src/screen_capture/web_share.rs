//! Watching a screen share in a browser over WebRTC.
//!
//! While web sharing is on, the frames the stream encoder produces (H.264, Annex B) are
//! also sent as a standard WebRTC video track to each browser that connected, so the page
//! is a plain `<video>` and the browser decodes it. Nothing runs and no port is open while
//! it is off. Prototype stage: the page and the offer/answer exchange are served by the app
//! itself over HTTP on the local network.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use log::{info, warn};
use str0m::change::{SdpAnswer, SdpOffer};
use str0m::format::Codec;
use str0m::media::{Frequency, MediaKind, MediaTime, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcConfig};

/// Each browser is one more upload of the whole stream from this machine.
pub const MAX_WEB_VIEWERS: usize = 3;

const PAGE: &str = include_str!("../../assets/web/watch.html");

static ACTIVE: AtomicBool = AtomicBool::new(false);
static FRAMES: Mutex<Option<SyncSender<Arc<[u8]>>>> = Mutex::new(None);
static SESSION: Mutex<Option<WebShare>> = Mutex::new(None);

struct WebShare {
    url: String,
    stop: Arc<AtomicBool>,
}

/// Starts web sharing (or returns the running session's link).
pub fn start() -> std::io::Result<String> {
    let mut session = SESSION.lock().unwrap();
    if let Some(s) = session.as_ref() {
        return Ok(s.url.clone());
    }
    install_crypto_provider();

    let host_ip = local_ip().unwrap_or(IpAddr::from([127, 0, 0, 1]));
    let http = TcpListener::bind((host_ip, 0))?;
    let media = UdpSocket::bind((IpAddr::from([0, 0, 0, 0]), 0))?;
    let key = random_key();
    let url = format!("http://{}/?k={key}", http.local_addr()?);
    let stop = Arc::new(AtomicBool::new(false));

    let (offer_tx, offer_rx) = mpsc::channel::<OfferRequest>();
    let (frame_tx, frame_rx) = mpsc::sync_channel::<Arc<[u8]>>(8);
    *FRAMES.lock().unwrap() = Some(frame_tx);
    ACTIVE.store(true, Ordering::Relaxed);

    let media_addr = SocketAddr::new(host_ip, media.local_addr()?.port());
    let stop_rtc = Arc::clone(&stop);
    std::thread::Builder::new()
        .name("web-share-rtc".into())
        .spawn(move || run_rtc(media, media_addr, offer_rx, frame_rx, stop_rtc))?;
    let stop_http = Arc::clone(&stop);
    std::thread::Builder::new()
        .name("web-share-http".into())
        .spawn(move || run_http(http, key, offer_tx, stop_http))?;

    info!("🌐 [WEB SHARE] Transmissão pela web ativa em {url}");
    *session = Some(WebShare { url: url.clone(), stop });
    Ok(url)
}

/// Stops web sharing and disconnects every browser.
pub fn stop() {
    if let Some(s) = SESSION.lock().unwrap().take() {
        s.stop.store(true, Ordering::Relaxed);
        ACTIVE.store(false, Ordering::Relaxed);
        *FRAMES.lock().unwrap() = None;
        // Wake the HTTP thread out of accept().
        let _ = TcpStream::connect_timeout(&url_addr(&s.url), Duration::from_millis(200));
        info!("🌐 [WEB SHARE] Transmissão pela web encerrada.");
    }
}

pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Called by the stream encoder for every frame. Free while web sharing is off; while on,
/// a frame the WebRTC thread cannot take right away is dropped rather than waited for.
pub fn offer_frame(annex_b: &[u8]) {
    if !ACTIVE.load(Ordering::Relaxed) || !looks_like_h264(annex_b) {
        return;
    }
    if let Some(tx) = FRAMES.lock().unwrap().as_ref() {
        let _ = tx.try_send(Arc::from(annex_b));
    }
}

fn looks_like_h264(data: &[u8]) -> bool {
    data.starts_with(&[0, 0, 0, 1]) || data.starts_with(&[0, 0, 1])
}

struct OfferRequest {
    offer: SdpOffer,
    reply: mpsc::Sender<Result<SdpAnswer, u16>>,
}

struct Viewer {
    rtc: Rtc,
    video: Option<(Mid, Pt)>,
}

fn run_rtc(
    socket: UdpSocket,
    candidate_addr: SocketAddr,
    offers: Receiver<OfferRequest>,
    frames: Receiver<Arc<[u8]>>,
    stop: Arc<AtomicBool>,
) {
    let started = Instant::now();
    let local = socket.local_addr().ok();
    let mut viewers: Vec<Viewer> = Vec::new();
    let mut buf = vec![0u8; 2000];

    while !stop.load(Ordering::Relaxed) {
        // New browsers.
        while let Ok(req) = offers.try_recv() {
            if viewers.len() >= MAX_WEB_VIEWERS {
                let _ = req.reply.send(Err(409));
                continue;
            }
            let mut rtc = RtcConfig::new().build(Instant::now());
            let candidate_added = Candidate::host(candidate_addr, "udp")
                .ok()
                .and_then(|c| rtc.add_local_candidate(c))
                .is_some();
            let answer = if candidate_added { rtc.sdp_api().accept_offer(req.offer).ok() } else { None };
            match answer {
                Some(answer) => {
                    let _ = req.reply.send(Ok(answer));
                    viewers.push(Viewer { rtc, video: None });
                    info!("🌐 [WEB SHARE] Navegador conectando ({} de {}).", viewers.len(), MAX_WEB_VIEWERS);
                }
                None => {
                    let _ = req.reply.send(Err(400));
                }
            }
        }

        // Frames from the encoder go to every connected browser.
        while let Ok(frame) = frames.try_recv() {
            let rtp_time = MediaTime::new(started.elapsed().as_micros() as u64 * 9 / 100, Frequency::NINETY_KHZ);
            for v in viewers.iter_mut() {
                if let Some((mid, pt)) = v.video {
                    if let Some(writer) = v.rtc.writer(mid) {
                        if let Err(e) = writer.write(pt, Instant::now(), rtp_time, Arc::clone(&frame)) {
                            warn!("🌐 [WEB SHARE] Falha ao enviar quadro: {e:?}");
                        }
                    }
                }
            }
        }

        // Let every connection send what it has and report when it next needs time.
        let mut next = Instant::now() + Duration::from_millis(50);
        viewers.retain_mut(|v| loop {
            match v.rtc.poll_output() {
                Ok(Output::Transmit(t)) => {
                    let _ = socket.send_to(&t.contents, t.destination);
                }
                Ok(Output::Timeout(t)) => {
                    next = next.min(t);
                    break true;
                }
                Ok(Output::Event(e)) => match e {
                    Event::MediaAdded(m) if m.kind == MediaKind::Video => {
                        v.video = pick_h264(&mut v.rtc, m.mid).map(|pt| (m.mid, pt));
                        if v.video.is_none() {
                            warn!("🌐 [WEB SHARE] O navegador não ofereceu H.264.");
                        }
                    }
                    Event::IceConnectionStateChange(IceConnectionState::Connected) => {
                        info!("🌐 [WEB SHARE] Navegador conectado.");
                        super::request_intra_keyframe();
                    }
                    Event::KeyframeRequest(_) => super::request_intra_keyframe(),
                    Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                        info!("🌐 [WEB SHARE] Navegador desconectou.");
                        break false;
                    }
                    _ => {}
                },
                Err(e) => {
                    warn!("🌐 [WEB SHARE] Conexão encerrada: {e:?}");
                    break false;
                }
            }
        });

        // Wait for a packet, the next timeout, or (at most 10 ms) new frames.
        let wait = next.saturating_duration_since(Instant::now()).clamp(Duration::from_millis(1), Duration::from_millis(10));
        let _ = socket.set_read_timeout(Some(wait));
        match socket.recv_from(&mut buf) {
            Ok((n, source)) => {
                let Some(destination) = local else { continue };
                let Ok(contents) = buf[..n].try_into() else { continue };
                let input = Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source, destination, contents });
                if let Some(v) = viewers.iter_mut().find(|v| v.rtc.accepts(&input)) {
                    let _ = v.rtc.handle_input(input);
                }
            }
            Err(_) => {
                let now = Instant::now();
                for v in viewers.iter_mut() {
                    let _ = v.rtc.handle_input(Input::Timeout(now));
                }
            }
        }
    }
}

/// The H.264 payload type the browser accepted for this video track.
fn pick_h264(rtc: &mut Rtc, mid: Mid) -> Option<Pt> {
    let writer = rtc.writer(mid)?;
    let pt = writer
        .payload_params()
        .find(|p| p.spec().codec == Codec::H264)
        .map(|p| p.pt());
    pt
}

fn run_http(listener: TcpListener, key: String, offers: mpsc::Sender<OfferRequest>, stop: Arc<AtomicBool>) {
    for stream in listener.incoming() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let Ok(stream) = stream else { continue };
        let offers = offers.clone();
        let key = key.clone();
        let _ = std::thread::Builder::new()
            .name("web-share-request".into())
            .spawn(move || handle_http(stream, &key, &offers));
    }
}

fn handle_http(mut stream: TcpStream, key: &str, offers: &mpsc::Sender<OfferRequest>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0).min(64 * 1024);
            }
        }
    }
    let mut parts = request_line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let key_ok = query.split('&').any(|kv| kv == format!("k={key}"));

    let respond = |stream: &mut TcpStream, status: &str, kind: &str, body: &[u8]| {
        let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", body.len());
        let _ = stream.write_all(body);
    };

    match (method, path) {
        ("GET", "/") if key_ok => respond(&mut stream, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes()),
        ("POST", "/offer") if key_ok => {
            let mut body = vec![0u8; content_length];
            if reader.read_exact(&mut body).is_err() {
                return respond(&mut stream, "400 Bad Request", "text/plain", b"bad request");
            }
            let Ok(offer) = serde_json::from_slice::<SdpOffer>(&body) else {
                return respond(&mut stream, "400 Bad Request", "text/plain", b"bad offer");
            };
            let (reply_tx, reply_rx) = mpsc::channel();
            if offers.send(OfferRequest { offer, reply: reply_tx }).is_err() {
                return respond(&mut stream, "503 Service Unavailable", "text/plain", b"stopped");
            }
            match reply_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok(answer)) => {
                    let json = serde_json::to_vec(&answer).unwrap_or_default();
                    respond(&mut stream, "200 OK", "application/json", &json);
                }
                Ok(Err(409)) => respond(&mut stream, "409 Conflict", "text/plain", b"full"),
                _ => respond(&mut stream, "400 Bad Request", "text/plain", b"rejected"),
            }
        }
        _ => respond(&mut stream, "404 Not Found", "text/plain", b"not found"),
    }
}

fn install_crypto_provider() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| str0m::crypto::from_feature_flags().install_process_default());
}

fn local_ip() -> Option<IpAddr> {
    super::get_local_lan_addresses(0)
        .into_iter()
        .map(|a| a.ip())
        .find(|ip| !ip.is_loopback())
}

fn url_addr(url: &str) -> SocketAddr {
    url.trim_start_matches("http://")
        .split('/')
        .next()
        .and_then(|a| a.parse().ok())
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 0)))
}

/// 128 random bits for the link, from the OS random source used for the stream keys.
fn random_key() -> String {
    let mut bytes = [0u8; 16];
    let _ = getrandom_fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn getrandom_fill(bytes: &mut [u8]) -> Result<(), ()> {
    use rand::RngCore;
    rand::rngs::OsRng.try_fill_bytes(bytes).map_err(|_| ())
}
