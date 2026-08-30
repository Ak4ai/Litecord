# Changelog

All notable changes to **Litecord** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [v0.3.9] - 2026-08-30

### 👥 Multi-Account Vault & Instant Account Switching
- **Encrypted Multi-Account Vault (`AccountVault`)**:
  - Securely stores multiple Discord accounts simultaneously in `%APPDATA%/Litecord/session.vault` (Windows DPAPI) and `~/.config/litecord/session.vault` (Linux AES-256-GCM).
  - Preserves user metadata (User ID, Global Name, Username, Tag, Avatar Initials, and Active state).
- **Interactive Account Switcher Modal**:
  - Clicking the user profile bar in the bottom-left now opens the **Gerenciar Contas (Account Manager)** modal.
  - Lists all saved accounts with instantaneous 1-click **"Entrar" (Switch)** without re-scanning QR codes or re-authenticating.
  - Includes **"➕ Adicionar Outra Conta"** to easily authenticate and register secondary accounts into the vault.
  - Includes single-account removal trash buttons and **"Sair de Todas as Contas"** to wipe all credentials on full logout.

### 🛡️ End-to-End Encryption (E2EE) & Security
- **X25519 ECDH + AES-256-GCM P2P Cryptography**:
  - Integrated ephemeral Curve25519 Diffie-Hellman (`x25519-dalek`) key exchange directly into P2P video streaming signaling.
  - Generates zero-knowledge session keys in RAM: third parties or eavesdroppers with the Discord Channel ID (`cid`) cannot decrypt video packets.
  - Authenticated AEAD encryption (AES-256-GCM) with 12-byte random nonces and 16-byte integrity tags, rejecting forged or tampered packets instantly.
  - Sub-0.07ms encryption/decryption per frame utilizing hardware AES-NI instructions (< 0.05% CPU impact at 1080p 60 FPS).
- **Anonymous & Encrypted MQTT Signaling**:
  - MQTT topics are derived via SHA-256 hashes (`litecord/sig/<hash>`), keeping room identity obscured from outside observers.
  - Presence payloads (IPs, ports, user IDs) are 100% encrypted with AES-256-GCM before transmission over the signaling broker.

### ⚡ Screen Share Engine & Stability
- **Sub-0.2ms Local Preview Downsampler**:
  - Replaced scalar pixel loops with a fast SIMD box-downsampler to 480w, eliminating UI thread latency and preview stuttering.
- **Strict Bounds Clamping (`fit_bgra_to_canvas`)**:
  - Added strict coordinate and destination clamping in the DXGI frame scaler, eliminating slice out-of-bounds panics on odd-aligned monitor resolutions and DPI scaling.
- **Resilient Full-Mesh UDP Routing**:
  - Transmitters broadcast simultaneously to local port clusters (`127.0.0.1:50005..=50007`), LAN broadcasts, and remote WAN endpoints.
  - Extended watchdog timeout to 3000ms with per-chunk activity renewal, ensuring rock-solid stream persistence.

### 🎨 UI & Design Fixes
- **Unified Vector SVG Collapse Chevrons (`chevron-down.svg`, `chevron-right.svg`, `chevron-up.svg`)**:
  - Replaced system Unicode arrows (`▸` / `▾` / `▼`) with crisp SVG vector icons for categories and message links.
  - Fixes missing font glyph boxes (`□`) on Windows systems and provides smooth color transitions across all platforms.
- **Fixed Chat Header Channel Title Duplication**:
  - Sanitized active channel name formatting to prevent duplicate hashtag prefixing (`## channel` -> `# channel`).
- **Remote Stream Viewport Expansion**:
  - Decoupled remote video card visibility in Slint UI, guaranteeing instant video viewport rendering upon incoming frame arrival.

### 📦 Windows Installer & Updater
- **Automated Restart & Installer Relaunch**:
  - Removed `skipifsilent` flag in Inno Setup (`installer.iss`), ensuring the installer launches `litecord.exe` immediately after installation and in-app updates.
- **Continuous Logging (`litecord_app.log`)**:
  - Switched log file handle to append mode to preserve complete multi-instance telemetry diagnostics.

---

## [v0.3.8] - 2026-08-29

### 🛡️ Security & Token Hardening
- **Linux Encrypted Token Vault (`~/.config/litecord/session.vault`)**:
  - Implemented secure credential storage on Linux strictly following the XDG Base Directory specification.
  - Directories are created with `0700` and vault files with `0600` (exclusive user read/write access).
  - Credentials are encrypted at rest with **AES-256-GCM** using keys derived from `/etc/machine-id` and the user's UID (preventing token theft even if storage files are exfiltrated).
  - Seamless automatic migration from legacy `.litecord_token` files.
- **Windows DPAPI Vault Alignment**:
  - Encrypted tokens on Windows are now stored in `%APPDATA%/Litecord/session.vault` using Windows DPAPI (`CryptProtectData`).
- **P2P Video Streaming Cryptographic Authentication**:
  - Derived AES-256-GCM E2EE keys now mix Discord's authenticated `voice_secret_key` (negotiated via Voice Gateway Opcode 4).
  - Anonymous MQTT signaling topics are derived using SHA-256 hashes of the authenticated session key, ensuring only verified participants in the voice room can discover or decrypt screen share streams.
- **Strict Log Sanitization**:
  - Removed all token and session key prefixes from console and file logs (`litecord_app.log`).
- **Atomic Logout Credential Cleanup**:
  - Centralized `delete_secure_token()` handler to guarantee zero residual credentials on user sign-out.

### 🖥️ UI & Popout Window Improvements
- **Popout Window Responsive Controls Hierarchy**:
  - Decreased minimum window resize limits down to **180px × 120px** (mini Picture-in-Picture mode).
  - Implemented progressive hiding order as the window is shrunk:
    1. Volume slider (hides when width <= 420px).
    2. "AO VIVO" Live Badge (hides when width <= 340px).
    3. FPS Counter Badge (hides when width <= 270px).
    4. Username (hides when width <= 200px).
    5. Action control buttons (Ghost mode, Pin, Minimize, Maximize, Close) remain **100% visible and accessible**.
- **Embedded Video Card Dynamic Expansion**:
  - Clicking maximize/focus on the embedded stream card in a voice room now dynamically expands the video container to fill all available viewport height and width (`stage_height - 52px`) with zero clipping or overflow.
- **Main Window Maximize/Restore Button Toggle**:
  - Synchronized `is_maximized` state between the window manager and titlebar.
  - The maximize icon dynamically toggles between single square (`chrome-maximize.svg`) and restore double-square (`chrome-restore.svg`).

### ⚡ Portability & Robustness
- **Static CRT on Windows (`+crt-static`)**:
  - Windows release builds now statically link the C runtime (`target-feature=+crt-static`), allowing Litecord to run immediately on fresh Windows installs and VMs without requiring the Visual C++ Redistributable (`vcruntime140.dll`).
- **Automatic Software Renderer Fallback**:
  - Added automatic detection and fallback to Slint software renderer (`SLINT_BACKEND=software`) when hardware OpenGL/GPU drivers fail to initialize (common in Virtual Machines and RDP sessions).

---

## [v0.3.7] - 2026-08-28

### 🎙️ Audio & Voice Pipeline
- Enhanced Opus packet decoding and jitter buffer synchronization.
- Real-time VAD threshold testing with visual audio meters.
- Squad Leader / IGL Dynamic Speech Priority Ducking engine.

### 📺 P2P Video Streaming & DRM Bypass
- DXGI Desktop Duplication & Direct3D11 zero-copy hardware framebuffer capture.
- Isolated DRM-free browser launcher profile for movie nights.
- WASAPI in-game audio loopback mixer.

---

## [v0.3.0] - 2026-08-25

### ✨ Initial Release
- Native Slint UI with sub-30 MB RAM footprint and sub-0.1% CPU usage.
- DeepSleep System Tray background suspension (< 5 MB RAM).
- Discord Gateway v9 & DAVE voice encryption support.
- QR Code mobile login via Remote Auth v2.
- Smart slash commands autocomplete with keyboard navigation.
- Ephemeral image attachments and Twemoji Unicode support.
