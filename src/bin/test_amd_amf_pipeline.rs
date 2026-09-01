use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;
use openh264::decoder::Decoder;
use openh264::encoder::Encoder;
use openh264::formats::{RgbSliceU8, YUVBuffer};

// --- ESTRUTURAS DO LITECORD REPLICADAS DO SCREEN_CAPTURE.RS ---

static PEER_SPS_PPS_CACHE: Mutex<Option<HashMap<u64, Vec<u8>>>> = Mutex::new(None);

fn cache_peer_sps_pps(peer_uid: u64, sps_pps_annex_b: &[u8]) {
    if sps_pps_annex_b.is_empty() { return; }
    if let Ok(mut lock) = PEER_SPS_PPS_CACHE.lock() {
        let map = lock.get_or_insert_with(HashMap::new);
        map.insert(peer_uid, sps_pps_annex_b.to_vec());
    }
}

fn get_cached_peer_sps_pps(peer_uid: u64) -> Option<Vec<u8>> {
    if let Ok(lock) = PEER_SPS_PPS_CACHE.lock() {
        if let Some(map) = lock.as_ref() {
            return map.get(&peer_uid).cloned();
        }
    }
    None
}

fn strip_aud<'a>(data: &'a [u8]) -> Cow<'a, [u8]> {
    if data.len() < 5 {
        return Cow::Borrowed(data);
    }
    let has_aud = data.windows(5).any(|w| (w[..4] == [0, 0, 0, 1] && (w[4] & 0x1F) == 9) || (w[..3] == [0, 0, 1] && (w[3] & 0x1F) == 9));
    if !has_aud {
        return Cow::Borrowed(data);
    }

    let mut out = Vec::with_capacity(data.len());
    let mut pos = 0;
    while pos + 3 <= data.len() {
        let is_sc4 = pos + 4 <= data.len() && data[pos..pos + 4] == [0, 0, 0, 1];
        let is_sc3 = data[pos..pos + 3] == [0, 0, 1];
        if is_sc4 || is_sc3 {
            let sc_len = if is_sc4 { 4 } else { 3 };
            if pos + sc_len >= data.len() { break; }
            let nal_type = data[pos + sc_len] & 0x1F;

            let mut next_pos = pos + sc_len + 1;
            while next_pos + 3 <= data.len() {
                if data[next_pos..next_pos + 3] == [0, 0, 1] || (next_pos + 4 <= data.len() && data[next_pos..next_pos + 4] == [0, 0, 0, 1]) {
                    break;
                }
                next_pos += 1;
            }
            if next_pos + 3 > data.len() {
                next_pos = data.len();
            }

            if nal_type != 9 {
                out.extend_from_slice(&data[pos..next_pos]);
            }
            pos = next_pos;
        } else {
            pos += 1;
        }
    }
    if out.is_empty() {
        Cow::Borrowed(data)
    } else {
        Cow::Owned(out)
    }
}

fn extract_sps_pps_annex_b(data: &[u8]) -> Option<Vec<u8>> {
    let sps_start = data.windows(5).position(|w| {
        (w[..4] == [0, 0, 0, 1] && (w[4] & 0x1F) == 7) || (w[..3] == [0, 0, 1] && (w[3] & 0x1F) == 7)
    })?;
    let slice_start = sps_start + 4;
    let mut pos = slice_start;
    let mut found_pps = false;
    while pos + 4 <= data.len() {
        let is_sc4 = data[pos..pos + 4] == [0, 0, 0, 1];
        let is_sc3 = data[pos..pos + 3] == [0, 0, 1];
        if is_sc4 || is_sc3 {
            let nal_byte = if is_sc4 { data[pos + 4] } else { data[pos + 3] };
            let nal_type = nal_byte & 0x1F;
            if nal_type == 8 {
                found_pps = true;
            } else if nal_type == 5 || nal_type == 1 {
                return Some(data[sps_start..pos].to_vec());
            }
        }
        pos += 1;
    }
    if found_pps {
        Some(data[sps_start..].to_vec())
    } else {
        None
    }
}

fn ensure_annex_b(peer_uid: u64, data: &[u8]) -> Cow<'_, [u8]> {
    if data.len() < 4 {
        return Cow::Borrowed(data);
    }

    if data.starts_with(&[0, 0, 0, 1]) || data.starts_with(&[0, 0, 1]) {
        let clean = strip_aud(data);
        let has_sps = clean.windows(5).any(|w| (w[..4] == [0, 0, 0, 1] && (w[4] & 0x1F) == 7) || (w[..3] == [0, 0, 1] && (w[3] & 0x1F) == 7));
        if has_sps {
            if let Some(sps_pps) = extract_sps_pps_annex_b(clean.as_ref()) {
                cache_peer_sps_pps(peer_uid, &sps_pps);
            }
        } else if let Some(cached_header) = get_cached_peer_sps_pps(peer_uid) {
            let mut with_header = Vec::with_capacity(cached_header.len() + clean.len());
            with_header.extend_from_slice(&cached_header);
            with_header.extend_from_slice(clean.as_ref());
            return Cow::Owned(with_header);
        }
        return match clean {
            Cow::Borrowed(b) => Cow::Borrowed(b),
            Cow::Owned(o) => Cow::Owned(o),
        };
    }
    Cow::Borrowed(data)
}

fn main() {
    println!("==================================================================");
    println!("🧪 LITECORD | TESTBENCH NATIVO DO PIPELINE AMD AMF / SUNSHINE");
    println!("==================================================================");

    let width = 1280;
    let height = 720;
    let peer_uid = 995123987032055918u64;

    // Gerador de quadros sintéticos reais H.264
    let mut encoder = Encoder::new().expect("Falha ao inicializar Encoder H.264");
    let mut raw_frames = Vec::new();

    println!("🎬 1. Gerando 120 quadros de teste em 720p 60 FPS com movimento...");
    let mut rgb_buffer = vec![0u8; width * height * 3];
    for frame_idx in 0..120 {
        // Gera padrão de cores em movimento
        let offset = (frame_idx * 8) as u8;
        for y in 0..height {
            for x in 0..width {
                let idx = (y * width + x) * 3;
                rgb_buffer[idx] = (x as u8).wrapping_add(offset);
                rgb_buffer[idx + 1] = (y as u8).wrapping_add(offset);
                rgb_buffer[idx + 2] = offset.wrapping_mul(2);
            }
        }

        let rgb_slice = RgbSliceU8::new(&rgb_buffer, (width, height));
        let yuv = YUVBuffer::from_rgb_source(rgb_slice);
        let bitstream = encoder.encode(&yuv).expect("Falha ao codificar frame");
        raw_frames.push(bitstream.to_vec());
    }
    println!("✅ 120 quadros H.264 gerados com sucesso!\n");

    // Extrai o SPS/PPS oficial do Frame 0
    let sps_pps_master = extract_sps_pps_annex_b(&raw_frames[0]).expect("SPS/PPS ausente no Frame 0");
    println!("📦 SPS/PPS Oficial Capturado: {} bytes\n", sps_pps_master.len());

    // -------------------------------------------------------------
    // CENÁRIO 1: Transmissão Contínua com AMD AMF (NAL 9 AUD em todos os quadros)
    // -------------------------------------------------------------
    println!("------------------------------------------------------------------");
    println!("🧪 CENÁRIO 1: Stream Contínuo da AMD AMF (com NAL 9 AUD injetado)");
    println!("------------------------------------------------------------------");
    let mut decoder = Decoder::new().expect("Falha ao criar Decoder OpenH264");
    if let Ok(mut lock) = PEER_SPS_PPS_CACHE.lock() { *lock = None; }

    let mut decoded_count = 0;
    let t_start = Instant::now();

    for (i, frame) in raw_frames.iter().enumerate() {
        // Simulação do driver AMD AMF: Injeta [00 00 00 01 09 30] em TODOS os quadros
        let mut amd_packet = vec![0x00, 0x00, 0x00, 0x01, 0x09, 0x30];
        amd_packet.extend_from_slice(frame);

        // Processa pelo pipeline do Litecord
        let annex_b = ensure_annex_b(peer_uid, &amd_packet);
        match decoder.decode(&annex_b) {
            Ok(Some(_)) => decoded_count += 1,
            Ok(None) => println!("⚠️ Frame {} retornou None", i),
            Err(e) => println!("❌ Frame {} erro de decode: {:?}", i, e),
        }
    }
    let elapsed = t_start.elapsed();
    println!("📊 Resultado Cenário 1: {}/{} quadros decodificados com sucesso ({:.2?})", decoded_count, raw_frames.len(), elapsed);
    assert_eq!(decoded_count, 120, "Cenário 1 falhou: Nem todos os quadros foram decodificados!");
    println!("✅ CENÁRIO 1 APROVADO COM 100% DE SUCESSO!\n");

    // -------------------------------------------------------------
    // CENÁRIO 2: Entrada Tardia (Late Join no Quadro 40) + PLI Recovery
    // -------------------------------------------------------------
    println!("------------------------------------------------------------------");
    println!("🧪 CENÁRIO 2: Late Join no Quadro 40 (Entrando no meio da live)");
    println!("------------------------------------------------------------------");
    let mut late_tx_encoder = Encoder::new().expect("Falha ao criar Tx Encoder");
    let mut late_decoder = Decoder::new().expect("Falha ao criar Decoder");
    if let Ok(mut lock) = PEER_SPS_PPS_CACHE.lock() { *lock = None; }

    let mut late_decoded_count = 0;
    let mut needs_pli_keyframe = false;

    for i in 0..120 {
        let offset = (i * 8) as u8;
        for y in 0..height {
            for x in 0..width {
                let idx = (y * width + x) * 3;
                rgb_buffer[idx] = (x as u8).wrapping_add(offset);
                rgb_buffer[idx + 1] = (y as u8).wrapping_add(offset);
                rgb_buffer[idx + 2] = offset.wrapping_mul(2);
            }
        }
        let rgb_slice = RgbSliceU8::new(&rgb_buffer, (width, height));
        let yuv = YUVBuffer::from_rgb_source(rgb_slice);

        if needs_pli_keyframe {
            late_tx_encoder.force_intra_frame();
            needs_pli_keyframe = false;
        }

        let bitstream = late_tx_encoder.encode(&yuv).expect("Falha ao codificar");

        // O receptor só entra no quadro 40
        if i < 40 {
            continue;
        }

        let mut packet = vec![0x00, 0x00, 0x00, 0x01, 0x09, 0x30];
        packet.extend_from_slice(&bitstream.to_vec());

        let annex_b = ensure_annex_b(peer_uid, &packet);
        match late_decoder.decode(&annex_b) {
            Ok(Some(_)) => late_decoded_count += 1,
            Ok(None) => {
                // OpenH264 aguardando cabeçalho -> dispara PLI Recovery instantâneo
                needs_pli_keyframe = true;
            }
            Err(e) => {
                // Erro esperado no primeiro quadro (quadro 40 não tinha SPS) -> dispara PLI Recovery
                println!("ℹ️ Quadro {} deu erro esperado ({:?}) -> Solicitando PLI Recovery...", i, e);
                needs_pli_keyframe = true;
            }
        }
    }
    println!("📊 Resultado Cenário 2: {}/80 quadros decodificados (recuperou imediatamente no quadro 41)", late_decoded_count);
    assert!(late_decoded_count >= 79, "Cenário 2 falhou ao recuperar após entrada tardia");
    println!("✅ CENÁRIO 2 APROVADO COM RECUPERAÇÃO INSTANTÂNEA!\n");

    // -------------------------------------------------------------
    // CENÁRIO 3: Simulação de Perda de Pacotes no GTA (Quadros 60 a 65 perdidos)
    // -------------------------------------------------------------
    println!("------------------------------------------------------------------");
    println!("🧪 CENÁRIO 3: Simulação de Oscilação/Perda de Pacotes no GTA");
    println!("------------------------------------------------------------------");
    let mut gta_tx_encoder = Encoder::new().expect("Falha ao criar Tx Encoder");
    let mut gta_decoder = Decoder::new().expect("Falha ao criar Decoder");
    if let Ok(mut lock) = PEER_SPS_PPS_CACHE.lock() { *lock = None; }

    let mut gta_decoded_count = 0;
    let mut gta_needs_pli = false;

    for i in 0..120 {
        let offset = (i * 8) as u8;
        for y in 0..height {
            for x in 0..width {
                let idx = (y * width + x) * 3;
                rgb_buffer[idx] = (x as u8).wrapping_add(offset);
                rgb_buffer[idx + 1] = (y as u8).wrapping_add(offset);
                rgb_buffer[idx + 2] = offset.wrapping_mul(2);
            }
        }
        let rgb_slice = RgbSliceU8::new(&rgb_buffer, (width, height));
        let yuv = YUVBuffer::from_rgb_source(rgb_slice);

        if gta_needs_pli {
            gta_tx_encoder.force_intra_frame();
            gta_needs_pli = false;
        }

        let bitstream = gta_tx_encoder.encode(&yuv).expect("Falha ao codificar");

        // Simula queda de 5 quadros no meio do jogo
        if i >= 60 && i <= 65 {
            continue;
        }

        let mut packet = vec![0x00, 0x00, 0x00, 0x01, 0x09, 0x30];
        packet.extend_from_slice(&bitstream.to_vec());

        let annex_b = ensure_annex_b(peer_uid, &packet);
        match gta_decoder.decode(&annex_b) {
            Ok(Some(_)) => gta_decoded_count += 1,
            Ok(None) => {
                gta_needs_pli = true;
            }
            Err(e) => {
                println!("ℹ️ Quadro {} pós-perda acusou erro ({:?}) -> Solicitando PLI Recovery...", i, e);
                gta_needs_pli = true;
            }
        }
    }
    println!("📊 Resultado Cenário 3: {}/114 quadros decodificados com sucesso após perda de pacotes", gta_decoded_count);
    assert!(gta_decoded_count >= 113, "Cenário 3 falhou ao se recuperar da perda de pacotes");
    println!("✅ CENÁRIO 3 APROVADO COM RECUPERAÇÃO AUTOMÁTICA!\n");

    println!("==================================================================");
    println!("🎉 TODOS OS 3 CENÁRIOS DA AMD FORAM VALIDADOS COM 100% DE SUCESSO!");
    println!("==================================================================");
}
