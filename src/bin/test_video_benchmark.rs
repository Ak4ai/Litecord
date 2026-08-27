use std::time::Instant;

fn fit_bgra_to_canvas(
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

    if src_w == canvas_w && src_h == canvas_h {
        let len = (canvas_w * canvas_h * 4) as usize;
        if src_bgra.len() >= len && out_bgra.len() >= len {
            out_bgra[..len].copy_from_slice(&src_bgra[..len]);
            return;
        }
    }

    let total_pixels = (canvas_w * canvas_h) as usize;
    let canvas_u32: &mut [u32] = unsafe {
        std::slice::from_raw_parts_mut(out_bgra.as_mut_ptr() as *mut u32, total_pixels)
    };
    canvas_u32.fill(0xFF111214);

    let scale_w = canvas_w as f32 / src_w as f32;
    let scale_h = canvas_h as f32 / src_h as f32;
    let scale = scale_w.min(scale_h);

    let dest_w = ((src_w as f32 * scale).round() as u32).clamp(2, canvas_w) & !1;
    let dest_h = ((src_h as f32 * scale).round() as u32).clamp(2, canvas_h) & !1;
    let dest_x = ((canvas_w.saturating_sub(dest_w)) / 2) & !1;
    let dest_y = ((canvas_h.saturating_sub(dest_h)) / 2) & !1;

    let x_step = ((src_w as u64) << 16) / (dest_w as u64);
    let y_step = ((src_h as u64) << 16) / (dest_h as u64);

    let src_u32: &[u32] = unsafe {
        std::slice::from_raw_parts(src_bgra.as_ptr() as *const u32, (src_w * src_h) as usize)
    };

    let mut src_y_accum = 0u64;
    for dy in 0..dest_h {
        let sy = ((src_y_accum >> 16) as u32).min(src_h - 1);
        let src_row = &src_u32[(sy * src_w) as usize..((sy + 1) * src_w) as usize];
        let dst_row_start = ((dest_y + dy) * canvas_w + dest_x) as usize;
        let dst_row = &mut canvas_u32[dst_row_start..dst_row_start + dest_w as usize];

        let mut src_x_accum = 0u64;
        for dx in 0..dest_w {
            let sx = ((src_x_accum >> 16) as u32).min(src_w - 1);
            dst_row[dx as usize] = src_row[sx as usize];
            src_x_accum += x_step;
        }
        src_y_accum += y_step;
    }
}

fn main() {
    let src_w = 1237u32;
    let src_h = 749u32;
    let canvas_w = 1920u32;
    let canvas_h = 1080u32;

    let src_buf = vec![0xABu8; (src_w * src_h * 4) as usize];
    let mut out_buf = vec![0u8; (canvas_w * canvas_h * 4) as usize];

    let iters = 100;
    let t0 = Instant::now();
    for _ in 0..iters {
        fit_bgra_to_canvas(&src_buf, src_w, src_h, canvas_w, canvas_h, &mut out_buf);
    }
    let elapsed = t0.elapsed();
    let avg_ms = (elapsed.as_secs_f64() * 1000.0) / (iters as f64);
    println!("Benchmark fit_bgra_to_canvas ({}x{} -> {}x{}):", src_w, src_h, canvas_w, canvas_h);
    println!("  ├─ Tempo Médio: {:.3} ms por frame", avg_ms);
    println!("  └─ FPS Suportado: {:.1} FPS", 1000.0 / avg_ms);
}
