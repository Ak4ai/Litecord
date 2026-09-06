#![allow(dead_code)]

#[cfg(windows)]
use super::super::types::{CapturableWindowItem, MonitorItemInfo};

#[cfg(windows)]
pub fn list_screens() -> Vec<MonitorItemInfo> {
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
    };
    use windows_sys::Win32::Foundation::{BOOL, LPARAM, RECT, TRUE};

    struct MonitorEnumData {
        screens: Vec<MonitorItemInfo>,
        count: i32,
    }

    unsafe extern "system" fn monitor_proc(
        hmon: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let data = &mut *(lparam as *mut MonitorEnumData);
        data.count += 1;

        let mut info: MONITORINFOEXW = std::mem::zeroed();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;

        let is_primary = if GetMonitorInfoW(hmon, &mut info.monitorInfo as *mut _ as _) != 0 {
            (info.monitorInfo.dwFlags & 1) != 0 // MONITORINFOF_PRIMARY = 1
        } else {
            data.count == 1
        };

        let width = (info.monitorInfo.rcMonitor.right - info.monitorInfo.rcMonitor.left).abs();
        let height = (info.monitorInfo.rcMonitor.bottom - info.monitorInfo.rcMonitor.top).abs();
        let (w, h) = if width > 0 && height > 0 { (width, height) } else { (1920, 1080) };

        let name = if is_primary {
            format!("Tela {} (Principal)", data.count)
        } else {
            format!("Tela {}", data.count)
        };

        data.screens.push(MonitorItemInfo {
            id: data.count - 1,
            name,
            resolution: format!("{} × {}", w, h),
            is_primary,
            hwnd: 0,
        });

        TRUE
    }

    let mut data = MonitorEnumData { screens: Vec::new(), count: 0 };
    unsafe {
        EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(monitor_proc), &mut data as *mut _ as LPARAM);
    }

    if data.screens.is_empty() {
        data.screens.push(MonitorItemInfo {
            id: 0,
            name: "Tela 1 (Principal)".to_string(),
            resolution: "1920 × 1080".to_string(),
            is_primary: true,
            hwnd: 0,
        });
    }

    data.screens
}

#[cfg(windows)]
pub fn list_capturable_windows() -> Vec<CapturableWindowItem> {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, TRUE};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowLongPtrW, GetWindowTextW, IsWindowVisible, GWL_EXSTYLE, GWL_STYLE,
        WS_EX_TOOLWINDOW, WS_VISIBLE,
    };

    struct EnumData {
        windows: Vec<CapturableWindowItem>,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let data = &mut *(lparam as *mut EnumData);
        if IsWindowVisible(hwnd) == 0 {
            return TRUE;
        }

        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;

        if (style & WS_VISIBLE) == 0 || (ex_style & WS_EX_TOOLWINDOW) != 0 {
            return TRUE;
        }

        let mut title_buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), 512);
        if len > 0 {
            let title = String::from_utf16_lossy(&title_buf[..len as usize]).trim().to_string();
            let lower = title.to_lowercase();
            // Filter out internal and background overlay windows
            if !title.is_empty() 
                && !lower.starts_with("program manager") 
                && !lower.starts_with("settings") 
                && !lower.starts_with("configurações")
                && !lower.starts_with("windows input")
                && !lower.starts_with("msctfime ui")
                && !lower.starts_with("default ime")
                && !lower.starts_with("litecord - transmissão") 
            {
                use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
                use windows_sys::Win32::System::ProcessStatus::GetModuleFileNameExW;
                use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

                let mut pid: u32 = 0;
                GetWindowThreadProcessId(hwnd, &mut pid);
                let is_own_process = pid == std::process::id();

                let mut exe_name = String::new();
                if pid != 0 {
                    let hproc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                    if !hproc.is_null() {
                        let mut path_buf = [0u16; 1024];
                        let len = GetModuleFileNameExW(hproc, std::ptr::null_mut(), path_buf.as_mut_ptr(), 1024);
                        windows_sys::Win32::Foundation::CloseHandle(hproc);
                        if len > 0 {
                            let full_path = String::from_utf16_lossy(&path_buf[..len as usize]);
                            if let Some(filename) = full_path.split('\\').last() {
                                exe_name = filename.to_lowercase();
                            }
                        }
                    }
                }

                // Strictly detect Litecord first (own PID or litecord exe/title)
                let app_type = if is_own_process || exe_name.contains("litecord") || lower.contains("litecord") {
                    "Litecord".to_string()
                } else if exe_name.contains("chrome") || lower.contains("chrome") {
                    "Google Chrome".to_string()
                } else if exe_name.contains("firefox") || lower.contains("firefox") {
                    "Mozilla Firefox".to_string()
                } else if exe_name.contains("msedge") || lower.contains("edge") {
                    "Microsoft Edge".to_string()
                } else if exe_name.contains("code") || lower.contains("visual studio code") {
                    "Visual Studio Code".to_string()
                } else if exe_name.contains("discord") {
                    "Discord".to_string()
                } else if exe_name.contains("spotify") || lower.contains("spotify") {
                    "Spotify".to_string()
                } else if exe_name.contains("telegram") || lower.contains("telegram") {
                    "Telegram".to_string()
                } else if exe_name.contains("terminal") || exe_name.contains("powershell") || exe_name.contains("cmd") || lower.contains("terminal") {
                    "Terminal".to_string()
                } else if exe_name.contains("notepad") || lower.contains("bloco de notas") {
                    "Bloco de Notas".to_string()
                } else if !exe_name.is_empty() {
                    let clean = exe_name.trim_end_matches(".exe");
                    let mut chars = clean.chars();
                    match chars.next() {
                        None => "Janela".to_string(),
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    }
                } else {
                    "Janela".to_string()
                };

                let icon_rgba = extract_window_icon_rgba(hwnd);

                data.windows.push(CapturableWindowItem {
                    id: (hwnd as isize).to_string(),
                    title: title.clone(),
                    app_name: app_type,
                    icon_rgba,
                });
            }
        }
        TRUE
    }

    let mut data = EnumData { windows: Vec::new() };
    unsafe {
        EnumWindows(Some(enum_proc), &mut data as *mut _ as LPARAM);
    }
    data.windows
}

#[cfg(windows)]
unsafe fn extract_window_icon_rgba(hwnd: windows_sys::Win32::Foundation::HWND) -> Option<(u32, u32, Vec<u8>)> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, GetClassLongPtrW, DestroyIcon, GetIconInfo,
        WM_GETICON, ICON_BIG, ICON_SMALL2, ICON_SMALL, GCLP_HICON, GCLP_HICONSM,
        SMTO_ABORTIFHUNG, HICON, ICONINFO,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        GetDC, ReleaseDC, CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits,
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };

    let mut hicon: HICON = std::ptr::null_mut();
    let mut need_destroy = false;

    // 1. Try WM_GETICON (BIG, then SMALL2, then SMALL)
    let mut res_icon: usize = 0;
    for &icon_type in &[ICON_BIG, ICON_SMALL2, ICON_SMALL] {
        if SendMessageTimeoutW(
            hwnd,
            WM_GETICON,
            icon_type as usize,
            0,
            SMTO_ABORTIFHUNG,
            50,
            &mut res_icon,
        ) != 0 && res_icon != 0 {
            hicon = res_icon as HICON;
            break;
        }
    }

    // 2. If null, try class icons
    if hicon.is_null() {
        let cls_icon = GetClassLongPtrW(hwnd, GCLP_HICON);
        if cls_icon != 0 {
            hicon = cls_icon as HICON;
        } else {
            let cls_icon_sm = GetClassLongPtrW(hwnd, GCLP_HICONSM);
            if cls_icon_sm != 0 {
                hicon = cls_icon_sm as HICON;
            }
        }
    }

    // 3. If null, try ExtractIconW from process executable path
    if hicon.is_null() {
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        use windows_sys::Win32::System::ProcessStatus::GetModuleFileNameExW;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

        #[link(name = "shell32")]
        extern "system" {
            fn ExtractIconW(
                hInst: isize,
                pszExeFileName: *const u16,
                nIconIndex: u32,
            ) -> HICON;
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != 0 {
            let hproc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if !hproc.is_null() {
                let mut path_buf = [0u16; 1024];
                let len = GetModuleFileNameExW(hproc, std::ptr::null_mut(), path_buf.as_mut_ptr(), 1024);
                windows_sys::Win32::Foundation::CloseHandle(hproc);
                if len > 0 {
                    let extracted = ExtractIconW(0, path_buf.as_ptr(), 0);
                    if !extracted.is_null() && extracted as usize > 1 {
                        hicon = extracted;
                        need_destroy = true;
                    }
                }
            }
        }
    }

    if hicon.is_null() {
        return None;
    }

    let mut icon_info: ICONINFO = std::mem::zeroed();
    if GetIconInfo(hicon, &mut icon_info) == 0 {
        if need_destroy { DestroyIcon(hicon); }
        return None;
    }

    let hbm_color = icon_info.hbmColor;
    let hbm_mask = icon_info.hbmMask;

    let hdc_screen = GetDC(std::ptr::null_mut());
    let hdc_mem = CreateCompatibleDC(hdc_screen);

    let (icon_w, icon_h) = (32u32, 32u32);
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = icon_w as i32;
    bmi.bmiHeader.biHeight = -(icon_h as i32); // Top-down
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB as u32;

    let mut bgra_bytes = vec![0u8; (icon_w * icon_h * 4) as usize];

    if !hbm_color.is_null() {
        GetDIBits(
            hdc_mem,
            hbm_color,
            0,
            icon_h,
            bgra_bytes.as_mut_ptr() as *mut _,
            &mut bmi,
            DIB_RGB_COLORS,
        );
    } else if !hbm_mask.is_null() {
        GetDIBits(
            hdc_mem,
            hbm_mask,
            0,
            icon_h,
            bgra_bytes.as_mut_ptr() as *mut _,
            &mut bmi,
            DIB_RGB_COLORS,
        );
    }

    // Clean up GDI icon resources
    DeleteDC(hdc_mem);
    ReleaseDC(std::ptr::null_mut(), hdc_screen);
    if !hbm_color.is_null() { DeleteObject(hbm_color); }
    if !hbm_mask.is_null() { DeleteObject(hbm_mask); }
    if need_destroy { DestroyIcon(hicon); }

    // Convert BGRA to RGBA and check if icon contains actual visual pixels
    let mut has_visible_pixels = false;
    for chunk in bgra_bytes.chunks_exact_mut(4) {
        chunk.swap(0, 2); // B <-> R
        if chunk[3] == 0 {
            // Fix alpha channel for 24-bit/non-alpha icons
            if chunk[0] > 0 || chunk[1] > 0 || chunk[2] > 0 {
                chunk[3] = 255;
                has_visible_pixels = true;
            }
        } else {
            has_visible_pixels = true;
        }
    }

    if has_visible_pixels {
        Some((icon_w, icon_h, bgra_bytes))
    } else {
        None
    }
}

#[cfg(windows)]
pub fn capture_screen_rgb(target_hwnd: isize, target_w: u32, target_h: u32, _target_fps: u64, out_bgra: &mut Vec<u8>) -> Option<(u128, u128)> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, CreateSolidBrush, DeleteDC, DeleteObject,
        FillRect, GetDC, ReleaseDC, SelectObject, SetBrushOrgEx, SetStretchBltMode, StretchBlt,
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, COLORONCOLOR, DIB_RGB_COLORS, SRCCOPY,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, GetWindowRect, IsIconic, IsWindow,
        SM_CXSCREEN, SM_CYSCREEN,
    };

    #[link(name = "user32")]
    extern "system" {
        fn PrintWindow(
            hwnd: windows_sys::Win32::Foundation::HWND,
            hdcBlt: windows_sys::Win32::Graphics::Gdi::HDC,
            nFlags: u32,
        ) -> windows_sys::Win32::Foundation::BOOL;
    }

    struct GdiCaptureContext {
        hdc_desktop: windows_sys::Win32::Graphics::Gdi::HDC,
        hdc_mem: windows_sys::Win32::Graphics::Gdi::HDC,
        hbm_dib: windows_sys::Win32::Graphics::Gdi::HBITMAP,
        p_bits: *mut u8,
        hdc_win_mem: windows_sys::Win32::Graphics::Gdi::HDC,
        hbm_win: windows_sys::Win32::Graphics::Gdi::HBITMAP,
        win_w: i32,
        win_h: i32,
        dark_brush: windows_sys::Win32::Graphics::Gdi::HBRUSH,
        target_w: u32,
        target_h: u32,
    }

    thread_local! {
        static GDI_CAPTURE_CACHE: std::cell::RefCell<Option<GdiCaptureContext>> = std::cell::RefCell::new(None);
    }

    unsafe {
        GDI_CAPTURE_CACHE.with(|cell| {
            let mut cache_opt = cell.borrow_mut();
            if let Some(ref cache) = *cache_opt {
                if cache.target_w != target_w || cache.target_h != target_h {
                    // Clean up old handles if dimensions changed
                    DeleteObject(cache.dark_brush);
                    if !cache.hbm_win.is_null() {
                        DeleteObject(cache.hbm_win);
                    }
                    if !cache.hdc_win_mem.is_null() {
                        DeleteDC(cache.hdc_win_mem);
                    }
                    DeleteObject(cache.hbm_dib);
                    DeleteDC(cache.hdc_mem);
                    ReleaseDC(std::ptr::null_mut(), cache.hdc_desktop);
                    *cache_opt = None;
                }
            }

            if cache_opt.is_none() {
                let hdc_desktop = GetDC(std::ptr::null_mut());
                if hdc_desktop.is_null() {
                    return None;
                }

                let hdc_mem = CreateCompatibleDC(hdc_desktop);
                if hdc_mem.is_null() {
                    ReleaseDC(std::ptr::null_mut(), hdc_desktop);
                    return None;
                }

                let mut bmi: BITMAPINFO = std::mem::zeroed();
                bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                bmi.bmiHeader.biWidth = target_w as i32;
                bmi.bmiHeader.biHeight = -(target_h as i32); // Top-down
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
                if hbm_dib.is_null() || p_bits.is_null() {
                    DeleteDC(hdc_mem);
                    ReleaseDC(std::ptr::null_mut(), hdc_desktop);
                    return None;
                }

                SelectObject(hdc_mem, hbm_dib);
                let hdc_win_mem = CreateCompatibleDC(hdc_desktop);
                let dark_brush = CreateSolidBrush(0x00141211); // COLORREF: 0x00BBGGRR

                *cache_opt = Some(GdiCaptureContext {
                    hdc_desktop,
                    hdc_mem,
                    hbm_dib,
                    p_bits: p_bits as *mut u8,
                    hdc_win_mem,
                    hbm_win: std::ptr::null_mut(),
                    win_w: 0,
                    win_h: 0,
                    dark_brush,
                    target_w,
                    target_h,
                });
            }

            let cache = cache_opt.as_mut().unwrap();
            let hdc_desktop = cache.hdc_desktop;
            let hdc_mem = cache.hdc_mem;
            let hdc_win_mem = cache.hdc_win_mem;
            let dark_brush = cache.dark_brush;
            let p_bits = cache.p_bits;

            let t_blt_start = std::time::Instant::now();
            if target_hwnd != 0 {
                let hwnd = target_hwnd as windows_sys::Win32::Foundation::HWND;
                if IsWindow(hwnd) == 0 {
                    return None;
                }

                let mut rc: RECT = std::mem::zeroed();
                GetWindowRect(hwnd, &mut rc);
                let win_w = (rc.right - rc.left).max(1);
                let win_h = (rc.bottom - rc.top).max(1);

                if cache.win_w != win_w || cache.win_h != win_h || cache.hbm_win.is_null() {
                    if !cache.hbm_win.is_null() {
                        DeleteObject(cache.hbm_win);
                    }
                    cache.hbm_win = CreateCompatibleBitmap(hdc_desktop, win_w, win_h);
                    cache.win_w = win_w;
                    cache.win_h = win_h;
                    SelectObject(hdc_win_mem, cache.hbm_win);
                }

                let capture_backend = crate::video_settings::get_video_capture_backend();
                let mut captured_ok = false;
                if capture_backend != "bitblt" && IsIconic(hwnd) == 0 {
                    // 1. Tenta PrintWindow com PW_RENDERFULLCONTENT (2) para janelas DirectX/DWM
                    if PrintWindow(hwnd, hdc_win_mem, 2) != 0 {
                        captured_ok = true;
                    }
                }

                // 2. Fallback para crop direto do desktop (garante fidelidade visual e zero rabiscos)
                if (!captured_ok || capture_backend == "bitblt") && IsIconic(hwnd) == 0 {
                    let src_x = rc.left.max(0);
                    let src_y = rc.top.max(0);
                    BitBlt(hdc_win_mem, 0, 0, win_w, win_h, hdc_desktop, src_x, src_y, SRCCOPY);
                }

                // Fill background with dark letterbox/pillarbox color (#111214)
                let target_rc = RECT { left: 0, top: 0, right: target_w as i32, bottom: target_h as i32 };
                FillRect(hdc_mem, &target_rc, dark_brush);

                // Compute aspect-ratio preserving dimensions
                let scale_w = target_w as f32 / win_w as f32;
                let scale_h = target_h as f32 / win_h as f32;
                let scale = scale_w.min(scale_h);

                let dest_w = ((win_w as f32 * scale).round() as i32).max(1);
                let dest_h = ((win_h as f32 * scale).round() as i32).max(1);
                let dest_x = ((target_w as i32 - dest_w) / 2).max(0);
                let dest_y = ((target_h as i32 - dest_h) / 2).max(0);

                SetStretchBltMode(hdc_mem, COLORONCOLOR);
                SetBrushOrgEx(hdc_mem, 0, 0, std::ptr::null_mut());
                StretchBlt(
                    hdc_mem,
                    dest_x,
                    dest_y,
                    dest_w,
                    dest_h,
                    hdc_win_mem,
                    0,
                    0,
                    win_w,
                    win_h,
                    SRCCOPY,
                );
            } else {
                // Full screen capture
                let screen_w = GetSystemMetrics(SM_CXSCREEN);
                let screen_h = GetSystemMetrics(SM_CYSCREEN);

                if screen_w == target_w as i32 && screen_h == target_h as i32 {
                    BitBlt(
                        hdc_mem,
                        0,
                        0,
                        target_w as i32,
                        target_h as i32,
                        hdc_desktop,
                        0,
                        0,
                        SRCCOPY,
                    );
                } else {
                    SetStretchBltMode(hdc_mem, COLORONCOLOR);
                    SetBrushOrgEx(hdc_mem, 0, 0, std::ptr::null_mut());
                    StretchBlt(
                        hdc_mem,
                        0,
                        0,
                        target_w as i32,
                        target_h as i32,
                        hdc_desktop,
                        0,
                        0,
                        screen_w,
                        screen_h,
                        SRCCOPY,
                    );
                }
            }
            let blt_dur_us = t_blt_start.elapsed().as_micros();

            let t_pix_start = std::time::Instant::now();
            let total_bytes = (target_w * target_h * 4) as usize;
            if out_bgra.len() != total_bytes {
                out_bgra.resize(total_bytes, 0);
            }
            std::ptr::copy_nonoverlapping(p_bits, out_bgra.as_mut_ptr(), total_bytes);
            let pix_dur_us = t_pix_start.elapsed().as_micros();

            Some((blt_dur_us, pix_dur_us))
        })
    }
}
