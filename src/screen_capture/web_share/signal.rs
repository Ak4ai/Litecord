use super::*;
use aes_gcm::{aead::{Aead, KeyInit}, Aes256Gcm, Nonce};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
pub(super) async fn connect(base: &str, room: &str, token: &str) -> Result<Socket, String> {
    let url = format!("{}/web/ws?room={room}&role=host&token={token}", base.trim_end_matches('/'));
    let (mut ws, _) = tokio::time::timeout(Duration::from_secs(8), connect_async(url)).await
        .map_err(|_| "Web sharing server timed out".to_string())?
        .map_err(|_| "Could not connect to the web sharing server; check that its web-sharing update is deployed".to_string())?;
    let hello = tokio::time::timeout(Duration::from_secs(5), ws.next()).await
        .map_err(|_| "Web sharing server did not register this stream".to_string())?;
    match hello {
        Some(Ok(Message::Text(text))) if serde_json::from_str::<Value>(&text).ok().is_some_and(|v| v["op"] == "ready") => Ok(ws),
        _ => Err("Web sharing server refused this stream".into()),
    }
}
pub(super) async fn run(mut ws: Socket, key: [u8; 32], commands: mpsc::Sender<Command>, mut stop: watch::Receiver<bool>) -> Result<(), String> {
    let mut ping = tokio::time::interval(Duration::from_secs(40));
    loop {
        tokio::select! {
            _ = stop.changed() => { let _ = ws.close(None).await; return Ok(()); },
            _ = ping.tick() => {
                ws.send(Message::Text("{\"op\":\"ping\"}".into())).await.map_err(|_| "Web sharing connection lost".to_string())?;
            },
            message = ws.next() => {
                let Some(Ok(message)) = message else { return Err("Web sharing connection lost; generate a new link to reconnect".into()); };
                let Message::Text(text) = message else { continue; };
                let Ok(v) = serde_json::from_str::<Value>(&text) else { continue; };
                let id = v["id"].as_str().unwrap_or("").to_owned();
                if v["op"] == "leave" { let _ = commands.send(Command::Leave(id)).await; continue; }
                if v["op"] != "offer" || id.len() != 32 { continue; }
                let Some(offer) = v["payload"].as_str().and_then(|s| decrypt(&key, s))
                    .and_then(|b| serde_json::from_slice::<SdpOffer>(&b).ok()) else {
                    ws.send(Message::Text(json!({"op":"reject", "id":id, "status":403}).to_string().into())).await.ok();
                    continue;
                };
                let (reply, rx) = oneshot::channel();
                if commands.send(Command::Offer { id: id.clone(), offer, reply }).await.is_err() { return Ok(()); }
                let response = match tokio::time::timeout(Duration::from_secs(5), rx).await {
                    Ok(Ok(Ok(answer))) => {
                        let bytes = serde_json::to_vec(&answer).map_err(|e| e.to_string())?;
                        json!({"op":"answer", "id":id, "payload":encrypt(&key, &bytes).ok_or("Could not encrypt answer")?})
                    },
                    Ok(Ok(Err(status))) => json!({"op":"reject", "id":id, "status":status}),
                    _ => json!({"op":"reject", "id":id, "status":503}),
                };
                ws.send(Message::Text(response.to_string().into())).await.map_err(|_| "Web sharing connection lost".to_string())?;
            }
        }
    }
}
pub(super) fn encrypt(key: &[u8; 32], bytes: &[u8]) -> Option<String> {
    let nonce: [u8; 12] = random_bytes().ok()?;
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    let mut out = nonce.to_vec();
    out.extend(cipher.encrypt(Nonce::from_slice(&nonce), bytes).ok()?);
    Some(STANDARD.encode(out))
}
pub(super) fn decrypt(key: &[u8; 32], text: &str) -> Option<Vec<u8>> {
    if text.len() > 96 * 1024 { return None; }
    let bytes = STANDARD.decode(text).ok()?;
    if bytes.len() < 28 { return None; }
    Aes256Gcm::new_from_slice(key).ok()?.decrypt(Nonce::from_slice(&bytes[..12]), &bytes[12..]).ok()
}
/// Use the actual WebRTC socket, never the separate native P2P socket's mapping.
pub(super) fn discover_public_address(socket: &UdpSocket) -> Option<SocketAddr> {
    use std::net::ToSocketAddrs;
    let mut req = [0u8; 20];
    req[..2].copy_from_slice(&[0, 1]);
    req[4..8].copy_from_slice(&[0x21, 0x12, 0xa4, 0x42]);
    req[8..].copy_from_slice(&random_bytes::<12>().ok()?);
    let mut buf = [0u8; 1024];
    for server in ["stun.cloudflare.com:3478", "stun.l.google.com:19302"] {
        let Some(dest) = server.to_socket_addrs().ok().and_then(|mut a| a.find(|a| a.is_ipv4())) else { continue; };
        for _ in 0..2 {
            let _ = socket.set_read_timeout(Some(Duration::from_millis(500)));
            let _ = socket.send_to(&req, dest);
            if let Ok((len, source)) = socket.recv_from(&mut buf) {
                if source == dest && len >= 20 && buf[8..20] == req[8..20] {
                    if let Some(addr) = super::super::net_diagnostics::parse_mapped_address(&buf[..len]) { return Some(addr); }
                }
            }
        }
    }
    warn!("[WEB SHARE] STUN did not respond; only local candidates are available.");
    None
}

pub(super) async fn refresh_public_address(socket: &tokio::net::UdpSocket) -> Option<SocketAddr> {
    let query = async {
        let dest = tokio::net::lookup_host("stun.cloudflare.com:3478").await.ok()?.find(|a| a.is_ipv4())?;
        let mut req = [0u8; 20];
        req[..2].copy_from_slice(&[0, 1]);
        req[4..8].copy_from_slice(&[0x21, 0x12, 0xa4, 0x42]);
        req[8..].copy_from_slice(&random_bytes::<12>().ok()?);
        socket.send_to(&req, dest).await.ok()?;
        let mut buf = [0u8; 1024];
        loop {
            let (len, source) = socket.recv_from(&mut buf).await.ok()?;
            if source == dest && len >= 20 && buf[8..20] == req[8..20] {
                return super::super::net_diagnostics::parse_mapped_address(&buf[..len]);
            }
        }
    };
    tokio::time::timeout(Duration::from_millis(800), query).await.ok().flatten()
}
