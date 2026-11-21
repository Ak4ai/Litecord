//! Scheduling priority for the threads a screen share depends on.
//!
//! With every core busy (a CPU-heavy game, a compile), threads at normal priority wait
//! their turn like any other and the stream drops frames: in a test with the CPU at
//! 100% the viewer fell to ~46 FPS with freezes. One step above normal lets the
//! capture, the encoder and the senders run first; they use little CPU, so the game
//! loses almost nothing.

/// Raises the calling thread one step above normal.
pub fn raise_current_thread() {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL};
        if SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL) == 0 {
            log::debug!("SetThreadPriority failed for {:?}", std::thread::current().name());
        }
    }
}

/// Given `(cpu_id, max_freq_khz)` pairs, returns the CPU IDs belonging to the
/// lower-frequency efficiency tier(s) on hybrid CPUs (e.g. Intel E-cores / LP E-cores).
/// Returns an empty vector on homogeneous CPUs where all cores share the same max frequency.
#[cfg(any(target_os = "linux", test))]
fn detect_efficiency_cores_from_freqs(cpus: &[(usize, u64)]) -> Vec<usize> {
    let Some(&max_freq) = cpus.iter().map(|(_, f)| f).max() else {
        return Vec::new();
    };
    let Some(&min_freq) = cpus.iter().map(|(_, f)| f).min() else {
        return Vec::new();
    };
    // Require at least a 5% frequency gap to consider the CPU hybrid (P-cores vs E-cores).
    if max_freq == 0 || min_freq * 100 >= max_freq * 95 {
        return Vec::new();
    }
    let threshold = max_freq * 95 / 100;
    cpus.iter()
        .filter(|(_, f)| *f > 0 && *f < threshold)
        .map(|(id, _)| *id)
        .collect()
}

/// Applies or clears efficiency-core CPU affinity across all threads of the current
/// process on hybrid Linux CPUs. Opt out globally with `LITECORD_ALLOW_P_CORES=1`.
pub fn apply_efficiency_cores_policy(enabled: bool) {
    #[cfg(target_os = "linux")]
    {
        let mut freqs: Vec<(usize, u64)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir("/sys/devices/system/cpu") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(s) = name.to_str() else { continue };
                let Some(num_str) = s.strip_prefix("cpu") else { continue };
                let Ok(cpu_id) = num_str.parse::<usize>() else { continue };
                let freq_path = entry.path().join("cpufreq/cpuinfo_max_freq");
                if let Ok(raw) = std::fs::read_to_string(freq_path) {
                    if let Ok(khz) = raw.trim().parse::<u64>() {
                        freqs.push((cpu_id, khz));
                    }
                }
            }
        }
        let e_cores = detect_efficiency_cores_from_freqs(&freqs);
        if e_cores.len() < 2 {
            return;
        }
        let target_cpus: Vec<usize> = if enabled && std::env::var_os("LITECORD_ALLOW_P_CORES").is_none() {
            e_cores
        } else {
            freqs.into_iter().map(|(id, _)| id).collect()
        };

        unsafe {
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            libc::CPU_ZERO(&mut set);
            for &cpu in &target_cpus {
                if cpu < libc::CPU_SETSIZE as usize {
                    libc::CPU_SET(cpu, &mut set);
                }
            }
            let size = std::mem::size_of::<libc::cpu_set_t>();
            let mut applied_any = false;
            if let Ok(tasks) = std::fs::read_dir("/proc/self/task") {
                for task in tasks.flatten() {
                    if let Ok(tid) = task.file_name().to_string_lossy().parse::<libc::pid_t>() {
                        if libc::sched_setaffinity(tid, size, &set) == 0 {
                            applied_any = true;
                        }
                    }
                }
            }
            if !applied_any {
                let _ = libc::sched_setaffinity(0, size, &set);
            }
            log::info!(
                "⚡ [CPU] Afinidad de hilos {}: {:?}",
                if enabled { "solo E-cores" } else { "todos los núcleos" },
                target_cpus
            );
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = enabled;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hybrid_cpu_selects_only_efficiency_cores() {
        // Simulated Intel Core Ultra 7 155H: 12 P-core threads @ 4.8GHz, 8 E-cores @ 3.8GHz, 2 LP E-cores @ 2.5GHz
        let mut cpus = Vec::new();
        for id in 0..12 {
            cpus.push((id, 4_800_000));
        }
        for id in 12..20 {
            cpus.push((id, 3_800_000));
        }
        for id in 20..22 {
            cpus.push((id, 2_500_000));
        }
        let e_cores = detect_efficiency_cores_from_freqs(&cpus);
        assert_eq!(e_cores, (12..22).collect::<Vec<_>>());
    }

    #[test]
    fn homogeneous_cpu_returns_empty() {
        let cpus: Vec<(usize, u64)> = (0..16).map(|id| (id, 4_500_000)).collect();
        assert!(detect_efficiency_cores_from_freqs(&cpus).is_empty());
    }
}
