#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::collections::{HashMap, VecDeque};
use std::num::NonZeroU16;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{sleep, Duration};
use tokio::net::UdpSocket;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use log::{info, error, warn};
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce
};
use opus_rs::{OpusEncoder, OpusDecoder, Application};
use davey::{DaveSession, ProposalsOperationType, MediaType};

use super::types::*;

pub async fn connect_voice_gateway(
    raw_endpoint: &str,
    guild_id: &str,
    user_id: &str,
    session_id: &str,
    token: &str,
    channel_id: &str,
    self_mute_state: Arc<std::sync::Mutex<bool>>,
    _event_tx: mpsc::Sender<GatewayEvent>,
) {
    let clean_endpoint = raw_endpoint.trim();
    let voice_url = if clean_endpoint.starts_with("wss://") || clean_endpoint.starts_with("ws://") {
        clean_endpoint.to_string()
    } else {
        format!("wss://{}/?v=4", clean_endpoint)
    };
    info!("Conectando Ã  Discord Voice Gateway: {}...", voice_url);

    let my_session_id = CURRENT_VOICE_SESSION_ID.fetch_add(1, Ordering::SeqCst) + 1;
    info!("Iniciando nova sessÃ£o de voz ID={}", my_session_id);
    let cid_num: u64 = channel_id.parse().unwrap_or(0);
    set_my_voice_channel_id(cid_num);

    let mut is_first_connect = true;
    loop {
        if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != my_session_id {
            break;
        }

        if !is_first_connect {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != my_session_id {
                break;
            }
            info!("🔄 [VOICE GATEWAY] Reconectando à Voice Gateway (sessão {})...", my_session_id);
        }
        is_first_connect = false;

        match connect_async(&voice_url).await {
            Ok((ws_stream, _)) => {
                info!("Conexão WebSocket com Voice Gateway estabelecida!");
                crate::screen_capture::invalidate_cached_stun_address();
                crate::screen_capture::force_signaling_broadcast();
                let (write, mut read) = ws_stream.split();
                let write_arc = Arc::new(Mutex::new(write));

                let guild_id = guild_id.to_string();
                let user_id = user_id.to_string();
                let session_id = session_id.to_string();
                let token = token.to_string();
                let channel_id_str = channel_id.to_string();
                sync_voice_channel_participants(&channel_id_str);
                let active_ssrc: Arc<std::sync::Mutex<u32>> = Arc::new(std::sync::Mutex::new(12345));
                let ssrc_to_userid: Arc<std::sync::Mutex<HashMap<u32, u64>>> = Arc::new(std::sync::Mutex::new(HashMap::new()));
                let secret_key_arc: Arc<std::sync::Mutex<Option<Vec<u8>>>> = Arc::new(std::sync::Mutex::new(None));

                // Create DAVE (Discord Audio/Video E2EE) session using the davey crate
                let uid_num: u64 = user_id.parse().unwrap_or(0);
                let cid_num: u64 = channel_id_str.parse().unwrap_or(0);
                let dave_session: Arc<std::sync::Mutex<Option<DaveSession>>> = Arc::new(std::sync::Mutex::new(
                    DaveSession::new(NonZeroU16::new(1).unwrap(), uid_num, cid_num, None)
                        .map_err(|e| { warn!("Falha ao criar DaveSession: {:?}", e); })
                        .ok()
                ));
                let saved_external_sender: Arc<std::sync::Mutex<Option<Vec<u8>>>> = Arc::new(std::sync::Mutex::new(None));

                // Read loop: wait for Opcode 8 HELLO from Voice Gateway before sending Opcode 0 Identify!
                while let Some(msg_result) = read.next().await {
                    if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != my_session_id {
                        break;
                    }
                    match msg_result {
                        Ok(Message::Text(text)) => {
                            if let Ok(val) = serde_json::from_str::<Value>(&text) {
                                let op = val["op"].as_u64().unwrap_or(99);
                                info!("Payload recebido da Voice Gateway: op={}", op);

                            match op {
                                8 => {
                                    // Voice Opcode 8: HELLO
                                    let raw_interval = val["d"]["heartbeat_interval"]
                                        .as_f64()
                                        .unwrap_or(20000.0);
                                    let heartbeat_interval = (raw_interval * 0.75).max(1000.0) as u64;
                                    info!("Voice Gateway HELLO recebido! Intervalo de Heartbeat configurado: {} ms (servidor={}, fator=0.75)", heartbeat_interval, raw_interval);

                                    // 1. Send Opcode 0 Voice Identify (with max_dave_protocol_version: 1 for E2EE DAVE support)
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

                                    info!("Enviando OP 0 Voice Identify para a Voice Gateway: {}", voice_identify);
                                    {
                                        let mut w = write_arc.lock().await;
                                        if let Err(e) = w.send(Message::Text(voice_identify.to_string().into())).await {
                                            error!("Erro ao enviar Opcode 0 Voice Identify: {:?}", e);
                                            break;
                                        }
                                    }

                                    // 2. Spawn Voice Heartbeat loop (Opcode 3 with epoch timestamp)
                                    let write_hb = Arc::clone(&write_arc);
                                    tokio::spawn(async move {
                                        loop {
                                            if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != my_session_id { break; }
                                            sleep(Duration::from_millis(heartbeat_interval)).await;
                                            let t_now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
                                            let hb = serde_json::json!({ "op": 3, "d": t_now });
                                            let mut w = write_hb.lock().await;
                                            if let Err(e) = w.send(Message::Text(hb.to_string().into())).await {
                                                warn!("⚠️ Falha ao enviar Heartbeat de voz (op=3): {:?}", e);
                                                break;
                                            }
                                        }
                                    });
                                }
                                2 => {
                                    // Voice Opcode 2: READY!
                                    set_is_connected_to_voice(true);
                                    let ssrc = val["d"]["ssrc"].as_u64().unwrap_or(12345) as u32;
                                    *active_ssrc.lock().unwrap() = ssrc;
                                    ssrc_to_userid.lock().unwrap().insert(ssrc, uid_num);
                                    register_voice_participant(ssrc, uid_num);
                                    register_voice_participant(999999, 999999);

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

                                    // Synthesize Join Voice Chime (523.25Hz C5 -> 659.25Hz E5) to test local audio playback
                                    let mut chime_samples = Vec::new();
                                    let sr = 48000.0f32;
                                    for i in 0..(0.12 * sr) as usize {
                                        let t = i as f32 / sr;
                                        let s = (t * 523.25 * 2.0 * std::f32::consts::PI).sin() * 0.4;
                                        chime_samples.push(s);
                                    }
                                    for i in 0..(0.18 * sr) as usize {
                                        let t = i as f32 / sr;
                                        let s = (t * 659.25 * 2.0 * std::f32::consts::PI).sin() * 0.4;
                                        chime_samples.push(s);
                                    }

                                    if let Ok(mut queues) = get_speaker_pcm_queues().lock() {
                                        let q = queues.entry(999999).or_insert_with(|| VecDeque::with_capacity(48000));
                                        q.clear();
                                        for &s in &chime_samples {
                                            q.push_back((s, s));
                                        }
                                    }
                                    info!("🎵 Efeito sonoro de entrada no canal de voz injetado na fila dos alto-falantes!");

                                    // UDP IP Discovery & Opcode 1 Select Protocol Handshake
                                    if !voice_ip.is_empty() && voice_port > 0 {
                                        let write_arc_proto = Arc::clone(&write_arc);
                                        let secret_key_udp = Arc::clone(&secret_key_arc);
                                        let dave_session_audio = Arc::clone(&dave_session);
                                        let ssrc_to_userid_audio = Arc::clone(&ssrc_to_userid);
                                        let self_mute_rx = Arc::clone(&self_mute_state);

                                        tokio::spawn(async move {
                                            let socket = match UdpSocket::bind("0.0.0.0:0").await {
                                                Ok(s) => Arc::new(s),
                                                Err(e) => { warn!("Falha ao criar socket UDP de voz: {:?}", e); return; }
                                            };
                                                let target_addr = format!("{}:{}", voice_ip, voice_port);
                                                info!("Socket UDP de Voz conectado a {}", target_addr);

                                                // 1. Send 74-byte UDP IP Discovery Packet (RFC / Discord spec: 2 bytes type, 2 bytes len, 4 bytes ssrc, 64 bytes addr, 2 bytes port)
                                                let mut discovery = [0u8; 74];
                                                discovery[0..2].copy_from_slice(&1u16.to_be_bytes());
                                                discovery[2..4].copy_from_slice(&70u16.to_be_bytes());
                                                discovery[4..8].copy_from_slice(&ssrc.to_be_bytes());

                                                let mut my_pub_ip = String::new();
                                                let mut my_pub_port = 0u16;

                                                for attempt in 1..=5 {
                                                    if socket.send_to(&discovery, &target_addr).await.is_ok() {
                                                        let mut buf = [0u8; 128];
                                                        if let Ok(Ok((len, _))) = tokio::time::timeout(Duration::from_millis(600), socket.recv_from(&mut buf)).await {
                                                            if len >= 70 {
                                                                let ip_slice = &buf[8..len.saturating_sub(2)];
                                                                let ip_end = ip_slice.iter().position(|&b| b == 0).unwrap_or(ip_slice.len());
                                                                let parsed_ip = String::from_utf8_lossy(&ip_slice[..ip_end]).trim().to_string();
                                                                let parsed_port = u16::from_be_bytes([buf[len - 2], buf[len - 1]]);

                                                                if !parsed_ip.is_empty() && parsed_port > 0 {
                                                                    my_pub_ip = parsed_ip;
                                                                    my_pub_port = parsed_port;
                                                                    info!("UDP IP Discovery resolvido com sucesso na tentativa {}: {}:{}", attempt, my_pub_ip, my_pub_port);
                                                                    break;
                                                                }
                                                            }
                                                        }
                                                    }
                                                    warn!("Tentativa {} de UDP IP Discovery falhou/timeout, tentando novamente...", attempt);
                                                }

                                                if my_pub_ip.is_empty() {
                                                    // Fallback to fetching public IP via HTTP if UDP discovery failed completely
                                                    if let Ok(res) = reqwest::get("https://api.ipify.org").await {
                                                        if let Ok(ip_text) = res.text().await {
                                                            let ip_trimmed = ip_text.trim().to_string();
                                                            if !ip_trimmed.is_empty() {
                                                                my_pub_ip = ip_trimmed;
                                                                my_pub_port = socket.local_addr().map(|a| a.port()).unwrap_or(50000);
                                                                warn!("Usando fallback HTTP para IP público: {}:{}", my_pub_ip, my_pub_port);
                                                            }
                                                        }
                                                    }
                                                }

                                                if my_pub_ip.is_empty() {
                                                    my_pub_ip = voice_ip.clone();
                                                    my_pub_port = socket.local_addr().map(|a| a.port()).unwrap_or(50000);
                                                    error!("CRÍTICO: Não foi possível determinar o IP público local!");
                                                }

                                                info!("UDP IP Discovery Concluído! IP: {}:{}", my_pub_ip, my_pub_port);

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

                                                // 3. Initialize Pure Rust OpusEncoder (48000Hz, Stereo 2 channels, Application::Voip)
                                                let mut opus_encoder = OpusEncoder::new(48000, 2, Application::Voip)
                                                    .expect("Falha ao inicializar o OpusEncoder nativo em Rust");

                                                // 4. Spawn incoming UDP voice receive loop
                                                let socket_rx = Arc::clone(&socket);
                                                let secret_key_rx = Arc::clone(&secret_key_udp);
                                                let dave_session_rx = Arc::clone(&dave_session_audio);
                                                let ssrc_to_userid_rx = Arc::clone(&ssrc_to_userid_audio);
                                                let speaker_queues_rx = get_speaker_pcm_queues();
                                                let my_ssrc = ssrc;
                                                let rx_session_id = my_session_id;
                                                tokio::spawn(async move {
                                                    let mut opus_decoders: HashMap<(u32, usize), OpusDecoder> = HashMap::new();
                                                    let mut ssrc_last_pkt_time: HashMap<u32, std::time::Instant> = HashMap::new();
                                                    let mut ssrc_expected_seq: HashMap<u32, u16> = HashMap::new();
                                                    let mut recv_buf = vec![0u8; 4096];
                                                    let mut pcm_out_buf = vec![0.0f32; 11520];
                                                    let mut detected_ssrcs = std::collections::HashSet::new();
                                                    let mut total_pkts_recv = 0u64;
                                                    let mut decrypt_err_cnt = 0u64;
                                                    let mut opus_err_cnt = 0u64;
                                                    let mut dave_decrypt_fail_cnt = 0u64;
                                                    let mut _dave_not_ready_cnt = 0u64;

                                                    info!("🎧 Loop UDP de recepção de voz INICIADO (Session ID={})!", rx_session_id);

                                                    loop {
                                                        if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != rx_session_id { break; }
                                                        match tokio::time::timeout(Duration::from_millis(100), socket_rx.recv_from(&mut recv_buf)).await {
                                                            Ok(Ok((len, addr))) => {
                                                                if len < 12 { continue; }
                                                                let pkt = &recv_buf[..len];
                                                                
                                                                // Parse RTP header
                                                                let version = (pkt[0] >> 6) & 0x3;
                                                                if version != 2 { continue; }
                                                                let pt = pkt[1] & 0x7F;
                                                                if pt != 120 { continue; } // Strictly allow ONLY Opus Audio packets (Payload Type 120)

                                                                let ssrc_recv = u32::from_be_bytes([pkt[8], pkt[9], pkt[10], pkt[11]]);
                                                                if ssrc_recv == my_ssrc { continue; }

                                                                total_pkts_recv += 1;

                                                                if detected_ssrcs.insert(ssrc_recv) {
                                                                    info!("🎙️ NOVO SSRC DE VOZ REMOTO RECEBIDO! SSRC={} de {}", ssrc_recv, addr);
                                                                }

                                                                 let cc = (pkt[0] & 0x0F) as usize;
                                                                let has_ext = (pkt[0] & 0x10) != 0;
                                                                let base_header_len = 12 + 4 * cc;

                                                                let ext_len_words = if has_ext && len >= base_header_len + 4 {
                                                                    u16::from_be_bytes([pkt[base_header_len + 2], pkt[base_header_len + 3]]) as usize
                                                                } else {
                                                                    0
                                                                };
                                                                let ext_bytes_len = ext_len_words * 4;

                                                                // rtpsize nonce: last 4 bytes of packet, padding byte before nonce if P bit set
                                                                let has_padding = (pkt[0] & 0x20) != 0;
                                                                let padding_len = if has_padding && len > 5 {
                                                                    pkt[len - 5] as usize
                                                                } else {
                                                                    0
                                                                };

                                                                let ciphertext_end = if len >= 4 + padding_len && len - 4 - padding_len > base_header_len {
                                                                    len - 4 - padding_len
                                                                } else {
                                                                    len - 4
                                                                };

                                                                if ciphertext_end <= base_header_len { continue; }

                                                                let nonce_bytes_rx: [u8; 4] = [pkt[len-4], pkt[len-3], pkt[len-2], pkt[len-1]];
                                                                let nonce_u32_le = u32::from_le_bytes(nonce_bytes_rx);
                                                                let nonce_u32_be = u32::from_be_bytes(nonce_bytes_rx);

                                                                let mut n1 = [0u8; 12]; n1[0..4].copy_from_slice(&nonce_bytes_rx);
                                                                let mut n2 = [0u8; 12]; n2[8..12].copy_from_slice(&nonce_bytes_rx);
                                                                let mut n3 = [0u8; 12]; n3[0..4].copy_from_slice(&nonce_u32_le.to_be_bytes());
                                                                let mut n4 = [0u8; 12]; n4[8..12].copy_from_slice(&nonce_u32_le.to_be_bytes());
                                                                let mut n5 = [0u8; 12]; n5[0..4].copy_from_slice(&nonce_u32_be.to_le_bytes());
                                                                let mut n6 = [0u8; 12]; n6[8..12].copy_from_slice(&nonce_u32_be.to_le_bytes());

                                                                let nonce_candidates = [&n1, &n2, &n3, &n4, &n5, &n6];

                                                                let key_opt = secret_key_rx.lock().unwrap().clone();
                                                                if let Some(key_bytes) = key_opt {
                                                                    if let Ok(cipher) = Aes256Gcm::new_from_slice(&key_bytes) {
                                                                        let candidates: &[(usize, usize)] = if has_ext {
                                                                            &[
                                                                                (base_header_len + 4, ext_bytes_len),
                                                                                (base_header_len + 4 + ext_bytes_len, 0),
                                                                                (base_header_len, ext_bytes_len + 4),
                                                                                (base_header_len, 0),
                                                                            ]
                                                                        } else {
                                                                            &[(base_header_len, 0)]
                                                                        };

                                                                        let mut decrypted_opt: Option<(Vec<u8>, usize)> = None;

                                                                        'try_decrypt: for &(test_aad_len, test_ext_len) in candidates {
                                                                            if test_aad_len >= ciphertext_end { continue; }
                                                                            let header = &pkt[..test_aad_len];
                                                                            let ciphertext = &pkt[test_aad_len..ciphertext_end];

                                                                            for &nonce_cand in &nonce_candidates {
                                                                                let payload = aes_gcm::aead::Payload { msg: ciphertext, aad: header };
                                                                                if let Ok(dec) = cipher.decrypt(Nonce::from_slice(nonce_cand), payload) {
                                                                                    if dec.len() >= test_ext_len {
                                                                                        decrypted_opt = Some((dec, test_ext_len));
                                                                                        break 'try_decrypt;
                                                                                    }
                                                                                }
                                                                            }
                                                                        }

                                                                        let (decrypted_raw, ext_skip) = match decrypted_opt {
                                                                            Some(res) => {
                                                                                static LOGGED_SUCCESS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                                                                                if !LOGGED_SUCCESS.swap(true, std::sync::atomic::Ordering::Relaxed) {
                                                                                    info!("🎉 Descriptografia AES-GCM BEM-SUCEDIDA para SSRC {}! ext_skip={} bytes", ssrc_recv, res.1);
                                                                                }
                                                                                res
                                                                            },
                                                                            None => {
                                                                                decrypt_err_cnt += 1;
                                                                                if decrypt_err_cnt % 50 == 1 {
                                                                                    warn!("Descriptografia AES-GCM falhou para SSRC {}: len={} has_ext={} cc={} base_len={} ext_words={} header={:02X?} nonce={:02X?}",
                                                                                        ssrc_recv, len, has_ext, cc, base_header_len, ext_len_words, &pkt[..16.min(len)], nonce_bytes_rx);
                                                                                }
                                                                                continue;
                                                                            }
                                                                        };

                                                                        if decrypted_raw.len() < ext_skip { continue; }
                                                                        let transport_payload = &decrypted_raw[ext_skip..];

                                                                        let user_id_opt = ssrc_to_userid_rx.lock().unwrap().get(&ssrc_recv).copied();
                                                                        let sender_user_id = user_id_opt.unwrap_or(ssrc_recv as u64);

                                                                        let (opus_data, can_decode) = {
                                                                            let mut sess = dave_session_rx.lock().unwrap();
                                                                            if let Some(ref mut s) = *sess {
                                                                                if s.is_ready() {
                                                                                    let res = s.decrypt(sender_user_id, MediaType::AUDIO, transport_payload);
                                                                                    let res = match res {
                                                                                        Ok(d) => Ok(d),
                                                                                        Err(_) => {
                                                                                            let swapped_uid = u64::from_be_bytes(sender_user_id.to_le_bytes());
                                                                                            s.decrypt(swapped_uid, MediaType::AUDIO, transport_payload)
                                                                                        }
                                                                                    };
                                                                                    let res = match res {
                                                                                        Ok(d) => Ok(d),
                                                                                        Err(_) => {
                                                                                            if let Some(uids) = s.get_user_ids() {
                                                                                                let mut found = None;
                                                                                                for &alt_uid in &uids {
                                                                                                    if alt_uid != s.user_id() {
                                                                                                        if let Ok(d) = s.decrypt(alt_uid, MediaType::AUDIO, transport_payload) {
                                                                                                            found = Some((d, alt_uid));
                                                                                                            break;
                                                                                                        }
                                                                                                    }
                                                                                                }
                                                                                                if let Some((d, matched_uid)) = found {
                                                                                                    if let Ok(mut m) = ssrc_to_userid_rx.lock() {
                                                                                                        m.insert(ssrc_recv, matched_uid);
                                                                                                    }
                                                                                                    register_voice_participant(ssrc_recv, matched_uid);
                                                                                                    Ok(d)
                                                                                                } else {
                                                                                                    Err(davey::errors::DecryptError::NoDecryptorForUser)
                                                                                                }
                                                                                            } else {
                                                                                                Err(davey::errors::DecryptError::NoDecryptorForUser)
                                                                                            }
                                                                                        }
                                                                                    };

                                                                                    match res {
                                                                                        Ok(d) => (d, true),
                                                                                        Err(e) => {
                                                                                            dave_decrypt_fail_cnt += 1;
                                                                                            if dave_decrypt_fail_cnt % 50 == 1 {
                                                                                                warn!("🔑 [DAVE DIAG] decrypt() FALHOU para SSRC={} UserID={} (payload_len={}, group_users={:?}): {:?}",
                                                                                                    ssrc_recv, sender_user_id, transport_payload.len(), s.get_user_ids(), e);
                                                                                            }
                                                                                            (Vec::new(), false)
                                                                                        }
                                                                                    }
                                                                                } else {
                                                                                    _dave_not_ready_cnt += 1;
                                                                                    let has_dave_magic = transport_payload.len() >= 4 && transport_payload.ends_with(&[0xFA, 0xFA]);
                                                                                    if has_dave_magic {
                                                                                        (Vec::new(), false)
                                                                                    } else {
                                                                                        (transport_payload.to_vec(), true)
                                                                                    }
                                                                                }
                                                                            } else {
                                                                                let has_dave_magic = transport_payload.len() >= 4 && transport_payload.ends_with(&[0xFA, 0xFA]);
                                                                                if has_dave_magic {
                                                                                    (Vec::new(), false)
                                                                                } else {
                                                                                    (transport_payload.to_vec(), true)
                                                                                }
                                                                            }
                                                                        };

                                                                        let mut decode_success = false;
                                                                        let mut decoded_count = 0;
                                                                        let mut plc_pairs: Vec<(f32, f32)> = Vec::new();
                                                                        let mut pkt_channels = 2usize;
                                                                        if can_decode && !opus_data.is_empty() {
                                                                            let mut raw_opus = opus_data.as_slice();

                                                                            if raw_opus.len() >= 4 && (raw_opus.starts_with(&[0xBE, 0xDE]) || raw_opus.starts_with(&[0x10, 0x00])) {
                                                                                let ext_words = u16::from_be_bytes([raw_opus[2], raw_opus[3]]) as usize;
                                                                                let ext_total_bytes = 4 + ext_words * 4;
                                                                                if raw_opus.len() > ext_total_bytes {
                                                                                    raw_opus = &raw_opus[ext_total_bytes..];
                                                                                }
                                                                            }

                                                                            if raw_opus.first() == Some(&0x00) && raw_opus.len() > 1 {
                                                                                raw_opus = &raw_opus[1..];
                                                                            }

                                                                            if (pkt[0] & 0x20) != 0 && !raw_opus.is_empty() {
                                                                                let pad_len = raw_opus[raw_opus.len() - 1] as usize;
                                                                                if pad_len > 0 && pad_len <= raw_opus.len() {
                                                                                    raw_opus = &raw_opus[..raw_opus.len() - pad_len];
                                                                                }
                                                                            }

                                                                            if raw_opus.is_empty() { continue; }

                                                                            pkt_channels = if (raw_opus[0] & 0x04) != 0 { 2 } else { 1 };
                                                                            let dec = opus_decoders.entry((ssrc_recv, pkt_channels)).or_insert_with(|| {
                                                                                OpusDecoder::new(48000, pkt_channels).expect("Falha ao inicializar OpusDecoder 48kHz")
                                                                            });

                                                                            let now = std::time::Instant::now();
                                                                            let rtp_seq = u16::from_be_bytes([pkt[2], pkt[3]]);
                                                                            let last_time_opt = ssrc_last_pkt_time.get(&ssrc_recv).copied();
                                                                            let is_new_talkspurt = match last_time_opt {
                                                                                Some(t) => now.duration_since(t) > Duration::from_millis(100),
                                                                                None => true,
                                                                            };

                                                                            if is_new_talkspurt {
                                                                                // New burst of speech after silence/VAD gap: sync sequence and skip false PLC to avoid pops
                                                                                ssrc_expected_seq.insert(ssrc_recv, rtp_seq.wrapping_add(1));
                                                                            } else {
                                                                                if let Some(last_seq) = ssrc_expected_seq.get(&ssrc_recv).copied() {
                                                                                    let missed = rtp_seq.wrapping_sub(last_seq);
                                                                                    if missed > 0 && missed <= 4 {
                                                                                        let mut plc_buf = [0.0f32; 1920];
                                                                                        for _ in 0..missed {
                                                                                            if let Ok(samples) = dec.decode(&[], 960, &mut plc_buf[..]) {
                                                                                                for i in 0..samples {
                                                                                                    if pkt_channels == 2 {
                                                                                                        plc_pairs.push((plc_buf[i * 2].clamp(-1.0, 1.0), plc_buf[i * 2 + 1].clamp(-1.0, 1.0)));
                                                                                                    } else {
                                                                                                        let m = plc_buf[i].clamp(-1.0, 1.0);
                                                                                                        plc_pairs.push((m, m));
                                                                                                    }
                                                                                                }
                                                                                            }
                                                                                        }
                                                                                    }
                                                                                }
                                                                                ssrc_expected_seq.insert(ssrc_recv, rtp_seq.wrapping_add(1));
                                                                            }
                                                                            ssrc_last_pkt_time.insert(ssrc_recv, now);

                                                                            let is_dtx_silence = raw_opus.len() <= 3 && raw_opus == [0xF8, 0xFF, 0xFE];
                                                                            if is_dtx_silence {
                                                                                decode_success = true;
                                                                                decoded_count = 960;
                                                                                for s in pcm_out_buf[..1920].iter_mut() { *s = 0.0; }
                                                                            } else {
                                                                                match dec.decode(raw_opus, 5760, &mut pcm_out_buf[..]) {
                                                                                    Ok(samples) => {
                                                                                        decode_success = true;
                                                                                        decoded_count = samples;
                                                                                    }
                                                                                    Err(e) => {
                                                                                        warn!("📊 [OPUS DIAG] Decode FALHOU: {:?}", e);
                                                                                    }
                                                                                }
                                                                            }
                                                                        }

                                                                        if decode_success && decoded_count > 0 {
                                                                            register_voice_participant(ssrc_recv, sender_user_id);
                                                                            let total_samples = if pkt_channels == 2 { decoded_count * 2 } else { decoded_count };
                                                                            let mut sum_sq = 0.0f32;
                                                                            let mut max_peak = 0.0f32;
                                                                            let mut zc_count = 0usize;
                                                                            let mut prev_s = 0.0f32;
                                                                            let mut diff_energy = 0.0f32;

                                                                            for (idx, &s) in pcm_out_buf[..total_samples].iter().enumerate() {
                                                                                let abs_s = s.abs();
                                                                                if abs_s > max_peak { max_peak = abs_s; }
                                                                                sum_sq += s * s;
                                                                                if idx > 0 {
                                                                                    if (s >= 0.0 && prev_s < 0.0) || (s < 0.0 && prev_s >= 0.0) {
                                                                                        zc_count += 1;
                                                                                    }
                                                                                    let diff = s - prev_s;
                                                                                    diff_energy += diff * diff;
                                                                                }
                                                                                prev_s = s;
                                                                            }
                                                                            let frame_rms = (sum_sq / total_samples.max(1) as f32).sqrt();
                                                                            let hf_noise_ratio = (diff_energy / (sum_sq + 1e-6)).sqrt();
                                                                            let is_silence = frame_rms < 0.003 || opus_data.len() <= 3;

                                                                            let mut dump_vec = Vec::with_capacity(decoded_count);
                                                                            if is_silence {
                                                                                for _ in 0..decoded_count { dump_vec.push((0.0, 0.0)); }
                                                                            } else if pkt_channels == 2 {
                                                                                for i in 0..decoded_count {
                                                                                    dump_vec.push((pcm_out_buf[i * 2].clamp(-1.0, 1.0), pcm_out_buf[i * 2 + 1].clamp(-1.0, 1.0)));
                                                                                }
                                                                            } else {
                                                                                for &s in &pcm_out_buf[..decoded_count] {
                                                                                    let mono = s.clamp(-1.0, 1.0);
                                                                                    dump_vec.push((mono, mono));
                                                                                }
                                                                            }
                                                                                    ssrc_last_pkt_time.insert(ssrc_recv, std::time::Instant::now());

                                                                                    if let Ok(mut queues) = speaker_queues_rx.lock() {
                                                                                        let q = queues.entry(ssrc_recv).or_insert_with(|| VecDeque::with_capacity(48000));
                                                                                        // Maintain real-time jitter buffer: cap at 1s (48000 samples)
                                                                                        while q.len() > 48000 {
                                                                                            q.pop_front();
                                                                                        }
                                                                                        // Push synthesized PLC concealed frames first to maintain unbroken audio stream
                                                                                        for &pair in &plc_pairs {
                                                                                            q.push_back(pair);
                                                                                        }
                                                                                        for &pair in &dump_vec {
                                                                                            q.push_back(pair);
                                                                                        }

                                                                                        if total_pkts_recv % 100 == 0 {
                                                                                            info!("📊 [ESPECTRO DE VOZ & NOISE DUMP] Pkts={:4} | SSRC={} | RMS={:.5} | Peak={:.4} | ZC={:3} | HF_Ratio={:.3} | QueueLen={:5}",
                                                                                                total_pkts_recv, ssrc_recv, frame_rms, max_peak, zc_count, hf_noise_ratio, q.len());
                                                                                        }
                                                                                    }
                                                                                } else if !opus_data.is_empty() {
                                                                                    opus_err_cnt += 1;
                                                                                    if opus_err_cnt % 50 == 1 {
                                                                                        warn!("📊 [MÉTRICA DE ERRO] Opus decode falhou para SSRC {} (Erros={})", ssrc_recv, opus_err_cnt);
                                                                                    }
                                                                                }
                                                                            }
                                                                        }
                                                            }
                                                            Ok(Err(e)) => {
                                                                warn!("Erro socket.recv_from: {:?}", e);
                                                            }
                                                            Err(_) => { /* timeout 100ms, keep looping */ }
                                                        }
                                                    }
                                                    info!("Loop de recepção de voz (ID={}) encerrado.", rx_session_id);
                                                });

                                                let speaker_queues_out = get_speaker_pcm_queues();
                                                let ssrc_to_userid_speaker = Arc::clone(&ssrc_to_userid_audio);
                                                let out_session_id = my_session_id;
                                                std::thread::spawn(move || {
                                                    use cpal::traits::{HostTrait, DeviceTrait, StreamTrait};
                                                    let mut current_target_dev = get_selected_output_device_store().lock().unwrap().clone();

                                                    while CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) == out_session_id {
                                                        let host = cpal::default_host();
                                                        let target_dev_name = current_target_dev.clone();
                                                        let device = if let Ok(devices) = host.output_devices() {
                                                            let dev_list: Vec<_> = devices.into_iter().collect();
                                                            if !target_dev_name.is_empty() && !target_dev_name.contains("Padrão") {
                                                                dev_list.into_iter().find(|d| d.name().map(|n| n == target_dev_name).unwrap_or(false))
                                                                    .or_else(|| host.default_output_device())
                                                            } else {
                                                                dev_list.into_iter().find(|d| {
                                                                    let n = d.name().unwrap_or_default();
                                                                    !n.contains("Litecord") && !n.contains("Virtual") && !n.contains("Null") && !n.contains("Steam")
                                                                }).or_else(|| host.default_output_device())
                                                            }
                                                        } else {
                                                            host.default_output_device()
                                                        };

                                                        let device = match device {
                                                            Some(d) => d,
                                                            None => {
                                                                warn!("Nenhum dispositivo de saída de áudio encontrado! Aguardando...");
                                                                std::thread::sleep(std::time::Duration::from_millis(500));
                                                                continue;
                                                            }
                                                        };

                                                        let config = match device.default_output_config() {
                                                            Ok(c) => c,
                                                            Err(e) => {
                                                                warn!("Falha ao obter config de saída: {:?}", e);
                                                                std::thread::sleep(std::time::Duration::from_millis(500));
                                                                continue;
                                                            }
                                                        };
                                                        let out_sample_rate = config.sample_rate().0;
                                                        let out_channels = config.channels() as usize;
                                                        info!("Saída de Áudio (Speaker): {}Hz, {} canal(is), formato={:?} (Dispositivo: {})",
                                                            out_sample_rate, out_channels, config.sample_format(), target_dev_name);

                                                    let sq = Arc::clone(&speaker_queues_out);
                                                    let ssrc_to_userid_spk = Arc::clone(&ssrc_to_userid_speaker);
                                                    let mut started_ssrcs = std::collections::HashSet::new();
                                                    let mut inactive_ticks = std::collections::HashMap::new();
                                                    // Fractional phase counters per SSRC for smooth Hermite cubic sample rate conversion (48kHz -> out_sample_rate)
                                                    let mut ssrc_phases: std::collections::HashMap<u32, f64> = std::collections::HashMap::new();
                                                    let mut ssrc_histories: std::collections::HashMap<u32, [(f32, f32); 4]> = std::collections::HashMap::new();
                                                    let _step = 48000.0f64 / out_sample_rate.max(1) as f64;

                                                    let stream_config: cpal::StreamConfig = config.clone().into();

                                                    let stream_res = match config.sample_format() {
                                                        cpal::SampleFormat::F32 => {
                                                            let sq_f32 = Arc::clone(&sq);
                                                            let ssrc_to_userid_f32 = Arc::clone(&ssrc_to_userid_spk);
                                                            device.build_output_stream(
                                                                &stream_config,
                                                                move |output: &mut [f32], _| {
                                                                    if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != out_session_id || is_self_deaf() {
                                                                        for s in output.iter_mut() { *s = 0.0; }
                                                                        return;
                                                                    }
                                                                    for s in output.iter_mut() { *s = 0.0; }

                                                                    if let Ok(mut queues) = sq_f32.lock() {
                                                                        let frames = output.len() / out_channels.max(1);
                                                                        let mut ready_ssrcs = Vec::new();
                                                                        let mut to_remove = Vec::new();

                                                                        for (&ssrc, q) in queues.iter() {
                                                                            if q.is_empty() {
                                                                                let ticks = inactive_ticks.entry(ssrc).or_insert(0);
                                                                                *ticks += 1;
                                                                                if *ticks > 3 {
                                                                                    started_ssrcs.remove(&ssrc);
                                                                                    ssrc_histories.remove(&ssrc);
                                                                                    ssrc_phases.remove(&ssrc);
                                                                                }
                                                                                if *ticks > 150 {
                                                                                    to_remove.push(ssrc);
                                                                                }
                                                                            } else {
                                                                                inactive_ticks.insert(ssrc, 0);
                                                                            }

                                                                            if ssrc >= 0x8000_0000 {
                                                                                if !q.is_empty() {
                                                                                    started_ssrcs.insert(ssrc);
                                                                                    ready_ssrcs.push(ssrc);
                                                                                }
                                                                            } else if started_ssrcs.contains(&ssrc) {
                                                                                if !q.is_empty() {
                                                                                    ready_ssrcs.push(ssrc);
                                                                                }
                                                                            } else if q.len() >= 1920 { // 40ms Jitter Pre-buffer
                                                                                started_ssrcs.insert(ssrc);
                                                                                ready_ssrcs.push(ssrc);
                                                                            }
                                                                        }

                                                                        for ssrc in to_remove {
                                                                            queues.remove(&ssrc);
                                                                            started_ssrcs.remove(&ssrc);
                                                                            inactive_ticks.remove(&ssrc);
                                                                            ssrc_phases.remove(&ssrc);
                                                                            ssrc_histories.remove(&ssrc);
                                                                        }

                                                                        if ready_ssrcs.is_empty() { return; }

                                                                        let mut ssrc_vol_map = std::collections::HashMap::new();
                                                                        let mut max_active_priority = 0i32;
                                                                        let active_spk_store = get_active_voice_participants_store();
                                                                        let active_spk_map = active_spk_store.lock().ok();
                                                                        if let Ok(spk_map) = ssrc_to_userid_f32.lock() {
                                                                            for &ssrc in &ready_ssrcs {
                                                                                let (is_muted, user_vol, user_prio) = if ssrc >= 0x8000_0000 {
                                                                                    (false, 1.0f32, 0i32)
                                                                                } else {
                                                                                    let uid_num = spk_map.get(&ssrc)
                                                                                        .or_else(|| active_spk_map.as_ref().and_then(|m| m.get(&ssrc)))
                                                                                        .copied()
                                                                                        .unwrap_or(ssrc as u64);

                                                                                    let (is_muted_uid, vol_uid, prio_uid) = get_user_audio_settings(&uid_num.to_string());
                                                                                    let (is_muted_ssrc, vol_ssrc, prio_ssrc) = get_user_audio_settings(&ssrc.to_string());
                                                                                    let is_muted = is_muted_uid || is_muted_ssrc;
                                                                                    let user_vol = if vol_uid != 1.0 { vol_uid } else { vol_ssrc };
                                                                                    let user_prio = prio_uid.max(prio_ssrc);
                                                                                    (is_muted, user_vol, user_prio)
                                                                                };

                                                                                if !is_muted {
                                                                                    if user_prio > max_active_priority {
                                                                                        max_active_priority = user_prio;
                                                                                    }
                                                                                }
                                                                                ssrc_vol_map.insert(ssrc, (is_muted, user_vol, user_prio));
                                                                            }
                                                                        }

                                                                        for f in 0..frames {
                                                                            let mut mixed_l = 0.0f32;
                                                                            let mut mixed_r = 0.0f32;
                                                                            for &ssrc in &ready_ssrcs {
                                                                                let (is_muted, user_vol, user_prio) = ssrc_vol_map.get(&ssrc).copied().unwrap_or((false, 1.0, 0));
                                                                                if is_muted { continue; }

                                                                                let priority_multiplier = if max_active_priority > 0 && user_prio < max_active_priority {
                                                                                    let diff = max_active_priority - user_prio;
                                                                                    let mult = 0.5f32 - (diff as f32 - 1.0f32) * 0.1f32;
                                                                                    mult.max(0.05f32)
                                                                                } else {
                                                                                    1.0f32
                                                                                };
                                                                                let effective_vol = user_vol * priority_multiplier;

                                                                                if let Some(q) = queues.get_mut(&ssrc) {
                                                                                    let phase = ssrc_phases.entry(ssrc).or_insert(0.0);
                                                                                    let hist = ssrc_histories.entry(ssrc).or_insert([((0.0, 0.0)), ((0.0, 0.0)), ((0.0, 0.0)), ((0.0, 0.0))]);

                                                                                    let step = 48000.0f64 / out_sample_rate.max(1) as f64;

                                                                                    *phase += step;
                                                                                    let pops = *phase as usize;
                                                                                    if pops > 0 {
                                                                                        *phase -= pops as f64;
                                                                                        for _ in 0..pops {
                                                                                            hist[0] = hist[1];
                                                                                            hist[1] = hist[2];
                                                                                            hist[2] = hist[3];
                                                                                            if let Some(next_p) = q.pop_front() {
                                                                                                if hist[0] == (0.0, 0.0) && hist[1] == (0.0, 0.0) && hist[2] == (0.0, 0.0) {
                                                                                                    hist[0] = next_p;
                                                                                                    hist[1] = next_p;
                                                                                                    hist[2] = next_p;
                                                                                                    hist[3] = next_p;
                                                                                                } else {
                                                                                                    hist[3] = next_p;
                                                                                                }
                                                                                            } else {
                                                                                                let decay_l = if hist[2].0.abs() < 0.0001 { 0.0 } else { hist[2].0 * 0.999 };
                                                                                                let decay_r = if hist[2].1.abs() < 0.0001 { 0.0 } else { hist[2].1 * 0.999 };
                                                                                                hist[3] = (decay_l, decay_r);
                                                                                            }
                                                                                        }
                                                                                    }
                                                                                    let t = (*phase as f32).clamp(0.0, 1.0);
                                                                                    let src_l = cubic_hermite(hist[0].0, hist[1].0, hist[2].0, hist[3].0, t);
                                                                                    let src_r = cubic_hermite(hist[0].1, hist[1].1, hist[2].1, hist[3].1, t);

                                                                                    mixed_l += src_l * effective_vol;
                                                                                    mixed_r += src_r * effective_vol;
                                                                                }
                                                                            }

                                                                            let limited_l = soft_limit(mixed_l);
                                                                            let limited_r = soft_limit(mixed_r);
                                                                            if out_channels >= 2 {
                                                                                output[f * out_channels + 0] = limited_l;
                                                                                output[f * out_channels + 1] = limited_r;
                                                                                for ch in 2..out_channels {
                                                                                    output[f * out_channels + ch] = 0.0;
                                                                                }
                                                                            } else {
                                                                                output[f] = soft_limit((mixed_l + mixed_r) * 0.5);
                                                                            }
                                                                        }
                                                                    }
                                                                },
                                                                |err| { warn!("Erro no stream de saída F32: {:?}", err); },
                                                                None,
                                                            )
                                                        }
                                                        cpal::SampleFormat::I16 => {
                                                            let sq_i16 = Arc::clone(&sq);
                                                            let ssrc_to_userid_i16 = Arc::clone(&ssrc_to_userid_spk);
                                                            device.build_output_stream(
                                                                &config.into(),
                                                                move |output: &mut [i16], _| {
                                                                    if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != out_session_id || is_self_deaf() {
                                                                        for s in output.iter_mut() { *s = 0; }
                                                                        return;
                                                                    }
                                                                    for s in output.iter_mut() { *s = 0; }

                                                                    if let Ok(mut queues) = sq_i16.lock() {
                                                                        let frames = output.len() / out_channels.max(1);
                                                                        let mut ready_ssrcs = Vec::new();
                                                                        let mut to_remove = Vec::new();

                                                                        for (&ssrc, q) in queues.iter() {
                                                                            if q.is_empty() {
                                                                                let ticks = inactive_ticks.entry(ssrc).or_insert(0);
                                                                                *ticks += 1;
                                                                                if *ticks > 3 {
                                                                                    started_ssrcs.remove(&ssrc);
                                                                                    ssrc_histories.remove(&ssrc);
                                                                                    ssrc_phases.remove(&ssrc);
                                                                                }
                                                                                if *ticks > 150 {
                                                                                    to_remove.push(ssrc);
                                                                                }
                                                                            } else {
                                                                                inactive_ticks.insert(ssrc, 0);
                                                                            }

                                                                            if ssrc >= 0x8000_0000 {
                                                                                if !q.is_empty() {
                                                                                    started_ssrcs.insert(ssrc);
                                                                                    ready_ssrcs.push(ssrc);
                                                                                }
                                                                            } else if started_ssrcs.contains(&ssrc) {
                                                                                if !q.is_empty() {
                                                                                    ready_ssrcs.push(ssrc);
                                                                                }
                                                                            } else if q.len() >= 1920 { // 40ms Jitter Pre-buffer
                                                                                started_ssrcs.insert(ssrc);
                                                                                ready_ssrcs.push(ssrc);
                                                                            }
                                                                        }

                                                                        for ssrc in to_remove {
                                                                            queues.remove(&ssrc);
                                                                            started_ssrcs.remove(&ssrc);
                                                                            inactive_ticks.remove(&ssrc);
                                                                            ssrc_phases.remove(&ssrc);
                                                                            ssrc_histories.remove(&ssrc);
                                                                        }

                                                                        if ready_ssrcs.is_empty() { return; }

                                                                        let mut ssrc_vol_map_i16 = std::collections::HashMap::new();
                                                                        let mut max_active_priority_i16 = 0i32;
                                                                        let active_spk_store_i16 = get_active_voice_participants_store();
                                                                        let active_spk_map_i16 = active_spk_store_i16.lock().ok();
                                                                        if let Ok(spk_map) = ssrc_to_userid_i16.lock() {
                                                                            for &ssrc in &ready_ssrcs {
                                                                                let (is_muted, user_vol, user_prio) = if ssrc >= 0x8000_0000 {
                                                                                    (false, 1.0f32, 0i32)
                                                                                } else {
                                                                                    let uid_num = spk_map.get(&ssrc)
                                                                                        .or_else(|| active_spk_map_i16.as_ref().and_then(|m| m.get(&ssrc)))
                                                                                        .copied()
                                                                                        .unwrap_or(ssrc as u64);

                                                                                    let (is_muted_uid, vol_uid, prio_uid) = get_user_audio_settings(&uid_num.to_string());
                                                                                    let (is_muted_ssrc, vol_ssrc, prio_ssrc) = get_user_audio_settings(&ssrc.to_string());
                                                                                    let is_muted = is_muted_uid || is_muted_ssrc;
                                                                                    let user_vol = if vol_uid != 1.0 { vol_uid } else { vol_ssrc };
                                                                                    let user_prio = prio_uid.max(prio_ssrc);
                                                                                    (is_muted, user_vol, user_prio)
                                                                                };

                                                                                if !is_muted {
                                                                                    if user_prio > max_active_priority_i16 {
                                                                                        max_active_priority_i16 = user_prio;
                                                                                    }
                                                                                }
                                                                                ssrc_vol_map_i16.insert(ssrc, (is_muted, user_vol, user_prio));
                                                                            }
                                                                        }

                                                                        for f in 0..frames {
                                                                            let mut mixed_l = 0.0f32;
                                                                            let mut mixed_r = 0.0f32;
                                                                            for &ssrc in &ready_ssrcs {
                                                                                let (is_muted, user_vol, user_prio) = ssrc_vol_map_i16.get(&ssrc).copied().unwrap_or((false, 1.0, 0));
                                                                                if is_muted { continue; }

                                                                                let priority_multiplier = if max_active_priority_i16 > 0 && user_prio < max_active_priority_i16 {
                                                                                    let diff = max_active_priority_i16 - user_prio;
                                                                                    let mult = 0.5f32 - (diff as f32 - 1.0f32) * 0.1f32;
                                                                                    mult.max(0.05f32)
                                                                                } else {
                                                                                    1.0f32
                                                                                };
                                                                                let effective_vol = user_vol * priority_multiplier;

                                                                                if let Some(q) = queues.get_mut(&ssrc) {
                                                                                    let phase = ssrc_phases.entry(ssrc).or_insert(0.0);
                                                                                    let hist = ssrc_histories.entry(ssrc).or_insert([((0.0, 0.0)), ((0.0, 0.0)), ((0.0, 0.0)), ((0.0, 0.0))]);

                                                                                    let step = 48000.0f64 / out_sample_rate.max(1) as f64;

                                                                                    *phase += step;
                                                                                    let pops = *phase as usize;
                                                                                    if pops > 0 {
                                                                                        *phase -= pops as f64;
                                                                                        for _ in 0..pops {
                                                                                            hist[0] = hist[1];
                                                                                            hist[1] = hist[2];
                                                                                            hist[2] = hist[3];
                                                                                            if let Some(next_p) = q.pop_front() {
                                                                                                hist[3] = next_p;
                                                                                            } else {
                                                                                                hist[3] = (hist[2].0 * 0.995, hist[2].1 * 0.995);
                                                                                            }
                                                                                        }
                                                                                    }
                                                                                    let t = (*phase as f32).clamp(0.0, 1.0);
                                                                                    let src_l = cubic_hermite(hist[0].0, hist[1].0, hist[2].0, hist[3].0, t);
                                                                                    let src_r = cubic_hermite(hist[0].1, hist[1].1, hist[2].1, hist[3].1, t);

                                                                                    mixed_l += src_l * effective_vol;
                                                                                    mixed_r += src_r * effective_vol;
                                                                                }
                                                                            }

                                                                            let clamped_l = (soft_limit(mixed_l) * 32767.0) as i16;
                                                                            let clamped_r = (soft_limit(mixed_r) * 32767.0) as i16;
                                                                            if out_channels >= 2 {
                                                                                output[f * out_channels + 0] = clamped_l;
                                                                                output[f * out_channels + 1] = clamped_r;
                                                                                for ch in 2..out_channels {
                                                                                    output[f * out_channels + ch] = 0;
                                                                                }
                                                                                    output[f] = (soft_limit((mixed_l + mixed_r) * 0.5) * 32767.0) as i16;
                                                                            }
                                                                        }
                                                                    }
                                                                },
                                                                |err| { warn!("Erro no stream de saída I16: {:?}", err); },
                                                                None,
                                                            )
                                                        }
                                                        _ => {
                                                            warn!("Formato de saída não suportado: {:?}", config.sample_format());
                                                            std::thread::sleep(std::time::Duration::from_millis(1000));
                                                            continue;
                                                        }
                                                    };

                                                    match stream_res {
                                                        Ok(stream) => {
                                                            if let Err(e) = stream.play() {
                                                                warn!("Falha ao iniciar stream de saída: {:?}", e);
                                                                std::thread::sleep(std::time::Duration::from_millis(500));
                                                                continue;
                                                            }
                                                            info!("🔊 Stream de Saída de Áudio (Speaker) ATIVO! Reproduzindo vozes dos outros usuários...");
                                                            // Keep stream alive until voice session ends OR user selects another output device!
                                                            loop {
                                                                std::thread::sleep(std::time::Duration::from_millis(100));
                                                                if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != out_session_id {
                                                                    info!("Stream de saída encerrado (sessão expirada).");
                                                                    return;
                                                                }
                                                                let new_target = get_selected_output_device_store().lock().unwrap().clone();
                                                                if new_target != current_target_dev {
                                                                    info!("🔄 Dispositivo de saída alterado de '{}' para '{}' durante a chamada! Reconfigurando alto-falante...", current_target_dev, new_target);
                                                                    current_target_dev = new_target;
                                                                    break; // Sai do loop interno, dropa o stream atual e o while externo reconstrói no novo dispositivo!
                                                                }
                                                            }
                                                        }
                                                        Err(e) => {
                                                            warn!("Falha ao criar stream de saída: {:?}", e);
                                                            std::thread::sleep(std::time::Duration::from_millis(500));
                                                        }
                                                    }
                                                }
                                                info!("Loop principal de áudio de saída encerrado.");
                                            });

                                                tokio::spawn(async move {
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

                                                // Flush any stale mic buffer accumulated before voice handshake completed
                                                if let Ok(mut q) = pcm_queue.lock() {
                                                    q.clear();
                                                }

                                                // Stream AES-256-GCM Encrypted Opus microphone audio RTP frames every 20ms over UDP
                                                let mut timer = tokio::time::interval(Duration::from_millis(20));
                                                timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                                                // Send initial 10 silence frames (200ms) to immediately punch UDP NAT hole and activate SFU downstream routing
                                                let mut remaining_silence_frames: usize = 10;
                                                let mut speech_hangover_frames: usize = 0;
                                                let mut silence_keepalive_counter: u32 = 0;

                                                loop {
                                                    timer.tick().await;
                                                    if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != my_session_id {
                                                        info!("Sessão de voz antiga (ID={}) encerrada, saindo do loop UDP!", my_session_id);
                                                        break;
                                                    }

                                                    // Monotonically advance media timestamp on every 20ms tick (RFC 3550 & WebRTC standard)
                                                    timestamp = timestamp.wrapping_add(960);

                                                    let is_muted = *self_mute_rx.lock().unwrap();

                                                    // Extract full 960 f32 PCM samples (20ms of audio at 48000Hz) from microphone buffer
                                                    let mut pcm_frame = [0.0f32; 960];
                                                    let mut has_audio = false;
                                                    {
                                                        if let Ok(mut q) = pcm_queue.lock() {
                                                            if is_muted {
                                                                q.clear();
                                                            } else {
                                                                // Keep queue latency ultra-low (cap at <= 200ms max backlog)
                                                                if q.len() > 960 * 10 {
                                                                    let drop_count = q.len() - 960 * 2;
                                                                    q.drain(0..drop_count);
                                                                }
                                                                // Only extract when full 20ms (960 samples) frame is available
                                                                if q.len() >= 960 {
                                                                    for i in 0..960 {
                                                                        pcm_frame[i] = q.pop_front().unwrap_or(0.0);
                                                                    }
                                                                    // Enforce VAD threshold gating
                                                                    let mut sum_sq = 0.0f32;
                                                                    for &s in pcm_frame.iter() {
                                                                        sum_sq += s * s;
                                                                    }
                                                                    let rms = (sum_sq / 960.0).sqrt();
                                                                    let level = (rms * 6.0).min(1.0);
                                                                    if level >= get_vad_threshold() {
                                                                        has_audio = true;
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }

                                                    let is_silence_packet = if has_audio {
                                                        speech_hangover_frames = 12; // 240ms hangover
                                                        remaining_silence_frames = 5;
                                                        silence_keepalive_counter = 0;
                                                        false
                                                    } else if speech_hangover_frames > 0 {
                                                        speech_hangover_frames -= 1;
                                                        false
                                                    } else if remaining_silence_frames > 0 {
                                                        remaining_silence_frames -= 1;
                                                        true
                                                    } else {
                                                        silence_keepalive_counter = silence_keepalive_counter.wrapping_add(1);
                                                        // Send a keepalive silence packet every 2.5 seconds (125 frames) to keep NAT route open
                                                        if silence_keepalive_counter >= 125 {
                                                            silence_keepalive_counter = 0;
                                                            true
                                                        } else {
                                                            continue;
                                                        }
                                                    };

                                                    seq = seq.wrapping_add(1);
                                                    nonce_cnt = nonce_cnt.wrapping_add(1);

                                                    let mut audio_header = [0u8; 12];
                                                    audio_header[0] = 0x80; // RTP Version 2
                                                    audio_header[1] = 0x78; // Opus Payload 120
                                                    audio_header[2..4].copy_from_slice(&seq.to_be_bytes());
                                                    audio_header[4..8].copy_from_slice(&timestamp.to_be_bytes());
                                                    audio_header[8..12].copy_from_slice(&ssrc.to_be_bytes());

                                                    // Prepare 1920 interleaved stereo samples for 48kHz Stereo Opus encoding
                                                    let mut pcm_stereo = [0.0f32; 1920];
                                                    for (i, &s) in pcm_frame.iter().enumerate() {
                                                        pcm_stereo[i * 2] = s;
                                                        pcm_stereo[i * 2 + 1] = s;
                                                    }

                                                    let opus_bytes: &[u8] = if !is_silence_packet {
                                                        if let Ok(encoded_len) = opus_encoder.encode(&pcm_stereo, 960, &mut opus_out) {
                                                            &opus_out[..encoded_len]
                                                        } else {
                                                            &[0xF8, 0xFF, 0xFE] // Silence frame fallback
                                                        }
                                                    } else {
                                                        &[0xF8, 0xFF, 0xFE] // 5x Silence frame trailing pulse
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
                                                                    info!("TransmissÃ£o de Ãudio: frames_enviados={}, pcm_queue_buffer={}, has_audio={}, dave_encrypted={}",
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
                                            });
                                        });
                                    }
                                }
                                4 => {
                                    // Voice Opcode 4: Session Description (Handshake Complete & Secret Key received!)
                                    let ssrc = *active_ssrc.lock().unwrap();
                                    info!("ðŸŽ‰ VOICE GATEWAY OPCODE 4 SESSION DESCRIPTION RECEBIDO! SessÃ£o de Voz Ativada com Sucesso!");

                                    if let Some(key_arr) = val["d"]["secret_key"].as_array() {
                                        let key_bytes: Vec<u8> = key_arr.iter().filter_map(|v| v.as_u64().map(|n| n as u8)).collect();
                                        if key_bytes.len() == 32 {
                                            info!("Chave de criptografia AES-256 de 32 bytes configurada com SUCESSO para os pacotes de áudio!");
                                            let mut fixed = [0u8; 32];
                                            fixed.copy_from_slice(&key_bytes);
                                            *get_voice_secret_key_store().lock().unwrap() = Some(fixed);
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
                                 5 => {
                                     // Voice Opcode 5: Speaking notification from another user
                                     let s_ssrc = val["d"]["ssrc"].as_u64().map(|n| n as u32).unwrap_or(0);
                                     let u_id: u64 = if let Some(s) = val["d"]["user_id"].as_str() {
                                         s.parse().unwrap_or(0)
                                     } else {
                                         val["d"]["user_id"].as_u64().unwrap_or(0)
                                     };
                                     if s_ssrc > 0 && u_id > 0 {
                                         ssrc_to_userid.lock().unwrap().insert(s_ssrc, u_id);
                                         register_voice_participant(s_ssrc, u_id);
                                         info!("Voice Gateway OP 5: Mapeado SSRC {} -> User ID {}", s_ssrc, u_id);
                                     }
                                 }
                                 6 => {
                                     // Voice Opcode 6: Heartbeat ACK!
                                     info!("Voice Gateway Heartbeat ACK (Opcode 6) recebido!");
                                 }
                                 11 | 12 => {
                                     // Voice Opcode 11/12: User Joined / Client Connect
                                     if let Some(arr) = val["d"]["user_ids"].as_array() {
                                         for v in arr {
                                             if let Some(uid_str) = v.as_str() {
                                                 if let Ok(uid) = uid_str.parse::<u64>() {
                                                     let ssrc = (uid & 0xFFFFFFFF) as u32;
                                                     register_voice_participant(ssrc, uid);
                                                     info!("Voice Gateway OP 11/12 (user_ids): Registrado participante User ID {}", uid);
                                                 }
                                             }
                                         }
                                     }
                                     let s_ssrc = val["d"]["audio_ssrc"].as_u64().or_else(|| val["d"]["ssrc"].as_u64()).unwrap_or(0) as u32;
                                     let u_id: u64 = if let Some(s) = val["d"]["user_id"].as_str() {
                                         s.parse().unwrap_or(0)
                                     } else {
                                         val["d"]["user_id"].as_u64().unwrap_or(0)
                                     };
                                     if s_ssrc > 0 && u_id > 0 {
                                         ssrc_to_userid.lock().unwrap().insert(s_ssrc, u_id);
                                         register_voice_participant(s_ssrc, u_id);
                                         info!("Voice Gateway OP 11/12: Mapeado SSRC {} -> User ID {}", s_ssrc, u_id);
                                     }
                                 }
                                18 => {
                                    // DAVE Opcode 18: DAVE_PREPARE_TRANSITION (Sent for all active voice channel participants)
                                    let transition_id = val["d"]["transition_id"].as_u64().unwrap_or(0);
                                    let protocol_version = val["d"]["protocol_version"].as_u64().unwrap_or(99);
                                    let u_id: u64 = if let Some(s) = val["d"]["user_id"].as_str() {
                                        s.parse().unwrap_or(0)
                                    } else {
                                        val["d"]["user_id"].as_u64().unwrap_or(0)
                                    };
                                    if u_id > 0 {
                                        let ssrc = (u_id & 0xFFFFFFFF) as u32;
                                        register_voice_participant(ssrc, u_id);
                                        info!("👥 Voice Gateway OP 18 DAVE Transition: Registrado participante no canal de voz! User ID {}", u_id);
                                    }
                                    info!("DAVE Prepare Transition (op=18): transition_id={}, protocol_version={}, payload={}",
                                        transition_id, protocol_version, val["d"]);

                                    let ready = serde_json::json!({
                                        "op": 23,
                                        "d": { "transition_id": transition_id }
                                    });
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Text(ready.to_string().into())).await;
                                    info!("DAVE: Ready For Transition (op=23) enviado para transition_id={}", transition_id);
                                }
                                19 => {
                                    // DAVE Opcode 19: DAVE_EXECUTE_TRANSITION (Server -> Client)
                                    let transition_id = val["d"]["transition_id"].as_u64().unwrap_or(0);
                                    info!("DAVE Execute Transition (op=19): transition_id={}, payload={}", transition_id, val["d"]);
                                    let ssrc = *active_ssrc.lock().unwrap();
                                    let speaking_pkt = serde_json::json!({
                                        "op": 5,
                                        "d": { "speaking": 1, "delay": 0, "ssrc": ssrc }
                                    });
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Text(speaking_pkt.to_string().into())).await;
                                    info!("DAVE OP 19: Enviado OP 5 Speaking para sincronizar SSRC {} na transição de época!", ssrc);
                                }
                                20 => {
                                    // DAVE Opcode 20: DAVE_TRANSITION_READY
                                }
                                21 => {
                                    // DAVE Opcode 21: DAVE_PREPARE_TRANSITION (Server -> Client)
                                    let transition_id = val["d"]["transition_id"].as_u64().unwrap_or(0);
                                    let protocol_version = val["d"]["protocol_version"].as_u64().unwrap_or(0);
                                    info!("DAVE Prepare Transition (op=21): transition_id={}, protocol_version={}", transition_id, protocol_version);
                                    let ready = serde_json::json!({
                                        "op": 23,
                                        "d": { "transition_id": transition_id }
                                    });
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Text(ready.to_string().into())).await;
                                    info!("DAVE: Ready For Transition (op=23) enviado para op=21 transition_id={}", transition_id);
                                }
                                22 => {
                                    // DAVE Opcode 22: DAVE_EXECUTE_TRANSITION (Server -> Client)
                                    let transition_id = val["d"]["transition_id"].as_u64().unwrap_or(0);
                                    info!("DAVE Execute Transition (op=22): transition_id={}, payload={}", transition_id, val["d"]);
                                    let ssrc = *active_ssrc.lock().unwrap();
                                    let speaking_pkt = serde_json::json!({
                                        "op": 5,
                                        "d": { "speaking": 1, "delay": 0, "ssrc": ssrc }
                                    });
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Text(speaking_pkt.to_string().into())).await;
                                    info!("DAVE OP 22: Enviado OP 5 Speaking para sincronizar SSRC {} na transição de época!", ssrc);
                                }
                                24 => {
                                    // DAVE Opcode 24: DAVE_PREPARE_EPOCH (Server -> Client)
                                    let epoch = val["d"]["epoch"].as_u64().unwrap_or(0);
                                    let protocol_version = val["d"]["protocol_version"].as_u64().unwrap_or(1);
                                    info!("DAVE Prepare Epoch (op=24): epoch={}, protocol_version={}", epoch, protocol_version);
                                    if epoch == 1 {
                                        let key_pkg_bytes = {
                                            let mut sess = dave_session.lock().unwrap();
                                            if let Some(ref mut s) = *sess {
                                                let _ = s.reset();
                                                if let Ok(mut new_s) = DaveSession::new(
                                                    NonZeroU16::new(protocol_version as u16).unwrap_or(NonZeroU16::new(1).unwrap()),
                                                    uid_num, cid_num, None
                                                ) {
                                                    if let Some(ref ext_bytes) = saved_external_sender.lock().unwrap().as_ref() {
                                                        let _ = new_s.set_external_sender(ext_bytes);
                                                    }
                                                    let kp = new_s.create_key_package().ok();
                                                    *s = new_s;
                                                    kp
                                                } else { None }
                                            } else { None }
                                        };
                                        if let Some(kp) = key_pkg_bytes {
                                            let mut pkt = vec![26u8];
                                            pkt.extend_from_slice(&kp);
                                            let mut w = write_arc.lock().await;
                                            let _ = w.send(Message::Binary(pkt.into())).await;
                                            info!("DAVE: Novo KeyPackage (op=26) gerado e enviado para Epoch Reset (op=24)!");
                                        }
                                    }
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
                                *saved_external_sender.lock().unwrap() = Some(payload.to_vec());
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
                                    info!("DAVE: KeyPackage (op=26) enviado Ã  Voice Gateway!");
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
                                                info!("DAVE: CommitWelcome gerado ({} commit bytes, welcome={})",
                                                    cw.commit.len(), cw.welcome.as_ref().map(|w| w.len()).unwrap_or(0));
                                                let mut out = cw.commit.clone();
                                                if let Some(w) = cw.welcome {
                                                    out.push(1u8); // RFC 9420 optional<Welcome> presence flag = 1
                                                    out.extend_from_slice(&w);
                                                } else {
                                                    out.push(0u8); // RFC 9420 optional<Welcome> presence flag = 0
                                                }
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
                                    info!("DAVE: CommitWelcome (op=28) enviado Ã  Voice Gateway!");
                                }
                            }
                            29 => {
                                // dave_mls_announce_commit_transition (29): [op: u8 (29)][transition_id: u16 (2 bytes)][commit_bytes]
                                let transition_id = if data.len() >= 3 {
                                    u16::from_be_bytes([data[1], data[2]]) as u64
                                } else { 0 };
                                let commit_payload = if data.len() > 3 { &data[3..] } else { &[][..] };
                                info!("DAVE: AnnounceCommitTransition (op=29) recebido: transition_id={}", transition_id);
                                let commit_ok = {
                                    let mut sess = dave_session.lock().unwrap();
                                    if let Some(ref mut s) = *sess {
                                        match s.process_commit(commit_payload) {
                                            Ok(()) => {
                                                info!("DAVE: Commit processado com sucesso! is_ready={}", s.is_ready());
                                                true
                                            }
                                            Err(e) => {
                                                warn!("DAVE: Falha ao processar commit: {:?}", e);
                                                false
                                            }
                                        }
                                    } else { false }
                                };
                                if commit_ok {
                                    let ready = serde_json::json!({
                                        "op": 23,
                                        "d": { "transition_id": transition_id }
                                    });
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Text(ready.to_string().into())).await;
                                    info!("DAVE: Ready For Transition (op=23) enviado para AnnounceCommit transition_id={}", transition_id);
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
                                                info!("ðŸŽ‰ DAVE: Welcome processado com SUCESSO! SessÃ£o ATIVA! is_ready={}", s.is_ready());
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
                                        "op": 23,
                                        "d": { "transition_id": transition_id }
                                    });
                                    let mut w = write_arc.lock().await;
                                    let _ = w.send(Message::Text(ready.to_string().into())).await;
                                    info!("DAVE: Ready For Transition (op=23) enviado para Welcome transition_id={}", transition_id);
                                }
                            }
                            _ => {
                                info!("DAVE: Opcode binÃ¡rio desconhecido: {}", dave_op);
                            }
                        }
                     }
                    Ok(Message::Close(frame)) => {
                        info!("Voice Gateway conexão fechada pelo servidor: {:?}", frame);
                        break;
                    }
                    Err(e) => {
                        warn!("Aviso de leitura na Voice Gateway: {:?}", e);
                        break;
                    }
                    _ => {}
                }
            }

            set_is_connected_to_voice(false);
            if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != my_session_id {
                info!("Sessão de voz {} finalizada pelo usuário.", my_session_id);
                clear_voice_participants();
                break;
            }
            warn!("⚠️ [VOICE GATEWAY] Conexão com Voice Gateway temporariamente perdida. Reconectando em 1.5s...");
        }
        Err(e) => {
            if CURRENT_VOICE_SESSION_ID.load(Ordering::SeqCst) != my_session_id {
                break;
            }
            warn!("Falha ao conectar na Voice Gateway: {:?}. Tentando reconectar em 800ms...", e);
            tokio::time::sleep(Duration::from_millis(800)).await;
        }
    }
}
}
