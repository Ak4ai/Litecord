#![allow(dead_code)]

#[allow(unused_imports)]
use slint::{Rgba8Pixel, SharedPixelBuffer};
#[allow(unused_imports)]
use super::super::types::{CapturableWindowItem, MonitorItemInfo};

#[cfg(not(windows))]
pub static PORTAL_FRAME: std::sync::Mutex<Option<Vec<u8>>> = std::sync::Mutex::new(None);
#[cfg(not(windows))]
pub static PORTAL_INITIALIZED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(not(windows))]
pub static PORTAL_CANCELLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(target_os = "linux")]
static PORTAL_LAST_ATTEMPT: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
#[cfg(target_os = "linux")]
pub static PORTAL_LOCAL_CB: std::sync::Mutex<Option<std::sync::Arc<dyn Fn(SharedPixelBuffer<Rgba8Pixel>) + Send + Sync + 'static>>> = std::sync::Mutex::new(None);
#[cfg(target_os = "linux")]
pub static PORTAL_CHILD: std::sync::Mutex<Option<std::process::Child>> = std::sync::Mutex::new(None);

#[cfg(target_os = "linux")]
pub fn reset_wayland_portal_cancelled() {
    PORTAL_CANCELLED.store(false, std::sync::atomic::Ordering::SeqCst);
    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(target_os = "linux")]
pub fn kill_portal_child() {
    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
    PORTAL_CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Ok(mut lock) = PORTAL_CHILD.lock() {
        if let Some(mut child) = lock.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(target_os = "linux")]
pub fn init_wayland_portal_screencast(target_w: u32, target_h: u32, target_fps: u64) {
    if PORTAL_CANCELLED.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }

    if let Ok(guard) = PORTAL_LAST_ATTEMPT.lock() {
        if let Some(last) = *guard {
            if last.elapsed() < std::time::Duration::from_secs(3) {
                return;
            }
        }
    }

    if PORTAL_INITIALIZED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }

    if let Ok(mut guard) = PORTAL_LAST_ATTEMPT.lock() {
        *guard = Some(std::time::Instant::now());
    }

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
            Ok(r) => r,
            Err(e) => {
                log::error!("Falha ao criar tokio runtime para Portal ScreenCast: {:?}", e);
                PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                return;
            }
        };

        let _guard = rt.enter();

        rt.block_on(async move {
            use ashpd::desktop::{
                PersistMode,
                screencast::{
                    CursorMode, OpenPipeWireRemoteOptions, Screencast,
                    SelectSourcesOptions, SourceType, StartCastOptions,
                },
                CreateSessionOptions,
            };
            use std::os::fd::AsRawFd;
            use std::process::{Command, Stdio};
            use std::io::Read;

            log::info!("📡 Solicitando sessão nativa do XDG Desktop Portal ScreenCast via conexão D-Bus dedicada...");
            let conn = match ashpd::zbus::connection::Builder::session() {
                Ok(b) => match b.build().await {
                    Ok(c) => c,
                    Err(e) => {
                        log::error!("Falha ao estabelecer conexão D-Bus privada para Portal: {:?}", e);
                        PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                        return;
                    }
                },
                Err(e) => {
                    log::error!("Falha ao construir builder D-Bus para Portal: {:?}", e);
                    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };

            let proxy = match Screencast::with_connection(conn).await {
                Ok(p) => p,
                Err(e) => {
                    log::error!("Falha ao conectar no Screencast Portal: {:?}", e);
                    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };

            let session = match proxy.create_session(CreateSessionOptions::default()).await {
                Ok(s) => s,
                Err(e) => {
                    log::error!("Falha ao criar sessão do ScreenCast: {:?}", e);
                    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };

            let select_opts = SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Embedded)
                .set_sources(SourceType::Monitor | SourceType::Window)
                .set_multiple(false)
                .set_restore_token(None)
                .set_persist_mode(PersistMode::DoNot);

            if let Err(e) = proxy.select_sources(&session, select_opts).await {
                log::error!("Falha ao selecionar fontes de captura no Portal: {:?}", e);
                let _ = session.close().await;
                PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                return;
            }

            let response = match proxy.start(&session, None, StartCastOptions::default()).await {
                Ok(r) => match r.response() {
                    Ok(resp) => resp,
                    Err(e) => {
                        log::error!("Resposta de erro do Portal ScreenCast: {:?}", e);
                        let _ = session.close().await;
                        PORTAL_CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
                        PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                        return;
                    }
                },
                Err(e) => {
                    log::error!("Falha ao iniciar captura no Portal ScreenCast: {:?}", e);
                    let _ = session.close().await;
                    PORTAL_CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
                    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };

            let stream = match response.streams().first() {
                Some(s) => s,
                None => {
                    log::error!("Nenhum stream retornado pelo Portal ScreenCast");
                    let _ = session.close().await;
                    PORTAL_CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
                    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };

            let node_id = stream.pipe_wire_node_id();
            let pw_fd = match proxy.open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default()).await {
                Ok(fd) => fd,
                Err(e) => {
                    log::error!("Falha ao abrir PipeWire Remote: {:?}", e);
                    let _ = session.close().await;
                    PORTAL_CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
                    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };

            let raw_fd = pw_fd.as_raw_fd();
            unsafe {
                let flags = libc::fcntl(raw_fd, libc::F_GETFD);
                if flags >= 0 {
                    libc::fcntl(raw_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
                }
            }

            log::info!("🎉 Conectando GStreamer ao PipeWire Node ID={}, FD={} ({}x{} @ {} FPS)...", node_id, raw_fd, target_w, target_h, target_fps);

            let (dst_w, dst_h) = (target_w, target_h);
            let frame_size = (dst_w * dst_h * 4) as usize;

            let mut gst_args = vec![
                "-q".to_string(),
                "pipewiresrc".to_string(),
                format!("fd={}", raw_fd),
                format!("path={}", node_id),
                "do-timestamp=true".to_string(),
                "!".to_string(),
                "videoconvert".to_string(),
                "n-threads=4".to_string(),
                "!".to_string(),
                "videoscale".to_string(),
                "!".to_string(),
            ];

            if target_fps > 0 && target_fps < 120 {
                gst_args.push("videorate".to_string());
                gst_args.push("!".to_string());
                gst_args.push(format!("video/x-raw,format=BGRA,width={},height={},framerate={}/1", dst_w, dst_h, target_fps));
            } else {
                gst_args.push(format!("video/x-raw,format=BGRA,width={},height={}", dst_w, dst_h));
            }

            gst_args.extend_from_slice(&[
                "!".to_string(),
                "queue".to_string(),
                "max-size-buffers=1".to_string(),
                "max-size-bytes=0".to_string(),
                "max-size-time=0".to_string(),
                "leaky=downstream".to_string(),
                "!".to_string(),
                "fdsink".to_string(),
                "fd=1".to_string(),
                "sync=false".to_string(),
            ]);

            log::info!("🔧 GStreamer pipeline args: {:?}", gst_args);

            let mut child = match Command::new("gst-launch-1.0")
                .env_remove("PIPEWIRE_NODE")
                .args(&gst_args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    log::error!("Falha ao iniciar pipeline GStreamer PipeWire: {:?}", e);
                    let _ = session.close().await;
                    PORTAL_CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
                    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };

            // Spawn a thread to log GStreamer stderr for debugging
            if let Some(stderr) = child.stderr.take() {
                std::thread::Builder::new()
                    .name("gst-stderr-logger".to_string())
                    .spawn(move || {
                        use std::io::BufRead;
                        let reader = std::io::BufReader::new(stderr);
                        for line in reader.lines() {
                            match line {
                                Ok(l) => log::warn!("🎬 [GStreamer STDERR]: {}", l),
                                Err(_) => break,
                            }
                        }
                    })
                    .ok();
            }

            let mut stdout = match child.stdout.take() {
                Some(s) => s,
                None => {
                    let _ = session.close().await;
                    PORTAL_CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
                    PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            };

            if let Ok(mut lock) = PORTAL_CHILD.lock() {
                if let Some(mut old) = lock.take() {
                    let _ = old.kill();
                }
                *lock = Some(child);
            }

            let mut raw_buf = vec![0u8; frame_size];
            let mut pw_frame_count = 0u64;
            let mut last_pw_stats = std::time::Instant::now();
            let mut last_local_preview_ts = std::time::Instant::now() - std::time::Duration::from_secs(1);
            let mut got_first_frame = false;

            log::info!("⏳ Aguardando primeiro quadro do GStreamer PipeWire (timeout 10s)...");

            while PORTAL_INITIALIZED.load(std::sync::atomic::Ordering::Relaxed) && stdout.read_exact(&mut raw_buf).is_ok() {
                if !got_first_frame {
                    got_first_frame = true;
                    log::info!("✅ Primeiro quadro do GStreamer PipeWire recebido com sucesso!");
                }
                pw_frame_count += 1;
                let now = std::time::Instant::now();
                if now.duration_since(last_pw_stats) >= std::time::Duration::from_secs(1) {
                    let elapsed_s = now.duration_since(last_pw_stats).as_secs_f64();
                    let fps = (pw_frame_count as f64) / elapsed_s;
                    log::info!("📊 [TELEMETRIA] PipeWire GStreamer Source: {:.1} FPS ({} frames em {:.2}s)", fps, pw_frame_count, elapsed_s);
                    pw_frame_count = 0;
                    last_pw_stats = now;
                }

                // Despacha miniatura da interface local apenas periodicamente (~7 FPS) para eliminar 500 MB/s de alocação
                if now.duration_since(last_local_preview_ts) >= std::time::Duration::from_millis(150) {
                    last_local_preview_ts = now;
                    if let Ok(cb_guard) = PORTAL_LOCAL_CB.lock() {
                        if let Some(ref local_cb) = *cb_guard {
                            let mut pixel_buffer = SharedPixelBuffer::<Rgba8Pixel>::new(dst_w, dst_h);
                            let bytes = pixel_buffer.make_mut_bytes();
                            bytes.copy_from_slice(&raw_buf);
                            for chunk in bytes.chunks_exact_mut(4) {
                                chunk.swap(0, 2); // BGRA -> RGBA para exibição na UI Slint
                            }
                            if let Some(digit) = get_test_watermark_digit() {
                                draw_test_watermark(pixel_buffer.make_mut_slice(), &mut raw_buf, dst_w, dst_h, digit);
                            }
                            local_cb(pixel_buffer);
                        }
                    }
                }

                if let Ok(mut slot) = PORTAL_FRAME.lock() {
                    if let Some(ref mut dest) = *slot {
                        if dest.len() != raw_buf.len() {
                            dest.resize(raw_buf.len(), 0);
                        }
                        dest.copy_from_slice(&raw_buf);
                    } else {
                        *slot = Some(raw_buf.clone());
                    }
                }
            }

            if !got_first_frame {
                log::error!("❌ GStreamer PipeWire encerrou sem produzir nenhum quadro! Pipeline falhou.");
                // Check if child exited with an error
                if let Ok(mut lock) = PORTAL_CHILD.lock() {
                    if let Some(ref mut child) = *lock {
                        match child.try_wait() {
                            Ok(Some(status)) => log::error!("❌ GStreamer exit status: {}", status),
                            Ok(None) => log::warn!("⚠️ GStreamer ainda está rodando mas stdout fechou"),
                            Err(e) => log::error!("❌ Erro ao verificar status do GStreamer: {:?}", e),
                        }
                    }
                }
                PORTAL_CANCELLED.store(true, std::sync::atomic::Ordering::SeqCst);
            }

            if let Ok(mut lock) = PORTAL_CHILD.lock() {
                if let Some(mut child) = lock.take() {
                    let _ = child.kill();
                }
            }
            let _ = session.close().await;
            log::info!("🛑 Sessão do XDG Desktop Portal ScreenCast fechada com sucesso via D-Bus!");
            PORTAL_INITIALIZED.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    });
}

#[cfg(not(windows))]
pub fn list_screens() -> Vec<MonitorItemInfo> {
    let mut screens = Vec::new();
    #[cfg(target_os = "linux")]
    {
        use x11rb::connection::Connection;
        if let Ok((conn, screen_num)) = x11rb::connect(None) {
            let root = &conn.setup().roots[screen_num];
            screens.push(MonitorItemInfo {
                id: 0,
                name: "Tela Principal".to_string(),
                resolution: format!("{} × {}", root.width_in_pixels, root.height_in_pixels),
                is_primary: true,
                hwnd: root.root as isize,
            });
        }
    }

    if screens.is_empty() {
        screens.push(MonitorItemInfo {
            id: 0,
            name: "Tela 1 (Principal)".to_string(),
            resolution: "1920 × 1080".to_string(),
            is_primary: true,
            hwnd: 0,
        });
    }
    screens
}

#[cfg(not(windows))]
pub fn list_capturable_windows() -> Vec<CapturableWindowItem> {
    let mut windows = Vec::new();
    #[cfg(target_os = "linux")]
    {
        windows.push(CapturableWindowItem {
            id: "0".to_string(),
            title: "Janela Ativa / Desktop".to_string(),
            app_name: "X11 / Wayland".to_string(),
            icon_rgba: None,
        });
    }
    windows
}

#[cfg(not(windows))]
pub fn capture_screen_rgb(_target_hwnd: isize, target_w: u32, target_h: u32, target_fps: u64, out_bgra: &mut Vec<u8>) -> Option<(u128, u128)> {
    #[cfg(target_os = "linux")]
    {
        let capture_backend = crate::video_settings::get_video_capture_backend();
        if capture_backend != "x11" {
            init_wayland_portal_screencast(target_w, target_h, target_fps);

            if let Ok(slot) = PORTAL_FRAME.lock() {
                if let Some(ref frame) = *slot {
                    let total = (target_w * target_h * 4) as usize;
                    if frame.len() == total {
                        if out_bgra.len() != total {
                            out_bgra.resize(total, 0);
                        }
                        out_bgra.copy_from_slice(frame);
                        return Some((0, 0));
                    }
                }
            }
        }
        return None;
    }

    #[cfg(not(target_os = "linux"))]
    {
        let total = (target_w * target_h * 4) as usize;
        if out_bgra.len() != total {
            out_bgra.resize(total, 24);
        }
        Some((0, 0))
    }
}

const GLYPH_1: [u8; 12] = [
    0b00011000,
    0b00111000,
    0b01111000,
    0b00011000,
    0b00011000,
    0b00011000,
    0b00011000,
    0b00011000,
    0b00011000,
    0b00011000,
    0b01111110,
    0b01111110,
];

const GLYPH_2: [u8; 12] = [
    0b00111100,
    0b01100110,
    0b01100110,
    0b00000110,
    0b00001100,
    0b00011000,
    0b00110000,
    0b01100000,
    0b01100000,
    0b01100000,
    0b01111110,
    0b01111110,
];

pub fn get_test_watermark_digit() -> Option<char> {
    if let Ok(val) = std::env::var("LITECORD_INSTANCE_ID") {
        let v = val.trim();
        if v == "1" { return Some('1'); }
        if v == "2" { return Some('2'); }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_str = cwd.to_string_lossy().to_lowercase();
        if cwd_str.contains("instance2") || cwd_str.contains("profile2") || cwd_str.ends_with("_2") {
            return Some('2');
        }
        if cwd_str.contains("instance1") || cwd_str.contains("profile1") || cwd_str.ends_with("_1") {
            return Some('1');
        }
    }
    None
}

pub fn draw_test_watermark(
    slice: &mut [Rgba8Pixel],
    bgra_bytes: &mut [u8],
    width: u32,
    height: u32,
    digit: char,
) {
    let glyph = match digit {
        '1' => &GLYPH_1,
        '2' => &GLYPH_2,
        _ => return,
    };

    let cx = (width / 2) as i32;
    let cy = (height / 2) as i32;

    // Badge Dimensions
    let half_w = 90i32;
    let half_h = 90i32;
    let border_thick = 4i32;

    // Accent color: Instance 1 = Blurple (#5865F2), Instance 2 = Pink/Magenta (#EB459E)
    let (border_r, border_g, border_b) = if digit == '1' {
        (88u8, 101u8, 242u8)
    } else {
        (235u8, 69u8, 158u8)
    };

    // Draw background badge (dark semi-transparent box with solid border)
    for dy in -half_h..=half_h {
        let y = cy + dy;
        if y < 0 || y >= height as i32 {
            continue;
        }
        for dx in -half_w..=half_w {
            let x = cx + dx;
            if x < 0 || x >= width as i32 {
                continue;
            }

            let idx = (y as usize) * (width as usize) + (x as usize);
            let offset_bgra = idx * 4;

            let is_border = dx.abs() >= half_w - border_thick || dy.abs() >= half_h - border_thick;

            let (r, g, b) = if is_border {
                (border_r, border_g, border_b)
            } else {
                let cur_b = bgra_bytes[offset_bgra];
                let cur_g = bgra_bytes[offset_bgra + 1];
                let cur_r = bgra_bytes[offset_bgra + 2];
                let bg_r = 17u8;
                let bg_g = 18u8;
                let bg_b = 20u8;
                let br = ((cur_r as u16 * 2 + bg_r as u16 * 8) / 10) as u8;
                let bg = ((cur_g as u16 * 2 + bg_g as u16 * 8) / 10) as u8;
                let bb = ((cur_b as u16 * 2 + bg_b as u16 * 8) / 10) as u8;
                (br, bg, bb)
            };

            slice[idx] = Rgba8Pixel::new(r, g, b, 255);
            bgra_bytes[offset_bgra] = b;
            bgra_bytes[offset_bgra + 1] = g;
            bgra_bytes[offset_bgra + 2] = r;
            bgra_bytes[offset_bgra + 3] = 255;
        }
    }

    // Draw the big scaled digit in the center of the badge
    let scale = 9i32; // 8 cols * 9 = 72px width, 12 rows * 9 = 108px height
    let glyph_w = 8 * scale;
    let glyph_h = 12 * scale;
    let start_x = cx - glyph_w / 2;
    let start_y = cy - glyph_h / 2;

    for (row_idx, &row_bits) in glyph.iter().enumerate() {
        for col_idx in 0..8 {
            if (row_bits & (1 << (7 - col_idx))) != 0 {
                for sy in 0..scale {
                    let y = start_y + (row_idx as i32) * scale + sy;
                    if y < 0 || y >= height as i32 {
                        continue;
                    }
                    for sx in 0..scale {
                        let x = start_x + (col_idx as i32) * scale + sx;
                        if x < 0 || x >= width as i32 {
                            continue;
                        }

                        let idx = (y as usize) * (width as usize) + (x as usize);
                        let offset_bgra = idx * 4;

                        let r = 255u8;
                        let g = 255u8;
                        let b = 255u8;

                        slice[idx] = Rgba8Pixel::new(r, g, b, 255);
                        bgra_bytes[offset_bgra] = b;
                        bgra_bytes[offset_bgra + 1] = g;
                        bgra_bytes[offset_bgra + 2] = r;
                        bgra_bytes[offset_bgra + 3] = 255;
                    }
                }
            }
        }
    }
}
