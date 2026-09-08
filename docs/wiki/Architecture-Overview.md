# 🏗️ Architecture Overview

Litecord is designed as an asynchronous, multi-threaded native application that decouples the user interface render thread from network I/O, audio digital signal processing (DSP), and hardware video encoding.

```text
┌────────────────────────────────────────────────────────────────────────┐
│                          Litecord Application                          │
├────────────────────────────────┬───────────────────────────────────────┤
│        Tokio Async Runtime     │           Slint UI Thread             │
│  ┌──────────────────────────┐  │  ┌──────────────────────────────────┐ │
│  │ Discord Gateway Client   │  │  │ Winit Window Loop & GPU Renderer │ │
│  │ (WebSocket, REST v9/v10) │  │  │ (Direct3D / Vulkan / Software)   │ │
│  └─────────────┬────────────┘  │  └──────────────────▲───────────────┘ │
│                │               │                     │                 │
│                ▼ mpsc Event Bus│        slint::invoke_from_event_loop  │
│  ┌──────────────────────────┐  │                     │                 │
│  │ Voice Gateway & DAVE     ├──┼─────────────────────┘                 │
│  │ (Opus, UDP, E2EE MLS)    │  │                                       │
│  └─────────────┬────────────┘  │  ┌──────────────────────────────────┐ │
│                │               │  │ Live System Tray & Tooltip Timer │ │
│                ▼ Lock-Free Ring│  └──────────────────▲───────────────┘ │
│  ┌──────────────────────────┐  │                     │ mpsc            │
│  │ CPAL / WASAPI Audio DSP  │  │  ┌──────────────────┴───────────────┐ │
│  │ (48kHz Stereo, VAD, Duck)│  │  │ Hardware Monitor Background Thd  │ │
│  └──────────────────────────┘  │  │ (CPU, WorkingSet MB, GPU PDH)    │ │
│                                │  └──────────────────────────────────┘ │
└────────────────────────────────┴───────────────────────────────────────┘
```

---

## 1. Concurrency Model

Litecord leverages a hybrid concurrency model:
- **Main Thread (GUI)**: Runs the Slint reactive UI event loop backed by `winit`. It handles window resizing, animations, custom titlebar drag/drop, input typing, and frame rasterization. The UI thread never performs network requests or disk I/O.
- **Tokio Multi-Threaded Runtime**: Coordinates all network operations: Discord Gateway v9/v10 WebSockets, Voice Gateway v4/v8 handshakes, Cloudflare Edge P2P signaling, REST API queries, image fetching, and audio streaming.
- **Dedicated Audio Thread (CPAL)**: Dedicated high-priority real-time audio thread executing OS audio callbacks (WASAPI on Windows, ALSA/PulseAudio on Linux) with sub-10ms buffer latency.
- **Hardware Monitor Thread**: Independent background worker that sleeps for 1000ms intervals, querying Windows performance telemetry APIs and notifying the UI and system tray without blocking.

---

## 2. Event Bridge: Tokio to Slint UI

Because GUI state is managed by the main thread, background tasks communicate with the UI via `slint::invoke_from_event_loop`:
```rust
let app_weak = app_weak.clone();
let _ = slint::invoke_from_event_loop(move || {
    if let Some(ui) = app_weak.upgrade() {
        ui.set_hardware_cpu_text(format!("CPU {}%", metrics.cpu_percent).into());
        ui.set_hardware_ram_text(format!("RAM {} MB", metrics.ram_mb).into());
    }
});
```
This guarantees thread safety and prevents data races.

---

## 3. DeepSleep™ State Machine

When Litecord is minimized to the system tray:
1. **Window Unmapping**: The OS window handle (`HWND`) is hidden, halting Slint GPU surface rendering.
2. **Memory Compaction**: Litecord triggers OS working set trim routines (`SetProcessWorkingSetSize(-1, -1)` on Windows), returning unused pages to the OS.
3. **RAM Drops to ~3 MB – 5 MB**: While maintaining voice connectivity, audio compression, and real-time call reception.
4. **Instant Wakeup (< 10 ms)**: Restoring the window remaps the Direct3D framebuffer instantly without reloading data.
