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
    stream_key: String,
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
    let socket_send = Arc::clone(&socket);
    let socket_recv = Arc::clone(&socket);
    let stream_key_clone = stream_key.clone();

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
            let pts_ms = stream_start.elapsed().as_millis() as u64;
            let packets = packetizer.packetize(&frame, pts_ms);

            for (header, payload) in packets {
                let cnt = nonce_counter.fetch_add(1, Ordering::Relaxed);
                if let Some(encrypted) = protect_rtp(&key, mode, &header, &payload, cnt) {
                    if let Err(e) = socket_send.send_to(&encrypted, target_addr).await {
                        warn!("⚠️ [GO LIVE TX] Falha ao enviar pacote RTP: {:?}", e);
                        break;
                    }
                }
            }

            frame_count += 1;
            if frame_count % 300 == 0 {
                info!(
                    "🎬 [GO LIVE TX] {} frames H.264 transmitidos com sucesso para Discord!",
                    frame_count
                );
            }
        }
        info!("🛑 [GO LIVE TX] Worker de envio de vídeo finalizado.");
    });

    // 2. Inbound RTCP Receiver Task (Handling PLI/FIR requests from Discord)
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        while IS_BROADCASTING.load(Ordering::Relaxed) {
            match socket_recv.recv_from(&mut buf).await {
                Ok((len, _from)) => {
                    if is_rtcp(&buf[..len]) {
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
