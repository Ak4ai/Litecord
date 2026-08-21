use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use log::{info, warn};
use slint::{Rgba8Pixel, SharedPixelBuffer};

pub struct ScreenCaptureManager {
    is_running: Arc<AtomicBool>,
}

impl ScreenCaptureManager {
    pub fn new() -> Self {
        Self {
            is_running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn is_active(&self) -> bool {
        self.is_running.load(Ordering::Relaxed)
    }

    pub fn stop(&self) {
        if self.is_running.swap(false, Ordering::SeqCst) {
            info!("🛑 Parando captura e transmissão de tela...");
        }
    }

    pub fn start<F>(&self, on_frame: F)
    where
        F: Fn(SharedPixelBuffer<Rgba8Pixel>) + Send + 'static,
    {
        if self.is_running.swap(true, Ordering::SeqCst) {
            warn!("Transmissão de tela já está em execução.");
            return;
        }

        let is_running = Arc::clone(&self.is_running);
        info!("🖥️ Iniciando captura de tela acelerada por hardware (30 FPS)...");

        std::thread::Builder::new()
            .name("screen-capture-thread".to_string())
            .spawn(move || {
                let target_fps = 30;
                let frame_interval = Duration::from_millis(1000 / target_fps);

                while is_running.load(Ordering::Relaxed) {
                    let start_time = Instant::now();

                    if let Some(frame) = capture_screen_frame() {
                        on_frame(frame);
                    }

                    let elapsed = start_time.elapsed();
                    if elapsed < frame_interval {
                        std::thread::sleep(frame_interval - elapsed);
                    }
                }
                info!("🖥️ Thread de captura de tela finalizada.");
            })
            .expect("Falha ao iniciar thread de captura de tela");
    }
}

#[cfg(windows)]
fn capture_screen_frame() -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
        ReleaseDC, SelectObject, StretchBlt, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        SRCCOPY,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetDesktopWindow, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
    };

    unsafe {
        let hwnd = GetDesktopWindow();
        let hdc_screen = GetDC(hwnd);
        if hdc_screen.is_null() {
            return None;
        }

        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);

        if screen_w <= 0 || screen_h <= 0 {
            ReleaseDC(hwnd, hdc_screen);
            return None;
        }

        // Scale to 720p 16:9 for optimal balance between sharp quality and sub-1% CPU load
        let target_w = 1280u32;
        let target_h = 720u32;

        let hdc_mem = CreateCompatibleDC(hdc_screen);
        if hdc_mem.is_null() {
            ReleaseDC(hwnd, hdc_screen);
            return None;
        }

        let hbm_screen = CreateCompatibleBitmap(hdc_screen, target_w as i32, target_h as i32);
        if hbm_screen.is_null() {
            DeleteDC(hdc_mem);
            ReleaseDC(hwnd, hdc_screen);
            return None;
        }

        let old_obj = SelectObject(hdc_mem, hbm_screen);

        // Hardware StretchBlt capture directly into memory DC
        StretchBlt(
            hdc_mem,
            0,
            0,
            target_w as i32,
            target_h as i32,
            hdc_screen,
            0,
            0,
            screen_w,
            screen_h,
            SRCCOPY,
        );

        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = target_w as i32;
        bmi.bmiHeader.biHeight = -(target_h as i32); // Top-down DIB
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;

        let total_pixels = (target_w * target_h) as usize;
        let mut raw_bytes = vec![0u8; total_pixels * 4];

        let result = GetDIBits(
            hdc_mem,
            hbm_screen,
            0,
            target_h,
            raw_bytes.as_mut_ptr() as *mut _,
            &mut bmi,
            DIB_RGB_COLORS,
        );

        // Cleanup Windows GDI handles
        SelectObject(hdc_mem, old_obj);
        DeleteObject(hbm_screen);
        DeleteDC(hdc_mem);
        ReleaseDC(hwnd, hdc_screen);

        if result == 0 {
            return None;
        }

        // Convert BGRA (Windows DIB standard) to RGBA in-place
        let mut pixel_buffer = SharedPixelBuffer::<Rgba8Pixel>::new(target_w, target_h);
        let slice = pixel_buffer.make_mut_slice();

        for (i, pixel) in slice.iter_mut().enumerate() {
            let offset = i * 4;
            let b = raw_bytes[offset];
            let g = raw_bytes[offset + 1];
            let r = raw_bytes[offset + 2];
            let a = 255u8;
            *pixel = Rgba8Pixel::new(r, g, b, a);
        }

        Some(pixel_buffer)
    }
}

#[cfg(not(windows))]
fn capture_screen_frame() -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    // Cross-platform Fallback animated test frame for Linux
    let target_w = 1280u32;
    let target_h = 720u32;
    let mut pixel_buffer = SharedPixelBuffer::<Rgba8Pixel>::new(target_w, target_h);
    let slice = pixel_buffer.make_mut_slice();

    static mut FRAME_COUNTER: u8 = 0;
    let counter = unsafe {
        FRAME_COUNTER = FRAME_COUNTER.wrapping_add(2);
        FRAME_COUNTER
    };

    for (i, pixel) in slice.iter_mut().enumerate() {
        let x = (i % target_w as usize) as u8;
        let y = (i / target_w as usize) as u8;
        *pixel = Rgba8Pixel::new(
            x.wrapping_add(counter),
            y.wrapping_add(counter / 2),
            180,
            255,
        );
    }

    Some(pixel_buffer)
}
