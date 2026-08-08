use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{sleep, Duration};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use futures_util::{SinkExt, StreamExt};
use log::{info, error, warn};
use tokio::net::UdpSocket;
use std::collections::VecDeque;
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce
};
use opus_rs::{OpusEncoder, Application};
use davey::{DaveSession, ProposalsOperationType};
use std::num::NonZeroU16;

#[derive(Debug, Clone)]
pub struct ChannelData {
    pub id: String,
    pub name: String,
    pub is_voice: bool,
}

#[derive(Debug, Clone)]
pub struct GuildData {
    pub id: String,
    pub name: String,
    pub channels: Vec<ChannelData>,
}

#[derive(Debug, Clone)]
pub enum GatewayEvent {
    Connected { user_tag: String },
    Disconnected { reason: String },
    MessageCreated {
        channel_id: String,
        author: String,
        content: String,
        timestamp: String,
    },
    GuildLoaded {
        guild: GuildData,
    },
}

#[derive(Debug, Clone)]
pub enum GatewayCommand {
    UpdateVoiceState {
        guild_id: String,
        channel_id: Option<String>,
        self_mute: bool,
        self_deaf: bool,
    },
}

#[derive(Serialize, Deserialize)]
struct HeartbeatPayload {
    op: u8,
    d: Option<u64>,
}

use std::sync::atomic::{AtomicU64, Ordering};

// Shared Microphone PCM Audio Queue (32-bit float PCM at 48000Hz)
pub static MIC_PCM_QUEUE: std::sync::OnceLock<Arc<std::sync::Mutex<VecDeque<f32>>>> = std::sync::OnceLock::new();
pub static CURRENT_VOICE_SESSION_ID: AtomicU64 = AtomicU64::new(0);

pub fn get_mic_pcm_queue() -> Arc<std::sync::Mutex<VecDeque<f32>>> {
    MIC_PCM_QUEUE.get_or_init(|| Arc::new(std::sync::Mutex::new(VecDeque::with_capacity(48000)))).clone()
}

pub fn format_discord_author(m: &Value) -> String {
    let author_obj = &m["author"];
    let name = author_obj["global_name"].as_str()
        .unwrap_or_else(|| author_obj["username"].as_str().unwrap_or("Unknown"));
    let is_bot = author_obj["bot"].as_bool().unwrap_or(false);

    if is_bot {
        format!("🤖 {} [BOT]", name)
    } else {
        name.to_string()
    }
}

pub fn format_discord_message(m: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();

    // 1. Raw Text Content
    if let Some(content) = m["content"].as_str() {
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            parts.push(trimmed.to_string());
        }
    }

    // 2. Embeds (Bot Messages, Webhooks, Links)
    if let Some(embeds) = m["embeds"].as_array() {
        for embed in embeds {
            let mut embed_parts: Vec<String> = Vec::new();

            if let Some(author_name) = embed["author"]["name"].as_str() {
                if !author_name.trim().is_empty() {
                    embed_parts.push(format!("📌 {}", author_name.trim()));
                }
            }

            if let Some(title) = embed["title"].as_str() {
                if !title.trim().is_empty() {
                    embed_parts.push(format!("🔹 {}", title.trim()));
                }
            }

            if let Some(desc) = embed["description"].as_str() {
                if !desc.trim().is_empty() {
                    embed_parts.push(desc.trim().to_string());
                }
            }

            if let Some(fields) = embed["fields"].as_array() {
                for field in fields {
                    let name = field["name"].as_str().unwrap_or("").trim();
                    let val = field["value"].as_str().unwrap_or("").trim();
                    if !name.is_empty() || !val.is_empty() {
                        embed_parts.push(format!("• {}: {}", name, val));
                    }
                }
            }

            if let Some(footer) = embed["footer"]["text"].as_str() {
                if !footer.trim().is_empty() {
                    embed_parts.push(format!("── {}", footer.trim()));
                }
            }

            if !embed_parts.is_empty() {
                parts.push(format!("📋 [EMBED]\n{}", embed_parts.join("\n")));
            }
        }
    }

    // 3. Attachments (Images, Files, Videos)
    if let Some(attachments) = m["attachments"].as_array() {
        for att in attachments {
            let filename = att["filename"].as_str().unwrap_or("arquivo");
            let url = att["url"].as_str().unwrap_or("");
            parts.push(format!("📎 [Anexo: {}] ({})", filename, url));
        }
    }

    // 4. Stickers
    if let Some(stickers) = m["sticker_items"].as_array() {
        for st in stickers {
            let st_name = st["name"].as_str().unwrap_or("Sticker");
            parts.push(format!("🎨 [Sticker: {}]", st_name));
        }
    }

    // Fallback if message is completely empty or system message
    if parts.is_empty() {
        let msg_type = m["type"].as_u64().unwrap_or(0);
        match msg_type {
            6 => "[Mensagem Fixada]".to_string(),
            7 => "[Novo membro entrou no servidor!]".to_string(),
            8 => "[Servidor Impulsionado!]".to_string(),
            _ => "[Conteúdo Mídia/Membro Especial]".to_string(),
        }
    } else {
        parts.join("\n")
    }
}

pub struct GatewayClient {
    token: String,
    event_tx: mpsc::Sender<GatewayEvent>,
    user_id: Arc<std::sync::Mutex<Option<String>>>,
    voice_session_id: Arc<std::sync::Mutex<Option<String>>>,
    voice_token: Arc<std::sync::Mutex<Option<String>>>,
    voice_endpoint: Arc<std::sync::Mutex<Option<String>>>,
    voice_guild_id: Arc<std::sync::Mutex<Option<String>>>,
    voice_channel_id: Arc<std::sync::Mutex<Option<String>>>,
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

        let prefix_len = token.len().min(12);
        info!("Token sanitizado (início): {}...", &token[..prefix_len]);

        Self {
            token,
            event_tx,
            user_id: Arc::new(std::sync::Mutex::new(None)),
            voice_session_id: Arc::new(std::sync::Mutex::new(None)),
            voice_token: Arc::new(std::sync::Mutex::new(None)),
            voice_endpoint: Arc::new(std::sync::Mutex::new(None)),
            voice_guild_id: Arc::new(std::sync::Mutex::new(None)),
            voice_channel_id: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    fn try_trigger_voice_connect(&self) {
        let user_id = self.user_id.lock().unwrap().clone();
        let voice_sid = self.voice_session_id.lock().unwrap().clone();
        let voice_tok = self.voice_token.lock().unwrap().clone();
        let voice_ep = self.voice_endpoint.lock().unwrap().clone();
        let voice_gid = self.voice_guild_id.lock().unwrap().clone();
        let voice_cid = self.voice_channel_id.lock().unwrap().clone().unwrap_or_default();

        if let (Some(uid), Some(sid), Some(tok), Some(ep), Some(gid)) = (user_id, voice_sid, voice_tok, voice_ep, voice_gid) {
            info!("Todas as credenciais de voz prontas! Conectando à Voice Gateway no endpoint {}...", ep);
            *self.voice_token.lock().unwrap() = None; // Reset so we only trigger once per payload

            tokio::spawn(async move {
                connect_voice_gateway(&ep, &gid, &uid, &sid, &tok, &voice_cid).await;
            });
        }
    }

    pub async fn start(self: Arc<Self>, mut cmd_rx: mpsc::Receiver<GatewayCommand>) {
        // Gateway v9 is required for User Account Tokens
        let url = "wss://gateway.discord.gg/?v=9&encoding=json";
        info!("Conectando à Gateway v9 do Discord...");

        match connect_async(url).await {
            Ok((ws_stream, _)) => {
                info!("Conexão WebSocket estabelecida com sucesso!");
                let (write, mut read) = ws_stream.split();
                let write_arc = Arc::new(Mutex::new(write));

                // Spawn GatewayCommand listener loop (OP 4 Voice State Update, etc)
                let write_cmd = Arc::clone(&write_arc);
                let client_cmd = Arc::clone(&self);
                tokio::spawn(async move {
                    while let Some(cmd) = cmd_rx.recv().await {
                        match cmd {
                            GatewayCommand::UpdateVoiceState { guild_id, channel_id, self_mute, self_deaf } => {
                                // Terminate any existing background UDP audio tasks
                                CURRENT_VOICE_SESSION_ID.fetch_add(1, Ordering::SeqCst);

                                // Reset voice session buffers for clean new connection
                                *client_cmd.voice_session_id.lock().unwrap() = None;
                                *client_cmd.voice_token.lock().unwrap() = None;
                                *client_cmd.voice_endpoint.lock().unwrap() = None;
                                *client_cmd.voice_guild_id.lock().unwrap() = Some(guild_id.clone());
                                *client_cmd.voice_channel_id.lock().unwrap() = channel_id.clone();

                                let payload = serde_json::json!({
                                    "op": 4,
                                    "d": {
                                        "guild_id": guild_id,
                                        "channel_id": channel_id,
                                        "self_mute": self_mute,
                                        "self_deaf": self_deaf
                                    }
                                });

                                info!("Enviando OP 4 VoiceStateUpdate à Gateway: {}", payload);
                                let mut w = write_cmd.lock().await;
                                if let Err(e) = w.send(Message::Text(payload.to_string().into())).await {
                                    warn!("Falha ao enviar Opcode 4 (VoiceStateUpdate): {:?}", e);
                                }
                            }
                        }
                    }
                });

                // Read incoming Gateway messages loop
                while let Some(msg_result) = read.next().await {
                    match msg_result {
                        Ok(Message::Text(text)) => {
                            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                let op = value["op"].as_u64().unwrap_or(99);

                                match op {
                                    10 => {
                                        // Opcode 10: HELLO
                                        let heartbeat_interval = value["d"]["heartbeat_interval"]
                                            .as_u64()
                                            .unwrap_or(41250);

                                        info!("Heartbeat interval recebido: {} ms", heartbeat_interval);

                                        // Send initial Heartbeat (Opcode 1)
                                        let hb_initial = serde_json::json!({ "op": 1, "d": null });
                                        {
                                            let mut w = write_arc.lock().await;
                                            let _ = w.send(Message::Text(hb_initial.to_string().into())).await;
                                        }

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
                                        {
                                            let mut w = write_arc.lock().await;
                                            if let Err(e) = w.send(Message::Text(identify.to_string().into())).await {
                                                error!("Erro ao enviar payload IDENTIFY: {:?}", e);
                                                return;
                                            }
                                        }

                                        // Spawn Heartbeat Loop
                                        let write_hb = Arc::clone(&write_arc);
                                        tokio::spawn(async move {
                                            loop {
                                                sleep(Duration::from_millis(heartbeat_interval)).await;
                                                let hb = serde_json::json!({ "op": 1, "d": null });
                                                let mut w = write_hb.lock().await;
                                                if let Err(e) = w.send(Message::Text(hb.to_string().into())).await {
                                                    warn!("Falha ao enviar Heartbeat: {:?}", e);
                                                    break;
                                                }
                                            }
                                        });
                                    }
                                    _ => {
                                        self.handle_event(&value).await;
                                    }
                                }
                            }
                        }
                        Ok(Message::Close(close_frame)) => {
                            warn!("Gateway fechou a conexão (Close Frame): {:?}", close_frame);
                            let reason = match close_frame {
                                Some(frame) => format!("Código {}: {}", frame.code, frame.reason),
                                None => "Conexão encerrada pelo servidor".to_string(),
                            };
                            let _ = self.event_tx.send(GatewayEvent::Disconnected { reason }).await;
                            break;
                        }
                        Err(e) => {
                            error!("Erro de leitura no WebSocket: {:?}", e);
                            break;
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => {
                error!("Falha ao conectar na Gateway do Discord: {:?}", e);
                let _ = self.event_tx.send(GatewayEvent::Disconnected {
                    reason: format!("Erro de conexão: {}", e)
                }).await;
            }
        }
    }

    async fn handle_event(&self, v: &Value) {
        if let Some(t) = v["t"].as_str() {
            match t {
                "READY" => {
                    let uid = v["d"]["user"]["id"].as_str().unwrap_or("").to_string();
                    *self.user_id.lock().unwrap() = Some(uid);

                    let username = v["d"]["user"]["username"].as_str().unwrap_or("User");
                    let global_name = v["d"]["user"]["global_name"].as_str().unwrap_or(username);
                    let discriminator = v["d"]["user"]["discriminator"].as_str().unwrap_or("0");
                    let user_tag = if discriminator == "0" {
                        global_name.to_string()
                    } else {
                        format!("{}#{}", global_name, discriminator)
                    };
                    info!("Login BEM-SUCEDIDO na Gateway! Usuário: {} (@{})", global_name, username);
                    let _ = self.event_tx.send(GatewayEvent::Connected { user_tag }).await;
                }
                "VOICE_STATE_UPDATE" => {
                    if let Some(sid) = v["d"]["session_id"].as_str() {
                        let my_uid = self.user_id.lock().unwrap().clone().unwrap_or_default();
                        let event_uid = v["d"]["user_id"].as_str().unwrap_or("");
                        if event_uid == my_uid || my_uid.is_empty() {
                            info!("VOICE_STATE_UPDATE recebido! Session ID: {}", sid);
                            *self.voice_session_id.lock().unwrap() = Some(sid.to_string());
                            self.try_trigger_voice_connect();
                        }
                    }
                }
                "VOICE_SERVER_UPDATE" => {
                    let token = v["d"]["token"].as_str().unwrap_or("").to_string();
                    let guild_id = v["d"]["guild_id"].as_str().unwrap_or("").to_string();
                    let endpoint = v["d"]["endpoint"].as_str().unwrap_or("").to_string();

                    if !token.is_empty() && !guild_id.is_empty() && !endpoint.is_empty() {
                        info!("VOICE_SERVER_UPDATE recebido! Endpoint: {}", endpoint);
                        *self.voice_token.lock().unwrap() = Some(token);
                        *self.voice_endpoint.lock().unwrap() = Some(endpoint);
                        *self.voice_guild_id.lock().unwrap() = Some(guild_id);
                        self.try_trigger_voice_connect();
                    }
                }
                "MESSAGE_CREATE" => {
                    let channel_id = v["d"]["channel_id"].as_str().unwrap_or("").to_string();
                    let author = format_discord_author(&v["d"]);
                    let content = format_discord_message(&v["d"]);
                    let timestamp = "Agora".to_string();

                    let _ = self.event_tx.send(GatewayEvent::MessageCreated {
                        channel_id,
                        author,
                        content,
                        timestamp,
                    }).await;
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

                            // type 0 = text, type 2 = voice
                            if ch_type == 0 || ch_type == 2 {
                                channels.push(ChannelData {
                                    id: ch_id,
                                    name: ch_name,
                                    is_voice: ch_type == 2,
                                });
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

pub async fn connect_voice_gateway(
    raw_endpoint: &str,
    guild_id: &str,
    user_id: &str,
    session_id: &str,
    token: &str,
    channel_id: &str,
) {
    let mut clean_endpoint = raw_endpoint.trim();
    if let Some(pos) = clean_endpoint.find(':') {
        clean_endpoint = &clean_endpoint[..pos];
    }
    let voice_url = format!("wss://{}/?v=4", clean_endpoint);
    info!("Conectando à Discord Voice Gateway v4: {}...", voice_url);

    let my_session_id = CURRENT_VOICE_SESSION_ID.fetch_add(1, Ordering::SeqCst) + 1;
    info!("Iniciando nova sessão de voz ID={}", my_session_id);

    match connect_async(&voice_url).await {
        Ok((ws_stream, _)) => {
            info!("Conexão WebSocket com Voice Gateway estabelecida!");
            let (write, mut read) = ws_stream.split();
            let write_arc = Arc::new(Mutex::new(write));

            let guild_id = guild_id.to_string();
            let user_id = user_id.to_string();
            let session_id = session_id.to_string();
            let token = token.to_string();
            let channel_id_str = channel_id.to_string();
            let active_ssrc: Arc<std::sync::Mutex<u32>> = Arc::new(std::sync::Mutex::new(12345));
            let secret_key_arc: Arc<std::sync::Mutex<Option<Vec<u8>>>> = Arc::new(std::sync::Mutex::new(None));

            // Create DAVE (Discord Audio/Video E2EE) session using the davey crate
            let uid_num: u64 = user_id.parse().unwrap_or(0);
            let cid_num: u64 = channel_id_str.parse().unwrap_or(0);
            let dave_session: Arc<std::sync::Mutex<Option<DaveSession>>> = Arc::new(std::sync::Mutex::new(
                DaveSession::new(NonZeroU16::new(1).unwrap(), uid_num, cid_num, None)
                    .map_err(|e| { warn!("Falha ao criar DaveSession: {:?}", e); })
                    .ok()
            ));

            // Read loop: wait for Opcode 8 HELLO from Voice Gateway before sending Opcode 0 Identify!
            while let Some(msg_result) = read.next().await {
                match msg_result {
                    Ok(Message::Text(text)) => {
                        if let Ok(val) = serde_json::from_str::<Value>(&text) {
                            let op = val["op"].as_u64().unwrap_or(99);
                            info!("Payload recebido da Voice Gateway: op={}", op);

                            match op {
                                8 => {
                                    // Voice Opcode 8: HELLO
                                    let heartbeat_interval = val["d"]["heartbeat_interval"]
                                        .as_u64()
                                        .unwrap_or(20000);
                                    info!("Voice Gateway HELLO recebido! Intervalo de Heartbeat: {} ms", heartbeat_interval);

                                    // 1. Send Opcode 0 Voice Identify (with max_dave_protocol_version: 1)
                                    let voice_identify = serde_json::json!({
                                        "op": 0,
                                        "d": {
                                            "server_id": guild_id,
                                            "user_id": user_id,
                                            "session_id": session_id,
                                            "token": token,
                                            "video": false,
                                            "streams": [],
                                            "max_dave_protocol_version": 1
                                        }
                                    });

                                    info!("Enviando OP 0 Voice Identify (DAVE v1) para a Voice Gateway...");
                                    {
                                        let mut w = write_arc.lock().await;
                                        if let Err(e) = w.send(Message::Text(voice_identify.to_string().into())).await {
                                            error!("Erro ao enviar Opcode 0 Voice Identify: {:?}", e);
                                            break;
                                        }
                                    }

                                    // 2. Spawn Voice Heartbeat loop (Opcode 3 with incremental nonces)
                                    let write_hb = Arc::clone(&write_arc);
                                    tokio::spawn(async move {
                                        let mut nonce: u64 = 1000;
                                        loop {
                                            if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != my_session_id { break; }
                                            sleep(Duration::from_millis(heartbeat_interval)).await;
                                            nonce += 1;
                                            let hb = serde_json::json!({ "op": 3, "d": nonce });
                                            let mut w = write_hb.lock().await;
                                            if let Err(_) = w.send(Message::Text(hb.to_string().into())).await {
                                                break;
                                            }
                                        }
                                    });
                                }
                                2 => {
                                    // Voice Opcode 2: READY!
                                    let ssrc = val["d"]["ssrc"].as_u64().unwrap_or(12345) as u32;
                                    *active_ssrc.lock().unwrap() = ssrc;

                                    let voice_ip = val["d"]["ip"].as_str().unwrap_or("").to_string();
                                    let voice_port = val["d"]["port"].as_u64().unwrap_or(0) as u16;

                                    let selected_mode = if let Some(modes) = val["d"]["modes"].as_array() {
                                        modes.iter()
                                            .find_map(|m| m.as_str())
                                            .unwrap_or("aead_aes256_gcm_rtpsize")
                                            .to_string()
                                    } else {
                                        "aead_aes256_gcm_rtpsize".to_string()
                                    };

                                    info!("🎉 VOICE GATEWAY PRONTA (Opcode 2 READY)! SSRC={}, IP={}:{}, Encryption Mode={}", ssrc, voice_ip, voice_port, selected_mode);

                                    // UDP IP Discovery & Opcode 1 Select Protocol Handshake
                                    if !voice_ip.is_empty() && voice_port > 0 {
                                        let write_arc_proto = Arc::clone(&write_arc);
                                        let secret_key_udp = Arc::clone(&secret_key_arc);
                                        let dave_session_audio = Arc::clone(&dave_session);

                                        tokio::spawn(async move {
                                            if let Ok(socket) = UdpSocket::bind("0.0.0.0:0").await {
                                                let target_addr = format!("{}:{}", voice_ip, voice_port);
                                                info!("Socket UDP de Voz conectado a {}", target_addr);

                                                // 1. Send 70-byte UDP IP Discovery Packet
                                                let mut discovery = [0u8; 70];
                                                discovery[0] = 0x00;
                                                discovery[1] = 0x01;
                                                discovery[2] = 0x00;
                                                discovery[3] = 0x46;
                                                let ssrc_bytes = ssrc.to_be_bytes();
                                                discovery[4..8].copy_from_slice(&ssrc_bytes);

                                                let mut my_pub_ip = String::new();
                                                let mut my_pub_port = 0u16;

                                                if let Ok(_) = socket.send_to(&discovery, &target_addr).await {
                                                    let mut buf = [0u8; 128];

                                                    if let Ok(Ok((len, _))) = tokio::time::timeout(Duration::from_secs(2), socket.recv_from(&mut buf)).await {
                                                        if len >= 70 {
                                                            let ip_slice = &buf[8..64.min(len)];
                                                            let ip_end = ip_slice.iter().position(|&b| b == 0).unwrap_or(ip_slice.len());
                                                            let parsed_ip = String::from_utf8_lossy(&ip_slice[..ip_end]).trim().to_string();
                                                            let parsed_port = u16::from_le_bytes([buf[68], buf[69]]);

                                                            if !parsed_ip.is_empty() && parsed_ip != "127.0.0.1" {
                                                                my_pub_ip = parsed_ip;
                                                                my_pub_port = parsed_port;
                                                            }
                                                        }
                                                    }
                                                }

                                                // Fallback: Fetch real public IP via HTTP API if UDP discovery timed out or returned local IP
                                                if my_pub_ip.is_empty() || my_pub_ip == "127.0.0.1" {
                                                    if let Ok(resp) = reqwest::get("https://api.ipify.org").await {
                                                        if let Ok(ip_text) = resp.text().await {
                                                            let clean_ip = ip_text.trim().to_string();
                                                            if !clean_ip.is_empty() {
                                                                my_pub_ip = clean_ip;
                                                                my_pub_port = 50000;
                                                            }
                                                        }
                                                    }
                                                }

                                                if my_pub_ip.is_empty() {
                                                    my_pub_ip = "127.0.0.1".to_string();
                                                    my_pub_port = 1337;
                                                }

                                                info!("UDP IP Discovery Concluído! IP Público Real: {}:{}", my_pub_ip, my_pub_port);

                                                // 2. Send Opcode 1 Select Protocol to Voice Gateway WebSocket
                                                let select_proto = serde_json::json!({
                                                    "op": 1,
                                                    "d": {
                                                        "protocol": "udp",
                                                        "data": {
                                                            "address": my_pub_ip,
                                                            "port": my_pub_port,
                                                            "mode": selected_mode
                                                        }
                                                    }
                                                });

                                                info!("Enviando Opcode 1 Select Protocol para a Voice Gateway...");
                                                {
                                                    let mut w = write_arc_proto.lock().await;
                                                    let _ = w.send(Message::Text(select_proto.to_string().into())).await;
                                                }

                                                // 3. Initialize Pure Rust OpusEncoder (48000Hz, Mono 1 channel, Application::Voip)
                                                let mut opus_encoder = OpusEncoder::new(48000, 1, Application::Voip)
                                                    .expect("Falha ao inicializar o OpusEncoder nativo em Rust");

                                                let pcm_queue = get_mic_pcm_queue();
                                                let mut seq: u16 = 0;
                                                let mut timestamp: u32 = 0;
                                                let mut nonce_cnt: u32 = 0;
                                                let mut opus_out = vec![0u8; 1000];
                                                let mut speaking_loop_counter: u32 = 0;

                                                // Wait for the secret key (received in op=4) before starting audio
                                                info!("Aguardando chave secreta (op=4) antes de iniciar o áudio...");
                                                loop {
                                                    if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != my_session_id { break; }
                                                    let has_key = secret_key_udp.lock().unwrap().is_some();
                                                    if has_key { break; }
                                                    sleep(Duration::from_millis(50)).await;
                                                }
                                                info!("Chave secreta recebida! Iniciando transmissão de áudio...");

                                                // Stream AES-256-GCM Encrypted Opus microphone audio RTP frames every 20ms over UDP
                                                let mut timer = tokio::time::interval(Duration::from_millis(20));
                                                timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                                                loop {
                                                    timer.tick().await;
                                                    if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != my_session_id {
                                                        info!("Sessão de voz antiga (ID={}) encerrada, saindo do loop UDP!", my_session_id);
                                                        break;
                                                    }
                                                    seq = seq.wrapping_add(1);
                                                    timestamp = timestamp.wrapping_add(960);
                                                    nonce_cnt = nonce_cnt.wrapping_add(1);

                                                    let mut audio_header = [0u8; 12];
                                                    audio_header[0] = 0x80; // RTP Version 2
                                                    audio_header[1] = 0x78; // Opus Payload 120
                                                    audio_header[2..4].copy_from_slice(&seq.to_be_bytes());
                                                    audio_header[4..8].copy_from_slice(&timestamp.to_be_bytes());
                                                    audio_header[8..12].copy_from_slice(&ssrc_bytes);

                                                    // Extract 960 f32 PCM samples (20ms of audio at 48000Hz) from microphone buffer
                                                    let mut pcm_frame = [0.0f32; 960];
                                                    let mut has_audio = false;
                                                    {
                                                        if let Ok(mut q) = pcm_queue.lock() {
                                                            if q.len() >= 960 {
                                                                for sample in pcm_frame.iter_mut() {
                                                                    *sample = q.pop_front().unwrap_or(0.0);
                                                                }
                                                                has_audio = true;
                                                            }
                                                        }
                                                    }

                                                    let opus_bytes: &[u8] = if has_audio {
                                                        if let Ok(encoded_len) = opus_encoder.encode(&pcm_frame, 960, &mut opus_out) {
                                                            &opus_out[..encoded_len]
                                                        } else {
                                                            &[0xF8, 0xFF, 0xFE] // Silence frame fallback
                                                        }
                                                    } else {
                                                        &[0xF8, 0xFF, 0xFE] // Silence frame fallback
                                                    };

                                                    let key_opt = secret_key_udp.lock().unwrap().clone();

                                                    // Apply DAVE frame encryption if session is ready
                                                    let mut dave_enc_ok = false;
                                                    let dave_encrypted: Option<Vec<u8>> = {
                                                        let mut sess = dave_session_audio.lock().unwrap();
                                                        if let Some(ref mut s) = *sess {
                                                            if s.is_ready() {
                                                                match s.encrypt_opus(opus_bytes) {
                                                                    Ok(cow) => {
                                                                        dave_enc_ok = true;
                                                                        Some(cow.into_owned())
                                                                    }
                                                                    Err(e) => {
                                                                        if speaking_loop_counter % 250 == 0 {
                                                                            warn!("DAVE: Falha em encrypt_opus: {:?}", e);
                                                                        }
                                                                        None
                                                                    }
                                                                }
                                                            } else { None }
                                                        } else { None }
                                                    };
                                                    let opus_payload: &[u8] = if let Some(ref v) = dave_encrypted { v } else { opus_bytes };

                                                    if let Some(key_bytes) = key_opt {
                                                        if let Ok(cipher) = Aes256Gcm::new_from_slice(&key_bytes) {
                                                            // 12-byte AES-GCM Nonce for aead_aes256_gcm_rtpsize:
                                                            // First 4 bytes = nonce counter (big-endian), last 8 bytes = zeros
                                                            let mut nonce_bytes = [0u8; 12];
                                                            nonce_bytes[0..4].copy_from_slice(&nonce_cnt.to_be_bytes());

                                                            let nonce = Nonce::from_slice(&nonce_bytes);

                                                            // Encrypt the (DAVE-encrypted) Opus payload with transport AES-256-GCM
                                                            let payload = Payload {
                                                                msg: opus_payload,
                                                                aad: &audio_header,
                                                            };

                                                            if let Ok(ciphertext) = cipher.encrypt(nonce, payload) {
                                                                let mut rtp_pkt = Vec::with_capacity(12 + ciphertext.len() + 4);
                                                                rtp_pkt.extend_from_slice(&audio_header);
                                                                rtp_pkt.extend_from_slice(&ciphertext);
                                                                // Append 4-byte nonce counter as suffix (rtpsize format)
                                                                rtp_pkt.extend_from_slice(&nonce_cnt.to_be_bytes());

                                                                if let Err(_) = socket.send_to(&rtp_pkt, &target_addr).await {
                                                                    break;
                                                                }

                                                                // Resend OP 5 Speaking and log audio stats every ~5 seconds (every 250 frames)
                                                                speaking_loop_counter = speaking_loop_counter.wrapping_add(1);
                                                                if speaking_loop_counter % 250 == 0 {
                                                                    let q_len = pcm_queue.lock().map(|q| q.len()).unwrap_or(0);
                                                                    info!("Transmissão de Áudio: frames_enviados={}, pcm_queue_buffer={}, has_audio={}, dave_encrypted={}",
                                                                        speaking_loop_counter, q_len, has_audio, dave_enc_ok);

                                                                    let speaking_pkt = serde_json::json!({
                                                                        "op": 5,
                                                                        "d": { "speaking": 1, "delay": 0, "ssrc": ssrc }
                                                                    });
                                                                    let mut w = write_arc_proto.lock().await;
                                                                    let _ = w.send(Message::Text(speaking_pkt.to_string().into())).await;
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        });
                                    }
                                }
                                4 => {
                                    // Voice Opcode 4: Session Description (Handshake Complete & Secret Key received!)
                                    let ssrc = *active_ssrc.lock().unwrap();
                                    info!("🎉 VOICE GATEWAY OPCODE 4 SESSION DESCRIPTION RECEBIDO! Sessão de Voz Ativada com Sucesso!");

                                    if let Some(key_arr) = val["d"]["secret_key"].as_array() {
                                        let key_bytes: Vec<u8> = key_arr.iter().filter_map(|v| v.as_u64().map(|n| n as u8)).collect();
                                        if key_bytes.len() == 32 {
                                            info!("Chave de criptografia AES-256 de 32 bytes configurada com SUCESSO para os pacotes de áudio!");
                                            *secret_key_arc.lock().unwrap() = Some(key_bytes);
                                        }
                                    }

                                    // Send Voice Opcode 5: Speaking
                                    let speaking_pkt = serde_json::json!({
                                        "op": 5,
                                        "d": {
                                            "speaking": 1,
                                            "delay": 0,
                                            "ssrc": ssrc
                                        }
                                    });

                                    info!("Enviando OP 5 Speaking para a Voice Gateway...");
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Text(speaking_pkt.to_string().into())).await;
                                }
                                6 => {
                                    // Voice Opcode 6: Heartbeat ACK!
                                    info!("Voice Gateway Heartbeat ACK (Opcode 6) recebido!");
                                }
                                18 => {
                                    // DAVE Opcode 18: DAVE_PREPARE_TRANSITION
                                    let transition_id = val["d"]["transition_id"].as_u64().unwrap_or(0);
                                    let protocol_version = val["d"]["protocol_version"].as_u64().unwrap_or(99);
                                    info!("DAVE Prepare Transition (op=18): transition_id={}, protocol_version={}, payload={}",
                                        transition_id, protocol_version, val["d"]);
                                    // Respond with op=21 (Execute Transition) to acknowledge readiness
                                    let ready = serde_json::json!({
                                        "op": 21,
                                        "d": { "transition_id": transition_id }
                                    });
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Text(ready.to_string().into())).await;
                                    info!("DAVE Execute Transition (op=21) enviado para transition_id={}", transition_id);
                                }
                                19 => {
                                    // DAVE Opcode 19: DAVE_EXECUTE_TRANSITION
                                    let transition_id = val["d"]["transition_id"].as_u64().unwrap_or(0);
                                    info!("DAVE Execute Transition (op=19): transition_id={}, payload={}", transition_id, val["d"]);
                                }
                                20 => {
                                    // DAVE Opcode 20: DAVE_TRANSITION_READY
                                    // DAVE Transition Ready (op=20): payload={}", val["d"]);
                                }
                                _ => {
                                    info!("Voice Gateway opcode JSON ignorado: op={}, data={}", op, val["d"]);
                                }
                            }
                        }
                    }
                    Ok(Message::Binary(data)) => {
                        if data.is_empty() { continue; }
                        let dave_op = data[0];
                        let payload = if data.len() > 1 { &data[1..] } else { &[][..] };

                        let preview: String = data[..data.len().min(16)].iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
                        info!("Voice Gateway BINARY: dave_op={}, {} bytes total, preview=[{}]", dave_op, data.len(), preview);

                        match dave_op {
                            25 => {
                                // dave_mls_external_sender_package (25): gateway's MLS credential
                                // Process it, then send our KeyPackage (opcode 26)
                                let key_pkg_bytes = {
                                    let mut sess = dave_session.lock().unwrap();
                                    if let Some(ref mut s) = *sess {
                                        match s.set_external_sender(payload) {
                                            Ok(()) => {
                                                info!("DAVE: External sender configurado com sucesso!");
                                                match s.create_key_package() {
                                                    Ok(kp) => { info!("DAVE: KeyPackage gerado ({} bytes)", kp.len()); Some(kp) }
                                                    Err(e) => { warn!("DAVE: Falha ao gerar KeyPackage: {:?}", e); None }
                                                }
                                            }
                                            Err(e) => { warn!("DAVE: Falha ao configurar external sender: {:?}", e); None }
                                        }
                                    } else { None }
                                };
                                if let Some(kp) = key_pkg_bytes {
                                    // Send key package as binary (opcode 26 = dave_mls_key_package)
                                    let mut pkt = vec![26u8];
                                    pkt.extend_from_slice(&kp);
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Binary(pkt.into())).await;
                                    info!("DAVE: KeyPackage (op=26) enviado à Voice Gateway!");
                                }
                            }
                            27 => {
                                // dave_mls_proposals (27): MLS proposals [op_type u8][VLBytes proposals]
                                let op_type = match data.get(1).copied().unwrap_or(0) {
                                    0 => ProposalsOperationType::APPEND,
                                    _ => ProposalsOperationType::REVOKE,
                                };
                                let proposals_data = if data.len() > 2 { &data[2..] } else { &[][..] };
                                info!("DAVE: Proposals (op=27) recebido, type={:?}", op_type);
                                let commit_bytes = {
                                    let mut sess = dave_session.lock().unwrap();
                                    if let Some(ref mut s) = *sess {
                                        match s.process_proposals(op_type, proposals_data, None) {
                                            Ok(Some(cw)) => {
                                                info!("DAVE: CommitWelcome gerado ({} commit bytes)", cw.commit.len());
                                                let mut out = cw.commit.clone();
                                                if let Some(w) = cw.welcome { out.extend_from_slice(&w); }
                                                Some(out)
                                            }
                                            Ok(None) => { info!("DAVE: process_proposals OK sem commit"); None }
                                            Err(e) => { warn!("DAVE: Falha ao processar proposals: {:?}", e); None }
                                        }
                                    } else { None }
                                };
                                if let Some(commit_data) = commit_bytes {
                                    // Send commit+welcome as binary (opcode 28 = dave_mls_commit_welcome)
                                    let mut pkt = vec![28u8];
                                    pkt.extend_from_slice(&commit_data);
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Binary(pkt.into())).await;
                                    info!("DAVE: CommitWelcome (op=28) enviado à Voice Gateway!");
                                }
                            }
                            29 => {
                                // dave_mls_announce_commit_transition (29): [op: u8 (29)][transition_id: u16 (2 bytes)][commit_bytes]
                                let transition_id = if data.len() >= 3 {
                                    u16::from_be_bytes([data[1], data[2]]) as u64
                                } else { 0 };
                                let commit_payload = if data.len() > 3 { &data[3..] } else { &[][..] };
                                info!("DAVE: AnnounceCommitTransition (op=29) recebido: transition_id={}", transition_id);
                                let mut sess = dave_session.lock().unwrap();
                                if let Some(ref mut s) = *sess {
                                    match s.process_commit(commit_payload) {
                                        Ok(()) => info!("DAVE: Commit processado com sucesso! is_ready={}", s.is_ready()),
                                        Err(e) => warn!("DAVE: Falha ao processar commit: {:?}", e),
                                    }
                                }
                            }
                            30 => {
                                // dave_mls_welcome (30): [op: u8 (30)][transition_id: u16 (2 bytes)][welcome_message]
                                let transition_id = if data.len() >= 3 {
                                    u16::from_be_bytes([data[1], data[2]]) as u64
                                } else { 0 };
                                let welcome_payload = if data.len() > 3 { &data[3..] } else { &[][..] };
                                info!("DAVE: Welcome (op=30) recebido! transition_id={}, payload len={}", transition_id, welcome_payload.len());
                                let is_active = {
                                    let mut sess = dave_session.lock().unwrap();
                                    if let Some(ref mut s) = *sess {
                                        match s.process_welcome(welcome_payload) {
                                            Ok(()) => {
                                                info!("🎉 DAVE: Welcome processado com SUCESSO! Sessão ATIVA! is_ready={}", s.is_ready());
                                                true
                                            }
                                            Err(e) => {
                                                warn!("DAVE: Falha ao processar welcome: {:?}", e);
                                                false
                                            }
                                        }
                                    } else { false }
                                };
                                if is_active {
                                    let ready = serde_json::json!({
                                        "op": 21,
                                        "d": { "transition_id": transition_id }
                                    });
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Text(ready.to_string().into())).await;
                                    info!("DAVE: Execute Transition (op=21) enviado para Welcome transition_id={}", transition_id);
                                }
                            }
                            _ => {
                                info!("DAVE: Opcode binário desconhecido: {}", dave_op);
                            }
                        }
                     }
                    Ok(Message::Close(frame)) => {
                        info!("Voice Gateway encerrada normalmente: {:?}", frame);
                        break;
                    }
                    Err(e) => {
                        warn!("Encerrando leitura da Voice Gateway: {:?}", e);
                        break;
                    }
                    _ => {}
                }
            }
        }
        Err(e) => {
            error!("Falha ao conectar na Voice Gateway: {:?}", e);
        }
    }
}
