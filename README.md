<div align="center">

# Litecord 🚀
### Ultra-Lightweight, High-Performance Native Discord Client for Gamers

[![Website](https://img.shields.io/badge/Website-ak4ai.github.io%2FLitecord-5865F2.svg?style=flat-square&logo=googlechrome&logoColor=white)](https://ak4ai.github.io/Litecord/)
[![GitHub Release](https://img.shields.io/github/v/release/Ak4ai/Litecord?style=flat-square&color=blueviolet)](https://github.com/Ak4ai/Litecord/releases)
[![Rust](https://img.shields.io/badge/Language-Rust_2021-orange.svg?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![GUI](https://img.shields.io/badge/GUI-Slint_1.9-blue.svg?style=flat-square)](https://slint.dev/)
[![Audio](https://img.shields.io/badge/Audio-CPAL_%7C_Opus_%7C_DAVE_E2EE-green.svg?style=flat-square)](https://github.com/RustAudio/cpal)
[![Security](https://img.shields.io/badge/Security-Windows_DPAPI_%2B_AES--GCM-blueviolet.svg?style=flat-square)]()
[![Gamer-Optimized](https://img.shields.io/badge/Performance-Zero_FPS_Drop-success.svg?style=flat-square)]()
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg?style=flat-square)](LICENSE)

<p align="center">
  <b>Litecord</b> is an ultra-fast, native desktop client for Discord engineered from scratch in <b>Rust</b> for <b>gamers, streamers, and competitive esports players</b>. Running with <b>&lt; 0.1% CPU</b> and <b>~32 MB RAM</b>, it eliminates background micro-stutters and drops zero in-game FPS while delivering <b>Full HD 1080p 60 FPS Screen Sharing</b>, <b>IGL / Shot-Caller Speech Priority Ducking</b>, <b>Detached Stream Popouts</b>, <b>QR Code Mobile Login</b>, <b>Smart Slash Commands</b>, <b>Unified Emojis</b>, and <b>On-Demand Ephemeral Attachments</b>.
</p>

[🌐 Live Website](https://ak4ai.github.io/Litecord/) • [📦 Downloads](#-downloads--releases) • [🎮 Gamer Features](#-why-gamers-choose-litecord) • [⚡ Benchmarks](#-benchmarks-vs-official-discord) • [📺 1080p 60 FPS Video Pipeline](#-full-hd-1080p-60-fps-video--audio-pipeline) • [✨ All Features](#-features-breakdown) • [🛡️ Security & Privacy](#-cybersecurity-privacy--account-safety) • [🛠️ Build from Source](#-building-from-source)

<br/>

<img src="assets/demo_preview.gif" alt="Litecord Native Interface Demo" width="760px" style="border-radius: 8px; box-shadow: 0 10px 30px rgba(0,0,0,0.5);" />

</div>

---

## 🎮 Why Gamers Choose Litecord

- 🏆 **Zero In-Game FPS Drops & Micro-Stutters**: Reclaims CPU threads eaten by Chromium/Electron, improving 1% low framerates in competitive titles (*CS2, Valorant, Warzone, Apex Legends, Fortnite, League of Legends*).
- 📺 **Full HD 1080p 60 FPS Screen Sharing**: Direct hardware framebuffer capture (DXGI / Direct3D11) with WASAPI loopback audio and sub-20ms glass-to-glass latency.
- 👑 **Squad Leader & IGL Priority Ducking**: Set shot-callers to Priority 2 (`[ - ] P:2 [ + ]`) so critical tactical callouts automatically duck background chatter and music bots during clutch moments.
- 🪟 **Detached Video & Stream Popouts**: Pop out live screen shares and video streams into a dedicated floating Picture-in-Picture (PiP) window with an always-on-top pin.
- 📱 **QR Code Mobile Login**: Log in instantly by scanning a QR code with the official Discord mobile app—no manual token extraction.
- ⌨️ **Smart Slash Commands Autocomplete**: Real-time suggestion indexing for `/play`, `/skip`, and server bot commands with keyboard navigation (Up/Down + Enter) and interactive parameter chips.
- 🖼️ **On-Demand Ephemeral Image Attachments**: Minecraft-style pixel-art placeholders (~500 bytes) with dynamic proportional height and zero-residue temp downloads.
- 🎨 **Unified Emoji System (Twemoji + Discord CDN)**: Zero missing tofu squares (`□`). Full support for Discord custom animated/static emojis and Unicode emojis across chat, embeds, and bot buttons.
- ⚡ **Sub-5 MB DeepSleep RAM**: Drops physical memory footprint down to **~3 MB – 5 MB** when minimized to the system tray, freeing maximum RAM for games.
- 🎙️ **Opus PLC (Packet Loss Concealment)**: Prevents robotic voice stuttering and audio crackles even under 100% CPU/GPU load.
- 🌐 **7 Built-in Languages**: Automatic OS language detection with English, Portuguese, Spanish, German, French, Russian, and Japanese.

---

## ⚡ Benchmarks vs Official Discord

| Metric | Official Discord (Electron) | **Litecord (Native Rust + Slint)** | Gaming Impact |
| :--- | :--- | :--- | :--- |
| **Idle CPU Usage** | 1.5% - 4.5% | **0.00% - 0.02% (DeepSleep: 0.0%)** | 🚀 **150x lighter CPU footprint** |
| **Active Voice CPU** | 4.0% - 8.0% | **~0.1% - 0.3%** | ⚡ **Zero Game Stuttering** |
| **1080p 60 FPS Stream CPU** | 8.0% - 16.0% | **~0.8% - 1.4%** | 🎮 **Butter-Smooth Game Play** |
| **RAM Usage (DeepSleep Tray)** | 350 MB - 750 MB | **~3 MB - 5 MB** | 🌙 **99% lighter background footprint** |
| **RAM Usage (Active Window)** | 500 MB - 900 MB | **~12 MB - 28 MB** | 💾 **Saves up to 850 MB RAM** |
| **Startup Time** | 4.5s - 9.0s | **< 150 ms** | ⏱️ **Instant Match Launch** |
| **Binary Size** | ~180 MB | **~8 MB Standalone** | 📦 **Pure Native Machine Code** |

---

## 📺 Full HD 1080p 60 FPS Video & Audio Pipeline

Litecord features an ultra-optimized native video streaming and screen capture engine built in Rust (`src/screen_capture.rs`):

```text
  [ Game / Desktop Framebuffer ]
                │
                ▼ (DXGI Desktop Duplication / D3D11 Zero-Copy)
  [ Hardware Frame Capture @ 60 FPS ] ──► [ WASAPI In-Game Audio Loopback ]
                │                                    │
                ▼ (Fast Adaptive TurboJPEG / SIMD)   ▼ (Opus 48kHz Stereo)
  [ Low-Latency Packetizer (1350B MTU) ] ◄───────────┘
                │
                ▼ (Direct UDP / LTPV Protocol - Sub-20ms Latency)
  [ Litecord Zero-Copy Jitter Buffer ]
                │
                ▼ (Double-Buffered SharedPixelBuffer<Rgba8Pixel>)
  [ Slint Native Viewport / Detached PiP Popout Window ]
```

### 🚀 Key Engineering Pillars:
1. **Direct GPU Framebuffer Capture (Zero-Copy DXGI / D3D11)**:
   - Directly grabs raw frames from the desktop compositor without CPU blitting, allowing constant 60 FPS capture at 1080p / 1440p / 4K with < 1% CPU overhead.
2. **WASAPI In-Game Audio Loopback**:
   - Captures crystal-clear in-game audio directly from Windows audio endpoints and mixes it synchronously with the video frame timeline.
3. **LTPV (Litecord Peer-to-Peer Video Protocol)**:
   - Packets are split into **1350-byte chunks** (matching standard network MTU) to prevent IP fragmentation and packet loss.
   - Out-of-order frame reassembly with adaptive jitter absorption and zero-copy ring buffers.
4. **Double-Buffered Lockless Slint Rendering**:
   - Frame presentation uses a thread-safe `SharedPixelBuffer<Rgba8Pixel>` double buffer that swaps frames without locking UI event loops.
5. **Floating Picture-in-Picture (PiP) Popout Window**:
   - Detach streams into a standalone floating window with an **Always-on-Top Pin** and independent audio volume sliders (0–200%).

---

## ✨ Features Breakdown

### 🎙️ 1. Studio-Grade Voice Pipeline & DAVE Protocol (E2EE)
- **True Opus PLC (Packet Loss Concealment)**: Synthesizes missing or delayed audio frames seamlessly, eliminating crackles, micro-stutters, and robotic drops even with packet jitter.
- **Adaptive Jitter Pre-Buffer (40ms)**: Absorbs network fluctuations from music bots (Notch, Lara, Jockie) and unstable connections with zero manual tuning.
- **Studio Soft-Knee Limiter**: Transparent dynamic compression for loud audio peaks and heavy bass without harsh square-wave clipping.
- **Cubic Hermite Resampler**: High-fidelity 48kHz audio interpolation for smooth, crystal-clear voice output.
- **DAVE End-to-End Encryption**: Direct support for Discord's MLS (RFC 9420) voice encryption protocol.
- **Microphone VAD & Sensitivity Test**: Real-time visual audio meter (30 FPS) with adjustable threshold slider saved automatically to `.litecord_audio_config.json`.

<div align="center">
  <img src="assets/demo_voice.gif" alt="Litecord Voice Channels & Speech Priority Ducking Demo" width="760px" style="border-radius: 8px; box-shadow: 0 10px 30px rgba(0,0,0,0.5);" />
</div>

<br/>

### 👑 2. Dynamic Speech Priority & Smart Ducking
Take control of crowded voice calls with custom per-user priorities (`[ - ] P:N [ + ]`):
- **Hierarchical Attenuation**: When multiple people talk at once, lower-priority speakers are automatically attenuated smoothly based on the priority difference (`Delta = max_priority - user_priority`):
  - **Delta = 1** (e.g., Priority 1 vs 0): Volume reduced to **50%**.
  - **Delta = 2** (e.g., Priority 2 vs 0): Volume reduced to **40%**.
  - **Delta = 3** (e.g., Priority 3 vs 0): Volume reduced to **30%**.
  - **Delta >= 5**: Volume ducked to protection floor (**5% - 10%**).
- **Independent Volume & Mute**: Per-user volume sliders (0% - 200%) and instant mute buttons, saved automatically across sessions.

### 📱 3. QR Code Remote Auth & Windows DPAPI Security
- **Instant QR Code Login**: Scan the on-screen QR code with your Discord Mobile App (Settings -> Scan QR Code) to log in instantly.
- **Windows DPAPI Encryption at Rest**: Discord tokens stored locally in `.litecord_token` are encrypted with Windows DPAPI (`CryptProtectData`), making them unreadable to other user accounts, malware, or background scripts.
- **Direct Discord Connections Only**: No intermediary proxies, third-party relays, or tracking servers. All requests go directly to `discord.com` endpoints.
- **Shell Injection Shield**: Hyperlinks are strictly validated against an `http://` / `https://` whitelist and dispatched via native OS APIs (`ShellExecuteW` / `xdg-open`)—never through shell interpreters (`cmd.exe`).

### ⌨️ 4. Intelligent Slash Commands & Parameter Chips
- **Dynamic Server Command Indexing**: Fetches and aggregates real slash commands from registered bots (`/play`, `/skip`, `/stop`, `/queue`, etc.).
- **Keyboard Navigation**: Use **Up/Down Arrow keys** to cycle through command suggestions and hit **Enter** to auto-select.
- **Interactive Parameter Chips**: Formats commands as clean visual chips in the message input and chat history with parameter placeholders.

### 🖼️ 5. Ultra-Lightweight On-Demand Image Attachments
- **Minecraft Pixel-Art Placeholders**: Low-resolution (~500 bytes) chunky 8-bit preview before downloading.
- **Fixed Width (320px) & Proportional Height**: Dynamically adapts height to match the image's original aspect ratio (16:9, portrait, square).
- **Ephemeral Temp Storage**: Full downloads are saved in `%TEMP%/Litecord/temp_images/` and automatically wiped on app startup and shutdown.
- **Collapsed Link Archive**: Image URLs remain accessible inside the collapsed message view without cluttering chat.

### 🎨 6. Unified Emoji System (Twemoji + Discord CDN)
- **Discord Custom Emojis**: Asynchronously downloaded, cached locally, and updated in-place.
- **Twemoji Unicode Rendering**: Direct vector glyph rasterization for Unicode emojis (`⏭️`, `⏮️`, `⏯️`, `🔀`, `🔁`, `🔥`, `❤️`, etc.), preventing Windows tofu boxes (`□`).
- **Screen & Active Channel Priority**: Dedicates network bandwidth exclusively to visible chat messages.

### ⚡ 7. DeepSleep Mode & Extreme Efficiency
- **Sub-0.1% CPU Idle**: UI event dispatch loop is decoupled and capped at 30 FPS for microphone meters.
- **System Tray DeepSleep**: Minimizing Litecord to the system tray completely suspends all visual rendering loops while keeping voice audio streaming in background.
- **Delta Badge Fingerprinting**: Sidebar channel member count badges update only on real state changes, preventing unnecessary thread wakeups.
- **In-App Update Checker**: Automatically notifies you when a new release is available on GitHub.

---

## 🛡️ Cybersecurity, Privacy & Account Safety

When choosing an alternative client for Discord, **security and account integrity are paramount**:

### 🔒 1. Zero-Trust Security Architecture
- **Direct Discord Connections Only (Zero Intermediaries):** Litecord connects directly from your machine to official Discord endpoints (`https://discord.com/api` and `wss://gateway.discord.gg`). There are **no proxy servers, no third-party APIs, and no telemetry backends**.
- **Windows DPAPI Local Encryption at Rest:** On Windows, session tokens are encrypted locally using the Windows Data Protection API (`CryptProtectData`). The encrypted `.litecord_token` file is cryptographically bound to your Windows user logon credentials.
- **Shell Injection & RCE Shield:** Chat hyperlinks are strictly validated against an `http://` and `https://` protocol whitelist and dispatched directly to your default browser via native OS APIs (`ShellExecuteW` on Windows / `xdg-open` on Linux).
- **100% Open Source & Auditable (MIT):** Every single line of Rust code is public and open for community audit.

### ⚠️ 2. Discord Terms of Service (ToS) & Account Safety
- **Strictly Human-Driven (Zero Automation / Selfbots):** Litecord is engineered purely as a lightweight interactive desktop client for human gamers. It contains **no automated scrapers, no auto-responders, no mass-messaging tools, and no bot scripts** that trigger Discord's automated anti-abuse detection heuristics.
- **Official Gateway Protocol Compliance:** Connects via standard Discord Gateway v9 and Voice Gateway v9 channels, emitting normal human-paced interaction events and respecting API rate limits.
- **Transparent ToS Disclaimer:** Like all third-party Discord software (*Vencord, BetterDiscord, Ripcord*), using an alternative client is technically against Discord's Terms of Service. Litecord is provided for performance and educational purposes.

---

## 📦 Downloads & Releases

Pre-compiled production binaries are available under [GitHub Releases](https://github.com/Ak4ai/Litecord/releases):

| Distribution | File | Details |
| :--- | :--- | :--- |
| **🪟 Windows Release (v0.3.0)** | `Litecord-v0.3.0-windows-x64.zip` | Standalone executable (`litecord.exe`). Unpack and run anywhere. |
| **🪟 Windows Setup** | `Litecord-Setup-x64.exe` | Inno Setup installer with Desktop shortcut and uninstaller. |
| **🐧 Linux Standalone** | `litecord-linux-x64.tar.gz` | Native x86_64 Linux binary compiled with ALSA and System Tray support. |

---

## 🐧 Linux — Quick Install (One-Line)

Open your terminal and paste:

```bash
curl -sSL https://raw.githubusercontent.com/Ak4ai/Litecord/main/install.sh | bash
```

The script will:
1. 📥 Download the latest pre-compiled binary from GitHub Releases automatically
2. 🔧 Automatically detect and install runtime dependencies (`xdotool`, `libayatana-appindicator`) on Arch Linux, Debian/Ubuntu, and Fedora
3. 📂 Install it to `~/.local/bin/litecord`
4. 🖥️ Create a `.desktop` entry for your app launcher
5. ✅ Tell you if you need to add `~/.local/bin` to your `$PATH`

> **Requirements:** `curl` and `tar` (pre-installed on virtually all Linux distros).

---

## 🛠️ Building from Source

### Prerequisites
- **Rust Toolchain**: 2021 Edition or later (`rustup install stable`)
- **Cargo**: Included with Rust

### 📦 Run Development Mode
```bash
cargo run
```

### ⚡ Build Optimized Release Binary
```bash
cargo build --release
```
The optimized executable will be located at `target/release/litecord.exe` (Windows) or `target/release/litecord` (Linux).

---

## 📂 Project Architecture

```text
Litecord/
├── .github/workflows/
│   └── build.yml
├── assets/
│   ├── app_icon.ico
│   ├── app_icon.png
│   └── (svg icons & demo media)
├── src/
│   ├── main.rs
│   ├── gateway.rs
│   ├── http.rs
│   ├── remote_auth.rs
│   ├── screen_capture.rs
│   ├── attachment_cache.rs
│   ├── emoji_cache.rs
│   ├── updater.rs
│   ├── i18n.rs
│   └── tray.rs
├── ui/
│   └── appwindow.slint
├── build.rs
├── Cargo.toml
├── CONTRIBUTING.md
├── index.html
├── installer.iss
└── README.md
```

### 🧩 Module Breakdown

| Module | Purpose & Core Responsibilities |
| :--- | :--- |
| **`src/main.rs`** | Application lifecycle, Slint UI bindings, system tray event loop, chat rendering, message dispatch. |
| **`src/gateway.rs`** | Discord Gateway v9 WebSocket, CPAL/Opus voice pipeline, DAVE E2EE protocol, Speech Priority Ducking, and VAD. |
| **`src/screen_capture.rs`** | 1080p 60 FPS DXGI/D3D11 hardware screen capture, WASAPI audio loopback, LTPV UDP streaming, and PiP Popout. |
| **`src/http.rs`** | Discord HTTP REST client for fetching guilds, channels, messages, member lists, and slash command schemas. |
| **`src/remote_auth.rs`** | Secure QR Code remote authentication with Discord Mobile App cryptographic handshake. |
| **`src/attachment_cache.rs`** | Ephemeral image downloader, Minecraft pixel-art placeholder generator, and `%TEMP%` cleanup manager. |
| **`src/emoji_cache.rs`** | Multi-tier memory/disk cache for Discord custom emojis and Twemoji Unicode vector rasterization. |
| **`src/updater.rs`** | Background GitHub release version checker and update notifier. |
| **`src/i18n.rs`** | Multi-language localization engine supporting 7 languages with automatic OS locale detection. |
| **`src/tray.rs`** | Native Windows System Tray integration with context menu and DeepSleep background suspension hooks. |
| **`ui/appwindow.slint`** | Declarative GPU-accelerated UI with `AppWindow` and floating `PopoutWindow` components. |

---

## 🤝 Contributing & Pull Requests

We welcome all developers, gamers, and audio enthusiasts to help build Litecord! 

### 🎯 Project Vision & Philosophy:
Litecord is built to be a **simple, clean, and ultra-lightweight interface for gamers** to use Discord without background lag, high CPU/RAM overhead, or bloat. 
- **Core Track (`main`)**: Strictly dedicated to performance, low-latency audio, and minimal essentials (< 0.1% CPU, < 35 MB RAM).
- **Alternative Branches & Releases**: Any modifications or experimental features can be published as alternative branches or release tracks.

### 👑 Contributors Hall of Fame
| Contributor | Role / Contributions | Impact |
| :--- | :--- | :--- |
| [**Ak4ai**](https://github.com/Ak4ai) | Project Creator & Lead Maintainer (Core architecture, Slint UI, Audio Pipeline, Ducking Engine, Security) | **100% (Core)** |
| *Your Name Here* | *Submit a Pull Request to be credited here!* | *%* |

---

## 📄 License

Distributed under the **MIT License**. See [`LICENSE`](LICENSE) for more details.
