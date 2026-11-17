#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use log::{info, error, warn};

use super::types::*;
use super::formatting::*;
use super::voice::connect_voice_gateway;

pub struct GatewayClient {
    token: String,
    event_tx: mpsc::Sender<GatewayEvent>,
    user_id: Arc<std::sync::Mutex<Option<String>>>,
    voice_session_id: Arc<std::sync::Mutex<Option<String>>>,
    voice_token: Arc<std::sync::Mutex<Option<String>>>,
    voice_endpoint: Arc<std::sync::Mutex<Option<String>>>,
    voice_guild_id: Arc<std::sync::Mutex<Option<String>>>,
    voice_channel_id: Arc<std::sync::Mutex<Option<String>>>,
    voice_self_mute: Arc<std::sync::Mutex<bool>>,
    ws_tx: Arc<tokio::sync::Mutex<Option<mpsc::Sender<Message>>>>,
    /// The session READY opened, kept so a dropped connection resumes it instead of
    /// identifying again: a new session makes Discord drop the voice call of the old one.
    resume: std::sync::Mutex<Option<ResumeSession>>,
}

#[derive(Clone)]
struct ResumeSession {
    session_id: String,
    url: String,
}

const GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=9&encoding=json";

/// Close codes after which the session cannot be resumed (Discord gateway docs).
fn close_ends_session(code: u16) -> bool {
    matches!(code, 4004 | 4007 | 4009 | 4010 | 4011 | 4012 | 4013 | 4014)
}

/// Dispatch events this client acts on. The others (presences, typing, reactions, member
/// updates...: most of what a big account receives) are dropped after reading their
/// header, without building a JSON tree for their payload.
const HANDLED_EVENTS: &[&str] = &[
    "READY",
    "READY_SUPPLEMENTAL",
    "RESUMED",
    "GUILD_CREATE",
    "GUILD_MEMBERS_CHUNK",
    "VOICE_STATE_UPDATE",
    "VOICE_SERVER_UPDATE",
    "MESSAGE_CREATE",
    "MESSAGE_UPDATE",
    "MESSAGE_DELETE",
    "CALL_CREATE",
    "CALL_UPDATE",
    "CALL_DELETE",
];

/// Events that change what the user may see in a server. They only drop the server's
/// cached permissions, so just the ids are read.
const PERMISSION_EVENTS: &[&str] = &[
    "GUILD_UPDATE",
    "GUILD_ROLE_CREATE",
    "GUILD_ROLE_UPDATE",
    "GUILD_ROLE_DELETE",
    "GUILD_MEMBER_UPDATE",
];

#[derive(Debug, PartialEq, Eq)]
struct Peek {
    op: u64,
    seq: Option<i64>,
    /// Worth parsing whole: a non-dispatch op, or a dispatch event in HANDLED_EVENTS.
    wanted: bool,
    /// One of PERMISSION_EVENTS.
    permissions: bool,
}

/// Reads only `op`, `s` and `t` of a gateway message; the payload is skipped, not built.
fn peek(text: &str) -> Option<Peek> {
    #[derive(serde::Deserialize)]
    struct Head<'a> {
        op: u64,
        #[serde(default)]
        s: Option<i64>,
        #[serde(default, borrow)]
        t: Option<std::borrow::Cow<'a, str>>,
    }
    let h: Head = serde_json::from_str(text).ok()?;
    let wanted = h.op != 0 || h.t.as_deref().is_some_and(|t| HANDLED_EVENTS.contains(&t));
    let permissions = h.op == 0 && h.t.as_deref().is_some_and(|t| PERMISSION_EVENTS.contains(&t));
    Some(Peek { op: h.op, seq: h.s, wanted, permissions })
}

/// The server whose permissions a PERMISSION_EVENTS event changes. Updates of other
/// members do not change the user's.
fn permission_change(text: &str, my_user_id: u64) -> Option<String> {
    use std::borrow::Cow;
    #[derive(serde::Deserialize)]
    struct Event<'a> {
        #[serde(borrow)]
        t: Cow<'a, str>,
        #[serde(borrow)]
        d: Data<'a>,
    }
    #[derive(serde::Deserialize)]
    struct Data<'a> {
        #[serde(default, borrow)]
        id: Option<Cow<'a, str>>,
        #[serde(default, borrow)]
        guild_id: Option<Cow<'a, str>>,
        #[serde(default, borrow)]
        user: Option<User<'a>>,
    }
    #[derive(serde::Deserialize)]
    struct User<'a> {
        #[serde(borrow)]
        id: Cow<'a, str>,
    }
    let e: Event = serde_json::from_str(text).ok()?;
    let guild = match e.t.as_ref() {
        "GUILD_UPDATE" => e.d.id,
        "GUILD_MEMBER_UPDATE" => e.d.user.filter(|u| u.id.parse() == Ok(my_user_id)).and(e.d.guild_id),
        _ => e.d.guild_id,
    };
    guild.map(Cow::into_owned)
}

/// The resume URL READY gives, with the query the first connection used.
fn resume_url(base: &str) -> String {
    format!("{}/?v=9&encoding=json", base.trim_end_matches('/'))
}

impl GatewayClient {
    pub fn new(raw_token: String, event_tx: mpsc::Sender<GatewayEvent>) -> Self {
        let mut token = raw_token.chars().filter(|c| !c.is_whitespace() && *c != '"' && *c != '\'').collect::<String>();

        if token.to_lowercase().starts_with("authorization:") {
            token = token[14..].to_string();
        }
        if token.to_lowercase().starts_with("bearer ") {
            token = token[7..].to_string();
        }

        let _prefix_len = token.len().min(12);
        info!("Token sanitizado carregado com sucesso (tamanho: {} chars).", token.len());

        Self {
            token,
            event_tx,
            user_id: Arc::new(std::sync::Mutex::new(None)),
            voice_session_id: Arc::new(std::sync::Mutex::new(None)),
            voice_token: Arc::new(std::sync::Mutex::new(None)),
            voice_endpoint: Arc::new(std::sync::Mutex::new(None)),
            voice_guild_id: Arc::new(std::sync::Mutex::new(None)),
            voice_channel_id: Arc::new(std::sync::Mutex::new(None)),
            voice_self_mute: Arc::new(std::sync::Mutex::new(false)),
            ws_tx: Arc::new(tokio::sync::Mutex::new(None)),
            resume: std::sync::Mutex::new(None),
        }
    }

    fn try_trigger_voice_connect(&self) {
        let user_id = self.user_id.lock().unwrap().clone();
        let voice_sid = self.voice_session_id.lock().unwrap().clone();
        let voice_tok = self.voice_token.lock().unwrap().clone();
        let voice_ep = self.voice_endpoint.lock().unwrap().clone();
        let voice_gid = self.voice_guild_id.lock().unwrap().clone().unwrap_or_default();
        let voice_cid = self.voice_channel_id.lock().unwrap().clone().unwrap_or_default();

        info!("Status de disparo de voz: user_id={:?}, sid={:?}, token={:?}, ep={:?}, gid='{}', cid='{}'",
            user_id.is_some(), voice_sid.is_some(), voice_tok.is_some(), voice_ep.is_some(), voice_gid, voice_cid);

        if let (Some(uid), Some(sid), Some(tok), Some(ep)) = (user_id, voice_sid, voice_tok, voice_ep) {
            let effective_gid = if voice_gid.is_empty() || voice_gid == "@me" { voice_cid.clone() } else { voice_gid };
            if !voice_cid.is_empty() || !effective_gid.is_empty() {
                info!("⚡ TODAS AS CREDENCIAIS DE VOZ PRONTAS! Conectando à Voice Gateway no endpoint {}...", ep);
                *self.voice_token.lock().unwrap() = None;
                *self.voice_session_id.lock().unwrap() = None;

                let self_mute_state = Arc::clone(&self.voice_self_mute);
                let event_tx_conn = self.event_tx.clone();
                tokio::spawn(async move {
                    connect_voice_gateway(&ep, &effective_gid, &uid, &sid, &tok, &voice_cid, self_mute_state, event_tx_conn).await;
                });
            }
        }
    }

    /// Rebuild the voice session after the media path dies (NAT rebind, interface
    /// change). Dropping and re-adding the voice state through the main gateway is
    /// the same sequence Discord uses; it is the only way to get a fresh UDP socket,
    /// endpoint and DAVE group without the user leaving the call.
    async fn force_voice_rejoin(&self) {
        let target_cid = match self.voice_channel_id.lock().unwrap().clone() {
            Some(cid) => cid,
            None => return,
        };
        let target_gid = self.voice_guild_id.lock().unwrap().clone().unwrap_or_default();
        let self_mute = *self.voice_self_mute.lock().unwrap();
        let self_deaf = is_self_deaf();
        warn!("🎧 [VOICE] Reconstruindo a sessão de voz de '{}' após perda de mídia...", target_cid);

        let effective_gid = if target_gid.trim().is_empty() || target_gid == "@me" {
            serde_json::Value::Null
        } else {
            serde_json::json!(target_gid)
        };

        // Send the reset first: if the main gateway socket is down, tearing the local
        // credentials down would leave voice unusable with no way to recover. In that
        // case keep everything and retry once the socket is back.
        let reset_payload = serde_json::json!({
            "op": 4,
            "d": { "guild_id": effective_gid.clone(), "channel_id": null, "self_mute": self_mute, "self_deaf": self_deaf }
        });
        let reset_sent = match self.ws_tx.lock().await.as_ref() {
            Some(tx) => tx.send(Message::Text(reset_payload.to_string().into())).await.is_ok(),
            None => false,
        };
        if !reset_sent {
            warn!("🎧 [VOICE] Gateway principal indisponível; nova tentativa de reconstrução em instantes.");
            crate::gateway::voice::VOICE_MEDIA_REJOIN_REQUESTED.store(true, Ordering::SeqCst);
            return;
        }

        IS_WATCHDOG_RESETTING.store(true, Ordering::SeqCst);
        // Tear the dead session down deterministically: the audio threads watch this
        // counter, so they stop even if the gateway never acknowledges the reset.
        CURRENT_VOICE_SESSION_ID.fetch_add(1, Ordering::SeqCst);
        *self.voice_session_id.lock().unwrap() = None;
        *self.voice_token.lock().unwrap() = None;
        *self.voice_endpoint.lock().unwrap() = None;
        // The old voice task may keep its WebSocket open for a while, so it can still
        // look "connected" when the rejoin arrives; that would make the state update
        // skip the reconnect and leave voice silent.
        set_is_connected_to_voice(false);
        // Let the gateway emit the disconnect before asking for the channel again;
        // sent back to back, both updates can be merged and the rejoin ignored.
        tokio::time::sleep(Duration::from_millis(600)).await;
        let rejoin_payload = serde_json::json!({
            "op": 4,
            "d": { "guild_id": effective_gid, "channel_id": target_cid, "self_mute": self_mute, "self_deaf": self_deaf }
        });
        let rejoin_sent = match self.ws_tx.lock().await.as_ref() {
            Some(tx) => tx.send(Message::Text(rejoin_payload.to_string().into())).await.is_ok(),
            None => false,
        };
        if !rejoin_sent {
            warn!("🎧 [VOICE] Não foi possível reentrar no canal após a reconstrução; tentando de novo.");
            crate::gateway::voice::VOICE_MEDIA_REJOIN_REQUESTED.store(true, Ordering::SeqCst);
        }
        tokio::time::sleep(Duration::from_millis(2000)).await;
        IS_WATCHDOG_RESETTING.store(false, Ordering::SeqCst);
    }

    pub async fn start(self: Arc<Self>, mut cmd_rx: mpsc::Receiver<GatewayCommand>) {
        // Voice media watchdog: the voice connection asks for a rebuild when its UDP
        // path stays dead. Rejoining needs the main gateway socket, which lives here.
        let rejoin_client = Arc::clone(&self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(3)).await;
                if !crate::gateway::voice::VOICE_MEDIA_REJOIN_REQUESTED.swap(false, Ordering::SeqCst) {
                    continue;
                }
                rejoin_client.force_voice_rejoin().await;
            }
        });

        // Spawn GatewayCommand listener loop (permanece vivo entre reconexões do WebSocket)
        let client_cmd = Arc::clone(&self);
        tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    GatewayCommand::UpdateVoiceState { guild_id, channel_id, self_mute, self_deaf } => {
                        *client_cmd.voice_self_mute.lock().unwrap() = self_mute;

                        let is_dm = guild_id == "@me" || guild_id.trim().is_empty();
                        let is_channel_change = {
                            let cur_gid = client_cmd.voice_guild_id.lock().unwrap();
                            let cur_cid = client_cmd.voice_channel_id.lock().unwrap();
                            if *cur_cid != channel_id {
                                true
                            } else if channel_id.is_none() {
                                true
                            } else if is_dm {
                                false
                            } else {
                                *cur_gid != Some(guild_id.clone())
                            }
                        };

                        if is_channel_change {
                            // Incrementa session ID para cancelar tarefas antigas de áudio UDP
                            CURRENT_VOICE_SESSION_ID.fetch_add(1, Ordering::SeqCst);

                            // Reset de credenciais de voz para garantir handshake limpo apenas em troca/desconexão
                            *client_cmd.voice_token.lock().unwrap() = None;
                            *client_cmd.voice_session_id.lock().unwrap() = None;
                            *client_cmd.voice_endpoint.lock().unwrap() = None;
                            *client_cmd.voice_guild_id.lock().unwrap() = Some(guild_id.clone());
                            *client_cmd.voice_channel_id.lock().unwrap() = channel_id.clone();
                        }

                        let effective_gid_val = if guild_id.trim().is_empty() || guild_id == "@me" {
                            serde_json::Value::Null
                        } else {
                            serde_json::json!(guild_id)
                        };

                        let payload = serde_json::json!({
                            "op": 4,
                            "d": {
                                "guild_id": effective_gid_val,
                                "channel_id": channel_id,
                                "self_mute": self_mute,
                                "self_deaf": self_deaf
                            }
                        });

                        info!("📡 Enviando OP 4 VoiceStateUpdate à Gateway: {}", payload);
                        if let Some(tx) = client_cmd.ws_tx.lock().await.as_ref() {
                            let _ = tx.send(Message::Text(payload.to_string().into())).await;
                        }

                        // 🛡️ Watchdog Anti-Estado Fantasma: Apenas ao conectar a um NOVO canal
                        if is_channel_change {
                            if let Some(target_cid) = channel_id {
                                let client_watchdog = Arc::clone(&client_cmd);
                                let target_gid = guild_id.clone();
                                tokio::spawn(async move {
                                    tokio::time::sleep(Duration::from_millis(8000)).await;
                                    let is_still_pending = {
                                        let cur_cid = client_watchdog.voice_channel_id.lock().unwrap();
                                        *cur_cid == Some(target_cid.clone()) && !is_connected_to_voice()
                                    };

                                    if is_still_pending {
                                        warn!("⚠️ [VOICE WATCHDOG] Detectado estado fantasma/timeout na Gateway! Forçando reset OP 4 para reconexão limpa...");
                                        IS_WATCHDOG_RESETTING.store(true, Ordering::SeqCst);
                                        let effective_gid = if target_gid.trim().is_empty() || target_gid == "@me" {
                                            serde_json::Value::Null
                                        } else {
                                            serde_json::json!(target_gid)
                                        };
                                        let reset_payload = serde_json::json!({
                                            "op": 4,
                                            "d": { "guild_id": effective_gid.clone(), "channel_id": null, "self_mute": false, "self_deaf": false }
                                        });
                                        if let Some(tx) = client_watchdog.ws_tx.lock().await.as_ref() {
                                            let _ = tx.send(Message::Text(reset_payload.to_string().into())).await;
                                        }

                                        tokio::time::sleep(Duration::from_millis(300)).await;

                                        let rejoin_payload = serde_json::json!({
                                            "op": 4,
                                            "d": { "guild_id": effective_gid, "channel_id": target_cid, "self_mute": false, "self_deaf": false }
                                        });
                                        if let Some(tx) = client_watchdog.ws_tx.lock().await.as_ref() {
                                            let _ = tx.send(Message::Text(rejoin_payload.to_string().into())).await;
                                        }

                                        tokio::time::sleep(Duration::from_millis(2000)).await;
                                        IS_WATCHDOG_RESETTING.store(false, Ordering::SeqCst);
                                    }
                                });
                            }
                        }
                    }
                    GatewayCommand::SubscribeGuild { .. } => {}
                }
            }
        });

        // Survives reconnections: RESUME replays what came after it.
        let last_seq_arc = Arc::new(std::sync::atomic::AtomicI64::new(-1));

        // Loop Infinito de Auto-Reconexão da Gateway
        loop {
            let resuming = self.resume.lock().unwrap().clone().filter(|_| last_seq_arc.load(Ordering::Relaxed) >= 0);
            let url = resuming.as_ref().map(|r| resume_url(&r.url)).unwrap_or_else(|| GATEWAY_URL.to_string());
            if resuming.is_some() {
                info!("Reconectando à Gateway v9 do Discord para retomar a sessão...");
            } else {
                info!("Conectando à Gateway v9 do Discord...");
            }

            match connect_async(url.as_str()).await {
                Ok((ws_stream, _)) => {
                    info!("Conexão WebSocket estabelecida com sucesso!");
                    crate::screen_capture::invalidate_cached_stun_address();
                    crate::screen_capture::force_signaling_broadcast();
                    let (mut write, mut read) = ws_stream.split();
                    let (ws_out_tx, mut ws_out_rx) = mpsc::channel::<Message>(100);

                    *self.ws_tx.lock().await = Some(ws_out_tx.clone());

                    // Worker para envio ordenado de mensagens no WebSocket
                    let writer_task = tokio::spawn(async move {
                        while let Some(msg) = ws_out_rx.recv().await {
                            if let Err(e) = write.send(msg).await {
                                warn!("Falha ao enviar mensagem no WebSocket: {:?}", e);
                                break;
                            }
                        }
                    });

                    let mut hb_task_handle: Option<tokio::task::JoinHandle<()>> = None;

                    // Read incoming Gateway messages loop
                    while let Some(msg_result) = read.next().await {
                        #[cfg(feature = "automation")]
                        if crate::utils::test_hooks::DROP_GATEWAY.swap(false, Ordering::Relaxed) {
                            warn!("🤖 [AUTOMATION] Conexão com a Gateway abandonada (teste de queda)");
                            break;
                        }
                        match msg_result {
                            Ok(Message::Text(text)) => {
                                // Every sequence counts (RESUME replays from the last one),
                                // but only events the app uses are parsed whole.
                                let Some(head) = peek(&text) else { continue };
                                if let Some(s) = head.seq {
                                    last_seq_arc.store(s, Ordering::Relaxed);
                                }
                                if head.permissions {
                                    if let Some(guild_id) = permission_change(&text, crate::gateway::get_my_user_id()) {
                                        crate::channel_permissions::forget(&guild_id);
                                    }
                                }
                                if !head.wanted {
                                    continue;
                                }
                                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                    let op = value["op"].as_u64().unwrap_or(99);
                                    if let Some(s) = value["s"].as_i64() {
                                        last_seq_arc.store(s, Ordering::Relaxed);
                                    }

                                    match op {
                                        10 => {
                                            // Opcode 10: HELLO
                                            let heartbeat_interval = value["d"]["heartbeat_interval"]
                                                .as_u64()
                                                .unwrap_or(41250);

                                            info!("Heartbeat interval recebido: {} ms", heartbeat_interval);

                                            // Send initial Heartbeat (Opcode 1)
                                            let last_s = last_seq_arc.load(Ordering::Relaxed);
                                            let hb_initial = if last_s >= 0 {
                                                serde_json::json!({ "op": 1, "d": last_s })
                                            } else {
                                                serde_json::json!({ "op": 1, "d": null })
                                            };
                                            let _ = ws_out_tx.send(Message::Text(hb_initial.to_string().into())).await;

                                            if let Some(r) = resuming.as_ref() {
                                                let resume = serde_json::json!({
                                                    "op": 6,
                                                    "d": {
                                                        "token": self.token,
                                                        "session_id": r.session_id,
                                                        "seq": last_seq_arc.load(Ordering::Relaxed)
                                                    }
                                                });
                                                info!("Enviando RESUME para a Gateway (seq {})...", last_seq_arc.load(Ordering::Relaxed));
                                                let _ = ws_out_tx.send(Message::Text(resume.to_string().into())).await;
                                            } else {
                                            // Send Identify Payload (Opcode 2) for Discord User Tokens
                                            let identify = serde_json::json!({
                                                "op": 2,
                                                "d": {
                                                    "token": self.token,
                                                    "capabilities": 16381,
                                                    "properties": crate::utils::client_identity::properties(),
                                                    "presence": {
                                                        "status": "online",
                                                        "since": 0,
                                                        "activities": [],
                                                        "afk": false
                                                    },
                                                    "compress": false,
                                                    "client_state": {
                                                        "guild_versions": {}
                                                    }
                                                }
                                            });

                                            info!("Enviando payload IDENTIFY v9 para a Gateway...");
                                            let _ = ws_out_tx.send(Message::Text(identify.to_string().into())).await;
                                            }

                                            // Spawn Heartbeat Loop
                                            let tx_hb = ws_out_tx.clone();
                                            let last_seq_hb = Arc::clone(&last_seq_arc);
                                            if let Some(h) = hb_task_handle.take() { h.abort(); }
                                            hb_task_handle = Some(tokio::spawn(async move {
                                                loop {
                                                    sleep(Duration::from_millis(heartbeat_interval)).await;
                                                    let last_s = last_seq_hb.load(Ordering::Relaxed);
                                                    let hb = if last_s >= 0 {
                                                        serde_json::json!({ "op": 1, "d": last_s })
                                                    } else {
                                                        serde_json::json!({ "op": 1, "d": null })
                                                    };
                                                    if let Err(_) = tx_hb.send(Message::Text(hb.to_string().into())).await {
                                                        break;
                                                    }
                                                }
                                            }));
                                        }
                                        1 => {
                                            // Opcode 1: Discord asks for a heartbeat now.
                                            let last_s = last_seq_arc.load(Ordering::Relaxed);
                                            let hb = serde_json::json!({ "op": 1, "d": if last_s >= 0 { serde_json::json!(last_s) } else { Value::Null } });
                                            let _ = ws_out_tx.send(Message::Text(hb.to_string().into())).await;
                                        }
                                        7 => {
                                            // Opcode 7: RECONNECT. The session stays; resume it on a new connection.
                                            info!("🔁 [GATEWAY] Discord pediu reconexão; a sessão será retomada.");
                                            break;
                                        }
                                        9 => {
                                            // Opcode 9: INVALID SESSION. `d` says whether it can still be resumed.
                                            if value["d"].as_bool() != Some(true) {
                                                warn!("⚠️ [GATEWAY] Sessão inválida; será aberta uma nova.");
                                                *self.resume.lock().unwrap() = None;
                                                last_seq_arc.store(-1, Ordering::Relaxed);
                                            } else {
                                                warn!("⚠️ [GATEWAY] Sessão inválida, mas ainda retomável.");
                                            }
                                            // Discord asks for a 1-5 s wait before identifying again.
                                            sleep(Duration::from_millis(1000 + rand::random::<u64>() % 4000)).await;
                                            break;
                                        }
                                        _ => {
                                            if value["t"].as_str() == Some("READY") {
                                                let sid = value["d"]["session_id"].as_str();
                                                let rurl = value["d"]["resume_gateway_url"].as_str();
                                                *self.resume.lock().unwrap() = sid.map(|s| ResumeSession {
                                                    session_id: s.to_string(),
                                                    url: rurl.unwrap_or("wss://gateway.discord.gg").to_string(),
                                                });
                                            } else if value["t"].as_str() == Some("RESUMED") {
                                                info!("✅ [GATEWAY] Sessão retomada; chamada de voz e transmissão continuam.");
                                            }
                                            self.handle_event(&value).await;
                                        }
                                    }
                                }
                            }
                            Ok(Message::Close(close_frame)) => {
                                warn!("Gateway fechou a conexão (Close Frame): {:?}", close_frame);
                                if close_frame.as_ref().is_some_and(|f| close_ends_session(u16::from(f.code))) {
                                    *self.resume.lock().unwrap() = None;
                                    last_seq_arc.store(-1, Ordering::Relaxed);
                                }
                                break;
                            }
                            Err(e) => {
                                error!("Erro de leitura no WebSocket: {:?}", e);
                                break;
                            }
                            _ => {}
                        }
                    }

                    if let Some(h) = hb_task_handle { h.abort(); }
                    writer_task.abort();
                    *self.ws_tx.lock().await = None;
                    warn!("⚠️ [GATEWAY] Conexão com o Discord perdida. Tentando reconectar automaticamente em 2s...");
                }
                Err(e) => {
                    if resuming.is_some() {
                        // The resume host may be gone; the main gateway still resumes the session.
                        if let Some(r) = self.resume.lock().unwrap().as_mut() {
                            r.url = "wss://gateway.discord.gg".to_string();
                        }
                    }
                    warn!("⚠️ [GATEWAY] Falha ao conectar na Gateway do Discord: {:?}. Tentando reconectar em 3s...", e);
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    async fn handle_event(&self, v: &Value) {
        if let Some(t) = v["t"].as_str() {
            match t {
                "READY" => {
                    let uid = v["d"]["user"]["id"].as_str().unwrap_or("").to_string();
                    *self.user_id.lock().unwrap() = Some(uid.clone());
                    let uid_num = uid.parse::<u64>().unwrap_or(0);

                    let username = v["d"]["user"]["username"].as_str().unwrap_or("User");
                    let global_name = v["d"]["user"]["global_name"].as_str().unwrap_or(username);
                    if uid_num > 0 {
                        set_my_user_id(uid_num);
                        set_my_username(global_name.to_string());
                        register_user_name(uid_num, global_name.to_string());
                        register_user_avatar(uid_num, v["d"]["user"]["avatar"].as_str().unwrap_or(""));
                    }

                    // Parse all user/bot objects in READY payload
                    if let Some(users_arr) = v["d"]["users"].as_array() {
                        for u in users_arr {
                            let u_id: u64 = if let Some(s) = u["id"].as_str() {
                                s.parse().unwrap_or(0)
                            } else {
                                u["id"].as_u64().unwrap_or(0)
                            };
                            if u_id > 0 {
                                let mut dname = String::new();
                                if let Some(gname) = u["global_name"].as_str() {
                                    if !gname.is_empty() { dname = gname.to_string(); }
                                }
                                if dname.is_empty() {
                                    if let Some(uname) = u["username"].as_str() {
                                        if !uname.is_empty() { dname = uname.to_string(); }
                                    }
                                }
                                if !dname.is_empty() {
                                    register_user_avatar(u_id, u["avatar"].as_str().unwrap_or(""));
                                    register_user_name(u_id, dname);
                                }
                            }
                        }
                    }

                    // Parse initial voice states in READY payload
                    if let Some(guilds_arr) = v["d"]["guilds"].as_array() {
                        for g in guilds_arr {
                            if let Some(vs_arr) = g["voice_states"].as_array() {
                                for vs in vs_arr {
                                    let u_id: u64 = if let Some(s) = vs["user_id"].as_str() {
                                        s.parse().unwrap_or(0)
                                    } else {
                                        vs["user_id"].as_u64().unwrap_or(0)
                                    };
                                    let c_str = if let Some(s) = vs["channel_id"].as_str() {
                                        s.to_string()
                                    } else if let Some(n) = vs["channel_id"].as_u64() {
                                        n.to_string()
                                    } else {
                                        String::new()
                                    };
                                    if u_id > 0 && !c_str.is_empty() {
                                        if let Ok(mut gvs) = get_guild_voice_states_store().lock() {
                                            gvs.insert(u_id, c_str.clone());
                                            info!("📌 [READY] Pré-carregado estado de voz: User {} -> Canal {}", u_id, c_str);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let discriminator = v["d"]["user"]["discriminator"].as_str().unwrap_or("0");
                    let user_tag = if discriminator == "0" {
                        global_name.to_string()
                    } else {
                        format!("{}#{}", global_name, discriminator)
                    };
                    info!("Login BEM-SUCEDIDO na Gateway! Usuário: {} (@{})", global_name, username);
                    let _ = self.event_tx.send(GatewayEvent::Connected { user_tag }).await;
                }
                "READY_SUPPLEMENTAL" => {
                    info!("📌 Evento READY_SUPPLEMENTAL recebido da Gateway! Keys: {:?}", v["d"].as_object().map(|m| m.keys().collect::<Vec<_>>()));
                    if let Some(vs_arr) = v["d"]["voice_states"].as_array() {
                        info!("📌 [READY_SUPPLEMENTAL] Encontrados {} voice_states na raiz!", vs_arr.len());
                        for vs in vs_arr {
                            let u_id: u64 = if let Some(s) = vs["user_id"].as_str() {
                                s.parse().unwrap_or(0)
                            } else {
                                vs["user_id"].as_u64().unwrap_or(0)
                            };
                            let c_str = if let Some(s) = vs["channel_id"].as_str() {
                                s.to_string()
                            } else if let Some(n) = vs["channel_id"].as_u64() {
                                n.to_string()
                            } else {
                                String::new()
                            };
                            if u_id > 0 && !c_str.is_empty() {
                                if let Ok(mut gvs) = get_guild_voice_states_store().lock() {
                                    gvs.insert(u_id, c_str.clone());
                                    info!("📌 [READY_SUPPLEMENTAL] Estado de voz pré-carregado: User {} -> Canal {}", u_id, c_str);
                                }
                            }
                        }
                    }
                    if let Some(guilds_arr) = v["d"]["guilds"].as_array() {
                        info!("📌 [READY_SUPPLEMENTAL] Encontrados {} guilds no READY_SUPPLEMENTAL!", guilds_arr.len());
                        for g in guilds_arr {
                            if let Some(vs_arr) = g["voice_states"].as_array() {
                                info!("📌 [READY_SUPPLEMENTAL] Guild {} tem {} voice_states!", g["id"], vs_arr.len());
                                for vs in vs_arr {
                                    let u_id: u64 = if let Some(s) = vs["user_id"].as_str() {
                                        s.parse().unwrap_or(0)
                                    } else {
                                        vs["user_id"].as_u64().unwrap_or(0)
                                    };
                                    let c_str = if let Some(s) = vs["channel_id"].as_str() {
                                        s.to_string()
                                    } else if let Some(n) = vs["channel_id"].as_u64() {
                                        n.to_string()
                                    } else {
                                        String::new()
                                    };
                                    if u_id > 0 && !c_str.is_empty() {
                                        if let Ok(mut gvs) = get_guild_voice_states_store().lock() {
                                            gvs.insert(u_id, c_str.clone());
                                            info!("📌 [READY_SUPPLEMENTAL] Estado de voz pré-carregado: User {} -> Canal {}", u_id, c_str);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    let _ = self.event_tx.send(GatewayEvent::VoiceStatesUpdated).await;
                }
                "GUILD_MEMBERS_CHUNK" => {
                    if let Some(members_arr) = v["d"]["members"].as_array() {
                        for m in members_arr {
                            let uid_str = m["user"]["id"].as_str().unwrap_or("");
                            if let Ok(uid) = uid_str.parse::<u64>() {
                                let mut display_name = String::new();
                                if let Some(nick) = m["nick"].as_str() {
                                    if !nick.is_empty() { display_name = nick.to_string(); }
                                }
                                if display_name.is_empty() {
                                    if let Some(gname) = m["user"]["global_name"].as_str() {
                                        if !gname.is_empty() { display_name = gname.to_string(); }
                                    }
                                }
                                if display_name.is_empty() {
                                    if let Some(uname) = m["user"]["username"].as_str() {
                                        if !uname.is_empty() { display_name = uname.to_string(); }
                                    }
                                }
                                if !display_name.is_empty() {
                                    register_user_name(uid, display_name);
                                }
                            }
                        }
                    }
                }
                "VOICE_STATE_UPDATE" => {
                    let event_uid: u64 = if let Some(s) = v["d"]["user_id"].as_str() {
                        s.parse().unwrap_or(0)
                    } else {
                        v["d"]["user_id"].as_u64().unwrap_or(0)
                    };

                    if event_uid > 0 {
                        let mut display_name = String::new();
                        if let Some(nick) = v["d"]["member"]["nick"].as_str() {
                            if !nick.is_empty() { display_name = nick.to_string(); }
                        }
                        if display_name.is_empty() {
                            if let Some(gname) = v["d"]["member"]["user"]["global_name"].as_str() {
                                if !gname.is_empty() { display_name = gname.to_string(); }
                            }
                        }
                        if display_name.is_empty() {
                            if let Some(uname) = v["d"]["member"]["user"]["username"].as_str() {
                                if !uname.is_empty() { display_name = uname.to_string(); }
                            }
                        }

                        if !display_name.is_empty() {
                            register_user_name(event_uid, display_name.clone());
                            register_user_avatar(event_uid, v["d"]["member"]["user"]["avatar"].as_str().unwrap_or(""));
                            info!("VOICE_STATE_UPDATE: Registrado nome de usuário {} -> {}", event_uid, display_name);
                        }

                        let event_chan = if let Some(s) = v["d"]["channel_id"].as_str() {
                            Some(s.to_string())
                        } else if let Some(n) = v["d"]["channel_id"].as_u64() {
                            Some(n.to_string())
                        } else {
                            None
                        };

                        let previous_chan = if let Ok(mut gvs) = get_guild_voice_states_store().lock() {
                            if let Some(ref cid) = event_chan {
                                info!("📌 [VOICE_STATE_UPDATE] User {} -> Canal {}", event_uid, cid);
                                gvs.insert(event_uid, cid.clone())
                            } else {
                                gvs.remove(&event_uid)
                            }
                        } else {
                            None
                        };
                        let my_voice_chan = self.voice_channel_id.lock().unwrap().clone();

                        if let Some(ref my_cid) = my_voice_chan {
                            if event_chan.as_deref() == Some(my_cid.as_str()) {
                                info!("👥 [VOICE CHANNEL JOIN] Usuário {} ({}) entrou no nosso canal de voz!", event_uid, display_name);
                                register_voice_participant(event_uid as u32, event_uid);
                            } else {
                                // Voice states of every server arrive here: only someone who was
                                // in our channel left it.
                                if previous_chan.as_deref() == Some(my_cid.as_str()) {
                                    info!("👥 [VOICE CHANNEL LEAVE] Usuário {} saiu do nosso canal de voz!", event_uid);
                                }
                                remove_voice_participant_by_user_id(event_uid);
                            }
                        }

                        // Check if this is MY OWN voice state update
                        let my_uid = self.user_id.lock().unwrap().clone().unwrap_or_default();
                        let event_uid_str = event_uid.to_string();
                        if !my_uid.is_empty() && event_uid_str == my_uid {
                            let event_gid = v["d"]["guild_id"].as_str().map(|s| s.to_string());
                            let my_active_gid = self.voice_guild_id.lock().unwrap().clone();
                            let my_active_cid = self.voice_channel_id.lock().unwrap().clone();

                            if let Some(sid) = v["d"]["session_id"].as_str() {
                                info!("VOICE_STATE_UPDATE do meu usuário recebido! Session ID: {}, Channel: {:?}, Guild: {:?}, MyActiveGuild: {:?}", sid, event_chan, event_gid, my_active_gid);
                                if let Some(cid) = event_chan {
                                    *self.voice_session_id.lock().unwrap() = Some(sid.to_string());
                                    *self.voice_channel_id.lock().unwrap() = Some(cid.to_string());
                                    if let Some(ref gid) = event_gid {
                                        *self.voice_guild_id.lock().unwrap() = Some(gid.clone());
                                    }
                                    // Só reconectar ao voice gateway se não estivermos conectados
                                    if !is_connected_to_voice() {
                                        self.try_trigger_voice_connect();
                                    } else {
                                        info!("🔇 VOICE_STATE_UPDATE: já conectado ao voice gateway, apenas atualizando session_id (mute/unmute).");
                                    }
                                } else {
                                    let is_matching_guild = match (&my_active_gid, &event_gid) {
                                        (Some(my_g), Some(ev_g)) => my_g == ev_g,
                                        (None, None) => true,
                                        _ => my_active_gid.is_none(),
                                    };

                                    if my_active_cid.is_some() && is_matching_guild {
                                        if IS_WATCHDOG_RESETTING.swap(false, Ordering::SeqCst) {
                                            info!("🛡️ [VOICE_STATE_UPDATE] Reset interno do Watchdog em andamento. Limpando credenciais antigas sem desconectar a UI...");
                                            CURRENT_VOICE_SESSION_ID.fetch_add(1, Ordering::SeqCst);
                                            *self.voice_session_id.lock().unwrap() = None;
                                            *self.voice_token.lock().unwrap() = None;
                                            *self.voice_endpoint.lock().unwrap() = None;
                                        } else {
                                            info!("🚪 [VOICE_STATE_UPDATE] Desconectado da sala de voz da guild {:?}", my_active_gid);
                                            CURRENT_VOICE_SESSION_ID.fetch_add(1, Ordering::SeqCst);
                                            *self.voice_session_id.lock().unwrap() = None;
                                            *self.voice_token.lock().unwrap() = None;
                                            *self.voice_endpoint.lock().unwrap() = None;
                                            *self.voice_channel_id.lock().unwrap() = None;
                                            *self.voice_guild_id.lock().unwrap() = None;
                                            clear_voice_participants();
                                            let _ = self.event_tx.send(GatewayEvent::VoiceDisconnected).await;
                                        }
                                    } else if my_active_cid.is_none() {
                                        // Our own leave: the channel was already cleared locally.
                                        info!("🚪 [VOICE_STATE_UPDATE] Saída do canal de voz confirmada pelo Discord.");
                                    } else {
                                        info!("🛡️ [VOICE_STATE_UPDATE] Ignorando evento de desconexão de outra guild ({:?}) enquanto estamos em ({:?})", event_gid, my_active_gid);
                                    }
                                }
                            }
                        }
                        let _ = self.event_tx.send(GatewayEvent::VoiceStatesUpdated).await;
                    }
                }
                "VOICE_SERVER_UPDATE" => {
                    info!("VOICE_SERVER_UPDATE bruto recebido: {:?}", v["d"]);
                    let token = v["d"]["token"].as_str().unwrap_or("").to_string();
                    let guild_id = v["d"]["guild_id"].as_str()
                        .or_else(|| v["d"]["channel_id"].as_str())
                        .unwrap_or("")
                        .to_string();
                    let endpoint = v["d"]["endpoint"].as_str().unwrap_or("").to_string();

                    if !token.is_empty() && !endpoint.is_empty() {
                        info!("VOICE_SERVER_UPDATE processado com Sucesso! Endpoint: {}, Guild/Channel ID: {}", endpoint, guild_id);
                        *self.voice_token.lock().unwrap() = Some(token);
                        *self.voice_endpoint.lock().unwrap() = Some(endpoint);
                        if !guild_id.is_empty() {
                            let is_dm = v["d"]["guild_id"].is_null() || v["d"]["guild_id"].as_str().is_none();
                            let gid_to_store = if is_dm {
                                "@me".to_string()
                            } else {
                                guild_id
                            };
                            *self.voice_guild_id.lock().unwrap() = Some(gid_to_store);
                        }
                        self.try_trigger_voice_connect();
                    } else {
                        warn!("VOICE_SERVER_UPDATE recebido com campos ausentes ou nulos: token_empty={}, endpoint_empty={}", token.is_empty(), endpoint.is_empty());
                    }
                }
                "MESSAGE_CREATE" => {
                    let id = v["d"]["id"].as_str().unwrap_or("").to_string();
                    let channel_id = v["d"]["channel_id"].as_str().unwrap_or("").to_string();
                    let my_uid = self.user_id.lock().unwrap().clone().unwrap_or_default();
                    let author_id = v["d"]["author"]["id"].as_str().unwrap_or("");
                    let is_self = !my_uid.is_empty() && author_id == my_uid;

                    let author = format_discord_author(&v["d"]);
                    let (content, commands, content_lines, embed_content, embed_lines, embed_color, embed_footer, code_block, reply_author, reply_content, reply_command, links, buttons, attachments) = format_discord_message_parts(&v["d"]);
                    let created_at = v["d"]["timestamp"].as_str().unwrap_or("").to_string();
                    let avatar_url = crate::ui::message_grouping::avatar_url_for_message(&v["d"]);
                    let is_bot = v["d"]["author"]["bot"].as_bool().unwrap_or(false);

                    let _ = self.event_tx.send(GatewayEvent::MessageCreated {
                        id,
                        channel_id,
                        author,
                        author_id: if matches!(v["d"]["type"].as_i64().unwrap_or(0), 0 | 19) {
                            author_id.to_string()
                        } else {
                            String::new()
                        },
                        created_at,
                        avatar_url,
                        is_bot,
                        content,
                        commands,
                        content_lines,
                        embed_content,
                        embed_lines,
                        embed_color,
                        embed_footer,
                        code_block,
                        reply_author,
                        reply_content,
                        reply_command,
                        links,
                        buttons,
                        attachments,
                        is_self,
                    }).await;
                }
                "MESSAGE_UPDATE" => {
                    let id = v["d"]["id"].as_str().unwrap_or("").to_string();
                    let channel_id = v["d"]["channel_id"].as_str().unwrap_or("").to_string();
                    let (content, commands, content_lines, embed_content, embed_lines, embed_color, embed_footer, code_block, reply_author, reply_content, reply_command, links, buttons, attachments) = format_discord_message_parts(&v["d"]);

                    let _ = self.event_tx.send(GatewayEvent::MessageUpdated {
                        id,
                        channel_id,
                        content,
                        commands,
                        content_lines,
                        embed_content,
                        embed_lines,
                        embed_color,
                        embed_footer,
                        code_block,
                        reply_author,
                        reply_content,
                        reply_command,
                        links,
                        buttons,
                        attachments,
                    }).await;
                }
                "MESSAGE_DELETE" => {
                    let id = v["d"]["id"].as_str().unwrap_or("").to_string();
                    let channel_id = v["d"]["channel_id"].as_str().unwrap_or("").to_string();
                    let _ = self.event_tx.send(GatewayEvent::MessageDeleted { id, channel_id }).await;
                }
                "CALL_CREATE" | "CALL_UPDATE" => {
                    let channel_id = v["d"]["channel_id"].as_str().unwrap_or("").to_string();
                    let my_uid = self.user_id.lock().unwrap().clone().unwrap_or_default();
                    info!("📞 {} recebido bruto: {:?}", t, v["d"]);

                    // Extrai IDs dos usuários presentes nos voice_states da chamada
                    let voice_uids: Vec<String> = v["d"]["voice_states"]
                        .as_array()
                        .map(|arr| {
                            arr.iter().filter_map(|vs| {
                                vs["user_id"].as_str().map(|s| s.to_string())
                                    .or_else(|| vs["user_id"].as_u64().map(|n| n.to_string()))
                            }).collect()
                        })
                        .unwrap_or_default();

                    // Se eu mesmo já estiver na chamada deste canal (voice_channel_id já definido ou meu uid nos voice_states),
                    // eu sou o chamador ou já atendi, logo NÃO exibir modal de chamada recebida.
                    let am_i_already_in_call = {
                        let cur_vc = self.voice_channel_id.lock().unwrap().clone();
                        cur_vc == Some(channel_id.clone()) || (!my_uid.is_empty() && voice_uids.contains(&my_uid))
                    };

                    if !am_i_already_in_call && !channel_id.is_empty() {
                        let caller_name = v["d"]["voice_states"]
                            .as_array()
                            .and_then(|vs_list| {
                                vs_list.iter().find(|vs| {
                                    let uid_str = vs["user_id"].as_str().map(|s| s.to_string())
                                        .or_else(|| vs["user_id"].as_u64().map(|n| n.to_string()))
                                        .unwrap_or_default();
                                    !my_uid.is_empty() && uid_str != my_uid
                                })
                            })
                            .and_then(|vs| {
                                vs["member"]["user"]["global_name"].as_str()
                                    .or_else(|| vs["member"]["user"]["username"].as_str())
                                    .or_else(|| vs["user"]["global_name"].as_str())
                                    .or_else(|| vs["user"]["username"].as_str())
                                    .or_else(|| vs["nick"].as_str())
                            })
                            .unwrap_or("")
                            .to_string();

                        info!("📞 {} processado como chamada recebida! Canal: {}, Chamador preliminar: '{}'", t, channel_id, caller_name);
                        let _ = self.event_tx.send(GatewayEvent::IncomingCall {
                            channel_id,
                            caller_name,
                        }).await;
                    } else {
                        info!("📞 {} ignorado (já estou na chamada deste canal). Canal: {}", t, channel_id);
                    }
                }
                "CALL_DELETE" => {
                    let channel_id = v["d"]["channel_id"].as_str().unwrap_or("").to_string();
                    if !channel_id.is_empty() {
                        info!("📴 CALL_DELETE recebido no canal {}", channel_id);
                        let _ = self.event_tx.send(GatewayEvent::CallEnded { channel_id }).await;
                    }
                }
                "GUILD_CREATE" => {
                    let id = v["d"]["id"].as_str().unwrap_or("").to_string();
                    let name = v["d"]["name"].as_str().unwrap_or("Servidor").to_string();
                    let mut channels = Vec::new();

                    if let Some(chans_arr) = v["d"]["channels"].as_array() {
                        for ch in chans_arr {
                            let ch_id = ch["id"].as_str().unwrap_or("").to_string();
                            let ch_name = ch["name"].as_str().unwrap_or("canal").to_string();
                            let ch_type = ch["type"].as_u64().unwrap_or(0);

                            // type 0 = text, type 2 = voice, type 4 = category, type 5 = news, type 13 = stage, type 15 = forum
                            if ch_type == 0 || ch_type == 2 || ch_type == 4 || ch_type == 5 || ch_type == 13 || ch_type == 15 {
                                channels.push(ChannelData {
                                    id: ch_id,
                                    name: if ch_type == 4 { ch_name.to_uppercase() } else { ch_name },
                                    is_voice: ch_type == 2 || ch_type == 13,
                                    is_category: ch_type == 4,
                                    parent_id: ch["parent_id"].as_str().map(|s| s.to_string()),
                                    position: ch["position"].as_i64().unwrap_or(0),
                                });
                            }
                        }
                    }

                    // Parse members to cache user display names
                    if let Some(members_arr) = v["d"]["members"].as_array() {
                        for m in members_arr {
                            let uid_str = m["user"]["id"].as_str().unwrap_or("");
                            if let Ok(uid) = uid_str.parse::<u64>() {
                                let mut display_name = String::new();
                                if let Some(nick) = m["nick"].as_str() {
                                    if !nick.is_empty() { display_name = nick.to_string(); }
                                }
                                if display_name.is_empty() {
                                    if let Some(gname) = m["user"]["global_name"].as_str() {
                                        if !gname.is_empty() { display_name = gname.to_string(); }
                                    }
                                }
                                if display_name.is_empty() {
                                    if let Some(uname) = m["user"]["username"].as_str() {
                                        if !uname.is_empty() { display_name = uname.to_string(); }
                                    }
                                }
                                if !display_name.is_empty() {
                                    register_user_avatar(uid, m["user"]["avatar"].as_str().unwrap_or(""));
                                    register_user_name(uid, display_name);
                                }
                            }
                        }
                    }

                    // Parse voice_states to populate existing voice channel participants
                    if let Some(vs_arr) = v["d"]["voice_states"].as_array() {
                        for vs in vs_arr {
                            let uid: u64 = if let Some(s) = vs["user_id"].as_str() {
                                s.parse().unwrap_or(0)
                            } else {
                                vs["user_id"].as_u64().unwrap_or(0)
                            };
                            let cid_str = if let Some(s) = vs["channel_id"].as_str() {
                                s.to_string()
                            } else if let Some(n) = vs["channel_id"].as_u64() {
                                n.to_string()
                            } else {
                                String::new()
                            };

                            if uid > 0 && !cid_str.is_empty() {
                                if let Ok(mut gvs) = get_guild_voice_states_store().lock() {
                                    gvs.insert(uid, cid_str.clone());
                                    info!("📌 [GUILD_CREATE] Pré-carregado estado de voz: User {} -> Canal {}", uid, cid_str);
                                }
                                let mut display_name = String::new();
                                if let Some(nick) = vs["member"]["nick"].as_str() {
                                    if !nick.is_empty() { display_name = nick.to_string(); }
                                }
                                if display_name.is_empty() {
                                    if let Some(gname) = vs["member"]["user"]["global_name"].as_str() {
                                        if !gname.is_empty() { display_name = gname.to_string(); }
                                    }
                                }
                                if display_name.is_empty() {
                                    if let Some(uname) = vs["member"]["user"]["username"].as_str() {
                                        if !uname.is_empty() { display_name = uname.to_string(); }
                                    }
                                }
                                if !display_name.is_empty() {
                                    register_user_avatar(uid, vs["member"]["user"]["avatar"].as_str().unwrap_or(""));
                                    register_user_name(uid, display_name);
                                }
                            }
                        }
                    }

                    info!("Servidor carregado: '{}' ({} canais)", name, channels.len());

                    let guild = GuildData { id, name, channels };
                    let _ = self.event_tx.send(GatewayEvent::GuildLoaded { guild }).await;
                }
                _ => {}
            }
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_events_the_app_uses_are_parsed_whole_but_every_sequence_counts() {
        // Presence and typing are most of what a big account receives: skipped.
        let presence = r#"{"t":"PRESENCE_UPDATE","s":42,"op":0,"d":{"user":{"id":"1"},"status":"online","activities":[{"name":"x"}]}}"#;
        assert_eq!(peek(presence), Some(Peek { op: 0, seq: Some(42), wanted: false, permissions: false }));
        // Fields in any order, nested objects named like the head fields.
        let msg = r#"{"op":0,"d":{"t":"x","s":1,"op":9,"content":"hi"},"s":43,"t":"MESSAGE_CREATE"}"#;
        assert_eq!(peek(msg), Some(Peek { op: 0, seq: Some(43), wanted: true, permissions: false }));
        for t in HANDLED_EVENTS {
            let ev = format!(r#"{{"op":0,"s":1,"t":"{t}","d":{{}}}}"#);
            assert!(peek(&ev).unwrap().wanted, "{t}");
        }
        // Everything that is not a dispatch (hello, ack, reconnect...) is parsed.
        assert_eq!(peek(r#"{"op":11,"d":null,"s":null,"t":null}"#), Some(Peek { op: 11, seq: None, wanted: true, permissions: false }));
        assert_eq!(peek(r#"{"op":10,"d":{"heartbeat_interval":41250}}"#), Some(Peek { op: 10, seq: None, wanted: true, permissions: false }));
        assert_eq!(peek("not json"), None);
    }

    #[test]
    fn role_server_and_own_member_changes_name_the_server_to_check_again() {
        let role = r#"{"op":0,"s":5,"t":"GUILD_ROLE_UPDATE","d":{"guild_id":"10","role":{"id":"3","permissions":"0"}}}"#;
        assert!(peek(role).unwrap().permissions);
        assert!(!peek(role).unwrap().wanted, "only the ids are read");
        assert_eq!(permission_change(role, 7).as_deref(), Some("10"));
        let guild = r#"{"op":0,"s":6,"t":"GUILD_UPDATE","d":{"id":"11","owner_id":"7","roles":[]}}"#;
        assert_eq!(permission_change(guild, 7).as_deref(), Some("11"));
        let mine = r#"{"op":0,"s":7,"t":"GUILD_MEMBER_UPDATE","d":{"guild_id":"12","roles":["3"],"user":{"id":"7"}}}"#;
        assert_eq!(permission_change(mine, 7).as_deref(), Some("12"));
        let other = r#"{"op":0,"s":8,"t":"GUILD_MEMBER_UPDATE","d":{"guild_id":"12","roles":["3"],"user":{"id":"8"}}}"#;
        assert_eq!(permission_change(other, 7), None);
        let presence = r#"{"t":"PRESENCE_UPDATE","s":42,"op":0,"d":{"guild_id":"10"}}"#;
        assert!(!peek(presence).unwrap().permissions);
    }

    #[test]
    fn resume_url_keeps_the_query_of_the_first_connection() {
        assert_eq!(resume_url("wss://gateway-us-east1-b.discord.gg"), "wss://gateway-us-east1-b.discord.gg/?v=9&encoding=json");
        assert_eq!(resume_url("wss://gateway-us-east1-b.discord.gg/"), "wss://gateway-us-east1-b.discord.gg/?v=9&encoding=json");
    }

    #[test]
    fn only_session_ending_close_codes_drop_the_session() {
        // 1001: "Discord WebSocket requesting client reconnect."; 4000: unknown error.
        for code in [1000, 1001, 1006, 4000, 4001, 4002, 4003, 4005, 4008] {
            assert!(!close_ends_session(code), "{code}");
        }
        for code in [4004, 4007, 4009, 4010, 4011, 4012, 4013, 4014] {
            assert!(close_ends_session(code), "{code}");
        }
    }
}
