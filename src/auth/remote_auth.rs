use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use rsa::{RsaPrivateKey, RsaPublicKey, Oaep, pkcs8::EncodePublicKey};
use sha2::{Sha256, Digest};
use base64::Engine;
use log::{info, error};

#[derive(Debug, Clone)]
pub enum RemoteAuthEvent {
    QrCodeUrl(String),
    UserScanned { username: String },
    TokenReceived(String),
    Cancelled,
    Error(String),
}

pub fn generate_qr_image(url: &str) -> Result<slint::Image, String> {
    use qrcode::QrCode;
    use qrcode::types::Color;

    let code = QrCode::new(url.as_bytes()).map_err(|e| format!("QR Code gen error: {:?}", e))?;
    let image_width = code.width();
    let scale = 5;
    let quiet_zone = 3;
    let total_width = (image_width + quiet_zone * 2) * scale;
    let mut raw_bytes = vec![255u8; (total_width * total_width * 3) as usize];

    // Draw dark modules with dark Discord color #111214 (r: 17, g: 18, b: 20)
    for (y, row) in code.to_colors().chunks(image_width).enumerate() {
        for (x, &color) in row.iter().enumerate() {
            if color == Color::Dark {
                let start_x = (x + quiet_zone) * scale;
                let start_y = (y + quiet_zone) * scale;
                for py in start_y..(start_y + scale) {
                    for px in start_x..(start_x + scale) {
                        let idx = ((py * total_width + px) * 3) as usize;
                        if idx + 2 < raw_bytes.len() {
                            raw_bytes[idx] = 17;     // R
                            raw_bytes[idx + 1] = 18; // G
                            raw_bytes[idx + 2] = 20; // B
                        }
                    }
                }
            }
        }
    }

    let pixel_buffer = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::clone_from_slice(
        &raw_bytes,
        total_width as u32,
        total_width as u32,
    );

    Ok(slint::Image::from_rgb8(pixel_buffer))
}

fn generate_rsa_key_pair() -> Result<(RsaPrivateKey, String), String> {
    let mut rng = rand::thread_rng();
    let private_key = RsaPrivateKey::new(&mut rng, 2048)
        .map_err(|e| format!("Falha ao gerar chave RSA 2048: {:?}", e))?;
    let public_key = RsaPublicKey::from(&private_key);

    let spki_der = public_key.to_public_key_der()
        .map_err(|e| format!("Falha ao exportar SPKI DER: {:?}", e))?;
    let encoded_public_key = base64::engine::general_purpose::STANDARD.encode(spki_der.as_bytes());

    Ok((private_key, encoded_public_key))
}

pub struct RemoteAuthSession {
    cancel_flag: Arc<AtomicBool>,
}

impl RemoteAuthSession {
    pub fn start(event_tx: mpsc::Sender<RemoteAuthEvent>) -> Self {
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let cancel_clone = Arc::clone(&cancel_flag);

        tokio::spawn(async move {
            if let Err(e) = run_remote_auth_loop(event_tx.clone(), cancel_clone).await {
                error!("❌ Remote Auth session error: {}", e);
                let _ = event_tx.send(RemoteAuthEvent::Error(e)).await;
            }
        });

        Self { cancel_flag }
    }

    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
    }
}

async fn exchange_ticket_for_token(
    http_client: &reqwest::Client,
    ticket: &str,
    private_key: &RsaPrivateKey,
) -> Result<String, String> {
    let res = http_client.post("https://discord.com/api/v9/users/@me/remote-auth/login")
        .header("Content-Type", "application/json")
        .header("Origin", "https://discord.com")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36")
        .json(&serde_json::json!({ "ticket": ticket }))
        .send()
        .await
        .map_err(|e| format!("Erro HTTP ao trocar ticket: {:?}", e))?;

    let json: serde_json::Value = res.json()
        .await
        .map_err(|e| format!("Erro JSON na resposta de login: {:?}", e))?;

    if let Some(enc_token_b64) = json["encrypted_token"].as_str() {
        let enc_bytes = base64::engine::general_purpose::STANDARD.decode(enc_token_b64)
            .map_err(|e| format!("Erro ao decodificar Base64 do token: {:?}", e))?;
        let decrypted = private_key.decrypt(Oaep::new::<Sha256>(), &enc_bytes)
            .map_err(|e| format!("Erro ao decifrar token: {:?}", e))?;
        let token_str = String::from_utf8(decrypted)
            .map_err(|e| format!("Token inválido UTF-8: {:?}", e))?;
        Ok(token_str)
    } else {
        Err(format!("Resposta sem encrypted_token: {:?}", json))
    }
}

async fn run_remote_auth_loop(
    event_tx: mpsc::Sender<RemoteAuthEvent>,
    cancel_flag: Arc<AtomicBool>,
) -> Result<(), String> {
    info!("📱 Iniciando sessão de Login por QR Code (Discord Remote Auth v2)...");

    // 1. Generate RSA 2048-bit key pair (in synchronous block so rng is not held across await)
    let (private_key, encoded_public_key) = generate_rsa_key_pair()?;

    // 2. Connect to Discord Remote Auth Gateway WebSocket with browser Origin header
    let req = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri("wss://remote-auth-gateway.discord.gg/?v=2")
        .header("Host", "remote-auth-gateway.discord.gg")
        .header("Origin", "https://discord.com")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", tokio_tungstenite::tungstenite::handshake::client::generate_key())
        .body(())
        .map_err(|e| format!("Erro ao criar requisição WS: {:?}", e))?;

    let (ws_stream, _) = connect_async(req).await
        .map_err(|e| format!("Falha ao conectar no Remote Auth Gateway: {:?}", e))?;

    info!("📡 Conectado ao Discord Remote Auth Gateway!");

    let (write, mut read) = ws_stream.split();

    while let Some(msg_res) = read.next().await {
        if cancel_flag.load(Ordering::SeqCst) {
            info!("🚫 Sessão de QR Code cancelada pelo usuário.");
            let _ = event_tx.send(RemoteAuthEvent::Cancelled).await;
            break;
        }

        let msg = match msg_res {
            Ok(m) => m,
            Err(e) => return Err(format!("Erro de leitura WebSocket: {:?}", e)),
        };

        if let Message::Text(text) = msg {
            let json: serde_json::Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let op = json["op"].as_str().unwrap_or("");
            match op {
                "hello" => {
                    let hb_interval = json["heartbeat_interval"].as_u64().unwrap_or(41250);
                    let write_arc = Arc::new(tokio::sync::Mutex::new(write));
                    let write_clone = Arc::clone(&write_arc);
                    let cancel_hb = Arc::clone(&cancel_flag);

                    // Spawn Heartbeat Task
                    tokio::spawn(async move {
                        loop {
                            sleep(Duration::from_millis(hb_interval)).await;
                            if cancel_hb.load(Ordering::SeqCst) { break; }
                            let hb_msg = serde_json::json!({ "op": "heartbeat" });
                            let mut w = write_clone.lock().await;
                            if let Err(_) = w.send(Message::Text(hb_msg.to_string().into())).await {
                                break;
                            }
                        }
                    });

                    // Send Init payload with our encoded RSA Public Key
                    let init_payload = serde_json::json!({
                        "op": "init",
                        "encoded_public_key": encoded_public_key
                    });
                    let mut w = write_arc.lock().await;
                    w.send(Message::Text(init_payload.to_string().into())).await
                        .map_err(|e| format!("Falha ao enviar init payload: {:?}", e))?;
                    drop(w);
                    return handle_auth_steps(read, write_arc, private_key, event_tx, cancel_flag).await;
                }
                _ => {}
            }
        }
    }

    Ok(())
}

async fn handle_auth_steps(
    mut read: futures_util::stream::SplitStream<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>>,
    write_arc: Arc<tokio::sync::Mutex<futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, Message>>>,
    private_key: RsaPrivateKey,
    event_tx: mpsc::Sender<RemoteAuthEvent>,
    cancel_flag: Arc<AtomicBool>,
) -> Result<(), String> {
    let http_client = reqwest::Client::new();

    while let Some(msg_res) = read.next().await {
        if cancel_flag.load(Ordering::SeqCst) {
            let _ = event_tx.send(RemoteAuthEvent::Cancelled).await;
            break;
        }

        let msg = match msg_res {
            Ok(m) => m,
            Err(e) => return Err(format!("Erro WebSocket: {:?}", e)),
        };

        if let Message::Text(text) = msg {
            let json: serde_json::Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let op = json["op"].as_str().unwrap_or("");
            match op {
                "nonce_proof" => {
                    if let Some(enc_nonce_b64) = json["encrypted_nonce"].as_str() {
                        if let Ok(enc_bytes) = base64::engine::general_purpose::STANDARD.decode(enc_nonce_b64) {
                            if let Ok(decrypted) = private_key.decrypt(Oaep::new::<Sha256>(), &enc_bytes) {
                                let hash = Sha256::digest(&decrypted);
                                let proof = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash);

                                let proof_msg = serde_json::json!({
                                    "op": "nonce_proof",
                                    "proof": proof
                                });
                                let mut w = write_arc.lock().await;
                                let _ = w.send(Message::Text(proof_msg.to_string().into())).await;
                            }
                        }
                    }
                }
                "fingerprint" | "pending_remote_init" if json["fingerprint"].is_string() => {
                    if let Some(fingerprint) = json["fingerprint"].as_str() {
                        let qr_url = format!("https://discord.com/ra/{}", fingerprint);
                        info!("📷 QR Code URL gerada com sucesso: {}", qr_url);
                        let _ = event_tx.send(RemoteAuthEvent::QrCodeUrl(qr_url)).await;
                    }
                }
                "pending_ticket" => {
                    // Mobile client scanned the QR code! Decrypt the user info payload
                    if let Some(enc_user_payload) = json["ticket"].as_str() {
                        if let Ok(enc_bytes) = base64::engine::general_purpose::STANDARD.decode(enc_user_payload) {
                            if let Ok(decrypted) = private_key.decrypt(Oaep::new::<Sha256>(), &enc_bytes) {
                                if let Ok(user_info) = String::from_utf8(decrypted) {
                                    // format: user_id:discriminator:avatar:username
                                    let parts: Vec<&str> = user_info.split(':').collect();
                                    let username = if parts.len() >= 4 { parts[3].to_string() } else { user_info };
                                    info!("📲 QR Code escaneado pelo usuário: {}!", username);
                                    let _ = event_tx.send(RemoteAuthEvent::UserScanned { username }).await;
                                }
                            }
                        }
                    }
                }
                "pending_login" => {
                    // User tapped "Yes, Log In" on mobile! Exchange ticket via REST API
                    if let Some(ticket) = json["ticket"].as_str() {
                        info!("🔑 Usuário aprovou o login no celular! Trocando ticket pelo token...");
                        match exchange_ticket_for_token(&http_client, ticket, &private_key).await {
                            Ok(token) => {
                                info!("🎉 Token de acesso recebido via QR Code com sucesso!");
                                let _ = event_tx.send(RemoteAuthEvent::TokenReceived(token)).await;
                                return Ok(());
                            }
                            Err(e) => {
                                error!("❌ Falha ao trocar ticket por token: {}", e);
                                let _ = event_tx.send(RemoteAuthEvent::Error(e)).await;
                            }
                        }
                    }
                }
                "finish" | "pending_finish" => {
                    let enc_token_opt = json["encrypted_token"].as_str()
                        .or_else(|| json["encrypted_user_payload"].as_str());

                    if let Some(enc_token_b64) = enc_token_opt {
                        if let Ok(enc_bytes) = base64::engine::general_purpose::STANDARD.decode(enc_token_b64) {
                            if let Ok(decrypted) = private_key.decrypt(Oaep::new::<Sha256>(), &enc_bytes) {
                                if let Ok(token_str) = String::from_utf8(decrypted) {
                                    info!("🎉 Token de acesso recebido via finish opcode!");
                                    let _ = event_tx.send(RemoteAuthEvent::TokenReceived(token_str)).await;
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
                "cancel" => {
                    info!("❌ Login por QR Code rejeitado no celular.");
                    let _ = event_tx.send(RemoteAuthEvent::Cancelled).await;
                    return Ok(());
                }
                _ => {}
            }
        }
    }
    Ok(())
}
