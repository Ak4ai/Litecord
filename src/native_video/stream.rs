//! Go Live receive connection.
//!
//! Watching a stream is a **second** voice connection: the main gateway gets
//! opcode 20 (Watch Stream), the server answers STREAM_CREATE +
//! STREAM_SERVER_UPDATE, and a new voice WebSocket/UDP pair is opened with its
//! own token, transport key and DAVE group.  The parent voice connection (and
//! its audio) is never touched.

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

/// Cancels a stream viewer (STREAM_DELETE, user stopped watching, call ended).
pub fn stop_stream_viewer(stream_key: &str) {
    NEXT_GENERATION.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut map) = generations().lock() {
        map.remove(stream_key);
    }
}

/// Cancels every Go Live viewer (left the voice call).
pub fn stop_all_stream_viewers() {
    NEXT_GENERATION.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut map) = generations().lock() {
        map.clear();
    }
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
    // The server may keep the voice state "streaming" for a moment; do not leave
    // an empty stream card in the UI unless the camera is still sending.
    let (_still_streaming, camera_on) = crate::screen_capture::types::get_user_stream_flags(owner_id);
    if !has_viewer_for(owner_id) && !camera_on {
        crate::screen_capture::types::set_peer_streaming(owner_id, false);
    }
    log::info!("🛑 [GO LIVE] Visualização encerrada key={}", stream_key);
}

fn normalize_url(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    // Same URL shape as the official client: bare path, no version query. The
    // server chooses the protocol and announces it in op 24 / op 4.
    if trimmed.starts_with("wss://") || trimmed.starts_with("ws://") {
        if trimmed.contains('?') {
            trimmed.to_string()
        } else {
            format!("{trimmed}/")
        }
    } else {
        format!("wss://{trimmed}/")
    }
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
                                "streams": [],
                                "max_dave_protocol_version": 1
                            }
                        });
                        send_text(&write, identify).await?;

                        let write_hb = Arc::clone(&write);
                        let key = stream_key.to_string();
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
                                    .send(Message::Text(json!({ "op": 3, "d": t }).to_string().into()))
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
                                    { "name": "opus", "type": "audio", "priority": 1000, "payload_type": 120 },
                                    { "name": "H264", "type": "video", "priority": 1000,
                                      "payload_type": 103, "rtx_payload_type": 104,
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
                        dave_group = value["d"]["media_session_id"]
                            .as_u64()
                            .filter(|id| *id != 0)
                            .unwrap_or_else(|| server_id.parse::<u64>().unwrap_or(1).saturating_sub(1))
                            .max(1);
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
                if data.is_empty() {
                    continue;
                }
                handle_dave_binary(
                    &data,
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
