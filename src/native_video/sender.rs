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
use davey::{Codec, DaveSession, MediaType};

use super::crypto::{protect_rtcp, protect_rtp, TransportMode};
use super::h264::H264Packetizer;
use super::rtp::is_rtcp;

static FRAME_TX: Mutex<Option<mpsc::Sender<Vec<u8>>>> = Mutex::new(None);
static AUDIO_TX: Mutex<Option<mpsc::Sender<Vec<u8>>>> = Mutex::new(None);
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

/// Hands a 20ms Opus audio frame to the outbound transmitter.
pub fn offer_outbound_audio(opus_frame: &[u8]) {
    if !is_broadcasting() || opus_frame.is_empty() {
        return;
    }
    let guard = AUDIO_TX.lock().unwrap();
    if let Some(tx) = guard.as_ref() {
        let _ = tx.try_send(opus_frame.to_vec());
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
        let mut atx_guard = AUDIO_TX.lock().unwrap();
        *atx_guard = None;
        info!("🛑 [GO LIVE TX] Transmissor nativo de stream encerrado: {}", stream_key);
    }
}

/// Starts outbound native RTP H.264 transmission over an established UDP socket
/// (the dedicated Go Live stream socket or the parent voice socket for camera).
#[allow(clippy::too_many_arguments)]
pub fn start_sender(
    socket: Arc<UdpSocket>,
    target_addr: SocketAddr,
    key: Vec<u8>,
    mode: TransportMode,
    audio_ssrc: u32,
    video_ssrc: u32,
    stream_key: String,
    dave: Arc<Mutex<Option<DaveSession>>>,
    // The parent voice connection already has an audio sender using this key:
    // video packets must take their own values from the same counter, or two
    // packets would be encrypted with the same nonce.
    nonce_counter: Arc<AtomicU32>,
    // A Go Live connection owns its UDP socket, so it can read RTCP feedback
    // here.  The parent camera shares the voice socket with the audio receive
    // loop, which handles RTCP instead.
    listen_for_rtcp: bool,
    // Go Live carries system/application audio as soundshare.  The camera does
    // not: the microphone already sends Opus on this same SSRC.
    send_audio: bool,
) {
    stop_sender("");

    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(16);
    *FRAME_TX.lock().unwrap() = Some(tx);
    *ACTIVE_STREAM_KEY.lock().unwrap() = Some(stream_key.clone());
    IS_BROADCASTING.store(true, Ordering::SeqCst);

    // Initial IDR keyframe to allow immediate playback by viewers
    crate::screen_capture::types::request_intra_keyframe();

    let stream_start = Instant::now();
    let total_packets = Arc::new(AtomicU32::new(0));
    let total_octets = Arc::new(AtomicU32::new(0));

    // 1b. Outbound Audio RTP Worker Task (Opus 48 kHz stereo for Go Live soundshare)
    if send_audio {
        let (atx, mut arx) = mpsc::channel::<Vec<u8>>(32);
        *AUDIO_TX.lock().unwrap() = Some(atx);
        let socket_send_audio = Arc::clone(&socket);
        let dave_audio = Arc::clone(&dave);
        let key_audio = key.clone();
        let nonce_counter_audio = Arc::clone(&nonce_counter);
        tokio::spawn(async move {
            info!(
                "🔊 [GO LIVE TX] Pumping de áudio nativo Opus iniciado para {} (SSRC={}, mode={:?})",
                target_addr, audio_ssrc, mode
            );
            let mut audio_seq: u16 = 0;
            let mut audio_ts: u32 = 0;

            while let Some(opus_frame) = arx.recv().await {
                if !IS_BROADCASTING.load(Ordering::Relaxed) {
                    break;
                }

                // Apply DAVE end-to-end audio encryption if active
                let encrypted_payload = if let Ok(mut dg) = dave_audio.lock() {
                    if let Some(ref mut d) = *dg {
                        if d.is_ready() {
                            d.encrypt(MediaType::AUDIO, Codec::OPUS, &opus_frame)
                                .ok()
                                .map(|cow| cow.into_owned())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };
                let payload_to_send = encrypted_payload.as_deref().unwrap_or(&opus_frame);

                // Construct 12-byte RTP header (PT=120 Discord Opus)
                let mut rtp_header = [0u8; 12];
                rtp_header[0] = 0x80; // V=2, P=0, X=0, CC=0
                rtp_header[1] = 120;  // Payload Type 120 (Opus)
                rtp_header[2..4].copy_from_slice(&audio_seq.to_be_bytes());
                rtp_header[4..8].copy_from_slice(&audio_ts.to_be_bytes());
                rtp_header[8..12].copy_from_slice(&audio_ssrc.to_be_bytes());

                audio_seq = audio_seq.wrapping_add(1);
                audio_ts = audio_ts.wrapping_add(960); // 20ms @ 48kHz = 960 samples

                let cnt = nonce_counter_audio.fetch_add(1, Ordering::Relaxed);
                if let Some(encrypted) = protect_rtp(&key_audio, mode, &rtp_header, payload_to_send, cnt) {
                    let _ = socket_send_audio.send_to(&encrypted, target_addr).await;
                }
            }
            info!("🛑 [GO LIVE TX] Worker de envio de áudio finalizado.");
        });
    }
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

    // 1. Outbound Video RTP Worker Task
    tokio::spawn(async move {
        info!(
            "🚀 [GO LIVE TX] Pumping de vídeo nativo H.264 iniciado para {} (SSRC={}, mode={:?})",
            target_addr, video_ssrc, mode
        );
        crate::gateway::set_stream_paused(false);
        let mut packetizer = H264Packetizer::new(video_ssrc, 103);
        let mut frame_count = 0u64;

        while let Some(frame) = rx.recv().await {
            if !IS_BROADCASTING.load(Ordering::Relaxed) {
                break;
            }

            // Apply DAVE end-to-end frame encryption if active and ready
            let dave_encrypted = if let Ok(mut dg) = dave.lock() {
                if let Some(ref mut d) = *dg {
                    if d.is_ready() {
                        match d.encrypt(MediaType::VIDEO, Codec::H264, &frame) {
                            Ok(cow) => Some(cow.into_owned()),
                            Err(error) => {
                                if frame_count % 120 == 0 {
                                    warn!("⚠️ [GO LIVE TX] DAVE não conseguiu cifrar o vídeo: {:?}", error);
                                }
                                None
                            }
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };
            let frame_to_packetize = dave_encrypted.as_deref().unwrap_or(&frame);

            let pts_ms = stream_start.elapsed().as_millis() as u64;
            let packets = packetizer.packetize(frame_to_packetize, pts_ms);

            for (header, payload) in packets {
                let cnt = nonce_counter.fetch_add(1, Ordering::Relaxed);
                total_packets_rtp.fetch_add(1, Ordering::Relaxed);
                total_octets_rtp.fetch_add(payload.len() as u32, Ordering::Relaxed);
                if let Some(encrypted) = protect_rtp(&key, mode, &header, &payload, cnt) {
                    if let Err(e) = socket_send.send_to(&encrypted, target_addr).await {
                        warn!("⚠️ [GO LIVE TX] Falha ao enviar pacote RTP: {:?}", e);
                        break;
                    }
                }
            }

            frame_count += 1;
            if frame_count == 1 {
                info!(
                    "🎬 [GO LIVE TX] Primeiro frame H.264 enviado ({} bytes, SSRC={})",
                    frame.len(),
                    video_ssrc
                );
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
    if listen_for_rtcp {
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            while IS_BROADCASTING.load(Ordering::Relaxed) {
                match socket_recv.recv_from(&mut buf).await {
                    Ok((len, _from)) => {
                        if is_rtcp(&buf[..len]) && crate::native_video::rtcp::wants_keyframe(&buf[..len]) {
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
    }

    info!("✅ [GO LIVE TX] Transmissão nativa ativa key={}", stream_key_clone);
}
