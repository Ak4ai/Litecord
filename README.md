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
  <b>Litecord</b> is an ultra-fast, native desktop client for Discord engineered from scratch in <b>Rust</b> for <b>gamers, streamers, and competitive esports players</b>. Running with <b>< 0.1% CPU</b> and <b>~32 MB RAM</b>, it eliminates background micro-stutters and drops zero in-game FPS while giving your squad revolutionary <b>IGL / Shot-Caller Speech Priority Ducking</b>.
</p>

[🌐 Live Website](https://ak4ai.github.io/Litecord/) • [📦 Downloads](#-downloads--releases) • [🎮 Gamer Features](#-why-gamers-choose-litecord) • [⚡ Benchmarks](#-benchmarks-vs-official-discord) • [🛡️ Security & Safety](#-cybersecurity-privacy--account-safety) • [🛠️ Build from Source](#-building-from-source)

<br/>

<img src="assets/demo_preview.gif" alt="Litecord Native Interface Demo" width="760px" style="border-radius: 8px; box-shadow: 0 10px 30px rgba(0,0,0,0.5);" />

</div>

---

## 🎮 Why Gamers Choose Litecord

- 🏆 **Zero In-Game FPS Drops & Micro-Stutters**: Reclaims CPU threads eaten by Chromium/Electron, improving 1% low framerates in competitive titles (*CS2, Valorant, Warzone, Apex Legends, Fortnite, League of Legends*).
- 👑 **Squad Leader & IGL Priority Ducking**: Set shot-callers to Priority 2 (`[ - ] P:2 [ + ]`) so critical tactical callouts automatically duck background noise and music during clutch rounds.
- ⚡ **Sub-5 MB DeepSleep RAM**: Drops physical memory down to **~3 MB – 5 MB** when minimized to the system tray, freeing maximum RAM for games.
- 🎙️ **Opus PLC (Packet Loss Concealment)**: Prevents robotic voice stuttering and crackles even when your GPU/CPU is at 100% load during intense firefights.
- 🌙 **DeepSleep Tray Suspension**: Drops visual rendering to 0.0% CPU and ~3 MB RAM when minimized, keeping crystal-clear voice communication alive in the background while you game.

---

## ⚡ Benchmarks vs Official Discord

| Metric | Official Discord (Electron) | **Litecord (Native Rust + Slint)** | Gaming Impact |
| :--- | :--- | :--- | :--- |
| **Idle CPU Usage** | 1.5% - 4.5% | **0.00% - 0.02% (DeepSleep: 0.0%)** | 🚀 **150x lighter CPU footprint** |
| **Active Voice CPU** | 4.0% - 8.0% | **~0.1% - 0.3%** | ⚡ **Zero Game Stuttering** |
| **RAM Usage (DeepSleep Tray)** | 350 MB - 750 MB | **~3 MB - 5 MB** | 🌙 **99% lighter background footprint** |
| **RAM Usage (Active Window)** | 500 MB - 900 MB | **~12 MB - 25 MB** | 💾 **Saves up to 850 MB RAM** |
| **Startup Time** | 4.5s - 9.0s | **< 150 ms** | ⏱️ **Instant Match Launch** |
| **Binary Size** | ~180 MB | **~8 MB Standalone** | 📦 **Pure Native Machine Code** |

---

## ✨ Key Features

### 🎙️ 1. Studio-Grade Voice Pipeline & DAVE Protocol (E2EE)
- **True Opus PLC (Packet Loss Concealment)**: Synthesizes missing or delayed audio frames seamlessly, eliminating crackles, micro-stutters, and robotic drops even with packet jitter.
- **Adaptive Jitter Pre-Buffer (40ms)**: Absorbs network fluctuations from music bots (Notch, Lara, Jockie) and unstable connections with zero manual tuning.
- **Studio Soft-Knee Limiter**: Transparent dynamic compression for loud audio peaks and heavy bass without harsh square-wave clipping.
- **Cubic Hermite Resampler**: High-fidelity 48kHz audio interpolation for smooth, crystal-clear voice output.
- **DAVE End-to-End Encryption**: Direct support for Discord's MLS (RFC 9420) voice encryption protocol.

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

### ⚡ 3. DeepSleep Mode & Extreme Efficiency
- **Sub-0.1% CPU Idle**: UI event dispatch loop is decoupled and capped at 30 FPS for microphone meters.
- **System Tray DeepSleep**: Minimizing Litecord to the system tray completely suspends all visual rendering loops while keeping voice audio streaming in background.
- **Delta Badge Fingerprinting**: Sidebar channel member count badges update only on real state changes, preventing unnecessary thread wakeups.

### 🛡️ 4. Enterprise-Grade Security & Privacy
- **DPAPI Token Encryption at Rest**: Discord tokens stored locally in `.litecord_token` are encrypted with Windows DPAPI (`CryptProtectData`), making them unreadable to unauthorized processes or other user accounts.
- **Shell Injection Shield**: Chat hyperlinks are strictly validated (`http://` and `https://`) and dispatched directly to the default browser via the native Windows `ShellExecuteW` API—never through `cmd.exe`.
- **Zero Telemetry**: Litecord does not track, collect, or upload any user analytics.

### 💾 5. Automatic Settings Persistence
- **Voice Activity Sensitivity (VAD Threshold)**: Slider position is written to `.litecord_audio_config.json` and restored on startup.
- **Audio Device Memory**: Remembers your preferred microphone and speaker devices.
- **User Audio Profiles**: Preserves volume and priority assignments per Discord user ID.

---

## 🛡️ Cybersecurity, Privacy & Account Safety

When choosing an alternative client for Discord, **security and account integrity are paramount**. Here is an exact breakdown of how Litecord protects your machine, your credentials, and your account:

### 🔒 1. Zero-Trust Security Architecture
- **Direct Discord Connections Only (Zero Intermediaries):** Litecord connects directly from your machine to official Discord endpoints (`https://discord.com/api` and `wss://gateway.discord.gg`). There are **no proxy servers, no third-party APIs, and no telemetry backends**.
- **Windows DPAPI Local Encryption at Rest:** On Windows, session tokens are encrypted locally using the Windows Data Protection API (`CryptProtectData`). The encrypted `.litecord_token` file is cryptographically bound to your Windows user logon credentials—preventing info-stealers, background scripts, or unauthorized accounts from reading it.
- **Shell Injection & RCE Shield:** Chat hyperlinks are strictly validated against an `http://` and `https://` protocol whitelist and dispatched directly to your default browser via native OS APIs (`ShellExecuteW` on Windows / `xdg-open` on Linux)—never through shell interpreters (`cmd.exe` or `sh`).
- **100% Open Source & Auditable (MIT):** Every single line of Rust code is public and open for community audit. You can inspect the entire networking, cryptographic DAVE, and token handling logic or compile the binary yourself from source.

### ⚠️ 2. Discord Terms of Service (ToS) & Account Safety
- **Strictly Human-Driven (Zero Automation / Selfbots):** Litecord is engineered purely as a lightweight interactive desktop client for human gamers. It contains **no automated scrapers, no auto-responders, no mass-messaging tools, and no bot scripts** that trigger Discord's automated anti-abuse detection heuristics.
- **Official Gateway Protocol Compliance:** Connects via standard Discord Gateway v9 and Voice Gateway v9 channels, emitting normal human-paced interaction events and respecting API rate limits.
- **Transparent ToS Disclaimer:** Like all third-party Discord software (*Vencord, BetterDiscord, Ripcord*), using an alternative client is technically against Discord's Terms of Service. In practice, Discord's automated enforcement systems target automated abuse, mass scraping, token raids, and spam bots rather than human users using lightweight voice clients. However, as with any third-party tool, Litecord is provided for performance and educational purposes, and users should use it with informed awareness.

---

## 📦 Downloads & Releases

Pre-compiled production binaries are available under [GitHub Releases](https://github.com/Ak4ai/Litecord/releases):

| Distribution | File | Details |
| :--- | :--- | :--- |
| **🪟 Windows Setup** | `Litecord-Setup-x64.exe` | Official Inno Setup installer with Desktop shortcut, Start Menu entry, and uninstaller. |
| **💼 Windows Portable** | `litecord-windows-x64-portable.zip` | Standalone zero-install executable (`litecord.exe`). Unpack and run anywhere. |
| **🐧 Linux Standalone** | `litecord-linux-x64.tar.gz` | Native x86_64 Linux binary compiled with ALSA and System Tray support. |

---

## 🐧 Linux — Quick Install (One-Line)

Open your terminal and paste:

```bash
curl -sSL https://raw.githubusercontent.com/Ak4ai/Litecord/main/install.sh | bash
```

The script will:
1. 📥 Download the latest pre-compiled binary from GitHub Releases automatically
2. 📂 Install it to `~/.local/bin/litecord`
3. 🖥️ Create a `.desktop` entry for your app launcher
4. ✅ Tell you if you need to add `~/.local/bin` to your `$PATH`

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
│   └── build.yml              # Automated multi-platform CI/CD release workflow
├── assets/
│   ├── app_icon.ico           # Multi-resolution Windows PE application icon (16-256px)
│   ├── app_icon.png           # Embedded high-res application & tray icon
│   └── globe.svg              # Language selector vector icon
├── src/
│   ├── main.rs                # Application lifecycle, Slint UI bindings, tray & auth
│   ├── gateway.rs             # Discord Gateway WS, CPAL/Opus voice pipeline, ducking & VAD
│   ├── http.rs                # Discord HTTP REST client (guilds, channels, messages)
│   ├── i18n.rs                # Internationalization module with 7 languages & OS detection
│   └── tray.rs                # Native Windows System Tray integration
├── ui/
│   └── appwindow.slint        # Modern, fluid, reactive UI declared in Slint
├── Cargo.toml                 # Rust dependencies & build target configurations
├── CONTRIBUTING.md            # Contributor guidelines and Pull Request policy
├── index.html                 # Official GitHub Pages web landing page
├── installer.iss              # Inno Setup Windows installer specification
└── README.md                  # Project documentation & reference
```

---

## 🤝 Contributing & Pull Requests

We welcome all developers, gamers, and audio enthusiasts to help build Litecord! 

### 🎯 Project Vision & Philosophy:
Litecord is built to be a **simple, clean, and ultra-lightweight interface for gamers** to use Discord without background lag, high CPU/RAM overhead, or bloat. 
- **Core Track (`main`)**: Strictly dedicated to performance, low-latency audio, and minimal essentials (< 0.1% CPU, < 35 MB RAM).
- **Alternative Branches & Releases**: Any modifications, experimental expansions, or heavier features that deviate from this minimal philosophy will still be reviewed with care, and merged/published as **alternative branches or alternative release tracks** to keep the core clean and lightning-fast.

### 💡 Contribution & Credit Policy:
- If you use or modify Litecord code to develop improvements, bug fixes, or new features, **please submit your work back to this repository as a [Pull Request (PR)](https://github.com/Ak4ai/Litecord/pulls)**.
- Every merged contribution will be **officially credited below in the Contributors Hall of Fame**, detailing your contributions and proportional impact on the project.
- Read our full [**Contributing Guidelines (`CONTRIBUTING.md`)**](CONTRIBUTING.md) for setup and code style instructions.

### 👑 Contributors Hall of Fame
| Contributor | Role / Contributions | Impact |
| :--- | :--- | :--- |
| [**Ak4ai**](https://github.com/Ak4ai) | Project Creator & Lead Maintainer (Core architecture, Slint UI, Audio Pipeline, Ducking Engine, Security) | **100% (Core)** |
| *Your Name Here* | *Submit a Pull Request to be credited here!* | *%* |

---

## 📄 License

Distributed under the **MIT License**. See [`LICENSE`](LICENSE) for more details.



