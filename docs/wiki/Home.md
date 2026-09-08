# 🚀 Litecord Technical Wiki & Engineering Documentation

Welcome to the official **Litecord Technical Wiki**. This knowledge base details the internal architecture, subsystems, cryptographic models, and engineering implementations powering Litecord.

Litecord is an ultra-lightweight, high-performance native desktop client for Discord written from scratch in **Rust** and powered by the **Slint** reactive UI toolkit. Designed primarily for gamers, streamers, and competitive esports players, Litecord achieves a footprint of **< 0.1% CPU** and **~32 MB RAM** during active calls while providing advanced multimedia features.

---

## 📑 Table of Contents

### 🏗️ Architecture & Core Subsystems
1. **[Architecture Overview](Architecture-Overview)**
   - Concurrency Model, Tokio Async Runtime & Slint UI Bridge
   - Cross-Thread Synchronization & Mutex Poisoning Mitigation
   - DeepSleep™ State Machine (Sub-5 MB Background RAM)
2. **[Voice Engine & Audio Pipeline](Voice-Engine-and-Audio-Pipeline)**
   - CPAL & WASAPI Audio Subsystem (48 kHz Stereo Pipeline)
   - Dynamic Audio Output Device Switching (Zero-Disconnection Hot-Swap)
   - Opus Codec with Packet Loss Concealment (PLC)
   - Voice Activity Detection (VAD) & Real-Time VU Meter
   - Tactical IGL / Shot-Caller Speech Priority Ducking
   - Discord DAVE Protocol (MLS End-to-End Encryption)
3. **[Screen Sharing & Video Pipeline](Screen-Sharing-and-Video-Pipeline)**
   - Zero-Copy Direct3D 11 / DXGI Desktop Duplication (1080p @ 60 FPS)
   - Multi-Encoder Architecture (NVENC, AMD AMF, WMF, FFmpeg HW, OpenH264 SIMD)
   - LTPV (Litecord Transport Protocol for Video) & XOR Forward Error Correction (FEC)
   - Jitter Buffer & Frame Reassembly
   - Floating Picture-in-Picture (PiP) Popout Window with Click-Through Ghost Mode
4. **[Hardware Monitoring HUD](Hardware-Monitoring-HUD)**
   - Per-Process CPU% Measurement via `GetProcessTimes` & `GetSystemTimes`
   - Real Process RAM Usage in Megabytes via `K32GetProcessMemoryInfo`
   - PDH GPU Performance Counter Engine Filtering by Process ID
   - Windows Taskbar Title (`Litecord - x%/ymb`) & Live System Tray Tooltip
5. **[Security & Cryptography](Security-and-Encryption)**
   - Multi-Account Vault (Windows DPAPI & Linux AES-256-GCM)
   - Zero-Knowledge Local Credential Storage
   - Ephemeral X25519 ECDH P2P Key Agreement
   - Safe Rust Memory Invariants & Secret Zeroization
6. **[Networking & Proxy](Networking-and-Proxy)**
   - Native HTTP/HTTPS, SOCKS5 (Tor/Shadowsocks/V2Ray) & System Proxy
   - Dynamic Reconfiguration (*Zero Downtime*)
   - Proactive Diagnostic Ping & Login Protection
7. **[User Interface & Design System](User-Interface-and-Design-System)**
   - Slint Reactive Declarative Model
   - Discord Dark Theme Palette & High-Contrast Visual Balance
   - Unified Emojis (Twemoji + Discord CDN Custom Emojis)
   - Smart Slash Command Indexing & Autocomplete
8. **[Building & Compiling](Building-and-Compiling)**
   - Build Prerequisites (Windows & Linux)
   - Cargo Profiles, Link-Time Optimization (LTO) & Static Linking
   - Windows Inno Setup Installer Generation

---

## ⚡ Performance Summary Matrix

| Metric | Official Discord (Electron) | Litecord (Native Rust + Slint) | Engineering Advantage |
| :--- | :--- | :--- | :--- |
| **Idle CPU** | 1.5% - 4.5% | **0.00% - 0.02% (DeepSleep: 0.0%)** | 150x lighter CPU footprint |
| **Voice Call CPU** | 4.0% - 8.0% | **~0.1% - 0.3%** | Zero game stuttering |
| **1080p 60fps Stream CPU**| 8.0% - 16.0% | **~0.8% - 1.4%** | Butter-smooth frametimes |
| **RAM (DeepSleep Tray)** | 350 MB - 750 MB | **~3 MB - 5 MB** | 99% background reduction |
| **RAM (Active Window)** | 500 MB - 900 MB | **~12 MB - 35 MB** | Saves up to 850 MB physical RAM |
| **Cold Startup Time** | 4.5s - 9.0s | **< 150 ms** | Instant launch |
| **Binary Size** | ~180 MB | **~8 MB Standalone** | Pure native machine code |
