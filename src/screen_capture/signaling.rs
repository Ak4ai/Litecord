#![allow(dead_code)]

use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use log::{error, info, warn};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use futures_util::{SinkExt, StreamExt};

use super::types::*;
use super::crypto::*;

pub fn invalidate_cached_stun_address() {
    if let Ok(mut guard) = CACHED_STUN_ADDR.lock() {
        *guard = None;
    }
}

pub fn resolve_public_stun_address() -> Option<SocketAddr> {
    if let Ok(guard) = CACHED_STUN_ADDR.lock() {
        if let Some((addr, time)) = *guard {
            if time.elapsed() < Duration::from_secs(10) {
                return Some(addr);
            }
        }
    }

    let stun_req: [u8; 20] = [
        0x00, 0x01, // Binding Request
        0x00, 0x00, // Length
        0x21, 0x12, 0xa4, 0x42, // Magic Cookie
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
    ];

    let stun_servers = [
        "stun.l.google.com:19302",
        "stun1.l.google.com:19302",
        "stun2.l.google.com:19302",
        "stun.cloudflare.com:3478",
        "74.125.250.129:19302",
        "162.159.192.1:3478",
    ];

    // Fase 1: Se o socket compartilhado estiver ativo, enviar pacotes STUN nele
    // e aguardar até 200ms para a thread screen-capture-rx processar a resposta.
    if let Some(socket) = get_shared_p2p_socket() {
        for server in stun_servers {
            if let Ok(addrs) = server.to_socket_addrs() {
                for addr in addrs {
                    if addr.is_ipv4() {
                        let _ = socket.send_to(&stun_req, addr);
                    }
                }
            }
        }

        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(200) {
            if let Ok(guard) = CACHED_STUN_ADDR.lock() {
                if let Some((addr, _)) = *guard {
                    return Some(addr);
                }
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    // Fase 2: Fallback síncrono dedicado caso a thread rx ainda não tenha preenchido o cache
    let my_rx_port = get_my_rx_port();
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        let _ = socket.set_read_timeout(Some(Duration::from_millis(150)));
        for server in stun_servers {
            if let Ok(addrs) = server.to_socket_addrs() {
                for addr in addrs {
                    if addr.is_ipv4() {
                        let _ = socket.send_to(&stun_req, addr);
                    }
                }
            }
        }
        let mut buf = [0u8; 1024];
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(250) {
            if let Ok((len, _)) = socket.recv_from(&mut buf) {
                if len >= 20 && buf[0] == 0x01 && buf[1] == 0x01 && &buf[4..8] == &[0x21, 0x12, 0xa4, 0x42] {
                    let mut i = 20;
                    while i + 4 <= len {
                        let attr_type = u16::from_be_bytes([buf[i], buf[i + 1]]);
                        let attr_len = u16::from_be_bytes([buf[i + 2], buf[i + 3]]) as usize;
                        if i + 4 + attr_len > len { break; }
                        if (attr_type == 0x0020 || attr_type == 0x0001) && attr_len >= 8 && buf[i + 5] == 0x01 {
                            let port = if attr_type == 0x0020 {
                                u16::from_be_bytes([buf[i + 6], buf[i + 7]]) ^ 0x2112
                            } else {
                                u16::from_be_bytes([buf[i + 6], buf[i + 7]])
                            };
                            let ip = if attr_type == 0x0020 {
                                std::net::Ipv4Addr::new(
                                    buf[i + 8] ^ 0x21,
                                    buf[i + 9] ^ 0x12,
                                    buf[i + 10] ^ 0xa4,
                                    buf[i + 11] ^ 0x42,
                                )
                            } else {
                                std::net::Ipv4Addr::new(buf[i + 8], buf[i + 9], buf[i + 10], buf[i + 11])
                            };
                            let resolved_port = if my_rx_port > 0 { my_rx_port } else { port };
                            let resolved = SocketAddr::new(std::net::IpAddr::V4(ip), resolved_port);
                            if let Ok(mut guard) = CACHED_STUN_ADDR.lock() {
                                *guard = Some((resolved, Instant::now()));
                            }
                            info!("🌐 [STUN SYNCHRONOUS RESOLUTION] IP Público WAN mapeado: {}", resolved);
                            return Some(resolved);
                        }
                        i += 4 + ((attr_len + 3) & !3);
                    }
                }
            }
        }
    }

    if let Ok(guard) = CACHED_STUN_ADDR.lock() {
        if let Some((addr, _)) = *guard {
            return Some(addr);
        }
    }

    None
}

pub fn process_signaling_json(
    json_bytes: &[u8],
    current_cid: u64,
    my_inst: u32,
    peers_store: &Arc<Mutex<HashMap<u64, (SocketAddr, Instant)>>>,
) {
    if let Ok(val) = serde_json::from_slice::<serde_json::Value>(json_bytes) {
        let pkt_cid = val["cid"].as_u64().unwrap_or(0);
        let pkt_uid = val["uid"].as_u64().unwrap_or(0);
        let pkt_inst = val["inst"].as_u64().unwrap_or(0) as u32;

        let my_uid = crate::gateway::get_my_user_id();
        if pkt_cid == current_cid && pkt_inst != my_inst && pkt_inst != 0 && (my_uid == 0 || pkt_uid != my_uid) {
            let is_streaming = val["streaming"].as_bool().unwrap_or(false);
            let was_streaming = {
                let mut guard = PEER_STREAMING_STATES.lock().unwrap_or_else(|e| e.into_inner());
                let map = guard.get_or_insert_with(HashMap::new);
                let prev = map.insert(pkt_uid, is_streaming);
                prev.unwrap_or(false)
            };

            info!("📡 [SIGNALING PRESENÇA] De UID={} (inst={}) | streaming={} (was={}) | WAN={:?} | LAN={:?}", pkt_uid, pkt_inst, is_streaming, was_streaming, val["wan_ip"], val["lan_ips"]);

            // Se o peer parou de transmitir tela, fecha imediatamente (<50ms) sem esperar timeout de pacotes UDP
            if !is_streaming && was_streaming {
                trigger_stream_stopped(pkt_uid);
            }

            // Negociação X25519 ECDH: computa a chave de sessão compartilhada exclusiva do peer
            if let Some(pub_hex) = val["ecdh_pub"].as_str() {
                if pub_hex.len() == 64 {
                    let mut pub_bytes = [0u8; 32];
                    let mut valid = true;
                    for i in 0..32 {
                        if let Ok(byte_val) = u8::from_str_radix(&pub_hex[i * 2..i * 2 + 2], 16) {
                            pub_bytes[i] = byte_val;
                        } else {
                            valid = false;
                            break;
                        }
                    }
                    if valid {
                        compute_peer_shared_key(pkt_uid, &pub_bytes, current_cid);
                    }
                }
            }

            let mut addrs_to_punch = Vec::new();
            let mut remote_wan_addr = None;

            // 1. WAN IP (Public Internet)
            if let Some(wan_str) = val["wan_ip"].as_str() {
                if let Ok(addr) = wan_str.parse::<SocketAddr>() {
                    if !is_tailscale_or_forbidden(&addr) {
                        addrs_to_punch.push(addr);
                        remote_wan_addr = Some(addr);
                    }
                }
            }

            // Atualiza imediatamente LAST_SEEN_PEER_ADDR para que o transmissor aponte para o novo IP fresco
            if let Some(wan) = remote_wan_addr {
                if let Ok(mut guard) = LAST_SEEN_PEER_ADDR.lock() {
                    let map = guard.get_or_insert_with(HashMap::new);
                    map.insert(pkt_uid, wan);
                }
            }

            // Determina se o peer está em WAN externa (diferente da nossa WAN)
            let is_cross_wan = if let Some(r_wan) = remote_wan_addr {
                if let Some(my_wan) = resolve_public_stun_address() {
                    r_wan.ip() != my_wan.ip()
                } else {
                    match r_wan.ip() {
                        std::net::IpAddr::V4(v4) => !v4.is_private() && !v4.is_loopback() && !v4.is_link_local(),
                        _ => false,
                    }
                }
            } else {
                false
            };

            // 2. LAN IPs: inclui SOMENTE se NÃO for cross-WAN (ex: ambos na mesma rede local)
            if !is_cross_wan {
                if let Some(lan_arr) = val["lan_ips"].as_array() {
                    for item in lan_arr {
                        if let Some(s) = item.as_str() {
                            if let Ok(addr) = s.parse::<SocketAddr>() {
                                if !is_tailscale_or_forbidden(&addr) && !addr.ip().is_loopback() && !addrs_to_punch.contains(&addr) {
                                    addrs_to_punch.push(addr);
                                }
                            }
                        }
                    }
                }
            } else {
                info!("🌐 [P2P WAN ROTEAMENTO] Peer UID={} está em WAN externa ({:?}) - ignorando LANs internas para evitar ICMP reset!", pkt_uid, remote_wan_addr);
            }

            if !addrs_to_punch.is_empty() {
                if let Ok(mut peers) = peers_store.lock() {
                    if is_cross_wan {
                        peers.retain(|&k, (a, _)| {
                            let key_uid = (k & 0xFFFF_FFFF) as u32;
                            if key_uid == (pkt_uid as u32) {
                                match a.ip() {
                                    std::net::IpAddr::V4(v4) => !v4.is_private() && !v4.is_loopback(),
                                    _ => true,
                                }
                            } else {
                                true
                            }
                        });
                    }
                    for (i, &addr) in addrs_to_punch.iter().enumerate() {
                        let key = ((pkt_uid as u64) ^ ((pkt_inst as u64) << 32)).wrapping_add(i as u64);
                        peers.insert(key, (addr, Instant::now()));
                    }
                }

                // Dispara burst de UDP Hole Punch imediato pelo socket compartilhado
                if let Some(socket) = get_shared_p2p_socket() {
                    let mut punch_pkt = Vec::with_capacity(32);
                    punch_pkt.extend_from_slice(MAGIC);
                    punch_pkt.extend_from_slice(&my_inst.to_be_bytes());
                    punch_pkt.push(OP_HEARTBEAT);
                    punch_pkt.extend_from_slice(&current_cid.to_be_bytes());
                    punch_pkt.extend_from_slice(&crate::gateway::get_my_user_id().to_be_bytes());
                    punch_pkt.push(0);
                    punch_pkt.push(2);
                    punch_pkt.push(0);
                    punch_pkt.extend_from_slice(&get_my_rx_port().to_be_bytes());

                    for &target in &addrs_to_punch {
                        info!("🥊 [UDP HOLE PUNCH] Enviando burst de STUN/Punch (10 pacotes) para {} a partir da sinalização", target);
                        for _ in 0..10 {
                            let _ = socket.send_to(&punch_pkt, target);
                        }
                    }
                }
            }
        }
    }
}

pub fn handle_incoming_ws_message(
    msg: tokio_tungstenite::tungstenite::Message,
    current_cid: u64,
    my_inst: u32,
    peers_store: &Arc<Mutex<HashMap<u64, (SocketAddr, Instant)>>>,
) {
    let key = get_voice_encryption_key(current_cid);

    match msg {
        tokio_tungstenite::tungstenite::Message::Binary(bytes) => {
            if let Some(decrypted) = decrypt_signaling_payload(&key, &bytes) {
                process_signaling_json(&decrypted, current_cid, my_inst, peers_store);
            } else if let Some(json_start) = bytes.iter().position(|&b| b == b'{') {
                if let Some(json_end) = bytes.iter().rposition(|&b| b == b'}') {
                    if json_end >= json_start {
                        process_signaling_json(&bytes[json_start..=json_end], current_cid, my_inst, peers_store);
                    }
                }
            }
        }
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            let text_bytes = text.as_bytes();
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                if val["op"] == "peer_leave" {
                    let pkt_uid = val["uid"].as_u64().unwrap_or(0);
                    if pkt_uid != 0 {
                        info!("🚪 [SIGNALING] Peer UID={} saiu da sala de sinalização Cloudflare.", pkt_uid);
                        trigger_stream_stopped(pkt_uid);
                        if let Ok(mut peers) = peers_store.lock() {
                            peers.retain(|&k, _| ((k & 0xFFFF_FFFF) as u32) != (pkt_uid as u32));
                        }
                        if let Ok(mut guard) = LAST_SEEN_PEER_ADDR.lock() {
                            if let Some(map) = guard.as_mut() {
                                map.remove(&pkt_uid);
                            }
                        }
                    }
                    return;
                } else if val["op"] == "pong" {
                    return;
                }
            }
            process_signaling_json(text_bytes, current_cid, my_inst, peers_store);
        }
        tokio_tungstenite::tungstenite::Message::Ping(_) => {
            // tungstenite responde pong automaticamente a frames ping RFC 6455
        }
        _ => {}
    }
}

pub async fn run_cloudflare_signaling_loop(
    channel_id: Arc<AtomicU64>,
    my_user_id: Arc<AtomicU64>,
    my_username: Arc<Mutex<String>>,
    is_tx_running: Arc<AtomicBool>,
    peers_store: Arc<Mutex<HashMap<u64, (SocketAddr, Instant)>>>,
) {
    let my_inst = get_process_instance_id();
    let cf_host = "litecord-signal.ricosgames-henrique.workers.dev";

    loop {
        let mut cid = channel_id.load(Ordering::Relaxed);
        if cid == 0 {
            cid = crate::gateway::get_my_voice_channel_id();
            if cid > 0 {
                channel_id.store(cid, Ordering::Relaxed);
            }
        }
        if cid == 0 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        }

        let mut my_uid = my_user_id.load(Ordering::Relaxed);
        if my_uid == 0 {
            my_uid = crate::gateway::get_my_user_id();
            if my_uid != 0 {
                my_user_id.store(my_uid, Ordering::Relaxed);
            }
        }
        if my_uid == 0 {
            for _ in 0..15 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                my_uid = crate::gateway::get_my_user_id();
                if my_uid != 0 {
                    my_user_id.store(my_uid, Ordering::Relaxed);
                    break;
                }
            }
        }

        let ws_url = format!("wss://{}/ws?channel={}&user={}", cf_host, cid, my_uid);
        info!("📡 [SIGNALING] Conectando ao Cloudflare Worker WebSocket em {}...", ws_url);

        let mut req = match ws_url.as_str().into_client_request() {
            Ok(r) => r,
            Err(e) => {
                warn!("⚠️ [SIGNALING] Falha ao construir requisição WebSocket: {:?}", e);
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        req.headers_mut().insert("User-Agent", "Litecord/1.0.0-beta".parse().unwrap());

        let (ws_stream, _resp) = match connect_async(req).await {
            Ok((s, r)) => {
                info!("🛡️ [SIGNALING] Conectado com sucesso ao Cloudflare Edge Signaling (HTTP {}) para canal {}", r.status(), cid);
                (s, r)
            }
            Err(e) => {
                warn!("⚠️ [SIGNALING] Falha na conexão WebSocket com Cloudflare: {:?}. Reconectando em 2s...", e);
                invalidate_cached_stun_address();
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        let (mut ws_write, mut ws_read) = ws_stream.split();
        let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<tokio_tungstenite::tungstenite::Message>();
        if let Ok(mut guard) = SIGNALING_OUT_TX.lock() {
            *guard = Some(out_tx);
        }

        let mut last_pub = Instant::now() - Duration::from_secs(10);
        let mut last_tx_state = false;
        let mut ping_interval = tokio::time::interval(Duration::from_secs(15));
        ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        let mut publish_ticker = tokio::time::interval(Duration::from_millis(300));
        publish_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            if channel_id.load(Ordering::Relaxed) != cid {
                info!("🛑 Canal de voz alterado. Reconectando sala de sinalização...");
                break;
            }

            let fresh_uid = crate::gateway::get_my_user_id();
            if my_uid == 0 && fresh_uid != 0 {
                my_user_id.store(fresh_uid, Ordering::Relaxed);
                info!("🔄 [SIGNALING] User ID autenticado ({}). Atualizando conexão com Cloudflare...", fresh_uid);
                break;
            }

            tokio::select! {
                Some(msg) = out_rx.recv() => {
                    if let Err(e) = ws_write.send(msg).await {
                        warn!("⚠️ Falha ao enviar mensagem de sinalização no WebSocket: {:?}", e);
                        break;
                    }
                }

                _ = ping_interval.tick() => {
                    if let Err(e) = ws_write.send(tokio_tungstenite::tungstenite::Message::Text("{\"op\":\"ping\"}".into())).await {
                        warn!("⚠️ Falha ao enviar keepalive ping no WebSocket: {:?}", e);
                        break;
                    }
                }

                _ = publish_ticker.tick() => {
                    let cur_tx = is_tx_running.load(Ordering::Relaxed);
                    let force_wake = SIGNALING_FORCE_WAKE.swap(false, Ordering::Relaxed);

                    let should_publish = force_wake || cur_tx != last_tx_state || last_pub.elapsed() >= Duration::from_secs(2);
                    if should_publish {
                        last_pub = Instant::now();
                        last_tx_state = cur_tx;

                        let mut uid = my_user_id.load(Ordering::Relaxed);
                        if uid == 0 {
                            uid = crate::gateway::get_my_user_id();
                        }

                        let my_rx_port = get_my_rx_port();
                        let wan_addr = resolve_public_stun_address();
                        let lan_addrs = get_local_lan_addresses(my_rx_port);

                        let my_ecdh_pub = get_or_create_local_ecdh_keypair();
                        let ecdh_hex: String = my_ecdh_pub.iter().map(|b| format!("{:02x}", b)).collect();

                        let uname = my_username.lock().unwrap_or_else(|e| e.into_inner()).clone();
                        let payload = serde_json::json!({
                            "op": "presence",
                            "cid": cid,
                            "uid": uid,
                            "inst": my_inst,
                            "uname": uname,
                            "ecdh_pub": ecdh_hex,
                            "streaming": cur_tx,
                            "rx_port": my_rx_port,
                            "lan_ips": lan_addrs.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                            "wan_ip": wan_addr.map(|a| a.to_string()),
                        });

                        let current_key = get_voice_encryption_key(cid);
                        if let Some(enc_payload) = encrypt_signaling_payload(&current_key, payload.to_string().as_bytes()) {
                            if let Err(e) = ws_write.send(tokio_tungstenite::tungstenite::Message::Binary(enc_payload.into())).await {
                                warn!("⚠️ Falha ao enviar presença no WebSocket: {:?}", e);
                                break;
                            }
                        }
                    }
                }

                msg_res = ws_read.next() => {
                    match msg_res {
                        Some(Ok(msg)) => {
                            handle_incoming_ws_message(msg, cid, my_inst, &peers_store);
                        }
                        Some(Err(e)) => {
                            warn!("⚠️ Erro de leitura no WebSocket da sinalização: {:?}", e);
                            break;
                        }
                        None => {
                            warn!("⚠️ Servidor de sinalização WebSocket fechou a conexão.");
                            break;
                        }
                    }
                }
            }
        }

        if let Ok(mut guard) = SIGNALING_OUT_TX.lock() {
            *guard = None;
        }
        invalidate_cached_stun_address();
        info!("🛑 Desconectado da sala de sinalização Cloudflare para o canal {}. Reconectando...", cid);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

pub fn start_global_signaling(
    channel_id: Arc<AtomicU64>,
    my_user_id: Arc<AtomicU64>,
    my_username: Arc<Mutex<String>>,
    is_tx_running: Arc<AtomicBool>,
    peers_store: Arc<Mutex<HashMap<u64, (SocketAddr, Instant)>>>,
) {
    std::thread::Builder::new()
        .name("global-p2p-signaling".to_string())
        .spawn(move || {
            crate::cpu_profiler::set_current_thread_name("global-p2p-signaling");
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(r) => r,
                Err(e) => {
                    error!("Falha ao iniciar runtime Tokio para sinalização Cloudflare: {:?}", e);
                    return;
                }
            };
            rt.block_on(run_cloudflare_signaling_loop(
                channel_id,
                my_user_id,
                my_username,
                is_tx_running,
                peers_store,
            ));
        })
        .expect("Falha ao iniciar thread de sinalização global P2P");
}
