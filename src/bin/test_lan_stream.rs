use openh264::decoder::Decoder;
use openh264::encoder::{Encoder, EncoderConfig, UsageType, RateControlMode};
use openh264::formats::{YUVSource, YUVSlices};
use rayon::prelude::*;
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

const CHUNK_SIZE: usize = 1350;
const MAGIC: &[u8; 4] = &[0x4C, 0x54, 0x43, 0x44]; // LTCD
const OP_VIDEO_CHUNK: u8 = 2;
const OP_FEC_PARITY: u8 = 7;

fn decode_yuv_to_rgba_fast(decoded_yuv: &impl YUVSource, rgba_out: &mut [u8]) {
    let (w, h) = decoded_yuv.dimensions();
    let (ys, us, _vs) = decoded_yuv.strides();
    let y_raw = decoded_yuv.y().as_ptr() as usize;
    let u_raw = decoded_yuv.u().as_ptr() as usize;
    let v_raw = decoded_yuv.v().as_ptr() as usize;
    let rgba_ptr = rgba_out.as_mut_ptr() as usize;

    (0..h).into_par_iter().with_min_len(32).for_each(|j| {
        let y_p = y_raw as *const u8;
        let u_p = u_raw as *const u8;
        let v_p = v_raw as *const u8;
        let rgba_p_u32 = rgba_ptr as *mut u32;

        let y_row = j * ys;
        let uv_row = (j / 2) * us;
        let dst_row = j * w;

        let mut i = 0;
        while i + 1 < w {
            unsafe {
                let u_val = *u_p.add(uv_row + (i / 2)) as i32;
                let v_val = *v_p.add(uv_row + (i / 2)) as i32;

                let d = u_val - 128;
                let e = v_val - 128;

                let r_add = 409 * e + 128;
                let g_add: i32 = -100 * d - 208 * e + 128;
                let b_add = 516 * d + 128;

                // Pixel 0
                let y0 = *y_p.add(y_row + i) as i32;
                let c0 = 298 * (y0 - 16);
                let r0 = ((c0 + r_add) >> 8).clamp(0, 255) as u8;
                let g0 = ((c0 + g_add) >> 8).clamp(0, 255) as u8;
                let b0 = ((c0 + b_add) >> 8).clamp(0, 255) as u8;
                *rgba_p_u32.add(dst_row + i) = u32::from_le_bytes([r0, g0, b0, 255]);

                // Pixel 1
                let y1 = *y_p.add(y_row + i + 1) as i32;
                let c1 = 298 * (y1 - 16);
                let r1 = ((c1 + r_add) >> 8).clamp(0, 255) as u8;
                let g1 = ((c1 + g_add) >> 8).clamp(0, 255) as u8;
                let b1 = ((c1 + b_add) >> 8).clamp(0, 255) as u8;
                *rgba_p_u32.add(dst_row + i + 1) = u32::from_le_bytes([r1, g1, b1, 255]);
            }
            i += 2;
        }
    });
}

fn run_receiver(port: u16, max_duration_secs: u64) {
    println!("📡 [RECEIVER] Iniciando socket UDP em 0.0.0.0:{}...", port);
    let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP)).unwrap();
    let _ = socket.set_reuse_address(true);
    let _ = socket.set_send_buffer_size(4 * 1024 * 1024);
    let _ = socket.set_recv_buffer_size(4 * 1024 * 1024);
    let addr: SocketAddr = format!("0.0.0.0:{}", port).parse().unwrap();
    socket.bind(&addr.into()).expect("Falha ao fazer bind");
    let std_sock: UdpSocket = socket.into();
    std_sock.set_read_timeout(Some(Duration::from_millis(50))).unwrap();

    struct InFlightFrame {
        total_chunks: u16,
        _total_len: usize,
        _pts_ms: u32,
        received: HashMap<u16, Vec<u8>>,
        parity: Option<Vec<u8>>,
        _t_first_pkt: Instant,
    }

    let mut in_flight: HashMap<u32, InFlightFrame> = HashMap::new();
    let mut decoder = Decoder::new().expect("Falha ao inicializar OpenH264 Decoder");
    let mut rgba_buf = vec![0u8; 1920 * 1080 * 4];

    let mut recv_buf = [0u8; 65535];
    let mut frames_received = 0u64;
    let mut fec_recovered = 0u64;
    let mut decode_errors = 0u64;
    let mut total_decode_us = 0u64;
    let mut min_decode_us = u64::MAX;
    let mut max_decode_us = 0u64;
    let mut frame_intervals_us = Vec::new();
    let mut last_frame_time = Instant::now();

    println!("✅ [RECEIVER] Pronto! Aguardando stream 1080p 60 FPS por {} segundos...", max_duration_secs);
    let start_time = Instant::now();

    while start_time.elapsed() < Duration::from_secs(max_duration_secs) {
        match std_sock.recv_from(&mut recv_buf) {
            Ok((len, _src)) => {
                if len < 25 || &recv_buf[0..4] != MAGIC {
                    continue;
                }
                let op = recv_buf[8];
                let _pkt_cid = u64::from_be_bytes(recv_buf[9..17].try_into().unwrap());
                let _pkt_uid = u64::from_be_bytes(recv_buf[17..25].try_into().unwrap());

                if op == OP_VIDEO_CHUNK && len >= 37 {
                    let seq = u32::from_be_bytes(recv_buf[25..29].try_into().unwrap());
                    let pts_ms = u32::from_be_bytes(recv_buf[29..33].try_into().unwrap());
                    let total = u16::from_be_bytes(recv_buf[33..35].try_into().unwrap());
                    let idx = u16::from_be_bytes(recv_buf[35..37].try_into().unwrap());
                    let chunk_data = recv_buf[37..len].to_vec();

                    let entry = in_flight.entry(seq).or_insert_with(|| InFlightFrame {
                        total_chunks: total,
                        _total_len: 0,
                        _pts_ms: pts_ms,
                        received: HashMap::with_capacity(total as usize),
                        parity: None,
                        _t_first_pkt: Instant::now(),
                    });
                    entry._pts_ms = pts_ms;
                    entry.total_chunks = total;
                    entry.received.insert(idx, chunk_data);

                    // Check FEC
                    if entry.received.len() == (total as usize).saturating_sub(1) && entry.parity.is_some() {
                        let mut missing = None;
                        for i in 0..total {
                            if !entry.received.contains_key(&i) { missing = Some(i); break; }
                        }
                        if let Some(m_idx) = missing {
                            if let Some(parity) = entry.parity.as_ref() {
                                let mut rec = parity.clone();
                                for chunk in entry.received.values() {
                                    for (k, &b) in chunk.iter().enumerate() {
                                        if k < rec.len() { rec[k] ^= b; }
                                    }
                                }
                                entry.received.insert(m_idx, rec);
                                fec_recovered += 1;
                            }
                        }
                    }

                    if entry.received.len() == (total as usize) {
                        let mut full_frame = Vec::new();
                        for i in 0..total {
                            if let Some(c) = entry.received.get(&i) {
                                full_frame.extend_from_slice(c);
                            }
                        }
                        in_flight.remove(&seq);

                        let t_dec = Instant::now();
                        match decoder.decode(&full_frame) {
                            Ok(Some(yuv)) => {
                                decode_yuv_to_rgba_fast(&yuv, &mut rgba_buf);
                                let d_us = t_dec.elapsed().as_micros() as u64;
                                total_decode_us += d_us;
                                min_decode_us = min_decode_us.min(d_us);
                                max_decode_us = max_decode_us.max(d_us);
                                frames_received += 1;

                                if frames_received > 1 {
                                    frame_intervals_us.push(last_frame_time.elapsed().as_micros() as u64);
                                }
                                last_frame_time = Instant::now();

                                if frames_received % 60 == 0 {
                                    let avg_d_ms = (total_decode_us as f64) / (frames_received as f64) / 1000.0;
                                    let cur_fps = (frames_received as f64) / start_time.elapsed().as_secs_f64();
                                    println!("📊 [TELEMETRIA RX] {} quadros | {:.1} FPS | Decode: {:.2}ms (Min: {:.2}ms, Max: {:.2}ms) | FEC: {} recuperados",
                                        frames_received, cur_fps, avg_d_ms, (min_decode_us as f64) / 1000.0, (max_decode_us as f64) / 1000.0, fec_recovered);
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                decode_errors += 1;
                                println!("❌ [DECODE ERRO] Frame {} falhou: {:?}", seq, e);
                            }
                        }
                    }
                } else if op == OP_FEC_PARITY && len >= 39 {
                    let seq = u32::from_be_bytes(recv_buf[25..29].try_into().unwrap());
                    let pts_ms = u32::from_be_bytes(recv_buf[29..33].try_into().unwrap());
                    let total = u16::from_be_bytes(recv_buf[33..35].try_into().unwrap());
                    let total_frame_len = u32::from_be_bytes(recv_buf[35..39].try_into().unwrap_or([0; 4])) as usize;
                    let parity_data = recv_buf[39..len].to_vec();

                    let entry = in_flight.entry(seq).or_insert_with(|| InFlightFrame {
                        total_chunks: total,
                        _total_len: total_frame_len,
                        _pts_ms: pts_ms,
                        received: HashMap::with_capacity(total as usize),
                        parity: None,
                        _t_first_pkt: Instant::now(),
                    });
                    entry.total_chunks = total;
                    entry._total_len = total_frame_len;
                    entry.parity = Some(parity_data);

                    if entry.received.len() == (total as usize).saturating_sub(1) {
                        let mut missing = None;
                        for i in 0..total {
                            if !entry.received.contains_key(&i) { missing = Some(i); break; }
                        }
                        if let Some(m_idx) = missing {
                            if let Some(parity) = entry.parity.as_ref() {
                                let mut rec = parity.clone();
                                for chunk in entry.received.values() {
                                    for (k, &b) in chunk.iter().enumerate() {
                                        if k < rec.len() { rec[k] ^= b; }
                                    }
                                }
                                entry.received.insert(m_idx, rec);
                                fec_recovered += 1;
                            }
                        }

                        if entry.received.len() == (total as usize) {
                            let mut full_frame = Vec::new();
                            for i in 0..total {
                                if let Some(c) = entry.received.get(&i) {
                                    full_frame.extend_from_slice(c);
                                }
                            }
                            in_flight.remove(&seq);

                            let t_dec = Instant::now();
                            match decoder.decode(&full_frame) {
                                Ok(Some(yuv)) => {
                                    decode_yuv_to_rgba_fast(&yuv, &mut rgba_buf);
                                    let d_us = t_dec.elapsed().as_micros() as u64;
                                    total_decode_us += d_us;
                                    min_decode_us = min_decode_us.min(d_us);
                                    max_decode_us = max_decode_us.max(d_us);
                                    frames_received += 1;

                                    if frames_received > 1 {
                                        frame_intervals_us.push(last_frame_time.elapsed().as_micros() as u64);
                                    }
                                    last_frame_time = Instant::now();
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    decode_errors += 1;
                                    println!("❌ [DECODE ERRO] Frame {} falhou: {:?}", seq, e);
                                }
                            }
                        }
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }

    let elapsed = start_time.elapsed().as_secs_f64();
    let effective_fps = (frames_received as f64) / elapsed;
    let avg_d_ms = if frames_received > 0 { (total_decode_us as f64) / (frames_received as f64) / 1000.0 } else { 0.0 };

    println!("============================================================");
    println!("🏁 [RELATÓRIO FINAL RX]");
    println!("   Duração: {:.2}s", elapsed);
    println!("   Quadros Recebidos & Decodificados: {}", frames_received);
    println!("   Taxa de Quadros Efetiva: {:.1} FPS (Alvo: 60 FPS)", effective_fps);
    println!("   Tempo Médio de Decode + RGBA: {:.2} ms (Min: {:.2}ms, Max: {:.2}ms)", avg_d_ms, (min_decode_us as f64)/1000.0, (max_decode_us as f64)/1000.0);
    println!("   Recuperações FEC (XOR Parity): {}", fec_recovered);
    println!("   Erros de Decode: {}", decode_errors);
    println!("============================================================");
}

fn run_transmitter(target_addr_str: &str, duration_secs: u64) {
    let target_addr: SocketAddr = target_addr_str.parse().expect("Endereço inválido");
    let socket = UdpSocket::bind("0.0.0.0:0").expect("Falha ao criar socket TX");
    let _ = socket.set_broadcast(true);

    let w = 1920usize;
    let h = 1080usize;
    let target_fps = 60u32;
    let total_frames = (duration_secs * (target_fps as u64)) as usize;

    let enc_config = EncoderConfig::new()
        .usage_type(UsageType::ScreenContentRealTime)
        .set_multiple_thread_idc(8)
        .set_bitrate_bps(6_000_000)
        .max_frame_rate(target_fps as f32)
        .enable_skip_frame(false)
        .rate_control_mode(RateControlMode::Quality);

    let mut encoder = Encoder::with_api_config(openh264::OpenH264API::from_source(), enc_config).unwrap();

    println!("🚀 [TRANSMITTER] Iniciando transmissão H.264 para {}...", target_addr);
    println!("   Config: 1080p @ 60 FPS | Bitrate: 6.00 Mbps | Total: {} quadros ({:.1}s)", total_frames, duration_secs as f64);

    let y_plane_len = w * h;
    let uv_plane_len = (w / 2) * (h / 2);
    let mut y_plane = vec![128u8; y_plane_len];
    let mut u_plane = vec![128u8; uv_plane_len];
    let mut v_plane = vec![128u8; uv_plane_len];

    let start_time = Instant::now();
    let frame_interval = Duration::from_nanos(1_000_000_000 / (target_fps as u64));
    let mut next_deadline = Instant::now();

    for frame_idx in 0..total_frames {
        // Animate a moving bright bar across the screen to simulate high-motion 60 FPS gaming/desktop
        let offset = (frame_idx * 16) % w;
        for j in 0..h {
            for i in 0..w {
                let dist = (i as i32 - offset as i32).abs();
                y_plane[j * w + i] = if dist < 80 { 240 } else { (16 + (j % 64)) as u8 };
            }
        }

        let yuv_source = YUVSlices::new((&y_plane, &u_plane, &v_plane), (w, h), (w, w/2, w/2));
        let bitstream = encoder.encode(&yuv_source).expect("Falha ao codificar frame");
        let raw_h264 = bitstream.to_vec();

        let seq = frame_idx as u32;
        let pts_ms = start_time.elapsed().as_millis() as u32;
        let chunks: Vec<&[u8]> = raw_h264.chunks(CHUNK_SIZE).collect();
        let total_chunks = chunks.len() as u16;

        // Compute XOR Parity across chunks
        let mut parity_data = vec![0u8; CHUNK_SIZE];
        for chunk in &chunks {
            for (i, &b) in chunk.iter().enumerate() {
                parity_data[i] ^= b;
            }
        }

        for (idx, chunk_slice) in chunks.iter().enumerate() {
            let mut pkt = Vec::with_capacity(37 + chunk_slice.len());
            pkt.extend_from_slice(MAGIC);
            pkt.extend_from_slice(&0x01020304u32.to_be_bytes()); // Instance ID
            pkt.push(OP_VIDEO_CHUNK);
            pkt.extend_from_slice(&123456789u64.to_be_bytes()); // Channel ID
            pkt.extend_from_slice(&999999999u64.to_be_bytes()); // Sender UID
            pkt.extend_from_slice(&seq.to_be_bytes());
            pkt.extend_from_slice(&pts_ms.to_be_bytes());
            pkt.extend_from_slice(&total_chunks.to_be_bytes());
            pkt.extend_from_slice(&(idx as u16).to_be_bytes());
            pkt.extend_from_slice(chunk_slice);

            let _ = socket.send_to(&pkt, target_addr);

            if total_chunks > 1 {
                let spin_start = Instant::now();
                while spin_start.elapsed() < Duration::from_micros(12) {
                    std::hint::spin_loop();
                }
            }
        }

        // Send FEC Parity
        let mut fec_pkt = Vec::with_capacity(39 + parity_data.len());
        fec_pkt.extend_from_slice(MAGIC);
        fec_pkt.extend_from_slice(&0x01020304u32.to_be_bytes());
        fec_pkt.push(OP_FEC_PARITY);
        fec_pkt.extend_from_slice(&123456789u64.to_be_bytes());
        fec_pkt.extend_from_slice(&999999999u64.to_be_bytes());
        fec_pkt.extend_from_slice(&seq.to_be_bytes());
        fec_pkt.extend_from_slice(&pts_ms.to_be_bytes());
        fec_pkt.extend_from_slice(&total_chunks.to_be_bytes());
        fec_pkt.extend_from_slice(&(raw_h264.len() as u32).to_be_bytes());
        fec_pkt.extend_from_slice(&parity_data);

        let _ = socket.send_to(&fec_pkt, target_addr);

        next_deadline += frame_interval;
        let now = Instant::now();
        if next_deadline > now {
            std::thread::sleep(next_deadline - now);
        }
    }

    println!("✅ [TRANSMITTER] Transmissão concluída com sucesso! Enviados {} quadros.", total_frames);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        println!("Uso:");
        println!("  test_lan_stream rx [porta=50005] [duracao_segundos=10]");
        println!("  test_lan_stream tx <ip_destino:porta> [duracao_segundos=10]");
        return;
    }

    match args[1].as_str() {
        "rx" => {
            let port = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(50005);
            let dur = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10);
            run_receiver(port, dur);
        }
        "tx" => {
            let target = args.get(2).cloned().unwrap_or_else(|| "127.0.0.1:50005".to_string());
            let dur = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10);
            run_transmitter(&target, dur);
        }
        _ => {
            println!("Modo inválido: use 'rx' ou 'tx'");
        }
    }
}
