#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Mutex;
use slint::{Rgba8Pixel, SharedPixelBuffer};

pub fn fit_bgra_to_canvas(
    src_bgra: &[u8],
    src_w: u32,
    src_h: u32,
    canvas_w: u32,
    canvas_h: u32,
    out_bgra: &mut [u8],
) {
    if src_w == 0 || src_h == 0 || canvas_w == 0 || canvas_h == 0 {
        return;
    }

    let total_pixels = (canvas_w as usize) * (canvas_h as usize);
    let total_bytes = total_pixels * 4;
    if out_bgra.len() < total_bytes {
        return;
    }

    // Calcula o stride real por linha da textura (GPU D3D11 Texture2D pitch)
    let src_stride_bytes = if src_h > 0 && src_bgra.len() >= (src_h as usize) {
        src_bgra.len() / (src_h as usize)
    } else {
        (src_w as usize) * 4
    };

    // Fast direct copy if dimensions match (even with GPU row pitch padding)
    if src_w == canvas_w && src_h == canvas_h {
        let row_bytes = (canvas_w as usize) * 4;
        if src_stride_bytes == row_bytes && src_bgra.len() >= total_bytes {
            out_bgra[..total_bytes].copy_from_slice(&src_bgra[..total_bytes]);
            return;
        }
        for y in 0..(canvas_h as usize) {
            let src_off = y * src_stride_bytes;
            let dst_off = y * row_bytes;
            if src_off + row_bytes <= src_bgra.len() && dst_off + row_bytes <= out_bgra.len() {
                out_bgra[dst_off..dst_off + row_bytes].copy_from_slice(&src_bgra[src_off..src_off + row_bytes]);
            }
        }
        return;
    }

    let canvas_u32: &mut [u32] = unsafe {
        std::slice::from_raw_parts_mut(out_bgra.as_mut_ptr() as *mut u32, total_pixels)
    };
    canvas_u32.fill(0xFF111214); // Fundo escuro (#111214) estilo Discord/YouTube

    let scale_w = canvas_w as f32 / src_w as f32;
    let scale_h = canvas_h as f32 / src_h as f32;
    let scale = scale_w.min(scale_h);

    let mut dest_w = ((src_w as f32 * scale).round() as u32).clamp(2, canvas_w) & !1;
    let mut dest_h = ((src_h as f32 * scale).round() as u32).clamp(2, canvas_h) & !1;
    let dest_x = ((canvas_w.saturating_sub(dest_w)) / 2) & !1;
    let dest_y = ((canvas_h.saturating_sub(dest_h)) / 2) & !1;

    // Bounds safety clamp: ensure dest_x + dest_w <= canvas_w and dest_y + dest_h <= canvas_h
    if dest_x + dest_w > canvas_w {
        dest_w = canvas_w.saturating_sub(dest_x) & !1;
    }
    if dest_y + dest_h > canvas_h {
        dest_h = canvas_h.saturating_sub(dest_y) & !1;
    }

    if dest_w == 0 || dest_h == 0 {
        return;
    }

    let x_step = ((src_w as u64) << 16) / (dest_w as u64);
    let y_step = ((src_h as u64) << 16) / (dest_h as u64);
    let src_stride_u32 = src_stride_bytes / 4;

    let src_u32: &[u32] = unsafe {
        std::slice::from_raw_parts(src_bgra.as_ptr() as *const u32, src_bgra.len() / 4)
    };

    let mut src_y_accum = 0u64;
    for dy in 0..dest_h {
        let sy = ((src_y_accum >> 16) as usize).min((src_h as usize).saturating_sub(1));
        let src_row_start = sy * src_stride_u32;
        if src_row_start >= src_u32.len() {
            break;
        }
        let src_row_end = (src_row_start + (src_w as usize)).min(src_u32.len());
        let src_row = &src_u32[src_row_start..src_row_end];

        let dst_y_idx = (dest_y + dy) as usize;
        if dst_y_idx >= (canvas_h as usize) {
            break;
        }
        let dst_row_start = dst_y_idx * (canvas_w as usize) + (dest_x as usize);
        let dst_row_end = (dst_row_start + (dest_w as usize)).min(total_pixels);
        if dst_row_start >= total_pixels || dst_row_start >= dst_row_end {
            break;
        }
        let dst_row = &mut canvas_u32[dst_row_start..dst_row_end];

        let mut src_x_accum = 0u64;
        for dx in 0..dst_row.len() {
            let sx = ((src_x_accum >> 16) as usize).min(src_row.len().saturating_sub(1));
            if sx < src_row.len() {
                dst_row[dx] = src_row[sx];
            }
            src_x_accum += x_step;
        }
        src_y_accum += y_step;
    }
}

static PEER_SPS_PPS_CACHE: Mutex<Option<HashMap<u64, Vec<u8>>>> = Mutex::new(None);

pub fn cache_peer_sps_pps(peer_uid: u64, sps_pps_annex_b: &[u8]) {
    if sps_pps_annex_b.is_empty() { return; }
    if let Ok(mut lock) = PEER_SPS_PPS_CACHE.lock() {
        let map = lock.get_or_insert_with(HashMap::new);
        map.insert(peer_uid, sps_pps_annex_b.to_vec());
    }
}

pub fn get_cached_peer_sps_pps(peer_uid: u64) -> Option<Vec<u8>> {
    if let Ok(lock) = PEER_SPS_PPS_CACHE.lock() {
        if let Some(map) = lock.as_ref() {
            return map.get(&peer_uid).cloned();
        }
    }
    None
}

pub fn strip_aud<'a>(data: &'a [u8]) -> std::borrow::Cow<'a, [u8]> {
    if data.len() < 5 {
        return std::borrow::Cow::Borrowed(data);
    }
    let has_aud = data.windows(5).any(|w| (w[..4] == [0, 0, 0, 1] && (w[4] & 0x1F) == 9) || (w[..3] == [0, 0, 1] && (w[3] & 0x1F) == 9));
    if !has_aud {
        return std::borrow::Cow::Borrowed(data);
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
        std::borrow::Cow::Borrowed(data)
    } else {
        std::borrow::Cow::Owned(out)
    }
}

pub fn extract_sps_pps_annex_b(data: &[u8]) -> Option<Vec<u8>> {
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

/// Garante que o bitstream esteja rigorosamente no padrão Annex B (00 00 00 01)
pub fn ensure_annex_b(peer_uid: u64, data: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if data.len() < 4 {
        return std::borrow::Cow::Borrowed(data);
    }

    // Varredura de metadados avcC (ISO/IEC 14496-15) em cabeçalhos MP4/WMF para extração dos parâmetros SPS/PPS
    if let Some(avcc_pos) = data.windows(4).position(|w| w == b"avcC") {
        let payload = &data[avcc_pos + 4..];
        if payload.len() >= 7 {
            let num_sps = (payload[5] & 0x1F) as usize;
            let mut p = 6;
            let mut extracted = Vec::new();
            for _ in 0..num_sps {
                if p + 2 <= payload.len() {
                    let sps_len = u16::from_be_bytes([payload[p], payload[p + 1]]) as usize;
                    p += 2;
                    if p + sps_len <= payload.len() {
                        extracted.extend_from_slice(&[0, 0, 0, 1]);
                        extracted.extend_from_slice(&payload[p..p + sps_len]);
                        p += sps_len;
                    }
                }
            }
            if p < payload.len() {
                let num_pps = payload[p] as usize;
                p += 1;
                for _ in 0..num_pps {
                    if p + 2 <= payload.len() {
                        let pps_len = u16::from_be_bytes([payload[p], payload[p + 1]]) as usize;
                        p += 2;
                        if p + pps_len <= payload.len() {
                            extracted.extend_from_slice(&[0, 0, 0, 1]);
                            extracted.extend_from_slice(&payload[p..p + pps_len]);
                            p += pps_len;
                        }
                    }
                }
            }
            if !extracted.is_empty() {
                cache_peer_sps_pps(peer_uid, &extracted);
            }
        }
    }

    // Pular caixas de contêiner MP4 (ex: ftyp, moov, free, mdat) se existirem
    let mut offset = 0;
    while offset + 8 <= data.len() {
        let box_size = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let box_tag = &data[offset + 4..offset + 8];
        if box_tag == b"ftyp" || box_tag == b"moov" || box_tag == b"free" {
            if box_size == 0 || offset + box_size > data.len() {
                break;
            }
            offset += box_size;
        } else if box_tag == b"mdat" {
            offset += 8;
            break;
        } else {
            break;
        }
    }

    let slice = &data[offset..];
    if slice.is_empty() {
        return std::borrow::Cow::Borrowed(data);
    }
    if slice.starts_with(&[0, 0, 0, 1]) || slice.starts_with(&[0, 0, 1]) {
        let clean = strip_aud(slice);
        let has_sps = clean.windows(5).any(|w| (w[..4] == [0, 0, 0, 1] && (w[4] & 0x1F) == 7) || (w[..3] == [0, 0, 1] && (w[3] & 0x1F) == 7));
        if has_sps {
            if let Some(sps_pps) = extract_sps_pps_annex_b(clean.as_ref()) {
                cache_peer_sps_pps(peer_uid, &sps_pps);
            }
        } else if let Some(cached_header) = get_cached_peer_sps_pps(peer_uid) {
            let is_idr = clean.windows(5).any(|w| (w[..4] == [0, 0, 0, 1] && (w[4] & 0x1F) == 5) || (w[..3] == [0, 0, 1] && (w[3] & 0x1F) == 5));
            if is_idr {
                let mut with_header = Vec::with_capacity(cached_header.len() + clean.len());
                with_header.extend_from_slice(&cached_header);
                with_header.extend_from_slice(clean.as_ref());
                return std::borrow::Cow::Owned(with_header);
            }
        }
        return match clean {
            std::borrow::Cow::Borrowed(b) => std::borrow::Cow::Borrowed(b),
            std::borrow::Cow::Owned(o) => std::borrow::Cow::Owned(o),
        };
    }

    // Converte AVCC (prefixos de tamanho de 4 bytes) para Annex B (00 00 00 01)
    let mut out = Vec::with_capacity(slice.len() + 64);
    let mut pos = 0;
    while pos + 4 <= slice.len() {
        let nal_len = u32::from_be_bytes(slice[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        if nal_len == 0 || pos + nal_len > slice.len() {
            break;
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&slice[pos..pos + nal_len]);
        pos += nal_len;
    }

    if out.is_empty() {
        std::borrow::Cow::Borrowed(data)
    } else {
        let clean = strip_aud(&out);
        let has_sps = clean.windows(5).any(|w| (w[..4] == [0, 0, 0, 1] && (w[4] & 0x1F) == 7) || (w[..3] == [0, 0, 1] && (w[3] & 0x1F) == 7));
        if has_sps {
            if let Some(sps_pps) = extract_sps_pps_annex_b(clean.as_ref()) {
                cache_peer_sps_pps(peer_uid, &sps_pps);
            }
        } else if let Some(cached_header) = get_cached_peer_sps_pps(peer_uid) {
            let is_idr = clean.windows(5).any(|w| (w[..4] == [0, 0, 0, 1] && (w[4] & 0x1F) == 5) || (w[..3] == [0, 0, 1] && (w[3] & 0x1F) == 5));
            if is_idr {
                let mut with_header = Vec::with_capacity(cached_header.len() + clean.len());
                with_header.extend_from_slice(&cached_header);
                with_header.extend_from_slice(clean.as_ref());
                return std::borrow::Cow::Owned(with_header);
            }
        }
        std::borrow::Cow::Owned(clean.into_owned())
    }
}

pub fn decode_video_frame(
    decoders: &mut HashMap<u64, openh264::decoder::Decoder>,
    peer_uid: u64,
    frame_data: &[u8],
) -> Option<(SharedPixelBuffer<Rgba8Pixel>, u32, u32)> {
    if frame_data.is_empty() { return None; }

    // Check if legacy JPEG header (0xFF, 0xD8)
    if frame_data.len() >= 2 && frame_data[0] == 0xFF && frame_data[1] == 0xD8 {
        return decode_jpeg(frame_data);
    }

    use openh264::formats::YUVSource;
    let decoder = decoders.entry(peer_uid).or_insert_with(|| {
        openh264::decoder::Decoder::new().expect("Falha ao criar Decoder H.264")
    });

    let annex_b = ensure_annex_b(peer_uid, frame_data);

    match decoder.decode(&annex_b) {
        Ok(Some(decoded_yuv)) => {
            let (w, h) = decoded_yuv.dimensions();
            if w > 0 && h > 0 {
                use rayon::prelude::*;
                let mut pixel_buffer = SharedPixelBuffer::<Rgba8Pixel>::new(w as u32, h as u32);
                let (ys, us, _vs) = decoded_yuv.strides();
                let y_raw = decoded_yuv.y().as_ptr() as usize;
                let u_raw = decoded_yuv.u().as_ptr() as usize;
                let v_raw = decoded_yuv.v().as_ptr() as usize;
                let rgba_ptr = pixel_buffer.make_mut_bytes().as_mut_ptr() as usize;

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
                            let g_add = -100 * d - 208 * e + 128;
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

                    if i < w {
                        unsafe {
                            let y = *y_p.add(y_row + i) as i32;
                            let u = *u_p.add(uv_row + (i / 2)) as i32;
                            let v = *v_p.add(uv_row + (i / 2)) as i32;

                            let c = 298 * (y - 16);
                            let d = u - 128;
                            let e = v - 128;

                            let r = ((c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
                            let g = ((c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
                            let b = ((c + 516 * d + 128) >> 8).clamp(0, 255) as u8;

                            *rgba_p_u32.add(dst_row + i) = u32::from_le_bytes([r, g, b, 255]);
                        }
                    }
                });

                return Some((pixel_buffer, w as u32, h as u32));
            }
        }
        Ok(None) => {
            log::info!("⏳ [DECODER RX] OpenH264 aguardando IDR Keyframe para peer {} (len={})", peer_uid, annex_b.len());
        }
        Err(e) => {
            log::warn!("⚠️ [DECODER RX] Frame descartado por erro/falta de referência para peer {}: {:?} (len={}, header={:02X?})",
                peer_uid, e, annex_b.len(), &annex_b[..annex_b.len().min(8)]);
        }
    }
    None
}

pub fn encode_jpeg(rgb_data: &[u8], width: u32, height: u32, quality: u8) -> Option<Vec<u8>> {
    let mut dest = Vec::with_capacity((width * height / 4) as usize);
    let encoder = jpeg_encoder::Encoder::new(&mut dest, quality);
    if encoder.encode(rgb_data, width as u16, height as u16, jpeg_encoder::ColorType::Rgb).is_ok() {
        Some(dest)
    } else {
        None
    }
}

pub fn decode_jpeg(jpeg_data: &[u8]) -> Option<(SharedPixelBuffer<Rgba8Pixel>, u32, u32)> {
    let mut decoder = zune_jpeg::JpegDecoder::new(jpeg_data);
    let rgb_pixels = decoder.decode().ok()?;
    let info = decoder.info()?;
    let (width, height) = (info.width as u32, info.height as u32);
    let total_pixels = (width * height) as usize;

    if rgb_pixels.len() < total_pixels * 3 {
        return None;
    }

    let mut pixel_buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    let dest_bytes = pixel_buffer.make_mut_bytes();

    let mut src_idx = 0;
    let mut dst_idx = 0;
    while src_idx + 2 < total_pixels * 3 && dst_idx + 3 < dest_bytes.len() {
        dest_bytes[dst_idx] = rgb_pixels[src_idx];
        dest_bytes[dst_idx + 1] = rgb_pixels[src_idx + 1];
        dest_bytes[dst_idx + 2] = rgb_pixels[src_idx + 2];
        dest_bytes[dst_idx + 3] = 255;
        src_idx += 3;
        dst_idx += 4;
    }

    Some((pixel_buffer, width, height))
}
