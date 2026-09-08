use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use crate::AppWindow;
use slint::ComponentHandle;

static MONITOR_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Default, Clone, Copy)]
pub struct HardwareMetrics {
    pub cpu_percent: u8,
    pub ram_mb: u32,
    pub gpu_percent: u8,
}

pub fn start_hardware_monitor_loop(
    app_weak: slint::Weak<AppWindow>,
    hwnd_store: std::sync::Arc<std::sync::Mutex<Option<isize>>>,
) {
    if MONITOR_RUNNING.swap(true, Ordering::SeqCst) {
        return; // Already running
    }

    std::thread::Builder::new()
        .name("hardware-monitor".into())
        .spawn(move || {
            let mut sampler = PlatformHardwareSampler::new();

            loop {
                std::thread::sleep(Duration::from_millis(1000));

                let metrics = sampler.sample();

                // Format tray tooltip (full detail) and taskbar window title (compact to fit)
                let tooltip_text = format!(
                    "Litecord - CPU {}% | RAM {} MB | GPU {}%",
                    metrics.cpu_percent, metrics.ram_mb, metrics.gpu_percent
                );

                let taskbar_title = format!(
                    "{}% / {}MB / {}%",
                    metrics.cpu_percent, metrics.ram_mb, metrics.gpu_percent
                );

                // 1. Update System Tray Tooltip
                crate::utils::tray::update_tray_tooltip(&tooltip_text);

                // 2. Update Windows Taskbar Application Title
                #[cfg(windows)]
                {
                    if let Ok(guard) = hwnd_store.lock() {
                        if let Some(hwnd) = *guard {
                            use windows_sys::Win32::UI::WindowsAndMessaging::SetWindowTextW;
                            let wide_title: Vec<u16> = taskbar_title.encode_utf16().chain(std::iter::once(0)).collect();
                            unsafe {
                                SetWindowTextW(hwnd as _, wide_title.as_ptr());
                            }
                        }
                    }
                }

                // 3. Update Slint UI properties
                let app_opt = app_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = app_opt.upgrade() {
                        let cpu_str = format!("CPU {}%", metrics.cpu_percent);
                        let ram_str = format!("RAM {} MB", metrics.ram_mb);
                        let gpu_str = format!("GPU {}%", metrics.gpu_percent);

                        app.set_hardware_cpu_text(cpu_str.into());
                        app.set_hardware_ram_text(ram_str.into());
                        app.set_hardware_gpu_text(gpu_str.into());
                        app.set_hardware_cpu_val(metrics.cpu_percent as i32);
                        app.set_hardware_ram_val(metrics.ram_mb as i32);
                        app.set_hardware_gpu_val(metrics.gpu_percent as i32);
                    }
                });
            }
        })
        .expect("Failed to spawn hardware-monitor thread");
}


#[cfg(windows)]
struct PlatformHardwareSampler {
    pid_needle: Vec<u16>,
    last_proc_total: u64,
    last_sys_total: u64,
    has_prev_cpu: bool,
    pdh_query: isize,
    pdh_counter: isize,
    pdh_initialized: bool,
}

#[cfg(windows)]
unsafe fn wide_contains(p: *const u16, needle: &[u16]) -> bool {
    if p.is_null() || needle.is_empty() {
        return false;
    }
    let mut len = 0;
    while *p.add(len) != 0 {
        len += 1;
        if len > 512 {
            break;
        }
    }
    let hay = std::slice::from_raw_parts(p, len);
    hay.windows(needle.len()).any(|w| w == needle)
}

#[cfg(windows)]
impl PlatformHardwareSampler {
    fn new() -> Self {
        let pid = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() };
        let pid_needle: Vec<u16> = format!("pid_{}_", pid).encode_utf16().collect();
        let mut s = Self {
            pid_needle,
            last_proc_total: 0,
            last_sys_total: 0,
            has_prev_cpu: false,
            pdh_query: 0,
            pdh_counter: 0,
            pdh_initialized: false,
        };
        s.init_pdh();
        s
    }

    fn init_pdh(&mut self) {
        unsafe {
            use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
            let pdh = LoadLibraryA(b"pdh.dll\0".as_ptr());
            if pdh.is_null() {
                return;
            }

            type PdhOpenQueryAFn = unsafe extern "system" fn(*const u8, usize, *mut isize) -> u32;
            type PdhAddEnglishCounterWFn = unsafe extern "system" fn(isize, *const u16, usize, *mut isize) -> u32;
            type PdhCollectQueryDataFn = unsafe extern "system" fn(isize) -> u32;

            let open_query_fn: Option<PdhOpenQueryAFn> = std::mem::transmute(GetProcAddress(pdh, b"PdhOpenQueryA\0".as_ptr()));
            let add_counter_fn: Option<PdhAddEnglishCounterWFn> = std::mem::transmute(GetProcAddress(pdh, b"PdhAddEnglishCounterW\0".as_ptr()));
            let collect_fn: Option<PdhCollectQueryDataFn> = std::mem::transmute(GetProcAddress(pdh, b"PdhCollectQueryData\0".as_ptr()));

            if let (Some(open_query), Some(add_counter), Some(collect)) = (open_query_fn, add_counter_fn, collect_fn) {
                let mut query: isize = 0;
                if open_query(std::ptr::null(), 0, &mut query) == 0 {
                    let path: Vec<u16> = "\\GPU Engine(*)\\Utilization Percentage\0".encode_utf16().collect();
                    let mut counter: isize = 0;
                    if add_counter(query, path.as_ptr(), 0, &mut counter) == 0 {
                        let _ = collect(query);
                        self.pdh_query = query;
                        self.pdh_counter = counter;
                        self.pdh_initialized = true;
                    }
                }
            }
        }
    }

    fn sample_cpu(&mut self) -> u8 {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes, GetSystemTimes};

        let mut sys_idle = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let mut sys_kernel = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let mut sys_user = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };

        let mut proc_creation = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let mut proc_exit = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let mut proc_kernel = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let mut proc_user = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };

        let sys_ok = unsafe { GetSystemTimes(&mut sys_idle, &mut sys_kernel, &mut sys_user) };
        let proc_ok = unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut proc_creation,
                &mut proc_exit,
                &mut proc_kernel,
                &mut proc_user,
            )
        };

        if sys_ok == 0 || proc_ok == 0 {
            return 0;
        }

        let sys_k = ((sys_kernel.dwHighDateTime as u64) << 32) | (sys_kernel.dwLowDateTime as u64);
        let sys_u = ((sys_user.dwHighDateTime as u64) << 32) | (sys_user.dwLowDateTime as u64);
        let sys_total = sys_k + sys_u;

        let proc_k = ((proc_kernel.dwHighDateTime as u64) << 32) | (proc_kernel.dwLowDateTime as u64);
        let proc_u = ((proc_user.dwHighDateTime as u64) << 32) | (proc_user.dwLowDateTime as u64);
        let proc_total = proc_k + proc_u;

        if !self.has_prev_cpu {
            self.last_proc_total = proc_total;
            self.last_sys_total = sys_total;
            self.has_prev_cpu = true;
            return 0;
        }

        let proc_delta = proc_total.saturating_sub(self.last_proc_total);
        let sys_delta = sys_total.saturating_sub(self.last_sys_total);

        self.last_proc_total = proc_total;
        self.last_sys_total = sys_total;

        if sys_delta == 0 {
            return 0;
        }

        let pct = ((proc_delta as f64 * 100.0) / (sys_delta as f64)).round() as u8;
        pct.min(100)
    }

    fn sample_ram(&mut self) -> u32 {
        use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        unsafe {
            let mut pmc: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
            pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            let ok = K32GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut pmc,
                std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            );

            if ok != 0 {
                (pmc.WorkingSetSize / (1024 * 1024)) as u32
            } else {
                0
            }
        }
    }

    fn sample_gpu(&mut self) -> u8 {
        if !self.pdh_initialized {
            return 0;
        }

        #[repr(C)]
        struct PdhFmtCountervalue {
            status: u32,
            reserved: u32,
            double_val: f64,
        }

        #[repr(C)]
        struct PdhFmtCountervalueItemW {
            sz_name: *mut u16,
            fmt_value: PdhFmtCountervalue,
        }

        unsafe {
            use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
            let pdh = LoadLibraryA(b"pdh.dll\0".as_ptr());
            if pdh.is_null() {
                return 0;
            }

            type PdhCollectQueryDataFn = unsafe extern "system" fn(isize) -> u32;
            type PdhGetFormattedCounterArrayWFn = unsafe extern "system" fn(
                isize,
                u32,
                *mut u32,
                *mut u32,
                *mut PdhFmtCountervalueItemW,
            ) -> u32;

            let collect_fn: Option<PdhCollectQueryDataFn> = std::mem::transmute(GetProcAddress(pdh, b"PdhCollectQueryData\0".as_ptr()));
            let get_array_fn: Option<PdhGetFormattedCounterArrayWFn> = std::mem::transmute(GetProcAddress(pdh, b"PdhGetFormattedCounterArrayW\0".as_ptr()));

            if let (Some(collect), Some(get_array)) = (collect_fn, get_array_fn) {
                if collect(self.pdh_query) != 0 {
                    return 0;
                }

                const PDH_FMT_DOUBLE: u32 = 0x00000200;
                const PDH_MORE_DATA: u32 = 0x800007D2;

                let mut buffer_size: u32 = 0;
                let mut item_count: u32 = 0;

                let status = get_array(
                    self.pdh_counter,
                    PDH_FMT_DOUBLE,
                    &mut buffer_size,
                    &mut item_count,
                    std::ptr::null_mut(),
                );

                if (status == 0 || status == PDH_MORE_DATA) && buffer_size > 0 {
                    let mut buffer: Vec<u8> = vec![0u8; buffer_size as usize];
                    let p_items = buffer.as_mut_ptr() as *mut PdhFmtCountervalueItemW;

                    let status2 = get_array(
                        self.pdh_counter,
                        PDH_FMT_DOUBLE,
                        &mut buffer_size,
                        &mut item_count,
                        p_items,
                    );

                    if status2 == 0 && item_count > 0 {
                        let items_slice = std::slice::from_raw_parts(p_items, item_count as usize);
                        let mut total_gpu = 0.0f64;
                        for item in items_slice {
                            if item.fmt_value.status == 0 && item.fmt_value.double_val > 0.0 {
                                if !item.sz_name.is_null() && wide_contains(item.sz_name, &self.pid_needle) {
                                    total_gpu += item.fmt_value.double_val;
                                }
                            }
                        }
                        return (total_gpu.round() as u64).min(100) as u8;
                    }
                }
            }
        }

        0
    }

    fn sample(&mut self) -> HardwareMetrics {
        HardwareMetrics {
            cpu_percent: self.sample_cpu(),
            ram_mb: self.sample_ram(),
            gpu_percent: self.sample_gpu(),
        }
    }
}

#[cfg(not(windows))]
struct PlatformHardwareSampler;

#[cfg(not(windows))]
impl PlatformHardwareSampler {
    fn new() -> Self {
        Self
    }

    fn sample(&mut self) -> HardwareMetrics {
        let mut metrics = HardwareMetrics::default();
        if let Ok(content) = std::fs::read_to_string("/proc/self/status") {
            for line in content.lines() {
                if line.starts_with("VmRSS:") {
                    let kb: u32 = line.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
                    metrics.ram_mb = kb / 1024;
                    break;
                }
            }
        }
        metrics
    }
}
