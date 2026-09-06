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
            let effective_gid = if voice_gid.is_empty() { voice_cid.clone() } else { voice_gid };
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

    pub async fn start(self: Arc<Self>, mut cmd_rx: mpsc::Receiver<GatewayCommand>) {
        // Spawn GatewayCommand listener loop (permanece vivo entre reconexões do WebSocket)
        let client_cmd = Arc::clone(&self);
        tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    GatewayCommand::UpdateVoiceState { guild_id, channel_id, self_mute, self_deaf } => {
                        *client_cmd.voice_self_mute.lock().unwrap() = self_mute;

                        let is_channel_change = {
                            let cur_gid = client_cmd.voice_guild_id.lock().unwrap();
                            let cur_cid = client_cmd.voice_channel_id.lock().unwrap();
                            *cur_gid != Some(guild_id.clone()) || *cur_cid != channel_id
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

                        let effective_gid_val = if guild_id.trim().is_empty() {
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
                                        let effective_gid = if target_gid.trim().is_empty() { serde_json::Value::Null } else { serde_json::json!(target_gid) };
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

        let url = "wss://gateway.discord.gg/?v=9&encoding=json";

        // Loop Infinito de Auto-Reconexão da Gateway
        loop {
            info!("Conectando à Gateway v9 do Discord...");

            match connect_async(url).await {
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

                    let last_seq_arc = Arc::new(std::sync::atomic::AtomicI64::new(-1));
                    let mut hb_task_handle: Option<tokio::task::JoinHandle<()>> = None;

                    // Read incoming Gateway messages loop
                    while let Some(msg_result) = read.next().await {
                        match msg_result {
                            Ok(Message::Text(text)) => {
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

                                            // Send Identify Payload (Opcode 2) for Discord User Tokens
                                            let identify = serde_json::json!({
                                                "op": 2,
                                                "d": {
                                                    "token": self.token,
                                                    "capabilities": 16381,
                                                    "properties": {
                                                        "os": "Windows",
                                                        "browser": "Chrome",
                                                        "device": "",
                                                        "system_locale": "pt-BR",
                                                        "browser_user_agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/127.0.0.0 Safari/537.36",
                                                        "browser_version": "127.0.0.0",
                                                        "os_version": "10.0.19045",
                                                        "referrer": "",
                                                        "referring_domain": "",
                                                        "referrer_current": "",
                                                        "referring_domain_current": "",
                                                        "release_channel": "stable",
                                                        "client_build_number": 320000,
                                                        "client_event_source": null
                                                    },
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
                                        _ => {
                                            self.handle_event(&value).await;
                                        }
                                    }
                                }
                            }
                            Ok(Message::Close(close_frame)) => {
                                warn!("Gateway fechou a conexão (Close Frame): {:?}", close_frame);
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
                            info!("VOICE_STATE_UPDATE: Registrado nome de usuário {} -> {}", event_uid, display_name);
                        }

                        let event_chan = if let Some(s) = v["d"]["channel_id"].as_str() {
                            Some(s.to_string())
                        } else if let Some(n) = v["d"]["channel_id"].as_u64() {
                            Some(n.to_string())
                        } else {
                            None
                        };

                        if let Ok(mut gvs) = get_guild_voice_states_store().lock() {
                            if let Some(ref cid) = event_chan {
                                gvs.insert(event_uid, cid.clone());
                                info!("📌 [VOICE_STATE_UPDATE] User {} -> Canal {}", event_uid, cid);
                            } else {
                                gvs.remove(&event_uid);
                            }
                        }
                        let my_voice_chan = self.voice_channel_id.lock().unwrap().clone();

                        if let Some(ref my_cid) = my_voice_chan {
                            if event_chan.as_deref() == Some(my_cid.as_str()) {
                                info!("👥 [VOICE CHANNEL JOIN] Usuário {} ({}) entrou no nosso canal de voz!", event_uid, display_name);
                                register_voice_participant(event_uid as u32, event_uid);
                            } else {
                                info!("👥 [VOICE CHANNEL LEAVE] Usuário {} saiu do nosso canal de voz!", event_uid);
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
                                    self.try_trigger_voice_connect();
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
                            *self.voice_guild_id.lock().unwrap() = Some(guild_id);
                        }
                        self.try_trigger_voice_connect();
                    } else {
                        warn!("VOICE_SERVER_UPDATE recebido com campos ausentes ou nulos: token_empty={}, endpoint_empty={}", token.is_empty(), endpoint.is_empty());
                    }
                }
                "MESSAGE_CREATE" => {
                    let id = v["d"]["id"].as_str().unwrap_or("").to_string();
                    let channel_id = v["d"]["channel_id"].as_str().unwrap_or("").to_string();
                    let author = format_discord_author(&v["d"]);
                    let (content, commands, content_lines, embed_content, embed_lines, embed_color, embed_footer, code_block, reply_author, reply_content, reply_command, links, buttons, attachments) = format_discord_message_parts(&v["d"]);
                    let timestamp = "Agora".to_string();

                    let _ = self.event_tx.send(GatewayEvent::MessageCreated {
                        id,
                        channel_id,
                        author,
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
                        timestamp,
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

