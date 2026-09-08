# Changelog

All notable changes to **Litecord** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [v1.0.0-beta] - 2026-09-08 — Litecord Beta 1.0.0

### 🚀 Transição Oficial para Fase Beta
- **Marco de Estabilidade & Confiabilidade**:
  - Consolidação da arquitetura nativa Rust + Slint após o ciclo de testes Alfa (v0.1.0 até v0.3.9).
  - Renomeação histórica das versões prévias como lançamentos Alfa.

### 📊 Monitor de Hardware em Tempo Real & HUD
- **Métricas Específicas do Processo (Consumo Real do Litecord)**:
  - **CPU**: Coleta o tempo de kernel e usuário do processo do Litecord via `GetProcessTimes` em relação ao tempo total decorrido do sistema (`GetSystemTimes`), correspondendo fielmente ao cálculo do Gerenciador de Tarefas do Windows.
  - **RAM (em MB)**: Coleta do Working Set real consumido pelo Litecord via `K32GetProcessMemoryInfo` (retorna em MB, ex.: `42 MB`), sem porcentagem global confusa.
  - **GPU**: Coleta via PDH (`\GPU Engine(*)\Utilization Percentage`) filtrando estritamente pelas instâncias de engines pertencentes ao PID atual (`pid_<PID>_...`), isolando o uso do app.
- **Integração com Barra de Tarefas e Bandeja**:
  - Título dinâmico e compacto na Barra de Tarefas do Windows: `Litecord - x%/ymb` (ex: `Litecord - 2%/45MB`), sem cortar na barra.
  - Tooltip vivo na bandeja do sistema (System Tray): `Litecord - CPU x% | RAM y MB | GPU z%`.
  - HUD minimalista e elegante integrado à barra superior ao lado do ícone de configurações, com suporte a toggle nas configurações.

### 🔊 Troca Dinâmica de Dispositivo de Saída de Áudio
- **Reconfiguração em Tempo Real no Voice Gateway**:
  - Alterne entre fones de ouvido, caixas de som e DACs USB no meio de uma chamada sem desconectar, sem derrubar a sala e sem precisar reiniciar o aplicativo.
  - Recriação atômica da stream CPAL de reprodução no canal de voz, preservando o estado do buffer de jitter e a sessão DAVE E2EE.

### 🎨 Padronização de Ícones SVG & Saneamento de Glifos
- **Eliminação de Glifos Quebrados (Quadrados / Tofus)**:
  - Substituição de emojis não-suportados em fontes de botões do Windows por ícones vetoriais SVG nítidos (`microphone.svg`, `headphones.svg`, `rocket.svg`).
  - O botão de teste de microfone agora alterna dinamicamente seu ícone vetorial entre estado parado e testando.
  - Saneamento de caracteres nas abas de idiomas, status de atualizações e mensagens do sistema.

### 🧹 Otimização de Armazenamento & Compilação
- **Limpeza do Ambiente de Desenvolvimento**:
  - Limpeza profunda de artefatos de debug liberando mais de 28 GB de disco no ambiente de compilação.
  - Preservação dos caches otimizados de release para compilações ultra-rápidas.

---

## [v0.3.10] - 2026-09-07

### 🌐 Suporte a Proxy & Roteamento de Rede
- **Proxy HTTP/HTTPS, SOCKS5 e Proxy do Sistema**:
  - Motor nativo em Rust que injeta proxies dinamicamente no `reqwest::Client` (`apply_proxy_to_builder`).
  - Suporte completo a túneis HTTPS `CONNECT` e credenciais de autenticação Basic Auth para proxies corporativos.
  - Opção de roteamento seletivo de mídias (`route_media`) para economia de banda em proxies limitados.
  - Recarregamento a quente (*zero downtime*): salvar configurações de proxy reconstrói o cliente HTTP em tempo real sem fechar o aplicativo.
  - Layout da aba de configurações comprimido e responsivo, eliminando qualquer rolagem horizontal.

### 🛡️ Diagnóstico Inteligente & Proteção de Login
- **Aviso Dinâmico de Falha de Proxy**:
  - Distinção clara entre falhas de rede/proxy (`10061`, recusa TCP, timeout, resolução DNS) e credenciais rejeitadas pelo Discord (`401 Unauthorized`).
  - Verificação proativa de conectividade no início da aplicação em segundo plano.
  - Banner de alerta visual elástico (`min-height: 38px`, `word-wrap`) na tela de login com atalho interativo: um clique no banner abre imediatamente a aba de Proxy para desativá-lo.
  - Limpeza automática do banner de alerta vermelho ao selecionar o modo "Desativado".

### 📱 Experiência na Tela de Login & Barra de Título
- **Acesso Global às Configurações**:
  - Botão com ícone de engrenagem ⚙️ integrado à barra de título personalizada, permitindo acesso irrestrito às configurações mesmo na tela de login.
- **Re-geração de QR Code em 1 Clique**:
  - Botão "Re-gerar QR Code" adicionado à interface para reiniciar sessões de QR Code expiradas sem reiniciar o aplicativo.
  - Card de login expandido para 400px com alinhamento vertical dos botões e ícones.

### 🧪 Ferramentas de Teste
- **Servidor Proxy Local de Testes (`scripts/test_proxy_server.py`)**:
  - Script Python leve, sem dependências externas, para emulação de proxies locais na porta 8080 com suporte a túneis HTTPS do Discord.

---

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
