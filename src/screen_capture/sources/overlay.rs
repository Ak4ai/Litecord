#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

static OVERLAY_ACTIVE: AtomicBool = AtomicBool::new(false);
static OVERLAY_HWND: Mutex<Option<isize>> = Mutex::new(None);

#[cfg(windows)]
pub fn start_window_border_overlay(target_hwnd: isize) {
    if target_hwnd == 0 {
        return;
    }
    stop_window_border_overlay();
    OVERLAY_ACTIVE.store(true, Ordering::SeqCst);

    std::thread::Builder::new()
        .name("window-border-overlay".to_string())
        .spawn(move || {
            use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
            use windows_sys::Win32::Graphics::Gdi::{
                BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, FrameRect,
                InvalidateRect, PAINTSTRUCT,
            };
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetWindowRect,
                IsIconic, IsWindow, PeekMessageW, PostQuitMessage, RegisterClassW,
                SetLayeredWindowAttributes, SetWindowPos, ShowWindow, TranslateMessage, CS_HREDRAW,
                CS_VREDRAW, HWND_TOPMOST, LWA_COLORKEY, MSG, PM_REMOVE, SWP_NOACTIVATE,
                SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE, WM_DESTROY, WM_ERASEBKGND, WM_PAINT,
                WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
                WS_EX_TRANSPARENT, WS_POPUP,
            };

            unsafe extern "system" fn overlay_wndproc(
                hwnd: HWND,
                msg: u32,
                wparam: WPARAM,
                lparam: LPARAM,
            ) -> LRESULT {
                match msg {
                    WM_PAINT => {
                        let mut ps: PAINTSTRUCT = std::mem::zeroed();
                        let hdc = BeginPaint(hwnd, &mut ps);
                        if !hdc.is_null() {
                            let mut rc: RECT = std::mem::zeroed();
                            GetWindowRect(hwnd, &mut rc);
                            let width = rc.right - rc.left;
                            let height = rc.bottom - rc.top;

                            // Fill entire area with black color key (transparent & click-through)
                            let black_brush = CreateSolidBrush(0x00000000);
                            let client_rc = RECT { left: 0, top: 0, right: width, bottom: height };
                            FillRect(hdc, &client_rc, black_brush);
                            DeleteObject(black_brush);

                            // Draw a vibrant 3px purple border (Discord-style Blurple / Purple #7C5CFC: 0x00FC5C7C)
                            let purple_brush = CreateSolidBrush(0x00FC5C7C); // COLORREF: 0x00BBGGRR -> R:124(0x7C), G:92(0x5C), B:252(0xFC)
                            for thickness in 0..3 {
                                let border_rc = RECT {
                                    left: thickness,
                                    top: thickness,
                                    right: width - thickness,
                                    bottom: height - thickness,
                                };
                                FrameRect(hdc, &border_rc, purple_brush);
                            }
                            DeleteObject(purple_brush);

                            EndPaint(hwnd, &mut ps);
                        }
                        0
                    }
                    WM_ERASEBKGND => 1,
                    WM_DESTROY => {
                        PostQuitMessage(0);
                        0
                    }
                    _ => DefWindowProcW(hwnd, msg, wparam, lparam),
                }
            }

            unsafe {
                let class_name: Vec<u16> = "LitecordCaptureBorder\0".encode_utf16().collect();
                let mut wc: WNDCLASSW = std::mem::zeroed();
                wc.style = CS_HREDRAW | CS_VREDRAW;
                wc.lpfnWndProc = Some(overlay_wndproc);
                wc.lpszClassName = class_name.as_ptr();
                RegisterClassW(&wc);

                let hwnd_target = target_hwnd as HWND;
                if IsWindow(hwnd_target) == 0 {
                    return;
                }

                #[link(name = "dwmapi")]
                extern "system" {
                    fn DwmGetWindowAttribute(
                        hwnd: HWND,
                        dwAttribute: u32,
                        pvAttribute: *mut std::ffi::c_void,
                        cbAttribute: u32,
                    ) -> i32;
                }

                let get_window_bounds = |h: HWND| -> RECT {
                    let mut rc: RECT = std::mem::zeroed();
                    // DWMWA_EXTENDED_FRAME_BOUNDS = 9: Obtém as dimensões visíveis exatas da janela, excluindo sombras invisíveis do DWM
                    let hr = DwmGetWindowAttribute(
                        h,
                        9,
                        &mut rc as *mut _ as *mut std::ffi::c_void,
                        std::mem::size_of::<RECT>() as u32,
                    );
                    if hr != 0 || (rc.right - rc.left) <= 0 || (rc.bottom - rc.top) <= 0 {
                        GetWindowRect(h, &mut rc);
                    }
                    rc
                };

                let target_rc = get_window_bounds(hwnd_target);
                let x = target_rc.left;
                let y = target_rc.top;
                let w = (target_rc.right - target_rc.left).max(1);
                let h = (target_rc.bottom - target_rc.top).max(1);

                let overlay_hwnd = CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                    class_name.as_ptr(),
                    std::ptr::null(),
                    WS_POPUP,
                    x,
                    y,
                    w,
                    h,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );

                if overlay_hwnd.is_null() {
                    return;
                }

                // Set colorkey transparency (0x00000000 black is transparent)
                SetLayeredWindowAttributes(overlay_hwnd, 0x00000000, 255, LWA_COLORKEY);

                // Exclude overlay from all screen captures (WDA_EXCLUDEFROMCAPTURE = 0x00000011)
                #[link(name = "user32")]
                extern "system" {
                    fn SetWindowDisplayAffinity(hwnd: HWND, dwAffinity: u32) -> windows_sys::Win32::Foundation::BOOL;
                }
                SetWindowDisplayAffinity(overlay_hwnd, 0x00000011);

                if let Ok(mut g) = OVERLAY_HWND.lock() {
                    *g = Some(overlay_hwnd as isize);
                }

                ShowWindow(overlay_hwnd, SW_SHOWNOACTIVATE);

                let mut last_rc: RECT = std::mem::zeroed();

                while OVERLAY_ACTIVE.load(Ordering::Relaxed) {
                    if IsWindow(hwnd_target) == 0 || IsIconic(hwnd_target) != 0 {
                        ShowWindow(overlay_hwnd, SW_HIDE);
                    } else {
                        let current_rc = get_window_bounds(hwnd_target);

                        if current_rc.left != last_rc.left
                            || current_rc.top != last_rc.top
                            || current_rc.right != last_rc.right
                            || current_rc.bottom != last_rc.bottom
                        {
                            last_rc = current_rc;
                            let nx = current_rc.left;
                            let ny = current_rc.top;
                            let nw = (current_rc.right - current_rc.left).max(1);
                            let nh = (current_rc.bottom - current_rc.top).max(1);

                            SetWindowPos(
                                overlay_hwnd,
                                HWND_TOPMOST,
                                nx,
                                ny,
                                nw,
                                nh,
                                SWP_NOACTIVATE | SWP_SHOWWINDOW,
                            );
                            InvalidateRect(overlay_hwnd, std::ptr::null(), 0);
                        } else {
                            ShowWindow(overlay_hwnd, SW_SHOWNOACTIVATE);
                        }
                    }

                    // Process pending overlay window messages
                    let mut msg: MSG = std::mem::zeroed();
                    while PeekMessageW(&mut msg, overlay_hwnd, 0, 0, PM_REMOVE) != 0 {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }

                    std::thread::sleep(Duration::from_millis(30));
                }

                ShowWindow(overlay_hwnd, SW_HIDE);
                DestroyWindow(overlay_hwnd);
                if let Ok(mut g) = OVERLAY_HWND.lock() {
                    *g = None;
                }
            }
        })
        .expect("Falha ao criar thread de overlay da borda de janela");
}

#[cfg(not(windows))]
pub fn start_window_border_overlay(_target_hwnd: isize) {}

pub fn stop_window_border_overlay() {
    OVERLAY_ACTIVE.store(false, Ordering::SeqCst);
}
