use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use crate::AppWindow;
use slint::ComponentHandle;

static MONITOR_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Default, Clone, Copy)]
pub struct HardwareMetrics {
    pub cpu_percent: u8,
    pub ram_percent: u8,
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

                // Format tooltip and window title
                let status_line = format!(
                    "Litecord - CPU {}% | RAM {}% | GPU {}%",
                    metrics.cpu_percent, metrics.ram_percent, metrics.gpu_percent
                );

                // 1. Update System Tray Tooltip
                crate::utils::tray::update_tray_tooltip(&status_line);

                // 2. Update Windows Taskbar Application Title
                #[cfg(windows)]
                {
                    if let Ok(guard) = hwnd_store.lock() {
                        if let Some(hwnd) = *guard {
                            use windows_sys::Win32::UI::WindowsAndMessaging::SetWindowTextW;
                            let wide_title: Vec<u16> = status_line.encode_utf16().chain(std::iter::once(0)).collect();
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
                        let ram_str = format!("RAM {}%", metrics.ram_percent);
                        let gpu_str = format!("GPU {}%", metrics.gpu_percent);

                        app.set_hardware_cpu_text(cpu_str.into());
                        app.set_hardware_ram_text(ram_str.into());
                        app.set_hardware_gpu_text(gpu_str.into());
                        app.set_hardware_cpu_val(metrics.cpu_percent as i32);
                        app.set_hardware_ram_val(metrics.ram_percent as i32);
                        app.set_hardware_gpu_val(metrics.gpu_percent as i32);
                    }
                });
            }
        })
        .expect("Failed to spawn hardware-monitor thread");
}


#[cfg(windows)]
struct PlatformHardwareSampler {
    last_idle_time: u64,
    last_kernel_time: u64,
    last_user_time: u64,
    has_prev_cpu: bool,
    pdh_query: isize,
    pdh_counter: isize,
    pdh_initialized: bool,
}

#[cfg(windows)]
impl PlatformHardwareSampler {
    fn new() -> Self {
        let mut s = Self {
            last_idle_time: 0,
            last_kernel_time: 0,
            last_user_time: 0,
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
        use windows_sys::Win32::System::Threading::GetSystemTimes;

        let mut idle_time = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let mut kernel_time = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let mut user_time = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };

        let ok = unsafe {
            GetSystemTimes(&mut idle_time, &mut kernel_time, &mut user_time)
        };

        if ok == 0 {
            return 0;
        }

        let idle = ((idle_time.dwHighDateTime as u64) << 32) | (idle_time.dwLowDateTime as u64);
        let kernel = ((kernel_time.dwHighDateTime as u64) << 32) | (kernel_time.dwLowDateTime as u64);
        let user = ((user_time.dwHighDateTime as u64) << 32) | (user_time.dwLowDateTime as u64);

        if !self.has_prev_cpu {
            self.last_idle_time = idle;
            self.last_kernel_time = kernel;
            self.last_user_time = user;
            self.has_prev_cpu = true;
            return 0;
        }

        let idle_delta = idle.saturating_sub(self.last_idle_time);
        let kernel_delta = kernel.saturating_sub(self.last_kernel_time);
        let user_delta = user.saturating_sub(self.last_user_time);

        self.last_idle_time = idle;
        self.last_kernel_time = kernel;
        self.last_user_time = user;

        let total_system = kernel_delta + user_delta;
        if total_system == 0 {
            return 0;
        }

        let total_busy = total_system.saturating_sub(idle_delta);
        let pct = (total_busy * 100) / total_system;
        pct.min(100) as u8
    }

    fn sample_ram(&mut self) -> u8 {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

        let mut mem = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            dwMemoryLoad: 0,
            ullTotalPhys: 0,
            ullAvailPhys: 0,
            ullTotalPageFile: 0,
            ullAvailPageFile: 0,
            ullTotalVirtual: 0,
            ullAvailVirtual: 0,
            ullAvailExtendedVirtual: 0,
        };

        let ok = unsafe {
            GlobalMemoryStatusEx(&mut mem)
        };

        if ok != 0 {
            mem.dwMemoryLoad.min(100) as u8
        } else {
            0
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
                                total_gpu += item.fmt_value.double_val;
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
            ram_percent: self.sample_ram(),
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

        if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
            let mut total_kb: u64 = 0;
            let mut avail_kb: u64 = 0;
            for line in content.lines() {
                if line.starts_with("MemTotal:") {
                    total_kb = line.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
                } else if line.starts_with("MemAvailable:") {
                    avail_kb = line.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
                }
            }
            if total_kb > 0 {
                let used_kb = total_kb.saturating_sub(avail_kb);
                metrics.ram_percent = ((used_kb * 100) / total_kb).min(100) as u8;
            }
        }

        metrics
    }
}
