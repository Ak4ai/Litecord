//! Outbound RTP video transmission for Discord Go Live and camera.
//!
//! Encodes/packetizes raw Annex-B H.264 video frames into RFC 6184 RTP packets,
//! encrypts them via the negotiated transport key (AES-256-GCM / XChaCha20),
//! and sends them to Discord's stream SFU UDP endpoint.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use log::{info, warn};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

use super::crypto::{protect_rtcp, protect_rtp, TransportMode};
use super::h264::H264Packetizer;
use super::rtp::is_rtcp;

static FRAME_TX: Mutex<Option<mpsc::Sender<Vec<u8>>>> = Mutex::new(None);
static IS_BROADCASTING: AtomicBool = AtomicBool::new(false);
static ACTIVE_STREAM_KEY: Mutex<Option<String>> = Mutex::new(None);

/// Returns true if an outbound Discord stream is actively broadcasting.
pub fn is_broadcasting() -> bool {
    IS_BROADCASTING.load(Ordering::Relaxed)
}

/// Hands an Annex-B encoded H.264 frame to the outbound transmitter.
/// Non-blocking: drops frames if the queue is full to avoid adding latency.
pub fn offer_outbound_frame(annex_b: &[u8]) {
    if !is_broadcasting() || annex_b.is_empty() {
        return;
    }
    let guard = FRAME_TX.lock().unwrap();
    if let Some(tx) = guard.as_ref() {
        let _ = tx.try_send(annex_b.to_vec());
    }
}

/// Stops any active outbound stream sender.
pub fn stop_sender(stream_key: &str) {
    let mut key_guard = ACTIVE_STREAM_KEY.lock().unwrap();
    if key_guard.as_deref() == Some(stream_key) || stream_key.is_empty() {
        *key_guard = None;
        IS_BROADCASTING.store(false, Ordering::SeqCst);
        let mut tx_guard = FRAME_TX.lock().unwrap();
        *tx_guard = None;
        info!("🛑 [GO LIVE TX] Transmissor nativo de stream encerrado: {}", stream_key);
    }
}

/// Starts outbound native RTP H.264 transmission over the established stream UDP socket.
pub fn start_sender(
    socket: Arc<UdpSocket>,
    target_addr: SocketAddr,
    key: Vec<u8>,
    mode: TransportMode,
    video_ssrc: u32,
    audio_ssrc: u32,
    stream_key: String,
    dave: Option<Arc<Mutex<Option<davey::DaveSession>>>>,
) {
    stop_sender("");

    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(5);
    *FRAME_TX.lock().unwrap() = Some(tx);
    *ACTIVE_STREAM_KEY.lock().unwrap() = Some(stream_key.clone());
    IS_BROADCASTING.store(true, Ordering::SeqCst);

    // Initial IDR keyframe to allow immediate playback by viewers
    crate::screen_capture::types::request_intra_keyframe();

    let stream_start = Instant::now();
    let nonce_counter = Arc::new(AtomicU32::new(1));
    let total_packets = Arc::new(AtomicU32::new(0));
    let total_octets = Arc::new(AtomicU32::new(0));

    let socket_send = Arc::clone(&socket);
    let socket_send_rtcp = Arc::clone(&socket);
    let socket_recv = Arc::clone(&socket);
    let stream_key_clone = stream_key.clone();

    let total_packets_rtp = Arc::clone(&total_packets);
    let total_octets_rtp = Arc::clone(&total_octets);
    let total_packets_rtcp = Arc::clone(&total_packets);
    let total_octets_rtcp = Arc::clone(&total_octets);
    let nonce_counter_rtcp = Arc::clone(&nonce_counter);
    let key_rtcp = key.clone();
    let key_rr = key.clone();

    // 1. Outbound Video RTP Worker Task
    tokio::spawn(async move {
        info!(
            "🚀 [GO LIVE TX] Pumping de vídeo nativo H.264 iniciado para {} (SSRC={}, mode={:?})",
            target_addr, video_ssrc, mode
        );
        crate::gateway::set_stream_paused(false);
        // Five Opus silence frames on the stream's audio SSRC, as official
        // clients send when a stream has no sound, so the server sees the
        // connection's audio track as started.
        {
            let mut seq: u16 = 1;
            for i in 0..5u32 {
                let mut header = [0u8; 12];
                header[0] = 0x80;
                header[1] = 111;
                header[2..4].copy_from_slice(&seq.to_be_bytes());
                header[4..8].copy_from_slice(&(i * 960).to_be_bytes());
                header[8..12].copy_from_slice(&audio_ssrc.to_be_bytes());
                seq = seq.wrapping_add(1);
                let cnt = nonce_counter.fetch_add(1, Ordering::Relaxed);
                if let Some(pkt) = protect_rtp(&key, mode, &header, &[0xF8, 0xFF, 0xFE], cnt) {
                    let _ = socket_send.send_to(&pkt, target_addr).await;
                }
            }
        }
        // Debug knobs, read once per stream: LITECORD_GL_MTU, LITECORD_GL_PT,
        // LITECORD_GL_NO_DAVE=1 (send frames without E2EE).
        let env_num = |name: &str, default: usize| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
        let mtu = env_num("LITECORD_GL_MTU", 900);
        let pt = env_num("LITECORD_GL_PT", 102) as u8;
        let no_dave = std::env::var("LITECORD_GL_NO_DAVE").map(|v| v == "1").unwrap_or(false);
        let dave = if no_dave { None } else { dave };
        info!("🧪 [GO LIVE TX] mtu={} pt={} dave={}", mtu, pt, dave.is_some());
        let mut packetizer = H264Packetizer::new(video_ssrc, pt).with_mtu(mtu);
        let mut frame_count = 0u64;

        while let Some(frame) = rx.recv().await {
            if !IS_BROADCASTING.load(Ordering::Relaxed) {
                break;
            }
            // With DAVE the whole frame is end-to-end encrypted before RTP;
            // until the group is ready nothing may be sent in the clear.
            let frame = match dave.as_ref() {
                None => frame,
                Some(dave) => {
                    let mut guard = dave.lock().unwrap_or_else(|e| e.into_inner());
                    match guard.as_mut().filter(|s| s.is_ready()) {
                        Some(session) => match session.encrypt(davey::MediaType::VIDEO, davey::Codec::H264, &frame) {
                            Ok(encrypted) => encrypted.into_owned(),
                            Err(e) => {
                                warn!("⚠️ [GO LIVE TX] DAVE encrypt falhou: {:?}", e);
                                continue;
                            }
                        },
                        None => continue,
                    }
                }
            };
            let pts_ms = stream_start.elapsed().as_millis() as u64;
            let packets = packetizer.packetize(&frame, pts_ms);

            for (header, payload) in packets {
                let cnt = nonce_counter.fetch_add(1, Ordering::Relaxed);
                total_packets_rtp.fetch_add(1, Ordering::Relaxed);
                total_octets_rtp.fetch_add(payload.len() as u32, Ordering::Relaxed);
                // Like the official client: X bit + one-byte header extension
                // (RFC 8285) carrying playout-delay (id 5) = 0.  In rtpsize mode
                // the 4-byte extension header stays clear, its body is encrypted.
                let mut clear = [0u8; 16];
                clear[..12].copy_from_slice(&header);
                clear[0] |= 0x10;
                clear[12..16].copy_from_slice(&[0xBE, 0xDE, 0x00, 0x01]);
                let mut body = Vec::with_capacity(4 + payload.len());
                body.extend_from_slice(&[0x52, 0x00, 0x00, 0x00]);
                body.extend_from_slice(&payload);
                // The server accepts our plain 12-byte audio packets (its reports
                // count them) but never counted a video packet carrying the
                // extension, so video goes out in the same plain layout.
                let _ = (&clear, &body);
                if let Some(encrypted) = protect_rtp(&key, mode, &header, &payload, cnt) {
                    if let Err(e) = socket_send.send_to(&encrypted, target_addr).await {
                        warn!("⚠️ [GO LIVE TX] Falha ao enviar pacote RTP: {:?}", e);
                        break;
                    }
                }
            }

            frame_count += 1;
            if frame_count == 1 {
                let nal_types: Vec<u8> = frame.windows(4).filter(|w| w[..3] == [0, 0, 1]).map(|w| w[3] & 0x1F).take(8).collect();
                info!("🎬 [GO LIVE TX] primeiro frame enviado: {} bytes, NALs={:?}, dave={}", frame.len(), nal_types, dave.is_some());
            }
            if frame_count % 300 == 0 {
                info!(
                    "🎬 [GO LIVE TX] {} frames H.264 transmitidos com sucesso para Discord!",
                    frame_count
                );
            }
        }
        info!("🛑 [GO LIVE TX] Worker de envio de vídeo finalizado.");
    });

    // 2. Periodic RTCP Sender Report Task (RFC 3550 / Discord lip-sync & clock sync keepalive)
    tokio::spawn(async move {
        while IS_BROADCASTING.load(Ordering::Relaxed) {
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            if !IS_BROADCASTING.load(Ordering::Relaxed) {
                break;
            }
            let now = std::time::SystemTime::now();
            let unix_dur = now.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            let ntp_secs = (unix_dur.as_secs() + 2_208_988_800) as u32;
            let ntp_frac = ((unix_dur.subsec_nanos() as u64 * (1u64 << 32)) / 1_000_000_000) as u32;
            let pts_ms = stream_start.elapsed().as_millis() as u64;
            let rtp_ts = (pts_ms * 90) as u32;
            let pkt_cnt = total_packets_rtcp.load(Ordering::Relaxed);
            let oct_cnt = total_octets_rtcp.load(Ordering::Relaxed);

            let mut sr = [0u8; 28];
            sr[0] = 0x80; // V=2, P=0, RC=0
            sr[1] = 200;  // PT=200 (Sender Report)
            sr[2..4].copy_from_slice(&6u16.to_be_bytes()); // length in 32-bit words - 1 = 6
            sr[4..8].copy_from_slice(&video_ssrc.to_be_bytes());
            sr[8..12].copy_from_slice(&ntp_secs.to_be_bytes());
            sr[12..16].copy_from_slice(&ntp_frac.to_be_bytes());
            sr[16..20].copy_from_slice(&rtp_ts.to_be_bytes());
            sr[20..24].copy_from_slice(&pkt_cnt.to_be_bytes());
            sr[24..28].copy_from_slice(&oct_cnt.to_be_bytes());

            let cnt = nonce_counter_rtcp.fetch_add(1, Ordering::Relaxed);
            if let Some(encrypted_sr) = protect_rtcp(&key_rtcp, mode, &sr, cnt) {
                let _ = socket_send_rtcp.send_to(&encrypted_sr, target_addr).await;
            }
        }
    });

    // 2. Inbound RTCP Receiver Task (Handling PLI/FIR requests from Discord)
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        while IS_BROADCASTING.load(Ordering::Relaxed) {
            match socket_recv.recv_from(&mut buf).await {
                Ok((len, _from)) => {
                    if is_rtcp(&buf[..len]) {
                        let pt = buf.get(1).copied().unwrap_or(0);
                        let detail = if pt == 201 { describe_rr(&key_rr, &buf[..len]) } else { String::new() };
                        info!("📨 [GO LIVE TX] RTCP do Discord: pt={} {} bytes {}", pt, len, detail);
                        // Discord sends RTCP Feedback (PLI=1 / FIR=4) when viewers connect
                        crate::screen_capture::types::request_intra_keyframe();
                    }
                }
                Err(_) => {
                    if !IS_BROADCASTING.load(Ordering::Relaxed) {
                        break;
                    }
                }
            }
        }
    });

    info!("✅ [GO LIVE TX] Transmissão nativa ativa key={}", stream_key_clone);
}

/// Decrypts a receiver report (aead_aes256_gcm_rtpsize) and shows what the
/// server says it got from us: source SSRC, lost packets and highest sequence.
fn describe_rr(key: &[u8], packet: &[u8]) -> String {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use aes_gcm::{Aes256Gcm, Nonce};
    if packet.len() < 8 + 16 + 4 {
        return String::new();
    }
    let (aad, rest) = packet.split_at(8);
    let (ct, suffix) = rest.split_at(rest.len() - 4);
    let mut nonce = [0u8; 12];
    nonce[..4].copy_from_slice(suffix);
    let Ok(cipher) = Aes256Gcm::new_from_slice(key) else { return String::new() };
    match cipher.decrypt(Nonce::from_slice(&nonce), Payload { msg: ct, aad }) {
        Ok(plain) if plain.len() >= 24 => {
            let blocks = (packet[0] & 0x1F) as usize;
            let mut out = format!("RR blocos={} plain={}", blocks, plain.len());
            for b in plain.chunks_exact(24).take(blocks.max(1)) {
                let ssrc = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
                let lost = u32::from_be_bytes([0, b[5], b[6], b[7]]);
                let seq = u32::from_be_bytes([b[8], b[9], b[10], b[11]]);
                out.push_str(&format!(" | ssrc={} perdidos={} maior_seq={}", ssrc, lost, seq));
            }
            out
        }
        Ok(plain) => format!("RR sem blocos ({} bytes)", plain.len()),
        Err(_) => "RR não decifrou".to_string(),
    }
}
