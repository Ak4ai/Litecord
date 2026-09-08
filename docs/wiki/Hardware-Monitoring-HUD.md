# 📊 Hardware Monitoring HUD & Telemetry

Litecord includes a built-in, ultra-efficient hardware telemetry HUD designed specifically to verify application performance without third-party tools.

---

## 1. Process-Specific Measurements

Unlike standard monitors that show total system load, Litecord displays **only its own resource usage**:

### CPU Usage (%)
Measured via the Win32 `GetProcessTimes` API compared against total system time from `GetSystemTimes`:
```rust
let proc_delta = proc_total.saturating_sub(self.last_proc_total);
let sys_delta = sys_total.saturating_sub(self.last_sys_total);
let cpu_pct = ((proc_delta as f64 * 100.0) / (sys_delta as f64)).round() as u8;
```
This formula directly mirrors the calculation used by the Windows Task Manager.

### RAM (Megabytes)
Measured using the Win32 PSAPI function `K32GetProcessMemoryInfo`:
```rust
let ok = K32GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, size);
let ram_mb = (pmc.WorkingSetSize / (1024 * 1024)) as u32;
```
Reports the exact physical Working Set in MB (e.g. `24 MB`), avoiding ambiguous percentages.

### GPU Usage (%)
Measured using the Windows Performance Data Helper (PDH) querying `\GPU Engine(*)\Utilization Percentage`.
To isolate Litecord from games, the engine counter array iterates over instances and filters exclusively by the current process ID (`pid_<PID>_...`):
```rust
if !item.sz_name.is_null() && wide_contains(item.sz_name, &self.pid_needle) {
    total_gpu += item.fmt_value.double_val;
}
```

---

## 2. Multi-Point Presentation

1. **Titlebar HUD**: Displayed next to the settings gear button: `CPU 0% • RAM 28 MB • GPU 0%`.
2. **Windows Taskbar Button**: Displays a compact title formatted as `Litecord - x%/ymb` (e.g. `Litecord - 1%/32MB`), perfectly fitting within minimized taskbar tabs without text clipping.
3. **System Tray Tooltip**: Full live detail on mouse hover: `Litecord - CPU 1% | RAM 32 MB | GPU 0%`.
