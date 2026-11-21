//! Outbound camera (webcam) on the **parent** voice connection.
//!
//! Discord treats camera video as a second SSRC family of the voice call: the
//! local video SSRCs come in voice opcode 2 (Ready), are announced with opcode
//! 12 and the frames travel over the same UDP socket, transport key and DAVE
//! session as the microphone.  This is different from Go Live, which opens a
//! separate stream connection.
//!
//! Capture and encoding stay in `screen_capture::manager`; it calls
//! `offer_outbound_frame` like it does for Go Live.  This module only owns the
//! signalling (op 12 / op 5) and the RTP pump.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use davey::DaveSession;
use futures_util::SinkExt;
use serde_json::{json, Value};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use tokio_tungstenite::tungstenite::Message;

use super::crypto::TransportMode;
use super::stream::VoiceSink;

/// Everything the camera pump needs from the live parent voice connection.
pub struct CameraContext {
    pub write: Arc<AsyncMutex<VoiceSink>>,
    pub socket: Arc<UdpSocket>,
    pub target: SocketAddr,
    pub key: Arc<Mutex<Option<Vec<u8>>>>,
    pub mode: Arc<Mutex<String>>,
    pub dave: Arc<Mutex<Option<DaveSession>>>,
    pub audio_ssrc: u32,
    pub video_ssrc: u32,
    pub rtx_ssrc: u32,
    /// Shared with the microphone sender: one nonce sequence for the whole
    /// connection, otherwise audio and video would reuse counters.
    pub nonce: Arc<AtomicU32>,
}

enum CameraCommand {
    Start,
    Stop,
}

/// Whether the user wants the camera on.  It survives a voice reconnect so the
/// camera comes back with the call; leaving the call clears it.
static WANTED: AtomicBool = AtomicBool::new(false);

fn control() -> &'static Mutex<Option<mpsc::UnboundedSender<CameraCommand>>> {
    static CONTROL: OnceLock<Mutex<Option<mpsc::UnboundedSender<CameraCommand>>>> = OnceLock::new();
    CONTROL.get_or_init(|| Mutex::new(None))
}

/// Cancels the control loop of the connection that is being replaced.  Without
/// it, a `start` already inside an `await` could finish after `uninstall` and
/// leave a sender pumping frames into a dead socket.
fn alive_token() -> &'static Mutex<Option<Arc<AtomicBool>>> {
    static ALIVE: OnceLock<Mutex<Option<Arc<AtomicBool>>>> = OnceLock::new();
    ALIVE.get_or_init(|| Mutex::new(None))
}

/// Serializes "the connection ended" with "start/stop the pump", so an old
/// control loop cannot start or stop the sender of the connection that
/// replaced it.
fn lifecycle() -> &'static Mutex<()> {
    static LIFECYCLE: OnceLock<Mutex<()>> = OnceLock::new();
    LIFECYCLE.get_or_init(|| Mutex::new(()))
}

/// Registers the parent connection that is currently ready to carry camera
/// video.  Called after voice opcode 4 (the transport key exists by then).
pub fn install(context: CameraContext) {
    uninstall();
    log::info!(
        "📷 [CAMERA] Conexão de voz pronta para câmera (audio_ssrc={}, video_ssrc={}, rtx_ssrc={})",
        context.audio_ssrc,
        context.video_ssrc,
        context.rtx_ssrc
    );
    let (tx, rx) = mpsc::unbounded_channel();
    let alive = Arc::new(AtomicBool::new(true));
    if let Ok(mut guard) = control().lock() {
        *guard = Some(tx);
    }
    if let Ok(mut guard) = alive_token().lock() {
        *guard = Some(Arc::clone(&alive));
    }
    tokio::spawn(control_loop(context, rx, alive));
    if WANTED.load(Ordering::SeqCst) {
        send(CameraCommand::Start);
    }
}

/// Forgets the connection (voice reconnect or call ended).  Keeps `WANTED` so a
/// reconnect restores the camera on its own.
pub fn uninstall() {
    let _lifecycle = lifecycle().lock();
    if let Ok(mut guard) = alive_token().lock() {
        if let Some(alive) = guard.take() {
            alive.store(false, Ordering::SeqCst);
        }
    }
    if let Ok(mut guard) = control().lock() {
        *guard = None;
    }
    super::sender::stop_sender("");
}

/// The user turned the camera on.
pub fn start_camera() {
    WANTED.store(true, Ordering::SeqCst);
    let sent = control()
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|tx| tx.send(CameraCommand::Start).is_ok()))
        .unwrap_or(false);
    if !sent {
        log::info!("📷 [CAMERA] Câmera marcada; aguardando a conexão de voz ficar pronta.");
    }
}

/// The user turned the camera off.
pub fn stop_camera() {
    WANTED.store(false, Ordering::SeqCst);
    send(CameraCommand::Stop);
}

/// Leaves the call: do not restore the camera in the next one.
pub fn reset() {
    WANTED.store(false, Ordering::SeqCst);
    uninstall();
    crate::gateway::set_self_video(false);
}

pub fn is_camera_wanted() -> bool {
    WANTED.load(Ordering::SeqCst)
}

fn send(command: CameraCommand) {
    if let Ok(guard) = control().lock() {
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(command);
        }
    }
}

async fn control_loop(
    context: CameraContext,
    mut rx: mpsc::UnboundedReceiver<CameraCommand>,
    alive: Arc<AtomicBool>,
) {
    while let Some(command) = rx.recv().await {
        match command {
            CameraCommand::Start => start(&context, &alive).await,
            CameraCommand::Stop => stop(&context, &alive).await,
        }
    }
}

async fn start(context: &CameraContext, alive: &AtomicBool) {
    if !alive.load(Ordering::SeqCst) {
        return;
    }
    if context.video_ssrc == 0 || context.audio_ssrc == 0 {
        log::warn!(
            "📷 [CAMERA] O servidor de voz não reservou SSRCs de vídeo (audio={}, video={}); câmera não iniciada.",
            context.audio_ssrc,
            context.video_ssrc
        );
        return;
    }
    let key = context.key.lock().ok().and_then(|guard| guard.clone());
    let Some(key) = key else {
        log::warn!("📷 [CAMERA] Sem chave de transporte; câmera não iniciada.");
        return;
    };
    let mode = context
        .mode
        .lock()
        .map(|guard| TransportMode::from_name(&guard))
        .unwrap_or(TransportMode::Aes256Gcm);

    // Discord's main gateway flag for the camera tile.  Send it before the
    // voice op 12 so viewers start asking for the track as soon as it exists.
    crate::gateway::set_self_video(true);

    // Announce the video SSRC before the first RTP packet.  Do not send an
    // opcode 5 for it: speaking binds an SSRC as audio, and the microphone
    // already sent its own opcode 5 on this connection.
    send_ws(context, json!({
        "op": 12,
        "d": {
            "audio_ssrc": context.audio_ssrc,
            "video_ssrc": context.video_ssrc,
            "rtx_ssrc": context.rtx_ssrc,
            "streams": [{
                "type": "video",
                "rid": "100",
                "ssrc": context.video_ssrc,
                "active": true,
                "quality": 100,
                "rtx_ssrc": context.rtx_ssrc,
                "max_bitrate": 2_500_000,
                "max_framerate": 30,
                "max_resolution": { "type": "fixed", "width": 1280, "height": 720 }
            }]
        }
    })).await;
    // The WebSocket and the UDP socket are different transports: give the voice
    // server a moment to register the video state before the first RTP packet,
    // otherwise it can answer with Invalid SSRC while a viewer connects.
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    // The voice connection may have gone away while we waited.
    let _lifecycle = lifecycle().lock();
    if !alive.load(Ordering::SeqCst) {
        crate::gateway::set_self_video(false);
        return;
    }

    log::info!(
        "📷 [CAMERA] Transmitindo webcam na conexão de voz (audio_ssrc={}, video_ssrc={}, rtx={}, mode={:?})",
        context.audio_ssrc,
        context.video_ssrc,
        context.rtx_ssrc,
        mode
    );

    let stream_key = format!("camera:{}", context.audio_ssrc);
    super::sender::start_sender(
        Arc::clone(&context.socket),
        context.target,
        key,
        mode,
        context.audio_ssrc,
        context.video_ssrc,
        stream_key,
        Arc::clone(&context.dave),
        Arc::clone(&context.nonce),
        // The voice receive loop owns this socket and forwards RTCP PLI/FIR.
        false,
        // The microphone already sends Opus on this connection.
        false,
    );
}

async fn stop(context: &CameraContext, alive: &AtomicBool) {
    {
        let _lifecycle = lifecycle().lock();
        if !alive.load(Ordering::SeqCst) {
            return;
        }
        super::sender::stop_sender("");
        crate::gateway::set_self_video(false);
    }
    send_ws(context, json!({
        "op": 12,
        "d": {
            "audio_ssrc": context.audio_ssrc,
            "video_ssrc": 0,
            "rtx_ssrc": 0,
            "streams": []
        }
    })).await;
    log::info!("📷 [CAMERA] Webcam encerrada.");
}

async fn send_ws(context: &CameraContext, value: Value) {
    let mut sink = context.write.lock().await;
    if let Err(error) = sink
        .send(Message::Text(value.to_string().into()))
        .await
    {
        log::warn!("📷 [CAMERA] Falha ao enviar sinalização: {:?}", error);
    }
}
