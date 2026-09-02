use openh264::decoder::Decoder;
use openh264::encoder::{Encoder, EncoderConfig};
use openh264::formats::{YUVSlices, YUVSource};
use openh264::OpenH264API;
use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

const CHUNK_SIZE: usize = 1200;
const MAGIC: &[u8; 4] = &[0x4C, 0x54, 0x43, 0x44]; // LTCD
const OP_VIDEO_CHUNK: u8 = 2;
const OP_PUNCH: u8 = 99;

fn query_stun_server(socket: &UdpSocket, server: &str) -> Option<SocketAddr> {
    let addrs: Vec<SocketAddr> = server.to_socket_addrs().ok()?.collect();
    let stun_addr = addrs.iter().find(|a| a.is_ipv4())?;

    let stun_req: [u8; 20] = [
        0x00, 0x01, // Binding Request
        0x00, 0x00, // Length
        0x21, 0x12, 0xa4, 0x42, // Magic Cookie
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
    ];

    let _ = socket.send_to(&stun_req, stun_addr);

    let mut buf = [0u8; 1024];
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(1000) {
        if let Ok((len, src)) = socket.recv_from(&mut buf) {
            if src.ip() == stun_addr.ip() && len >= 20 && buf[0] == 0x01 && buf[1] == 0x01 {
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
                        return Some(SocketAddr::new(std::net::IpAddr::V4(ip), port));
                    }
                    i += 4 + ((attr_len + 3) & !3);
                }
            }
        }
    }
    None
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(|s| s.as_str()).unwrap_or("receiver");
    let local_port: u16 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(50005);
    let target_str = args.get(3).map(|s| s.as_str());

    println!("==================================================================");
    println!("🌐 TEST_P2P_WAN: Teste Direto pela Internet Pública (Modo: {})", mode.to_uppercase());
    println!("==================================================================");

    let socket = UdpSocket::bind(format!("0.0.0.0:{}", local_port))
        .or_else(|_| UdpSocket::bind("0.0.0.0:0"))
        .expect("Falha ao abrir socket UDP local");
    socket.set_read_timeout(Some(Duration::from_millis(10))).unwrap();
    println!("🎧 Porta Local Vinculada: {}", socket.local_addr().unwrap().port());

    // Resolve IP público via STUN
    let mut my_wan_addr = None;
    for server in ["stun.l.google.com:19302", "stun.cloudflare.com:3478"] {
        if let Some(wan) = query_stun_server(&socket, server) {
            println!("🌐 [STUN] Meu IP:Porta Público (WAN): {}", wan);
            my_wan_addr = Some(wan);
            break;
        }
    }

    let is_sender = mode == "sender";

    if let Some(target_addr_str) = target_str {
        if let Ok(target_addr) = target_addr_str.parse::<SocketAddr>() {
            println!("🎯 Endereço de destino configurado: {}", target_addr);

            // 1. Hole Punching Burst
            println!("🥊 Enviando pacotes de Hole Punching para {}...", target_addr);
            let mut punch_pkt = Vec::new();
            punch_pkt.extend_from_slice(MAGIC);
            punch_pkt.push(OP_PUNCH);
            punch_pkt.extend_from_slice(b"PUNCH_BURST");

            for _ in 0..10 {
                let _ = socket.send_to(&punch_pkt, target_addr);
                std::thread::sleep(Duration::from_millis(20));
            }

            let mut remote_peer = target_addr;
            let mut recv_buf = [0u8; 65535];
            let start = Instant::now();

            if is_sender {
                println!("🎥 Transmitindo fluxo H.264 720p/60fps com Sunshine Pacing via WAN Pública...");
                let mut config = EncoderConfig::new();
                config.set_bitrate_bps(4_500_000);
                let mut encoder = Encoder::with_api_config(OpenH264API::from_source(), config).unwrap();

                let w = 1280usize;
                let h = 720usize;
                let mut y_plane = vec![128u8; w * h];
                let u_plane = vec![128u8; (w / 2) * (h / 2)];
                let v_plane = vec![128u8; (w / 2) * (h / 2)];
                let mut seq = 0u32;
                let mut cur_bitrate = 4_500_000u32;

                while start.elapsed() < Duration::from_secs(10) {
                    if let Ok((len, src)) = socket.recv_from(&mut recv_buf) {
                        if len >= 5 && &recv_buf[0..4] == MAGIC {
                            if src != remote_peer {
                                println!("🎯 [SENDER] Peer remoto conectado via WAN: {}", src);
                                remote_peer = src;
                            }
                        }
                    }

                    // Dinâmica de bitrate a cada 3 segundos
                    if seq == 60 {
                        cur_bitrate = 3_500_000;
                        let mut cfg = EncoderConfig::new();
                        cfg.set_bitrate_bps(cur_bitrate);
                        if let Ok(new_enc) = Encoder::with_api_config(OpenH264API::from_source(), cfg) {
                            encoder = new_enc;
                            encoder.force_intra_frame();
                            println!("📉 [BITRATE ADAPTATIVO] Ajustando taxa para 3.50 Mbps...");
                        }
                    } else if seq == 180 {
                        cur_bitrate = 5_000_000;
                        let mut cfg = EncoderConfig::new();
                        cfg.set_bitrate_bps(cur_bitrate);
                        if let Ok(new_enc) = Encoder::with_api_config(OpenH264API::from_source(), cfg) {
                            encoder = new_enc;
                            encoder.force_intra_frame();
                            println!("📈 [BITRATE ADAPTATIVO] Ajustando taxa para 5.00 Mbps...");
                        }
                    }

                    seq = seq.wrapping_add(1);
                    let offset = (seq * 4) as u8;
                    for b in y_plane.iter_mut() {
                        *b = b.wrapping_add(offset);
                    }

                    if seq == 1 || seq % 30 == 0 {
                        encoder.force_intra_frame();
                    }

                    let yuv = YUVSlices::new((&y_plane, &u_plane, &v_plane), (w, h), (w, w / 2, w / 2));
                    if let Ok(stream) = encoder.encode(&yuv) {
                        let frame_bytes = stream.to_vec();
                        let total_chunks = ((frame_bytes.len() + CHUNK_SIZE - 1) / CHUNK_SIZE) as u16;

                        for (idx, chunk) in frame_bytes.chunks(CHUNK_SIZE).enumerate() {
                            let mut pkt = Vec::with_capacity(37 + chunk.len());
                            pkt.extend_from_slice(MAGIC);
                            pkt.extend_from_slice(&1u32.to_be_bytes());
                            pkt.push(OP_VIDEO_CHUNK);
                            pkt.extend_from_slice(&1u64.to_be_bytes());
                            pkt.extend_from_slice(&1u64.to_be_bytes());
                            pkt.extend_from_slice(&seq.to_be_bytes());
                            pkt.extend_from_slice(&(start.elapsed().as_millis() as u32).to_be_bytes());
                            pkt.extend_from_slice(&total_chunks.to_be_bytes());
                            pkt.extend_from_slice(&(idx as u16).to_be_bytes());
                            pkt.extend_from_slice(chunk);

                            let _ = socket.send_to(&pkt, remote_peer);

                            // Sunshine micro-pacing: pausa microscópica a cada 4 pacotes
                            if (idx + 1) % 4 == 0 && (idx + 1) < total_chunks as usize {
                                std::thread::yield_now();
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_micros(16666));
                }
                println!("🏁 Envio concluído com sucesso!");
            } else {
                println!("📥 Aguardando e decodificando fluxo de vídeo pela Internet...");
                let mut decoder = Decoder::new().unwrap();
                let mut in_flight: HashMap<u32, (u16, HashMap<u16, Vec<u8>>)> = HashMap::new();
                let mut frames_ok = 0u32;
                let mut last_stat = Instant::now();
                let mut window_frames = 0u32;

                while start.elapsed() < Duration::from_secs(12) {
                    if let Ok((len, src)) = socket.recv_from(&mut recv_buf) {
                        if len < 37 || &recv_buf[0..4] != MAGIC { continue; }
                        let op = recv_buf[8];
                        if op == OP_PUNCH {
                            let _ = socket.send_to(&punch_pkt, src);
                            continue;
                        }
                        if op == OP_VIDEO_CHUNK {
                            let seq = u32::from_be_bytes(recv_buf[25..29].try_into().unwrap());
                            let total = u16::from_be_bytes(recv_buf[33..35].try_into().unwrap());
                            let idx = u16::from_be_bytes(recv_buf[35..37].try_into().unwrap());
                            let chunk_data = recv_buf[37..len].to_vec();

                            let entry = in_flight.entry(seq).or_insert_with(|| (total, HashMap::new()));
                            entry.1.insert(idx, chunk_data);

                            if entry.1.len() == total as usize {
                                let mut full_frame = Vec::new();
                                for i in 0..total {
                                    if let Some(c) = entry.1.get(&i) {
                                        full_frame.extend_from_slice(c);
                                    }
                                }
                                in_flight.remove(&seq);

                                let t_dec = Instant::now();
                                if let Ok(Some(yuv)) = decoder.decode(&full_frame) {
                                    frames_ok += 1;
                                    window_frames += 1;
                                    let dims = yuv.dimensions();
                                    let dec_ms = t_dec.elapsed().as_micros() as f64 / 1000.0;

                                    if last_stat.elapsed() >= Duration::from_secs(1) {
                                        let fps = (window_frames as f64) / last_stat.elapsed().as_secs_f64();
                                        println!("🎬 [RX WAN MONITOR] {:.1} FPS | {}x{} | Decode: {:.2}ms | Total OK: {}",
                                            fps, dims.0, dims.1, dec_ms, frames_ok);
                                        window_frames = 0;
                                        last_stat = Instant::now();
                                    }
                                }
                            }
                        }
                    }
                }
                println!("🏁 Recepção finalizada! Total de frames decodificados com sucesso: {}", frames_ok);
            }
        }
    }
}
