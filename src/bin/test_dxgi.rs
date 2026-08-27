use rayon::prelude::*;
use std::time::Instant;

fn main() {
    let w = 1920usize;
    let h = 1080usize;
    let u_base = w * h;
    let v_base = u_base / 4;
    let total_yuv = u_base + v_base * 2;
    let bgra_data = vec![200u8; w * h * 4];
    let mut yuv_buffer = vec![0u8; total_yuv];

    // Benchmark Rayon Parallel Conversion
    let t0 = Instant::now();
    for _ in 0..10 {
        let (y_plane, uv_plane) = yuv_buffer.split_at_mut(u_base);
        let (u_plane, v_plane) = uv_plane.split_at_mut(v_base);

        let y_ptr = y_plane.as_mut_ptr() as usize;
        let u_ptr = u_plane.as_mut_ptr() as usize;
        let v_ptr = v_plane.as_mut_ptr() as usize;

        let row_pairs: Vec<usize> = (0..h).step_by(2).collect();
        row_pairs.par_chunks(32).for_each(|chunk| {
            let y_mut = y_ptr as *mut u8;
            let u_mut = u_ptr as *mut u8;
            let v_mut = v_ptr as *mut u8;
            for &j in chunk {
                let row0_bgra = &bgra_data[j * w * 4..(j + 1) * w * 4];
                let row1_bgra = &bgra_data[(j + 1) * w * 4..(j + 2) * w * 4];
                let base0 = j * w;
                let base1 = (j + 1) * w;
                let uv_row = (j / 2) * (w / 2);

                for i in (0..w).step_by(2) {
                    let b0 = row0_bgra[i * 4] as i32;
                    let g0 = row0_bgra[i * 4 + 1] as i32;
                    let r0 = row0_bgra[i * 4 + 2] as i32;
                    unsafe {
                        *y_mut.add(base0 + i) = (((66 * r0 + 129 * g0 + 25 * b0 + 128) >> 8) + 16) as u8;

                        let b1 = row0_bgra[(i + 1) * 4] as i32;
                        let g1 = row0_bgra[(i + 1) * 4 + 1] as i32;
                        let r1 = row0_bgra[(i + 1) * 4 + 2] as i32;
                        *y_mut.add(base0 + i + 1) = (((66 * r1 + 129 * g1 + 25 * b1 + 128) >> 8) + 16) as u8;

                        let b2 = row1_bgra[i * 4] as i32;
                        let g2 = row1_bgra[i * 4 + 1] as i32;
                        let r2 = row1_bgra[i * 4 + 2] as i32;
                        *y_mut.add(base1 + i) = (((66 * r2 + 129 * g2 + 25 * b2 + 128) >> 8) + 16) as u8;

                        let b3 = row1_bgra[(i + 1) * 4] as i32;
                        let g3 = row1_bgra[(i + 1) * 4 + 1] as i32;
                        let r3 = row1_bgra[(i + 1) * 4 + 2] as i32;
                        *y_mut.add(base1 + i + 1) = (((66 * r3 + 129 * g3 + 25 * b3 + 128) >> 8) + 16) as u8;

                        let r_avg = (r0 + r1 + r2 + r3) >> 2;
                        let g_avg = (g0 + g1 + g2 + g3) >> 2;
                        let b_avg = (b0 + b1 + b2 + b3) >> 2;

                        let uv_idx = uv_row + (i / 2);
                        *u_mut.add(uv_idx) = (((-38 * r_avg - 74 * g_avg + 112 * b_avg + 128) >> 8) + 128) as u8;
                        *v_mut.add(uv_idx) = (((112 * r_avg - 94 * g_avg - 18 * b_avg + 128) >> 8) + 128) as u8;
                    }
                }
            }
        });
    }
    let dur = t0.elapsed() / 10;
    println!("Rayon Parallel BGRA->YUV420 1080p time: {:.2} ms!", dur.as_secs_f64() * 1000.0);
}
