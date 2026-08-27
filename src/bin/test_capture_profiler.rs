use std::time::{Instant, Duration};

#[cfg(windows)]
fn main() {
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject,
        GetDC, GetDIBits, ReleaseDC, SelectObject, SetStretchBltMode, StretchBlt, BitBlt,
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, COLORONCOLOR, DIB_RGB_COLORS, SRCCOPY,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetDesktopWindow, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
    };

    use windows_sys::Win32::Graphics::Gdi::{
        GetDeviceCaps, DESKTOPHORZRES, DESKTOPVERTRES, HORZRES, VERTRES,
    };

    println!("=======================================================");
    println!("🔍 DIAGNÓSTICO PROFUNDO DE CAPTURA DE TELA DO WINDOWS");
    println!("=======================================================\n");

    let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    let (real_w, real_h) = unsafe {
        let hdc = GetDC(std::ptr::null_mut());
        let rw = GetDeviceCaps(hdc, DESKTOPHORZRES as i32);
        let rh = GetDeviceCaps(hdc, DESKTOPVERTRES as i32);
        ReleaseDC(std::ptr::null_mut(), hdc);
        (rw, rh)
    };

    let target_w = 1920i32;
    let target_h = 1080i32;
    println!("🖥️ Resolução Lógica (GetSystemMetrics): {}x{}", screen_w, screen_h);
    println!("🖥️ Resolução Física Real (DESKTOPHORZRES): {}x{}", real_w, real_h);
    println!("🎯 Resolução Alvo de Transmissão: {}x{}\n", target_w, target_h);

    let num_frames = 60;

    // -------------------------------------------------------------
    // Teste 1: Método com CreateCompatibleBitmap + GetDIBits
    // -------------------------------------------------------------
    println!("--- Teste 1: Método GetDIBits (DDB -> CPU Readback) ---");
    unsafe {
        let hwnd_desktop = GetDesktopWindow();
        let hdc_desktop = GetDC(hwnd_desktop);
        let hdc_mem = CreateCompatibleDC(hdc_desktop);
        let hbm_screen = CreateCompatibleBitmap(hdc_desktop, target_w, target_h);
        SelectObject(hdc_mem, hbm_screen);

        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = target_w;
        bmi.bmiHeader.biHeight = -target_h;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB as u32;

        let mut bgra_buf = vec![0u8; (target_w * target_h * 4) as usize];

        let mut total_blt = std::time::Duration::ZERO;
        let mut total_dib = std::time::Duration::ZERO;

        for _ in 0..num_frames {
            let t0 = Instant::now();
            if screen_w == target_w && screen_h == target_h {
                BitBlt(hdc_mem, 0, 0, target_w, target_h, hdc_desktop, 0, 0, SRCCOPY);
            } else {
                SetStretchBltMode(hdc_mem, COLORONCOLOR);
                StretchBlt(hdc_mem, 0, 0, target_w, target_h, hdc_desktop, 0, 0, screen_w, screen_h, SRCCOPY);
            }
            total_blt += t0.elapsed();

            let t1 = Instant::now();
            GetDIBits(hdc_mem, hbm_screen, 0, target_h as u32, bgra_buf.as_mut_ptr() as _, &mut bmi, DIB_RGB_COLORS);
            total_dib += t1.elapsed();
        }

        let avg_blt_ms = (total_blt.as_secs_f64() * 1000.0) / (num_frames as f64);
        let avg_dib_ms = (total_dib.as_secs_f64() * 1000.0) / (num_frames as f64);
        let total_ms = avg_blt_ms + avg_dib_ms;
        println!("  ├─ Tempo Blt (BitBlt/StretchBlt): {:.2} ms/frame", avg_blt_ms);
        println!("  ├─ Tempo GetDIBits (GPU Readback): {:.2} ms/frame", avg_dib_ms);
        println!("  └─ ⏱️ TOTAL Método 1: {:.2} ms/frame ({:.1} FPS)\n", total_ms, 1000.0 / total_ms);

        DeleteObject(hbm_screen);
        DeleteDC(hdc_mem);
        ReleaseDC(hwnd_desktop, hdc_desktop);
    }

    // -------------------------------------------------------------
    // Teste 2: Método CreateDIBSection (Direct Memory Ptr, Zero Readback)
    // -------------------------------------------------------------
    println!("--- Teste 2: Método CreateDIBSection (Ponteiro Direto em RAM) ---");
    unsafe {
        let hwnd_desktop = GetDesktopWindow();
        let hdc_desktop = GetDC(hwnd_desktop);
        let hdc_mem = CreateCompatibleDC(hdc_desktop);

        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = target_w;
        bmi.bmiHeader.biHeight = -target_h; // Top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB as u32;

        let mut p_bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let hbm_dib = CreateDIBSection(
            hdc_desktop,
            &bmi,
            DIB_RGB_COLORS,
            &mut p_bits,
            std::ptr::null_mut(),
            0,
        );
        SelectObject(hdc_mem, hbm_dib);

        let mut total_dibsec = std::time::Duration::ZERO;

        for _ in 0..num_frames {
            let t0 = Instant::now();
            if screen_w == target_w && screen_h == target_h {
                BitBlt(hdc_mem, 0, 0, target_w, target_h, hdc_desktop, 0, 0, SRCCOPY);
            } else {
                SetStretchBltMode(hdc_mem, COLORONCOLOR);
                StretchBlt(hdc_mem, 0, 0, target_w, target_h, hdc_desktop, 0, 0, screen_w, screen_h, SRCCOPY);
            }
            // Pixels are immediately in p_bits pointer!
            let _slice = std::slice::from_raw_parts(p_bits as *const u8, (target_w * target_h * 4) as usize);
            total_dibsec += t0.elapsed();
        }

        let avg_dibsec_ms = (total_dibsec.as_secs_f64() * 1000.0) / (num_frames as f64);
        println!("  ├─ Tempo Blt direto para memória: {:.2} ms/frame", avg_dibsec_ms);
        println!("  ├─ Tempo GetDIBits necessário: 0.00 ms (Eliminado completamente!)");
        println!("  └─ 🚀 TOTAL Método 2 (DIB Section): {:.2} ms/frame ({:.1} FPS)\n", avg_dibsec_ms, 1000.0 / avg_dibsec_ms);

        // -------------------------------------------------------------
        // Teste 3: Captura DIBSection + Conversão BT.601 + Encode H.264
        // -------------------------------------------------------------
        println!("--- Teste 3: Pipeline Completo (Captura DIBSection + Encode H.264 1080p 60 FPS) ---");
        use openh264::OpenH264API;
        use openh264::encoder::{Encoder, EncoderConfig, UsageType, RateControlMode};
        use openh264::formats::YUVSlices;

        let num_threads = std::thread::available_parallelism().map(|n| n.get() as u16).unwrap_or(4).clamp(2, 8);
        let enc_config = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .set_multiple_thread_idc(num_threads)
            .set_bitrate_bps(3_500_000)
            .max_frame_rate(60.0)
            .enable_skip_frame(true)
            .rate_control_mode(RateControlMode::Quality);

        let mut encoder = Encoder::with_api_config(OpenH264API::from_source(), enc_config).unwrap();
        let mut yuv_buf = vec![0u8; (target_w * target_h * 3 / 2) as usize];
        let u_base = (target_w * target_h) as usize;
        let v_base = u_base / 4;

        let mut total_full_cap = std::time::Duration::ZERO;
        let mut total_full_enc = std::time::Duration::ZERO;
        let mut total_bytes = 0usize;

        for _ in 0..num_frames {
            let t_cap = Instant::now();
            if screen_w == target_w && screen_h == target_h {
                BitBlt(hdc_mem, 0, 0, target_w, target_h, hdc_desktop, 0, 0, SRCCOPY);
            } else {
                SetStretchBltMode(hdc_mem, COLORONCOLOR);
                StretchBlt(hdc_mem, 0, 0, target_w, target_h, hdc_desktop, 0, 0, screen_w, screen_h, SRCCOPY);
            }
            let bgra_slice = std::slice::from_raw_parts(p_bits as *const u8, (target_w * target_h * 4) as usize);
            total_full_cap += t_cap.elapsed();

            let t_enc = Instant::now();
            {
                let (y_plane, uv_plane) = yuv_buf.split_at_mut(u_base);
                let (u_plane, v_plane) = uv_plane.split_at_mut(v_base);
                let w = target_w as usize;
                let h = target_h as usize;

                for j in 0..h {
                    let row_bgra = &bgra_slice[j * w * 4..(j + 1) * w * 4];
                    let row_y = &mut y_plane[j * w..(j + 1) * w];
                    let is_even_row = (j % 2) == 0;
                    let uv_row = (j / 2) * (w / 2);

                    for i in 0..w {
                        let b = row_bgra[i * 4] as i32;
                        let g = row_bgra[i * 4 + 1] as i32;
                        let r = row_bgra[i * 4 + 2] as i32;

                        row_y[i] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;

                        if is_even_row && (i % 2 == 0) {
                            let uv_idx = uv_row + (i / 2);
                            u_plane[uv_idx] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
                            v_plane[uv_idx] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
                        }
                    }
                }
            }

            let (y_plane, uv_plane) = yuv_buf.split_at(u_base);
            let (u_plane, v_plane) = uv_plane.split_at(v_base);
            let yuv_slices = YUVSlices::new((y_plane, u_plane, v_plane), (target_w as usize, target_h as usize), (target_w as usize, (target_w / 2) as usize, (target_w / 2) as usize));

            if let Ok(stream) = encoder.encode(&yuv_slices) {
                total_bytes += stream.to_vec().len();
            }
            total_full_enc += t_enc.elapsed();
        }

        let full_cap_ms = (total_full_cap.as_secs_f64() * 1000.0) / (num_frames as f64);
        let full_enc_ms = (total_full_enc.as_secs_f64() * 1000.0) / (num_frames as f64);
        let full_total_ms = full_cap_ms + full_enc_ms;
        let full_fps = 1000.0 / full_total_ms;

        println!("  ├─ ⏱️ Tempo de Captura (DIB Section): {:.2} ms/frame", full_cap_ms);
        println!("  ├─ ⏱️ Tempo de Conversão + Encode H.264: {:.2} ms/frame", full_enc_ms);
        println!("  ├─ 🎯 TEMPO TOTAL DO LOOP REAL: {:.2} ms/frame", full_total_ms);
        println!("  └─ 🏆 TAXA MÁXIMA REAL ALCANÇADA: {:.1} FPS! (Meta: 60 FPS)\n", full_fps);

        // -------------------------------------------------------------
        // Teste 5: Pacing Híbrido de 60 FPS Reais (Zero Oversleep)
        // -------------------------------------------------------------
        println!("--- Teste 5: Pacing de 60 FPS Reais com Hybrid Spin-Wait ---");
        let target_fps = 60u64;
        let frame_interval = Duration::from_nanos(1_000_000_000 / target_fps);
        let mut loop_frames = 0u64;
        let test_start = Instant::now();
        let mut next_frame_time = Instant::now() + frame_interval;

        for _ in 0..120 { // 2 segundos a 60 FPS = 120 frames
            // Simula captura + encode real (9.5 ms)
            let t_work = Instant::now();
            BitBlt(hdc_mem, 0, 0, target_w, target_h, hdc_desktop, 0, 0, SRCCOPY);
            let bgra_slice = std::slice::from_raw_parts(p_bits as *const u8, (target_w * target_h * 4) as usize);
            
            {
                let (y_plane, uv_plane) = yuv_buf.split_at_mut(u_base);
                let (u_plane, v_plane) = uv_plane.split_at_mut(v_base);
                let w = target_w as usize;
                let h = target_h as usize;

                for j in 0..h {
                    let row_bgra = &bgra_slice[j * w * 4..(j + 1) * w * 4];
                    let row_y = &mut y_plane[j * w..(j + 1) * w];
                    let is_even_row = (j % 2) == 0;
                    let uv_row = (j / 2) * (w / 2);

                    for i in 0..w {
                        let b = row_bgra[i * 4] as i32;
                        let g = row_bgra[i * 4 + 1] as i32;
                        let r = row_bgra[i * 4 + 2] as i32;

                        row_y[i] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;

                        if is_even_row && (i % 2 == 0) {
                            let uv_idx = uv_row + (i / 2);
                            u_plane[uv_idx] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
                            v_plane[uv_idx] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
                        }
                    }
                }
            }

            let (y_plane, uv_plane) = yuv_buf.split_at(u_base);
            let (u_plane, v_plane) = uv_plane.split_at(v_base);
            let yuv_slices = YUVSlices::new((y_plane, u_plane, v_plane), (target_w as usize, target_h as usize), (target_w as usize, (target_w / 2) as usize, (target_w / 2) as usize));
            let _ = encoder.encode(&yuv_slices);

            // Hybrid Pacing
            let now = Instant::now();
            if now < next_frame_time {
                let remaining = next_frame_time - now;
                if remaining > Duration::from_millis(3) {
                    std::thread::sleep(remaining - Duration::from_millis(2));
                }
                while Instant::now() < next_frame_time {
                    std::hint::spin_loop();
                }
            }
            next_frame_time = Instant::now() + frame_interval;
            loop_frames += 1;
        }

        let total_time_s = test_start.elapsed().as_secs_f64();
        let delivered_fps = (loop_frames as f64) / total_time_s;
        println!("  └─ 🚀 TAXA REAL DE ENTREGA NO RELÓGIO: {:.1} FPS! (Alvo: 60.0 FPS)\n", delivered_fps);

        DeleteObject(hbm_dib);
        DeleteDC(hdc_mem);
        ReleaseDC(hwnd_desktop, hdc_desktop);
    }
}

#[cfg(not(windows))]
fn main() {
    println!("Diagnóstico Windows apenas.");
}
