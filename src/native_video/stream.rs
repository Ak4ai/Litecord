//! Go Live transmit connection.
//!
//! Opcode 18 on the main gateway makes Discord answer with STREAM_CREATE and
//! STREAM_SERVER_UPDATE; this module then opens the separate voice WebSocket/UDP
//! pair of the stream and hands the socket to `sender`.  Nothing here runs
//! until the user starts a share with "Go Live" switched on.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::num::NonZeroU16;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use davey::{DaveSession, ProposalsOperationType};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::UdpSocket;
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::Message;

use super::receiver::{NativeVideoContext, NativeVideoSession};
use super::{handle_video_state, media_sink_wants};

type VoiceSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

fn generations() -> &'static Mutex<HashMap<String, u64>> {
    static GENERATIONS: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    GENERATIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

fn begin_generation(key: &str) -> u64 {
    let generation = NEXT_GENERATION.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut map) = generations().lock() {
        map.insert(key.to_string(), generation);
    }
    generation
}

fn is_current(key: &str, generation: u64) -> bool {
    generations()
        .lock()
        .ok()
        .and_then(|map| map.get(key).copied())
        == Some(generation)
}

/// Forgets this viewer when its connection ends normally (only if a newer
/// viewer for the same key has not replaced it).
fn end_generation(stream_key: &str, generation: u64) {
    if let Ok(mut map) = generations().lock() {
        if map.get(stream_key).copied() == Some(generation) {
            map.remove(stream_key);
        }
    }
}


/// Cancels the outbound stream connection (stopped sharing, call ended).
pub fn stop_stream_sender(stream_key: &str) {
    NEXT_GENERATION.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut map) = generations().lock() {
        if stream_key.is_empty() {
            map.clear();
        } else {
            map.remove(stream_key);
        }
    }
    super::sender::stop_sender(stream_key);
}

fn normalize_url(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    // Same URL shape as the official client: bare path, no version query. The
    // server chooses the protocol and announces it in op 24 / op 4.
    let base = if trimmed.starts_with("wss://") || trimmed.starts_with("ws://") {
        trimmed.to_string()
    } else {
        format!("wss://{trimmed}")
    };
    if base.contains('?') { base } else { format!("{base}/?v=8") }
}


async fn send_text(write: &Arc<AsyncMutex<VoiceSink>>, value: Value) -> Result<(), String> {
    let mut sink = write.lock().await;
    sink.send(Message::Text(value.to_string().into()))
        .await
        .map_err(|e| format!("send: {e}"))
}

async fn discover_public_address(
    socket: &UdpSocket,
    ssrc: u32,
    ip: &str,
    port: u16,
) -> Option<(String, u16)> {
    let target = format!("{ip}:{port}");
    let mut discovery = [0u8; 74];
    discovery[0..2].copy_from_slice(&1u16.to_be_bytes());
    discovery[2..4].copy_from_slice(&70u16.to_be_bytes());
    discovery[4..8].copy_from_slice(&ssrc.to_be_bytes());

    for _ in 0..5 {
        if socket.send_to(&discovery, &target).await.is_err() {
            continue;
        }
        let mut buffer = [0u8; 128];
        if let Ok(Ok((len, _))) =
            tokio::time::timeout(Duration::from_millis(600), socket.recv_from(&mut buffer)).await
        {
            if len >= 70 {
                let address = &buffer[8..len.saturating_sub(2)];
                let end = address.iter().position(|b| *b == 0).unwrap_or(address.len());
                let parsed_ip = String::from_utf8_lossy(&address[..end]).trim().to_string();
                let parsed_port = u16::from_be_bytes([buffer[len - 2], buffer[len - 1]]);
                if !parsed_ip.is_empty() && parsed_port > 0 {
                    return Some((parsed_ip, parsed_port));
                }
            }
        }
    }
    None
}

async fn handle_dave_binary(
    data: &[u8],
    dave: &Arc<Mutex<Option<DaveSession>>>,
    external_sender: &Arc<Mutex<Option<Vec<u8>>>>,
    user_id: u64,
    group_id: u64,
    write: &Arc<AsyncMutex<VoiceSink>>,
) -> Result<(), String> {
    let op = data[0];
    let payload = if data.len() > 1 { &data[1..] } else { &[][..] };
    match op {
        25 => {
            *external_sender.lock().unwrap() = Some(payload.to_vec());
            let key_package = {
                let mut guard = dave.lock().unwrap();
                if guard.is_none() {
                    if let Ok(session) = DaveSession::new(
                        NonZeroU16::new(1).unwrap(),
                        user_id,
                        group_id.max(1),
                        None,
                    ) {
                        *guard = Some(session);
                    }
                }
                guard.as_mut().and_then(|session| {
                    let _ = session.set_external_sender(payload);
                    session.create_key_package().ok()
                })
            };
            if let Some(key_package) = key_package {
                let mut packet = vec![26u8];
                packet.extend_from_slice(&key_package);
                let mut sink = write.lock().await;
                sink.send(Message::Binary(packet.into()))
                    .await
                    .map_err(|e| format!("send key package: {e}"))?;
            }
        }
        27 => {
            let operation = if data.get(1).copied().unwrap_or(0) == 0 {
                ProposalsOperationType::APPEND
            } else {
                ProposalsOperationType::REVOKE
            };
            let proposals = if data.len() > 2 { &data[2..] } else { &[][..] };
            let commit = {
                let mut guard = dave.lock().unwrap();
                guard.as_mut().and_then(|session| {
                    session.process_proposals(operation, proposals, None).ok().flatten()
                })
            };
            if let Some(commit) = commit {
                let mut packet = vec![28u8];
                packet.extend_from_slice(&commit.commit);
                if let Some(welcome) = commit.welcome {
                    packet.push(1);
                    packet.extend_from_slice(&welcome);
                } else {
                    packet.push(0);
                }
                let mut sink = write.lock().await;
                sink.send(Message::Binary(packet.into()))
                    .await
                    .map_err(|e| format!("send commit: {e}"))?;
            }
        }
        29 | 30 => {
            let transition = if data.len() >= 3 {
                u16::from_be_bytes([data[1], data[2]]) as u64
            } else {
                0
            };
            let body = if data.len() > 3 { &data[3..] } else { &[][..] };
            let ready = {
                let mut guard = dave.lock().unwrap();
                if let Some(session) = guard.as_mut() {
                    if op == 29 {
                        session.process_commit(body).is_ok()
                    } else {
                        session.process_welcome(body).is_ok()
                    }
                } else {
                    false
                }
            };
            if ready {
                send_text(
                    write,
                    json!({ "op": 23, "d": { "transition_id": transition } }),
                )
                .await?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn stop_stream_viewer(stream_key: &str) {
    NEXT_GENERATION.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut map) = generations().lock() {
        map.remove(stream_key);
    }
}


/// Cancels every Go Live viewer and sender (left the voice call).
pub fn stop_all_stream_viewers() {
    NEXT_GENERATION.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut map) = generations().lock() {
        map.clear();
    }
    super::sender::stop_sender("");
}

/// Whether a Go Live viewer is currently connected for `uid`.
///
/// Used to avoid the parent camera session clearing the streaming state of a
/// user whose video actually arrives through a separate stream connection.
pub fn has_viewer_for(uid: u64) -> bool {
    generations()
        .lock()
        .map(|map| {
            map.keys().any(|key| {
                key.rsplit(':')
                    .next()
                    .and_then(|s| s.parse::<u64>().ok())
                    == Some(uid)
            })
        })
        .unwrap_or(false)
}

#[allow(clippy::too_many_arguments)]
pub async fn connect_stream_viewer(
    endpoint: String,
    token: String,
    server_id: String,
    channel_id: String,
    session_id: String,
    user_id: u64,
    owner_id: u64,
    stream_key: String,
) {
    let generation = begin_generation(&stream_key);
    let url = normalize_url(&endpoint);

    for attempt in 0..3u32 {
        if !is_current(&stream_key, generation) {
            return;
        }
        log::info!(
            "🎬 [GO LIVE] Conectando ao servidor de stream {} (tentativa {}) key={}",
            url,
            attempt + 1,
            stream_key
        );

        match connect_once(
            &url,
            &token,
            &server_id,
            &channel_id,
            &session_id,
            user_id,
            owner_id,
            &stream_key,
            generation,
        )
        .await
        {
            Ok(()) => break,
            Err(error) => {
                log::warn!("🎬 [GO LIVE] Conexão encerrada: {}", error);
                if !is_current(&stream_key, generation) {
                    break;
                }
                sleep(Duration::from_millis(1500)).await;
            }
        }
    }

    end_generation(&stream_key, generation);
    // Only this viewer's SSRCs: the user may still be sharing their camera on
    // the parent voice connection.
    super::unregister_user_video_of(owner_id, true);
    // Keep the card (and its Watch button) while the user is still live.
    if !has_viewer_for(owner_id) && !super::is_native_live(owner_id) {
        crate::screen_capture::types::set_peer_streaming(owner_id, false);
    }
    log::info!("🛑 [GO LIVE] Visualização encerrada key={}", stream_key);
}

async fn connect_once(
    url: &str,
    token: &str,
    server_id: &str,
    channel_id: &str,
    session_id: &str,
    user_id: u64,
    owner_id: u64,
    stream_key: &str,
    generation: u64,
) -> Result<(), String> {
    let (ws, _) = crate::gateway::voice::connect_voice_ws(url)
        .await
        .map_err(|e| format!("WebSocket: {e:?}"))?;
    let (write, mut read) = ws.split();
    let write = Arc::new(AsyncMutex::new(write));

    let secret_key: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
    let mode: Arc<Mutex<String>> = Arc::new(Mutex::new("aead_aes256_gcm_rtpsize".to_string()));
    let dave: Arc<Mutex<Option<DaveSession>>> = Arc::new(Mutex::new(None));
    let ssrc_to_uid: Arc<Mutex<HashMap<u32, u64>>> = Arc::new(Mutex::new(HashMap::new()));
    let saved_external_sender: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));

    let mut socket: Option<Arc<UdpSocket>> = None;
    let mut target: Option<SocketAddr> = None;
    let mut sender_ssrc = 0u32;
    let mut udp_started = false;
    let mut dave_group = 0u64;
    // Video SSRCs announced before the session description; op 15 is only
    // meaningful once the transport key exists.
    let mut pending_wants: Vec<u32> = Vec::new();
    // Must exceed the heartbeat interval: after the first frames the socket is
    // idle until the server answers the next op 3 with op 6.
    let mut read_timeout = Duration::from_secs(60);
    let last_seq = Arc::new(std::sync::atomic::AtomicI64::new(-1));

    loop {
        if !is_current(stream_key, generation) {
            return Ok(());
        }
        let message = tokio::time::timeout(read_timeout, read.next())
            .await
            .map_err(|_| "timeout esperando al gateway de stream".to_string())?;
        let Some(Ok(message)) = message else {
            return Ok(());
        };

        match message {
            Message::Text(text) => {
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if let Some(seq) = value["seq"].as_i64() {
                    last_seq.store(seq, Ordering::Relaxed);
                }
                let op = value["op"].as_u64().unwrap_or(99);
                match op {
                    8 => {
                        let interval = value["d"]["heartbeat_interval"].as_f64().unwrap_or(41250.0);
                        let heartbeat = (interval * 0.75).max(1000.0) as u64;
                        read_timeout = Duration::from_millis(heartbeat.saturating_mul(2).saturating_add(5000));
                        let identify = json!({
                            "op": 0,
                            "d": {
                                "server_id": server_id,
                                "channel_id": channel_id,
                                "user_id": user_id.to_string(),
                                "session_id": session_id,
                                "token": token,
                                "video": true,
                                "streams": [{ "type": "screen", "rid": "100", "quality": 100 }],
                                "max_dave_protocol_version": 1
                            }
                        });
                        send_text(&write, identify).await?;

                        let write_hb = Arc::clone(&write);
                        let key = stream_key.to_string();
                        let seq_hb = Arc::clone(&last_seq);
                        tokio::spawn(async move {
                            loop {
                                sleep(Duration::from_millis(heartbeat)).await;
                                if !is_current(&key, generation) {
                                    break;
                                }
                                let t = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis() as u64;
                                let mut sink = write_hb.lock().await;
                                if sink
                                    .send(Message::Text(json!({ "op": 3, "d": { "t": t, "seq_ack": seq_hb.load(Ordering::Relaxed) } }).to_string().into()))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        });
                    }
                    2 => {
                        sender_ssrc = value["d"]["ssrc"].as_u64().unwrap_or(0) as u32;
                        let ip = value["d"]["ip"].as_str().unwrap_or("").to_string();
                        let port = value["d"]["port"].as_u64().unwrap_or(0) as u16;
                        if let Some(modes) = value["d"]["modes"].as_array() {
                            if let Some(selected) = modes.iter().find_map(|m| m.as_str()) {
                                *mode.lock().unwrap() = selected.to_string();
                            }
                        }
                        if ip.is_empty() || port == 0 {
                            return Err("Ready sin IP/puerto".to_string());
                        }

                        let udp = Arc::new(
                            UdpSocket::bind("0.0.0.0:0")
                                .await
                                .map_err(|e| format!("UDP bind: {e}"))?,
                        );
                        let discovered = discover_public_address(&udp, sender_ssrc, &ip, port).await;
                        let (public_ip, public_port) = discovered
                            .unwrap_or_else(|| (ip.clone(), udp.local_addr().map(|a| a.port()).unwrap_or(0)));
                        target = format!("{ip}:{port}").parse().ok();
                        socket = Some(Arc::clone(&udp));

                        let select = json!({
                            "op": 1,
                            "d": {
                                "protocol": "udp",
                                "data": {
                                    "address": public_ip,
                                    "port": public_port,
                                    "mode": mode.lock().unwrap().clone()
                                },
                                "codecs": [
                                    { "name": "opus", "type": "audio", "priority": 1000, "payload_type": 111, "rtx_payload_type": null },
                                    { "name": "H264", "type": "video", "priority": 1000,
                                      "payload_type": 102, "rtx_payload_type": 103,
                                      "encode": false, "decode": true }
                                ]
                            }
                        });
                        send_text(&write, select).await?;
                    }
                    4 => {
                        if let Some(keys) = value["d"]["secret_key"].as_array() {
                            let bytes: Vec<u8> = keys
                                .iter()
                                .filter_map(|v| v.as_u64().map(|n| n as u8))
                                .collect();
                            if bytes.len() == 32 {
                                *secret_key.lock().unwrap() = Some(bytes);
                            }
                        }
                        if let Some(codec) = value["d"]["video_codec"].as_str() {
                            log::info!("🎬 [GO LIVE] codec de vídeo negociado: {}", codec);
                        }
                        dave_group = channel_id.parse::<u64>().unwrap_or(1).max(1);
                        let version = value["d"]["dave_protocol_version"]
                            .as_u64()
                            .unwrap_or(1)
                            .max(1) as u16;
                        {
                            let mut session = dave.lock().unwrap();
                            if session.is_none() {
                                match DaveSession::new(
                                    NonZeroU16::new(version).unwrap_or(NonZeroU16::MIN),
                                    user_id,
                                    dave_group,
                                    None,
                                ) {
                                    Ok(created) => *session = Some(created),
                                    Err(error) => log::warn!("🎬 [GO LIVE] DAVE init falhou: {:?}", error),
                                }
                            }
                        }

                        if !pending_wants.is_empty() {
                            let wants = std::mem::take(&mut pending_wants);
                            send_text(
                                &write,
                                json!({ "op": 15, "d": media_sink_wants(&wants) }),
                            )
                            .await?;
                        }

                        if !udp_started {
                            udp_started = true;
                            spawn_video_receiver(
                                stream_key.to_string(),
                                generation,
                                owner_id,
                                socket.clone(),
                                target,
                                sender_ssrc,
                                Arc::clone(&secret_key),
                                Arc::clone(&mode),
                                Arc::clone(&dave),
                                Arc::clone(&ssrc_to_uid),
                            );
                        }
                    }
                    12 => {
                        let primary = handle_video_state(
                            &value["d"],
                            owner_id,
                            true,
                            &ssrc_to_uid,
                        );
                        if !primary.is_empty() {
                            if secret_key.lock().unwrap().is_none() {
                                for ssrc in primary {
                                    if !pending_wants.contains(&ssrc) {
                                        pending_wants.push(ssrc);
                                    }
                                }
                            } else {
                                send_text(
                                    &write,
                                    json!({ "op": 15, "d": media_sink_wants(&primary) }),
                                )
                                .await?;
                            }
                        }
                    }
                    21 | 18 => {
                        let transition = value["d"]["transition_id"].as_u64().unwrap_or(0);
                        send_text(
                            &write,
                            json!({ "op": 23, "d": { "transition_id": transition } }),
                        )
                        .await?;
                    }
                    22 | 19 => {
                        log::info!("🎬 [GO LIVE] DAVE execute transition");
                    }
                    _ => {}
                }
            }
            Message::Binary(data) => {
                // v8 binary frames: [seq u16][opcode][payload].
                if data.len() < 3 {
                    continue;
                }
                last_seq.store(u16::from_be_bytes([data[0], data[1]]) as i64, Ordering::Relaxed);
                handle_dave_binary(
                    &data[2..],
                    &dave,
                    &saved_external_sender,
                    user_id,
                    dave_group,
                    &write,
                )
                .await?;
            }
            Message::Close(_) => return Ok(()),
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_video_receiver(
    stream_key: String,
    generation: u64,
    owner_id: u64,
    socket: Option<Arc<UdpSocket>>,
    target: Option<SocketAddr>,
    sender_ssrc: u32,
    secret_key: Arc<Mutex<Option<Vec<u8>>>>,
    mode: Arc<Mutex<String>>,
    dave: Arc<Mutex<Option<DaveSession>>>,
    ssrc_to_uid: Arc<Mutex<HashMap<u32, u64>>>,
) {
    let Some(socket) = socket else {
        return;
    };
    tokio::spawn(async move {
        let ctx = NativeVideoContext {
            secret_key,
            mode,
            dave,
            ssrc_to_uid,
            stream_owner: Some(owner_id),
            socket: Some(Arc::clone(&socket)),
            target,
            sender_ssrc,
            rtcp_counter: super::new_rtcp_counter(),
        };
        let mut session = NativeVideoSession::new();
        let mut buffer = vec![0u8; 2048];
        let mut last_tick = std::time::Instant::now();
        loop {
            if !is_current(&stream_key, generation) {
                break;
            }
            match tokio::time::timeout(Duration::from_millis(500), socket.recv_from(&mut buffer)).await {
                Ok(Ok((len, _))) => session.ingest(&ctx, &buffer[..len]),
                Ok(Err(_)) => break,
                Err(_) => {}
            }
            if last_tick.elapsed() >= Duration::from_secs(1) {
                session.tick(&ctx);
                last_tick = std::time::Instant::now();
            }
        }
    });
}

/// Stops every Go Live viewer of `uid` (they stopped streaming).
pub fn stop_viewers_of(uid: u64) {
    let keys: Vec<String> = generations()
        .lock()
        .map(|map| {
            map.keys()
                .filter(|k| k.rsplit(':').next().and_then(|s| s.parse::<u64>().ok()) == Some(uid))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    for key in keys {
        stop_stream_viewer(&key);
    }
}

/// Connects to a Go Live stream RTC server as the transmitter (broadcaster).
pub async fn connect_stream_sender(
    endpoint: String,
    token: String,
    server_id: String,
    channel_id: String,
    session_id: String,
    user_id: u64,
    stream_key: String,
) {
    let generation = begin_generation(&stream_key);
    let url = normalize_url(&endpoint);

    log::info!("🎬 [GO LIVE TX] Conectando como EMISSOR a {} key={}", url, stream_key);

    let (ws, _) = match crate::gateway::voice::connect_voice_ws(&url).await {
        Ok(pair) => pair,
        Err(e) => {
            log::warn!("🎬 [GO LIVE TX] Falha ao conectar WebSocket: {:?}", e);
            return;
        }
    };

    let (write, mut read) = ws.split();
    let write = Arc::new(AsyncMutex::new(write));
    let mut sender_ssrc = 0u32;
    let mut video_ssrc = 0u32;
    let mut rtx_ssrc = 0u32;
    let mut mode_str = "aead_aes256_gcm_rtpsize".to_string();
    let mut socket: Option<Arc<UdpSocket>> = None;
    let mut target: Option<SocketAddr> = None;
    let mut read_timeout = Duration::from_secs(60);
    // Voice gateway v8: server messages carry a sequence number that heartbeats acknowledge.
    let last_seq = Arc::new(std::sync::atomic::AtomicI64::new(-1));
    // DAVE (E2EE): Discord refuses media from clients that do not join the group.
    let dave_group = channel_id.parse::<u64>().unwrap_or(1).max(1);
    let dave: Arc<Mutex<Option<DaveSession>>> = Arc::new(Mutex::new(
        DaveSession::new(NonZeroU16::new(1).unwrap(), user_id, dave_group, None).ok(),
    ));
    let external_sender: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));

    while is_current(&stream_key, generation) {
        let msg = match tokio::time::timeout(read_timeout, read.next()).await {
            Ok(Some(Ok(m))) => m,
            Ok(Some(Err(e))) => {
                log::warn!("🎬 [GO LIVE TX] erro no WebSocket: {:?}", e);
                break;
            }
            Ok(None) => {
                log::warn!("🎬 [GO LIVE TX] WebSocket terminou sem Close");
                break;
            }
            Err(_) => {
                log::warn!("🎬 [GO LIVE TX] sem resposta do servidor");
                break;
            }
        };

        match msg {
            Message::Text(text) => {
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if let Some(seq) = value["seq"].as_i64() {
                    last_seq.store(seq, Ordering::Relaxed);
                }
                let op = value["op"].as_u64().unwrap_or(99);
                match op {
                    8 => {
                        let interval = value["d"]["heartbeat_interval"].as_f64().unwrap_or(41250.0);
                        let heartbeat = (interval * 0.75).max(1000.0) as u64;
                        read_timeout = Duration::from_millis(heartbeat.saturating_mul(2).saturating_add(5000));
                        let identify = json!({
                            "op": 0,
                            "d": {
                                "server_id": server_id,
                                "channel_id": channel_id,
                                "user_id": user_id.to_string(),
                                "session_id": session_id,
                                "token": token,
                                "video": true,
                                "streams": [
                                    { "type": "screen", "rid": "100", "quality": 100 }
                                ],
                                "max_dave_protocol_version": 1
                            }
                        });
                        if let Err(e) = send_text(&write, identify).await {
                            log::warn!("🎬 [GO LIVE TX] Falha no Identify: {:?}", e);
                            break;
                        }

                        let write_hb = Arc::clone(&write);
                        let key = stream_key.clone();
                        let seq_hb = Arc::clone(&last_seq);
                        tokio::spawn(async move {
                            loop {
                                sleep(Duration::from_millis(heartbeat)).await;
                                if !is_current(&key, generation) {
                                    break;
                                }
                                let t = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis() as u64;
                                let mut sink = write_hb.lock().await;
                                if sink.send(Message::Text(json!({ "op": 3, "d": { "t": t, "seq_ack": seq_hb.load(Ordering::Relaxed) } }).to_string().into())).await.is_err() {
                                    break;
                                }
                            }
                        });
                    }
                    2 => {
                        sender_ssrc = value["d"]["ssrc"].as_u64().unwrap_or(0) as u32;
                        video_ssrc = value["d"]["streams"]
                            .as_array()
                            .and_then(|arr| arr.first())
                            .and_then(|s| s["ssrc"].as_u64())
                            .map(|s| s as u32)
                            .unwrap_or(sender_ssrc.saturating_add(1));
                        rtx_ssrc = value["d"]["streams"]
                            .as_array()
                            .and_then(|arr| arr.first())
                            .and_then(|s| s["rtx_ssrc"].as_u64())
                            .map(|s| s as u32)
                            .unwrap_or(video_ssrc.saturating_add(1));

                        let ip = value["d"]["ip"].as_str().unwrap_or("").to_string();
                        let port = value["d"]["port"].as_u64().unwrap_or(0) as u16;
                        if let Some(modes) = value["d"]["modes"].as_array() {
                            let available_modes: Vec<&str> = modes.iter().filter_map(|m| m.as_str()).collect();
                            if available_modes.contains(&"aead_aes256_gcm_rtpsize") {
                                mode_str = "aead_aes256_gcm_rtpsize".to_string();
                            } else if available_modes.contains(&"aead_xchacha20_poly1305_rtpsize") {
                                mode_str = "aead_xchacha20_poly1305_rtpsize".to_string();
                            } else if let Some(&selected) = available_modes.first() {
                                mode_str = selected.to_string();
                            }
                        }
                        if ip.is_empty() || port == 0 {
                            log::warn!("🎬 [GO LIVE TX] Ready sem IP/porta");
                            break;
                        }

                        let udp = match UdpSocket::bind("0.0.0.0:0").await {
                            Ok(u) => Arc::new(u),
                            Err(e) => {
                                log::warn!("🎬 [GO LIVE TX] Falha no UDP bind: {:?}", e);
                                break;
                            }
                        };
                        let discovered = discover_public_address(&udp, sender_ssrc, &ip, port).await;
                        let (pub_ip, pub_port) = discovered
                            .unwrap_or_else(|| (ip.clone(), udp.local_addr().map(|a| a.port()).unwrap_or(0)));
                        target = format!("{ip}:{port}").parse().ok();
                        socket = Some(Arc::clone(&udp));

                        let select = json!({
                            "op": 1,
                            "d": {
                                "protocol": "udp",
                                "data": {
                                    "address": pub_ip,
                                    "port": pub_port,
                                    "mode": mode_str.clone()
                                },
                                "codecs": [
                                    { "name": "opus", "type": "audio", "priority": 1000, "payload_type": 111, "rtx_payload_type": null },
                                    { "name": "H264", "type": "video", "priority": 1000,
                                      "payload_type": 102, "rtx_payload_type": 103,
                                      "encode": true, "decode": false }
                                ]
                            }
                        });
                        let _ = send_text(&write, select).await;
                    }
                    4 => {
                        let dave_version = value["d"]["dave_protocol_version"].as_u64().unwrap_or(0);
                        log::info!("🎬 [GO LIVE TX] sessão pronta: mode={:?} dave={}", value["d"]["mode"].as_str(), dave_version);
                        let dave_for_sender = if dave_version > 0 { Some(Arc::clone(&dave)) } else { None };
                        let mut secret_key_bytes = Vec::new();
                        if let Some(keys) = value["d"]["secret_key"].as_array() {
                            secret_key_bytes = keys.iter().filter_map(|v| v.as_u64().map(|n| n as u8)).collect();
                        }
                        if secret_key_bytes.len() != 32 {
                            log::warn!("🎬 [GO LIVE TX] Secret key inválida (len={})", secret_key_bytes.len());
                            break;
                        }

                        // Opcode 5 Speaking
                        let speaking = json!({
                            "op": 5,
                            "d": {
                                "speaking": 1,
                                "delay": 0,
                                "ssrc": sender_ssrc
                            }
                        });
                        let _ = send_text(&write, speaking).await;

                        // Opcode 12 Video
                        let video = json!({
                            "op": 12,
                            "d": {
                                "audio_ssrc": sender_ssrc,
                                "video_ssrc": video_ssrc,
                                "rtx_ssrc": rtx_ssrc,
                                "streams": [
                                    {
                                        "type": "video",
                                        "rid": "100",
                                        "ssrc": video_ssrc,
                                        "active": true,
                                        "quality": 100,
                                        "rtx_ssrc": rtx_ssrc,
                                        "max_bitrate": 8000000,
                                        "max_framerate": 60,
                                        "max_resolution": {
                                            "type": "fixed",
                                            "width": 1920,
                                            "height": 1080
                                        }
                                    }
                                ]
                            }
                        });
                        let _ = send_text(&write, video).await;

                        // Opcode 22 Unpause Video Stream
                        let unpause = json!({
                            "op": 22,
                            "d": {
                                "video_ssrc": video_ssrc,
                                "paused": false
                            }
                        });
                        let _ = send_text(&write, unpause).await;

                        if let (Some(udp), Some(target_addr)) = (socket.as_ref(), target) {
                            let mode = super::crypto::TransportMode::from_name(&mode_str);
                            super::sender::start_sender(
                                Arc::clone(udp),
                                target_addr,
                                secret_key_bytes,
                                mode,
                                video_ssrc,
                                sender_ssrc,
                                rtx_ssrc,
                                stream_key.clone(),
                                dave_for_sender,
                            );
                        }
                    }
                    21 => {
                        let transition = value["d"]["transition_id"].as_u64().unwrap_or(0);
                        if transition != 0 {
                            let _ = send_text(&write, json!({ "op": 23, "d": { "transition_id": transition } })).await;
                        }
                    }
                    24 => {
                        // New epoch: rejoin the group with a fresh key package.
                        let key_package = {
                            let mut guard = dave.lock().unwrap();
                            *guard = DaveSession::new(NonZeroU16::new(1).unwrap(), user_id, dave_group, None).ok();
                            let ext = external_sender.lock().unwrap().clone();
                            guard.as_mut().and_then(|s| {
                                if let Some(ext) = ext { let _ = s.set_external_sender(&ext); }
                                s.create_key_package().ok()
                            })
                        };
                        if let Some(kp) = key_package {
                            let mut packet = vec![26u8];
                            packet.extend_from_slice(&kp);
                            let mut sink = write.lock().await;
                            let _ = sink.send(Message::Binary(packet.into())).await;
                        }
                    }
                    3 | 6 => {}
                    _ => log::info!("🎬 [GO LIVE TX] op={} ignorado: {}", op, value["d"]),
                }
            }
            Message::Binary(data) => {
                if !data.is_empty() {
                    // v8 binary frames from the server: [seq u16][opcode][payload].
                    if data.len() < 3 {
                        continue;
                    }
                    last_seq.store(u16::from_be_bytes([data[0], data[1]]) as i64, Ordering::Relaxed);
                    let data = &data[2..];
                    log::info!("🎬 [GO LIVE TX] DAVE binário op={} ({} bytes)", data[0], data.len());
                    if let Err(e) = handle_dave_binary(data, &dave, &external_sender, user_id, dave_group, &write).await {
                        log::warn!("🎬 [GO LIVE TX] DAVE: {}", e);
                    }
                    let ready = dave.lock().unwrap().as_ref().map(|s| s.is_ready()).unwrap_or(false);
                    if ready {
                        crate::screen_capture::types::request_intra_keyframe();
                    }
                }
            }
            Message::Close(frame) => {
                log::warn!("🎬 [GO LIVE TX] Discord fechou a conexão: {:?}", frame);
                break;
            }
            _ => {}
        }
    }

    super::sender::stop_sender(&stream_key);
    end_generation(&stream_key, generation);
    log::info!("🛑 [GO LIVE TX] Conexão emissora encerrada key={}", stream_key);
}
