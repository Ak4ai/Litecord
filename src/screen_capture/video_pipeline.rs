#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Largest size (physical pixels, `w << 32 | h`) a received stream is shown at, set by the
/// UI. Frames are converted straight to that size: converting and then drawing a 1080p
/// frame shown in a 980 px window cost about four times the pixels that end up on screen.
static STREAM_VIEW_PX: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn set_stream_view_size(w: u32, h: u32) {
    STREAM_VIEW_PX.store((w as u64) << 32 | h as u64, Ordering::Relaxed);
}

fn stream_view_size() -> (u32, u32) {
    let v = STREAM_VIEW_PX.load(Ordering::Relaxed);
    ((v >> 32) as u32, v as u32)
}

/// Whether any window can show a received stream right now. When none can (Litecord in
/// the tray or minimized while a game runs, popout closed or minimized) the decoder skips
/// the frames instead of decoding them, and asks for a keyframe once a window shows the
/// stream again.
static MAIN_WINDOW_HAS_AREA: AtomicBool = AtomicBool::new(true);
static POPOUT_HAS_STREAM: AtomicBool = AtomicBool::new(false);
static POPOUT_HAS_AREA: AtomicBool = AtomicBool::new(true);

pub fn set_main_window_has_area(has: bool) {
    MAIN_WINDOW_HAS_AREA.store(has, Ordering::Relaxed);
}

pub fn set_popout_has_stream(has: bool) {
    POPOUT_HAS_STREAM.store(has, Ordering::Relaxed);
}

pub fn set_popout_has_area(has: bool) {
    POPOUT_HAS_AREA.store(has, Ordering::Relaxed);
}

pub fn streams_watched() -> bool {
    anyone_watching(
        crate::ui::helpers::APP_IS_VISIBLE.load(Ordering::Relaxed),
        MAIN_WINDOW_HAS_AREA.load(Ordering::Relaxed),
        POPOUT_HAS_STREAM.load(Ordering::Relaxed),
        POPOUT_HAS_AREA.load(Ordering::Relaxed),
    )
}

/// The main window can show a stream (visible, not minimized).
pub fn main_window_watching() -> bool {
    crate::ui::helpers::APP_IS_VISIBLE.load(Ordering::Relaxed) && MAIN_WINDOW_HAS_AREA.load(Ordering::Relaxed)
}

/// The popout shows a stream and is not minimized.
pub fn popout_watching() -> bool {
    POPOUT_HAS_STREAM.load(Ordering::Relaxed) && POPOUT_HAS_AREA.load(Ordering::Relaxed)
}

fn anyone_watching(app_visible: bool, main_has_area: bool, popout_has_stream: bool, popout_has_area: bool) -> bool {
    (app_visible && main_has_area) || (popout_has_stream && popout_has_area)
}

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

/// Decoders of the received streams: on the GPU when the machine and the stream allow it,
/// OpenH264 otherwise.
#[derive(Default)]
pub struct StreamDecoders {
    cpu: HashMap<u64, openh264::decoder::Decoder>,
    gpu: HashMap<u64, super::hw_decode::GpuDecoder>,
    /// Streams decoded on the CPU for good (GPU decoder missing, unsupported or failed).
    cpu_only: std::collections::HashSet<u64>,
}

impl StreamDecoders {
    /// Starts the decoders of this stream over (the next picture must be a keyframe).
    pub fn reset(&mut self, peer_uid: u64) {
        self.cpu.remove(&peer_uid);
        self.gpu.remove(&peer_uid);
    }

    /// Frees the GPU decoders (their device and surfaces) once streams stop arriving.
    pub fn release_gpu(&mut self) {
        self.gpu.clear();
    }
}

pub enum Decoded {
    Picture(SharedPixelBuffer<Rgba8Pixel>, u32, u32),
    /// Drawn by the GPU straight into the windows showing the stream.
    ShownByGpu,
    /// Nothing to show yet, and nothing wrong (parameter sets, a picture held for reordering).
    Nothing,
    /// The picture could not be decoded.
    Failed,
    /// Pictures are missing: wait for a keyframe and ask the sender for one.
    NeedKeyframe,
}

/// Pictures decoded on the GPU (read by tests).
pub static RX_GPU_FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Size a decoded picture is converted to: the largest size it is shown at.
fn shown_size(w: u32, h: u32) -> (u32, u32) {
    #[cfg(feature = "automation")]
    if crate::utils::test_hooks::FULL_SIZE_CONVERSION.load(Ordering::Relaxed) {
        return (w, h);
    }
    let (view_w, view_h) = stream_view_size();
    fit_within(w, h, view_w, view_h)
}

pub fn decode_video_frame(decoders: &mut StreamDecoders, peer_uid: u64, frame_data: &[u8]) -> Decoded {
    if frame_data.is_empty() {
        return Decoded::Failed;
    }
    // Legacy JPEG frames (0xFF, 0xD8).
    if frame_data.len() >= 2 && frame_data[0] == 0xFF && frame_data[1] == 0xD8 {
        return decode_jpeg(frame_data).map(|(b, w, h)| Decoded::Picture(b, w, h)).unwrap_or(Decoded::Failed);
    }
    let annex_b = ensure_annex_b(peer_uid, frame_data);

    #[cfg(feature = "automation")]
    let gpu_allowed = !crate::utils::test_hooks::CPU_DECODE.load(Ordering::Relaxed);
    #[cfg(not(feature = "automation"))]
    let gpu_allowed = true;
    if !gpu_allowed {
        // Its windows go back to CPU pictures once this decoder is gone.
        decoders.gpu.remove(&peer_uid);
    }
    if gpu_allowed && !decoders.cpu_only.contains(&peer_uid) {
        use super::hw_decode::{GpuDecoder, GpuPicture, HwError};
        let gpu = match decoders.gpu.entry(peer_uid) {
            std::collections::hash_map::Entry::Occupied(e) => Some(e.into_mut()),
            std::collections::hash_map::Entry::Vacant(v) => match GpuDecoder::new(peer_uid) {
                Ok(d) => {
                    log::info!("🎞️ [DECODER RX] Decodificando a transmissão do peer {} pela GPU", peer_uid);
                    Some(v.insert(d))
                }
                Err(e) => {
                    log::info!("🎞️ [DECODER RX] Sem decodificação pela GPU ({:?}); peer {} vai pela CPU", e, peer_uid);
                    decoders.cpu_only.insert(peer_uid);
                    None
                }
            },
        };
        if let Some(gpu) = gpu {
            match gpu.decode(&annex_b, shown_size) {
                Ok(Some(GpuPicture::Rgba(buf, w, h))) => {
                    RX_GPU_FRAMES.fetch_add(1, Ordering::Relaxed);
                    return Decoded::Picture(buf, w, h);
                }
                Ok(Some(GpuPicture::Shown)) => {
                    RX_GPU_FRAMES.fetch_add(1, Ordering::Relaxed);
                    return Decoded::ShownByGpu;
                }
                Ok(None) => return Decoded::Nothing,
                Err(HwError::NeedKeyframe(why)) => {
                    static WAITING: crate::utils::logger::LogThrottle = crate::utils::logger::LogThrottle::new(1000);
                    if let Some(n) = WAITING.hit() {
                        log::info!("⏳ [DECODER RX] GPU aguardando keyframe do peer {} ({}; {} quadro(s) desde o último aviso)", peer_uid, why, n);
                    }
                    return Decoded::NeedKeyframe;
                }
                Err(e) => {
                    // Unsupported stream or a GPU failure: this stream goes to the CPU from
                    // its next keyframe on.
                    log::warn!("🎞️ [DECODER RX] Decodificação pela GPU desligada para o peer {}: {:?}", peer_uid, e);
                    decoders.gpu.remove(&peer_uid);
                    decoders.cpu_only.insert(peer_uid);
                    decoders.cpu.remove(&peer_uid);
                    return Decoded::NeedKeyframe;
                }
            }
        }
    }
    match decode_on_cpu(&mut decoders.cpu, peer_uid, &annex_b) {
        Some((b, w, h)) => Decoded::Picture(b, w, h),
        None => Decoded::Failed,
    }
}

fn decode_on_cpu(
    decoders: &mut HashMap<u64, openh264::decoder::Decoder>,
    peer_uid: u64,
    annex_b: &[u8],
) -> Option<(SharedPixelBuffer<Rgba8Pixel>, u32, u32)> {
    use openh264::formats::YUVSource;
    let decoder = decoders.entry(peer_uid).or_insert_with(|| {
        openh264::decoder::Decoder::new().expect("Falha ao criar Decoder H.264")
    });

    match decoder.decode(annex_b) {
        Ok(Some(decoded_yuv)) => {
            let (w, h) = decoded_yuv.dimensions();
            let (out_w, out_h) = shown_size(w as u32, h as u32);
            if w > 0 && h > 0 && (out_w as usize, out_h as usize) != (w, h) {
                let mut pixel_buffer = SharedPixelBuffer::<Rgba8Pixel>::new(out_w, out_h);
                let (ys, us, vs) = decoded_yuv.strides();
                debug_assert_eq!(us, vs);
                i420_to_rgba_scaled(
                    decoded_yuv.y(), decoded_yuv.u(), decoded_yuv.v(), ys, us, w, h,
                    pixel_buffer.make_mut_bytes(), out_w as usize, out_h as usize,
                );
                return Some((pixel_buffer, out_w, out_h));
            }
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
            static WAITING_IDR: crate::utils::logger::LogThrottle = crate::utils::logger::LogThrottle::new(1000);
            if let Some(n) = WAITING_IDR.hit() {
                log::info!("⏳ [DECODER RX] OpenH264 aguardando IDR Keyframe para peer {} ({} quadro(s) desde o último aviso, len={})", peer_uid, n, annex_b.len());
            }
        }
        Err(e) => {
            static DROPPED: crate::utils::logger::LogThrottle = crate::utils::logger::LogThrottle::new(1000);
            if let Some(n) = DROPPED.hit() {
                log::warn!("⚠️ [DECODER RX] {} frame(s) descartado(s) por erro/falta de referência desde o último aviso, peer {}: {:?} (len={}, header={:02X?})",
                    n, peer_uid, e, annex_b.len(), &annex_b[..annex_b.len().min(8)]);
            }
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

/// Largest size that fits `max_w` x `max_h` with the frame's aspect ratio, never larger
/// than the frame itself (even dimensions; 0 means "no limit").
pub fn fit_within(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if max_w == 0 || max_h == 0 || w == 0 || h == 0 {
        return (w, h);
    }
    let scale = (max_w as f64 / w as f64).min(max_h as f64 / h as f64).min(1.0);
    if scale >= 1.0 {
        return (w, h);
    }
    let even = |v: f64| ((v.round() as u32) & !1).max(2);
    (even(w as f64 * scale), even(h as f64 * scale))
}

/// How a stream's YUV samples map to RGB (from the SPS VUI).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colour {
    /// 0-255 samples instead of 16-235 (NVENC streams are full range).
    pub full_range: bool,
    pub bt709: bool,
}

impl Colour {
    pub const BT601_LIMITED: Colour = Colour { full_range: false, bt709: false };

    /// (luma scale, R from V, G from U, G from V, B from U), in 1/256 steps.
    fn coefficients(self) -> (i32, i32, i32, i32, i32) {
        match (self.full_range, self.bt709) {
            (false, false) => (298, 409, 100, 208, 516),
            (false, true) => (298, 459, 55, 136, 541),
            (true, false) => (256, 359, 88, 183, 454),
            (true, true) => (256, 403, 48, 120, 475),
        }
    }
}

/// Converts an I420 frame to RGBA at `out_w` x `out_h` (smaller than or equal to the frame),
/// sampling luma bilinearly so text stays readable when shrunk.
#[allow(clippy::too_many_arguments)]
pub fn i420_to_rgba_scaled(
    y: &[u8], u: &[u8], v: &[u8], y_stride: usize, uv_stride: usize,
    w: usize, h: usize, out: &mut [u8], out_w: usize, out_h: usize,
) {
    yuv420_to_rgba_scaled(y, y_stride, |row, x| (u[row * uv_stride + x], v[row * uv_stride + x]), w, h, out, out_w, out_h, Colour::BT601_LIMITED);
}

/// The same for NV12 (interleaved UV rows, as GPU decoders produce), in the stream's colours.
#[allow(clippy::too_many_arguments)]
pub fn nv12_to_rgba_scaled(
    y: &[u8], uv: &[u8], y_stride: usize, uv_stride: usize,
    w: usize, h: usize, out: &mut [u8], out_w: usize, out_h: usize, colour: Colour,
) {
    yuv420_to_rgba_scaled(y, y_stride, |row, x| (uv[row * uv_stride + 2 * x], uv[row * uv_stride + 2 * x + 1]), w, h, out, out_w, out_h, colour);
}

/// `chroma(row, x)` returns (U, V) of chroma sample `x` on chroma row `row`.
#[allow(clippy::too_many_arguments)]
fn yuv420_to_rgba_scaled(
    y: &[u8], y_stride: usize, chroma: impl Fn(usize, usize) -> (u8, u8) + Sync,
    w: usize, h: usize, out: &mut [u8], out_w: usize, out_h: usize, colour: Colour,
) {
    use rayon::prelude::*;
    if out_w == 0 || out_h == 0 || w == 0 || h == 0 {
        return;
    }
    let (ky, kr, kgu, kgv, kb) = colour.coefficients();
    let black = if colour.full_range { 0 } else { 16 };
    // Source position of each output column, in 1/256 pixel steps.
    let map = |o: usize, n_out: usize, n_in: usize| -> (usize, usize, u32) {
        let pos = ((o as f64 + 0.5) * n_in as f64 / n_out as f64 - 0.5).max(0.0);
        let i0 = (pos as usize).min(n_in - 1);
        let i1 = (i0 + 1).min(n_in - 1);
        (i0, i1, ((pos - i0 as f64) * 256.0) as u32)
    };
    let cols: Vec<(usize, usize, u32)> = (0..out_w).map(|x| map(x, out_w, w)).collect();
    out[..out_w * out_h * 4].par_chunks_mut(out_w * 4).enumerate().for_each(|(oy, row)| {
        let (y0, y1, fy) = map(oy, out_h, h);
        let (r0, r1) = (&y[y0 * y_stride..], &y[y1 * y_stride..]);
        let chroma_row = y0.min(h - 1) / 2;
        for (ox, px) in row.chunks_exact_mut(4).enumerate() {
            let (x0, x1, fx) = cols[ox];
            let top = r0[x0] as u32 * (256 - fx) + r0[x1] as u32 * fx;
            let bottom = r1[x0] as u32 * (256 - fx) + r1[x1] as u32 * fx;
            let luma = ((top * (256 - fy) + bottom * fy + 32768) >> 16) as i32;
            let (u, v) = chroma(chroma_row, x0 / 2);
            let (d, e) = (u as i32 - 128, v as i32 - 128);
            let c = ky * (luma - black);
            px[0] = ((c + kr * e + 128) >> 8).clamp(0, 255) as u8;
            px[1] = ((c - kgu * d - kgv * e + 128) >> 8).clamp(0, 255) as u8;
            px[2] = ((c + kb * d + 128) >> 8).clamp(0, 255) as u8;
            px[3] = 255;
        }
    });
}

#[cfg(test)]
mod watching_tests {
    use super::*;

    #[test]
    fn main_window_counts_only_when_shown_and_not_minimized() {
        assert!(anyone_watching(true, true, false, false));
        assert!(!anyone_watching(false, true, false, false), "in the tray");
        assert!(!anyone_watching(true, false, false, false), "minimized");
    }

    #[test]
    fn popout_counts_only_with_a_stream_and_not_minimized() {
        assert!(anyone_watching(false, false, true, true));
        assert!(!anyone_watching(false, false, true, false), "popout minimized");
        assert!(!anyone_watching(false, false, false, true), "popout closed");
    }
}

#[cfg(test)]
mod scaled_conversion_tests {
    use super::*;

    fn solid_nv12(w: usize, h: usize, yv: u8, uv: u8, vv: u8) -> (Vec<u8>, Vec<u8>) {
        (vec![yv; w * h], [uv, vv].repeat((w / 2) * (h / 2)))
    }

    fn assert_close(px: &[u8], expect: [u8; 3]) {
        for c in 0..3 {
            assert!((px[c] as i32 - expect[c] as i32).abs() <= 3, "got {:?}, expected {:?}", &px[..3], expect);
        }
        assert_eq!(px[3], 255);
    }

    #[test]
    fn nv12_honours_range_and_matrix() {
        let full709 = Colour { full_range: true, bt709: true };
        let limited709 = Colour { full_range: false, bt709: true };
        let cases = [
            (full709, (255, 128, 128), [255, 255, 255]),
            (full709, (0, 128, 128), [0, 0, 0]),
            (full709, (54, 99, 255), [255, 0, 0]),
            (limited709, (235, 128, 128), [255, 255, 255]),
            (limited709, (16, 128, 128), [0, 0, 0]),
            (limited709, (63, 102, 240), [255, 0, 0]),
        ];
        for (colour, (yv, uv, vv), expect) in cases {
            let (y, uvp) = solid_nv12(16, 8, yv, uv, vv);
            let mut out = vec![0u8; 8 * 4 * 4];
            nv12_to_rgba_scaled(&y, &uvp, 16, 16, 16, 8, &mut out, 8, 4, colour);
            for px in out.chunks_exact(4) {
                assert_close(px, expect);
            }
        }
    }

    #[test]
    fn nv12_reads_interleaved_chroma_per_column() {
        // Left half blue (high U), right half red (high V), BT.601 limited.
        let (w, h) = (16, 4);
        let y = vec![81u8; w * h];
        let mut uv = Vec::new();
        for _ in 0..h / 2 {
            for x in 0..w / 2 {
                uv.extend_from_slice(if x < w / 4 { &[240, 110] } else { &[90, 240] });
            }
        }
        let mut out = vec![0u8; w * h * 4];
        nv12_to_rgba_scaled(&y, &uv, w, w, w, h, &mut out, w, h, Colour::BT601_LIMITED);
        let (left, right) = (&out[0..4], &out[(w - 1) * 4..w * 4]);
        assert!(left[2] > 200 && left[0] < 100, "left should be blue: {left:?}");
        assert!(right[0] > 200 && right[2] < 100, "right should be red: {right:?}");
    }

    #[test]
    fn fit_keeps_aspect_and_never_upscales() {
        assert_eq!(fit_within(1920, 1080, 980, 620), (980, 550));
        assert_eq!(fit_within(1920, 1080, 3840, 2160), (1920, 1080));
        assert_eq!(fit_within(1920, 1080, 0, 0), (1920, 1080));
        assert_eq!(fit_within(1080, 1920, 620, 620), (348, 620));
    }

    fn solid_i420(w: usize, h: usize, yv: u8, uv: u8, vv: u8) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        (vec![yv; w * h], vec![uv; (w / 2) * (h / 2)], vec![vv; (w / 2) * (h / 2)])
    }

    #[test]
    fn converts_colours_like_the_full_size_path() {
        // BT.601 limited range: Y=235/U=V=128 is white, Y=16 black, (81, 90, 240) red.
        for ((yv, uv, vv), expect) in [((235, 128, 128), [255, 255, 255]), ((16, 128, 128), [0, 0, 0]), ((81, 90, 240), [255, 0, 0])] {
            let (y, u, v) = solid_i420(16, 8, yv, uv, vv);
            let mut out = vec![0u8; 8 * 4 * 4];
            i420_to_rgba_scaled(&y, &u, &v, 16, 8, 16, 8, &mut out, 8, 4);
            for px in out.chunks_exact(4) {
                for c in 0..3 {
                    assert!((px[c] as i32 - expect[c] as i32).abs() <= 3, "got {:?}, expected {:?}", &px[..3], expect);
                }
                assert_eq!(px[3], 255);
            }
        }
    }

    #[test]
    fn downscaling_keeps_left_and_right_halves_apart() {
        let (w, h) = (64, 16);
        let mut y = vec![16u8; w * h];
        for row in 0..h { for col in w / 2..w { y[row * w + col] = 235; } }
        let (u, v) = (vec![128u8; (w / 2) * (h / 2)], vec![128u8; (w / 2) * (h / 2)]);
        let (ow, oh) = (16, 4);
        let mut out = vec![0u8; ow * oh * 4];
        i420_to_rgba_scaled(&y, &u, &v, w, w / 2, w, h, &mut out, ow, oh);
        for row in 0..oh {
            assert!(out[(row * ow) * 4] < 10, "left edge should stay black");
            assert!(out[(row * ow + ow - 1) * 4] > 245, "right edge should stay white");
        }
    }
}
