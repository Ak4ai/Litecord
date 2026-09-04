#[cfg(windows)]
use std::net::{SocketAddr, UdpSocket};
#[cfg(windows)]
use std::time::{Duration, Instant};

#[cfg(windows)]
#[path = "../gpu_encoder.rs"]
mod gpu_encoder;
#[cfg(windows)]
use gpu_encoder::VideoEncoder;
#[cfg(windows)]
use gpu_encoder::amd_amf::AmdAmfZeroCopyEncoder;

#[cfg(not(windows))]
fn main() {
    println!("test_amd_amf_sender is only supported on Windows.");
}

#[cfg(windows)]
fn main() {
    println!("==================================================================");
    println!("🚀 LITECORD | TEST_AMD_AMF_SENDER (AMF Native Zero-Copy Sender)");
    println!("==================================================================");

    let mut target_addr_str = "100.70.183.127:50006".to_string();
    let mut local_port = 50005;

    let args: Vec<String> = std::env::args().collect();
    for i in 1..args.len() {
        if args[i] == "--target" && i + 1 < args.len() {
            target_addr_str = args[i + 1].clone();
        } else if args[i] == "--port" && i + 1 < args.len() {
            local_port = args[i + 1].parse().unwrap_or(50005);
        }
    }

    println!("📡 Destino padrão UDP: {}", target_addr_str);
    println!("🎧 Porta Local: {}", local_port);

    let socket = match UdpSocket::bind(format!("0.0.0.0:{}", local_port)) {
        Ok(s) => s,
        Err(_) => UdpSocket::bind("0.0.0.0:0").expect("Falha ao abrir socket UDP"),
    };
    socket.set_nonblocking(true).unwrap();

    let width = 1920u32;
    let height = 1080u32;
    let target_fps = 60u32;

    println!("🛠️ Inicializando AmdAmfZeroCopyEncoder (1080p @ 60 FPS)...");
    let mut encoder = match AmdAmfZeroCopyEncoder::try_new(target_fps, true) {
        Ok(enc) => {
            println!("💎 AMD AMF Inicializado com SUCESSO na GPU: {}", enc.gpu_name);
            enc
        }
        Err(e) => {
            eprintln!("❌ Falha ao criar AmdAmfZeroCopyEncoder: {}", e);
            return;
        }
    };

    let mut bgra_frame = vec![0u8; (width * height * 4) as usize];
    let mut seq = 1u32;
    let sender_uid = 995123987032055918u64;
    let cid = 1310372456904654931u64;

    let frame_interval = Duration::from_micros(1_000_000 / target_fps as u64);
    let mut next_frame_time = Instant::now();
    let mut last_idr = Instant::now();

    let mut known_targets: Vec<SocketAddr> = Vec::new();
    if let Ok(addr) = target_addr_str.parse::<SocketAddr>() {
        known_targets.push(addr);
    }

    let mut rx_buf = [0u8; 2048];
    let mut keyframe_requested = true;
    let mut last_log = Instant::now();
    let mut frames_sent_window = 0u64;
    let mut bytes_sent_window = 0usize;

    println!("🚀 Transmissão iniciada! Pressione Ctrl+C para parar.");

    loop {
        // 1. Processar pacotes UDP recebidos (PLI / OP_KEYFRAME_REQ / ANNOUNCE)
        loop {
            match socket.recv_from(&mut rx_buf) {
                Ok((n, src)) => {
                    if !known_targets.contains(&src) {
                        println!("📡 [NOVO RECEPTOR DESCOBERTO] Conectado de: {}", src);
                        known_targets.push(src);
                    }

                    if n >= 9 && &rx_buf[..4] == MAGIC {
                        let op = rx_buf[8];
                        if op == OP_KEYFRAME_REQ {
                            println!("⚡ [PLI RECEBIDO] Receptor {} solicitou IDR Keyframe imediato!", src);
                            keyframe_requested = true;
                        } else if op == OP_ANNOUNCE {
                            println!("👋 [ANNOUNCE RECEBIDO] Receptor {} registrou presença!", src);
                            keyframe_requested = true;
                        }
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }

        // 2. Gerar padrão animado rápido no frame BGRA
        let color_offset = (seq * 4) as u8;
        for y in 0..height {
            let row_start = (y * width * 4) as usize;
            let bar_active = ((y + (seq * 4)) / 30) % 2 == 0;
            let b_val = if bar_active { 220 } else { 40 };
            let g_val = color_offset.wrapping_add((y % 255) as u8);
            let r_val = 120u8;

            for x in 0..width {
                let idx = row_start + (x as usize * 4);
                bgra_frame[idx] = b_val;
                bgra_frame[idx + 1] = g_val;
                bgra_frame[idx + 2] = r_val;
                bgra_frame[idx + 3] = 255;
            }
        }

        // 3. Forçar IDR se requisitado por PLI ou periodicamente
        if keyframe_requested || last_idr.elapsed() >= Duration::from_millis(2000) {
            encoder.force_intra_frame();
            last_idr = Instant::now();
            keyframe_requested = false;
        }

        // 4. Codificar frame na GPU AMD via Zero-Copy
        if let Some(encoded_bytes) = encoder.encode(&bgra_frame, width, height) {
            let total_chunks = ((encoded_bytes.len() + MAX_UDP_PAYLOAD - 1) / MAX_UDP_PAYLOAD).max(1);

            let mut nals = Vec::new();
            for w in encoded_bytes.windows(5) {
                if w[..4] == [0, 0, 0, 1] {
                    nals.push(w[4] & 0x1F);
                } else if w[..3] == [0, 0, 1] {
                    nals.push(w[3] & 0x1F);
                }
            }

            if nals.contains(&5) {
                println!("🔑 [TX IDR KEYFRAME] Seq #{} | Tamanho: {} bytes | NALs: {:?}", seq, encoded_bytes.len(), nals);
            }

            for (chunk_idx, slice) in encoded_bytes.chunks(MAX_UDP_PAYLOAD).enumerate() {
                let mut udp_pkt = Vec::with_capacity(37 + slice.len());
                udp_pkt.extend_from_slice(MAGIC); // [0..4]
                udp_pkt.extend_from_slice(&2466369941u32.to_be_bytes()); // [4..8]
                udp_pkt.push(OP_VIDEO_CHUNK); // [8]
                udp_pkt.extend_from_slice(&cid.to_be_bytes()); // [9..17]
                udp_pkt.extend_from_slice(&sender_uid.to_be_bytes()); // [17..25]
                udp_pkt.extend_from_slice(&seq.to_be_bytes()); // [25..29]
                udp_pkt.extend_from_slice(&(seq * 16).to_be_bytes()); // [29..33]
                udp_pkt.extend_from_slice(&(total_chunks as u16).to_be_bytes()); // [33..35]
                udp_pkt.extend_from_slice(&(chunk_idx as u16).to_be_bytes()); // [35..37]
                udp_pkt.extend_from_slice(slice); // [37..]

                for target in &known_targets {
                    let _ = socket.send_to(&udp_pkt, target);
                }
                bytes_sent_window += udp_pkt.len();
            }

            frames_sent_window += 1;
        }

        if last_log.elapsed() >= Duration::from_secs(2) {
            let elapsed_s = last_log.elapsed().as_secs_f64();
            let fps = (frames_sent_window as f64) / elapsed_s;
            let mbps = (bytes_sent_window as f64 * 8.0) / (elapsed_s * 1_000_000.0);
            println!("📊 [TX AMF STATUS] FPS: {:.1} | Bitrate: {:.2} Mbps | Frames: {} | Destinos: {:?}", fps, mbps, seq, known_targets);
            frames_sent_window = 0;
            bytes_sent_window = 0;
            last_log = Instant::now();
        }

        seq = seq.wrapping_add(1);
        next_frame_time += frame_interval;
        let now = Instant::now();
        if next_frame_time > now {
            std::thread::sleep(next_frame_time - now);
        } else {
            next_frame_time = now;
        }
    }
}
