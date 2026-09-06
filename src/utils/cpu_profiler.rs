#[cfg(target_os = "windows")]
pub fn set_current_thread_name(name: &str) {
    unsafe {
        use windows_sys::Win32::System::Threading::*;
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        #[link(name = "kernel32")]
        extern "system" {
            fn SetThreadDescription(hThread: windows_sys::Win32::Foundation::HANDLE, lpThreadDescription: *const u16) -> i32;
        }
        let h_curr = GetCurrentThread();
        let _ = SetThreadDescription(h_curr, wide.as_ptr());
    }
}

#[cfg(not(target_os = "windows"))]
pub fn set_current_thread_name(_name: &str) {}

#[cfg(target_os = "windows")]
pub fn start_thread_cpu_profiler() {
    std::thread::Builder::new()
        .name("cpu-profiler".to_string())
        .spawn(move || {
            use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
            use windows_sys::Win32::System::Threading::*;
            use windows_sys::Win32::Foundation::*;
            use std::collections::HashMap;
            use std::time::{Instant, Duration};

            let pid = unsafe { GetCurrentProcessId() };
            let num_cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8) as f64;

            let mut prev_times: HashMap<u32, (u64, Instant)> = HashMap::new();

            loop {
                std::thread::sleep(Duration::from_millis(2000));

                let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
                if snapshot == INVALID_HANDLE_VALUE {
                    continue;
                }

                let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
                entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;

                let mut current_threads = Vec::new();
                let now = Instant::now();

                if unsafe { Thread32First(snapshot, &mut entry) } != 0 {
                    loop {
                        if entry.th32OwnerProcessID == pid {
                            let tid = entry.th32ThreadID;
                            let h_thread = unsafe {
                                OpenThread(THREAD_QUERY_INFORMATION | THREAD_QUERY_LIMITED_INFORMATION, 0, tid)
                            };
                            if !h_thread.is_null() && h_thread != INVALID_HANDLE_VALUE {
                                let mut create: FILETIME = unsafe { std::mem::zeroed() };
                                let mut exit: FILETIME = unsafe { std::mem::zeroed() };
                                let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
                                let mut user: FILETIME = unsafe { std::mem::zeroed() };

                                if unsafe { GetThreadTimes(h_thread, &mut create, &mut exit, &mut kernel, &mut user) } != 0 {
                                    let k = ((kernel.dwHighDateTime as u64) << 32) | (kernel.dwLowDateTime as u64);
                                    let u = ((user.dwHighDateTime as u64) << 32) | (user.dwLowDateTime as u64);
                                    let total_100ns = k + u;

                                    let mut desc_str = format!("Thread-{}", tid);
                                    let mut p_desc: *mut u16 = std::ptr::null_mut();
                                    #[link(name = "kernel32")]
                                    extern "system" {
                                        fn GetThreadDescription(hThread: HANDLE, ppszThreadDescription: *mut *mut u16) -> i32;
                                    }
                                    if unsafe { GetThreadDescription(h_thread, &mut p_desc) } >= 0 && !p_desc.is_null() {
                                        let mut len = 0;
                                        while len < 256 && unsafe { *p_desc.add(len) } != 0 {
                                            len += 1;
                                        }
                                        let slice = unsafe { std::slice::from_raw_parts(p_desc, len) };
                                        if let Ok(name) = String::from_utf16(slice) {
                                            if !name.trim().is_empty() {
                                                desc_str = name;
                                            }
                                        }
                                        unsafe { windows_sys::Win32::System::Com::CoTaskMemFree(p_desc as *const _) };
                                    }

                                    current_threads.push((tid, desc_str, total_100ns));
                                }
                                unsafe { CloseHandle(h_thread) };
                            }
                        }

                        if unsafe { Thread32Next(snapshot, &mut entry) } == 0 {
                            break;
                        }
                    }
                }
                unsafe { CloseHandle(snapshot) };

                // Calcula % de CPU por thread
                let mut report = Vec::new();
                let mut total_proc_cpu = 0.0;

                for (tid, name, total_100ns) in current_threads {
                    if let Some((prev_100ns, prev_time)) = prev_times.insert(tid, (total_100ns, now)) {
                        let wall_elapsed_us = now.duration_since(prev_time).as_micros() as f64;
                        if wall_elapsed_us > 0.0 && total_100ns >= prev_100ns {
                            let cpu_used_us = ((total_100ns - prev_100ns) as f64) / 10.0;
                            let thread_cpu_pct = (cpu_used_us / (wall_elapsed_us * num_cpus)) * 100.0;
                            let core_cpu_pct = (cpu_used_us / wall_elapsed_us) * 100.0;
                            total_proc_cpu += thread_cpu_pct;
                            if thread_cpu_pct >= 0.1 || core_cpu_pct >= 1.0 {
                                report.push((name, thread_cpu_pct, core_cpu_pct));
                            }
                        }
                    }
                }

                if !report.is_empty() {
                    report.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                    let mut line = format!("🔥 [PROFILER DE CPU DO PROCESSO: {:.1}% Total]:", total_proc_cpu);
                    for (name, sys_pct, core_pct) in report {
                        line.push_str(&format!(" [{} -> {:.1}% sys ({:.1}% core)]", name, sys_pct, core_pct));
                    }
                    log::info!("{}", line);
                }
            }
        })
        .ok();
}

#[cfg(not(target_os = "windows"))]
pub fn start_thread_cpu_profiler() {}
