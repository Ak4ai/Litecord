# Changelog

All notable changes to **Litecord** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [v1.0.0-beta] - 2026-09-08 — Litecord Beta 1.0.0

Esta versão marca a **transição oficial do ciclo Alfa (v0.1.0 – v0.3.9) para a fase Beta 1.0.0**, consolidando uma reestruturação profunda da arquitetura do aplicativo, um novo motor de streaming de tela por hardware de latência zero (padrão Sunshine / Moonlight), isolamento de áudio de processos, suporte nativo a proxies, atalhos globais e monitor de consumo real em tempo real.

---

### 🚀 Arquitetura Modular & Motor de Vídeo por Hardware (Padrão Sunshine / Moonlight)
- **Reestruturação Arquitetural em Módulos de Domínio**:
  - Código-fonte completamente modularizado a partir de módulos monolíticos para diretórios de domínio coesos: `src/audio/`, `src/auth/`, `src/encoder/`, `src/gateway/`, `src/screen_capture/`, `src/ui/` e `src/utils/`.
  - Bibliotecas de runtime do FFmpeg/NVENC embutidas diretamente para compilação autônoma e execução sem dependências externas instaladas.
- **Aceleração Universal de Vídeo por GPU**:
  - Motores de codificação nativos por hardware: **NVIDIA NVENC** (via Direct3D 11 e bibliotecas dedicadas), **AMD AMF** (com sintonia de baixa latência), **Intel QuickSync (QSV)**, **Windows Media Foundation (WMF)** e fallback otimizado via **OpenH264 SIMD AVX2**.
  - **Sintonia de Latência Zero (Padrão Sunshine / OBS)**:
    - Configuração de perfil *Constrained Baseline* com `max_b_frames = 0`, `filler_data = 0` e desativação de AUDs.
    - Intervalo de quadro-chave (IDR/GOP) ajustado para 30 quadros (500ms a 1s), eliminando artefatos de imagem acumulados.
    - Normalização estrita de NALs Annex B com extração em duas pontas, cache persistente de cabeçalhos SPS/PPS e injeção atômica **exclusivamente em quadros IDR** (preservando a integridade das referências DPB em quadros P).
    - Mecanismo instantâneo de recuperação via Picture Loss Indication (PLI) e Full Intra Request (FIR) através de `OP_KEYFRAME_REQ` roteado aos pares ativos.
    - Decodificador de vídeo tolerante a falhas que sobrevive a oscilações de rede e quedas de quadros sem travamentos de tela ou reinicializações destrutivas.
    - Pacer contínuo de 60 FPS (CFR - *Constant Frame Rate*) e taxa de bits dinâmica adaptativa (padrão 4.5 Mbps com micro-pacing).
    - Conversão paralela de espaço de cor BGRA para NV12 vetorizada com Rayon/SIMD.

---

### 🌐 Streaming P2P WAN de Alta Performance & Nova Sinalização
- **Migração para Sinalização Cloudflare Worker WebSocket**:
  - Substituição da sinalização MQTT tradicional por WebSockets de ultra-baixa latência em Cloudflare Workers, eliminando sobrecarga de brokers, desconexões e *ghost streams*.
- **Descoberta Dinâmica de WAN via STUN & Conectividade P2P**:
  - Descoberta precisa de endereços públicos e portas mapeadas por NAT via STUN dinâmico, eliminando adivinhação de portas e sobreposições indevidas.
  - Enforce de MTU seguro em 1200 bytes por pacote UDP para evitar fragmentação em roteadores WAN e VPNs.
  - Filtragem proativa de adaptadores de rede virtuais (VPN, Hyper-V, WSL) e descarte de pacotes de loopback com o próprio UID no receptor P2P mesh.
  - Criptografia autenticada ponta a ponta (E2EE) com chaves efêmeras X25519 ECDH + AES-256-GCM gerando sobrecarga inferior a 0.07ms por quadro.

---

### 🔊 Subsistema de Áudio Avançado & Isolamento de Som (Loopback)
- **Isolamento de Áudio de Compartilhamento no Windows (WASAPI Process Loopback)**:
  - Captura nativa de áudio de jogos e janelas compartilhadas isolando processos específicos via WASAPI Process Loopback: transmite o som do jogo/vídeo sem capturar a chamada de voz ou o próprio microfone (zero eco).
- **Roteamento de Áudio no Linux (PipeWire / ALSA Virtual Sink)**:
  - Isolamento de áudio do sistema desktop em transmissões de tela via criação de virtual sink dinâmico, separando o monitor de áudio da reprodução de voz do aplicativo.
  - Roteamento direto dos fluxos ALSA do Litecord para a saída física (`PIPEWIRE_NODE`).
- **Troca Dinâmica de Alto-falante em Chamada**:
  - Alterne entre fones de ouvido, caixas de som e DACs USB no meio de uma chamada sem desconectar, sem derrubar a sala e sem precisar reiniciar o aplicativo.
  - Recriação atômica da stream CPAL de reprodução no canal de voz, preservando o estado do buffer de jitter e a sessão DAVE E2EE.
- **Ganho de Microfone & Calibração de Sensibilidade**:
  - Controle deslizante de ganho de software de microfone em tempo real ajustável de 0% a 200%.
  - Reorganização intuitiva da aba de Voz e Áudio nas configurações com monitor VU ao vivo e sensibilidade VAD.
- **Feedback Sonoro Tátil (Sound Effects)**:
  - Motor de síntese sonora tátil (`sound_effects.rs`) operando em thread dedicada de baixa latência via CPAL (com fallback nativo no Windows):
    - **Mute**: bipe harmônico descendente (440 Hz → 220 Hz) confirmando silenciamento.
    - **Unmute**: bipe harmônico ascendente (330 Hz → 660 Hz) confirmando reativação do microfone.
    - **Deafen / Undeafen**: sequências sonoras dedicadas para ensurdecimento e restauração de áudio.
    - **Entrada e Saída de Chamada**: alertas sonoros ao ingressar ou deixar salas de voz.

---

### ⌨️ Atalhos Globais de Teclado (Global Keybinds)
- **Captura Global com Foco em Jogos (Win32 / Linux evdev)**:
  - Módulo `src/utils/keybinds.rs` com listener em segundo plano funcional no Windows (hooks Win32 e `GetAsyncKeyState`) e no Linux (`evdev`), respondendo mesmo com jogos rodando em tela cheia exclusiva.
  - Atalhos padrão: **`Ctrl + Shift + M`** (Alternar Microfone) e **`Ctrl + Shift + D`** (Alternar Áudio).
  - Gravador interativo de atalhos e persistência de combinações personalizadas em `.litecord_keybinds.json`.
  - Sincronização em tempo real entre o estado da UI, confirmação sonora e despacho para o Discord Voice Gateway (`GatewayCommand::UpdateVoiceState`).

---

### 🌐 Suporte Nativo a Proxy HTTP/HTTPS, SOCKS5 & Sistema
- **Motor de Rede com Suporte a Múltiplos Protocolos**:
  - Injeção dinâmica de proxy em Rust no `reqwest::Client` com suporte a **HTTP/HTTPS** (túnel `CONNECT`), **SOCKS5** (compatível com Shadowsocks, V2Ray, Xray, Tor) e **Proxy do Sistema**.
  - Autenticação Basic Auth (usuário e senha) para proxies corporativos e privados.
  - Botão **"Testar Conexão"** integrado nas configurações com verificação em tempo real de latência e conectividade ao endpoint de gateway do Discord (`discord.com/api/v10/gateway`).
  - Recarregamento a quente (*zero downtime*): salvar configurações reconstrói o cliente HTTP instantaneamente sem fechar a aplicação.
- **Diagnóstico Inteligente de Conexão na Tela de Login**:
  - Banner elástico de aviso de falha de proxy com diferenciação clara entre falhas de rede (`10061`, recusa de conexão, DNS) e rejeição de credenciais (`401 Unauthorized`).
  - Atalho de 1 clique no banner direcionando diretamente à aba de Rede & Proxy nas configurações.

---

### 📊 Monitor de Hardware em Tempo Real & HUD Compacto
- **Métricas Fidedignas do Processo (Consumo Real do Litecord)**:
  - **CPU**: Coleta o tempo de kernel e usuário do processo do Litecord via `GetProcessTimes` em relação ao tempo total decorrido do sistema (`GetSystemTimes`), correspondendo fielmente ao cálculo do Gerenciador de Tarefas do Windows.
  - **RAM (em MB)**: Coleta do Working Set real consumido pelo Litecord via `K32GetProcessMemoryInfo` (retorna em MB, ex.: `42 MB`), sem porcentagem global confusa.
  - **GPU**: Coleta via PDH (`\GPU Engine(*)\Utilization Percentage`) filtrando estritamente pelas instâncias de engines pertencentes ao PID atual (`pid_<PID>_...`), isolando o uso real do app.
- **Integração com Barra de Tarefas e Bandeja**:
  - Título dinâmico e compacto na Barra de Tarefas do Windows: `Litecord - x%/ymb` (ex: `Litecord - 2%/45MB`), sem cortar na barra de tarefas.
  - Tooltip vivo na bandeja do sistema (System Tray): `Litecord - CPU x% | RAM y MB | GPU z%`.
  - HUD minimalista e elegante integrado à barra superior ao lado do ícone de configurações, com suporte a toggle liga/desliga nas configurações.

---

### 🧹 Gerenciamento Avançado de Memória & Paridade Linux
- **Rotina Multiplataforma `trim_process_memory()`**:
  - No Windows: esvaziamento do working set via `K32EmptyWorkingSet()`, derrubando o uso de RAM para menos de 5 MB ao minimizar para a bandeja ou entrar em repouso.
  - No Linux: chamadas a `malloc_trim(0)` e `mi_collect(true)` para forçar a devolução imediata de páginas de memória desfragmentadas do heap à `glibc`.
  - Desativação de timers de segundo plano no Slint ao fechar janelas popout e minimizar, permitindo que a aplicação hiberne no `epoll_wait()`.

---

### 📹 Pipeline de Câmera em Thread Isolada
- **Inicialização Segura de Webcam**:
  - Dispositivos de câmera inicializados em worker thread isolada com contexto COM próprio, prevenindo colisões de modelo de apartamento com a thread principal.
  - Dimensionamento dinâmico de buffer para o canvas de exibição, timeouts seguros e prevenção de fallbacks indevidos de captura GDI durante o streaming de vídeo.

---

### 🎨 Design, Ícones Vetoriais SVG & Identidade Cyber Sapphire
- **Padronização de Ícones Vetoriais SVG**:
  - Substituição de emojis e caracteres unicode propensos a falhas de renderização (tofus/quadrados vazios) por ícones vetoriais SVG nítidos (`microphone.svg`, `headphones.svg`, `rocket.svg`, `chevron-down.svg`, etc.).
  - Botão de teste de microfone com alternância visual animada entre estado ocioso e testando.
- **Aprimoramentos de Usabilidade & Estilo**:
  - Paleta **Cyber Sapphire** com badges no estilo squircle clássico do Discord e aviso de preview econômico compacto (20px) com truncamento inteligente (*elide*).
  - Rolagem automática do chat para o final ao alternar salas de voz ou canais de texto.
  - Botão de acesso direto às Configurações presente na barra de título em todas as telas (incluindo Login).
  - Botão **"Re-gerar QR Code"** de 1 clique para reiniciar autenticações móveis expiradas sem fechar o app.

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
