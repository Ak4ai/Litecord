<div align="center">

# Litecord 🚀
### Cliente Nativo do Discord Ultra-Leve e de Alta Performance para Gamers

[![Website](https://img.shields.io/badge/Website-ak4ai.github.io%2FLitecord-5865F2.svg?style=flat-square&logo=googlechrome&logoColor=white)](https://ak4ai.github.io/Litecord/)
[![GitHub Release](https://img.shields.io/github/v/release/Ak4ai/Litecord?style=flat-square&color=blueviolet)](https://github.com/Ak4ai/Litecord/releases)
[![CI Verification](https://github.com/Ak4ai/Litecord/actions/workflows/ci.yml/badge.svg)](https://github.com/Ak4ai/Litecord/actions/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/Linguagem-Rust_2021-orange.svg?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![GUI](https://img.shields.io/badge/GUI-Slint_1.9-blue.svg?style=flat-square)](https://slint.dev/)
[![Segurança](https://img.shields.io/badge/Seguran%C3%A7a-AES--256--GCM_%7C_DPAPI_%7C_DAVE_E2EE-blueviolet.svg?style=flat-square)](SECURITY.md)
[![Licença: MIT](https://img.shields.io/badge/Licen%C3%A7a-MIT-yellow.svg?style=flat-square)](LICENSE)

<p align="center">
  <b>Litecord</b> é um cliente desktop nativo para Discord criado do zero em <b>Rust</b> e <b>Slint</b> para <b>gamers, streamers e jogadores competitivos</b>. Rodando com <b>&lt; 0.1% de CPU</b> e <b>~32 MB de RAM</b>, ele elimina micro-travamentos e não perde nenhum FPS enquanto entrega <b>Transmissão Full HD 1080p 60 FPS</b>, <b>Prioridade de Fala / Ducking Inteligente</b>, <b>Janelas Flutuantes (PiP)</b>, <b>Login via QR Code</b> e <b>Comandos Slash Inteligentes</b>.
</p>

[🌐 Site Oficial](https://ak4ai.github.io/Litecord/) • [📦 Downloads](#-downloads--instalação) • [🎮 Recursos](#-por-que-gamers-usam-o-litecord) • [⚡ Benchmarks](#-benchmarks-vs-discord-oficial) • [🛡️ Segurança](#-segurança-e-privacidade) • [🛠️ Compilar do Código](#-compilando-do-código-fonte)

<br/>

<img src="assets/demo_preview.gif" alt="Interface Nativa do Litecord" width="760px" style="border-radius: 8px; box-shadow: 0 10px 30px rgba(0,0,0,0.5);" />

</div>

---

## 🎮 Por que Gamers usam o Litecord

- 🏆 **Zero Quedas de FPS e Sem Micro-Gaguejos**: Libera os núcleos da CPU antes devorados pelo Chromium/Electron, elevando o 1% low FPS em jogos competitivos (*CS2, Valorant, Warzone, Apex Legends, Fortnite, League of Legends*).
- 📺 **Transmissão Full HD 1080p 60 FPS**: Captura direta de hardware (DXGI / Direct3D11) com áudio in-game por loopback WASAPI e latência sub-20ms.
- 👑 **Prioridade de Fala e Ducking para Capitães/IGL**: Defina prioridades (`[ - ] P:2 [ + ]`) para que chamadas táticas em momentos clutch atenuem automaticamente o áudio de bots de música e conversa de fundo.
- 🪟 **Janela Flutuante Desanexada (PiP)**: Desanexe transmissões em uma janela flutuante com pin para fixar no topo, controles responsivos e modo fantasma clique-através.
- 📱 **Login Rápido por QR Code**: Faça login escaneando o QR Code direto pelo aplicativo do Discord no celular—sem precisar extrair token manualmente.
- 🔒 **Cofres Criptografados de Sessão**: Credenciais protegidas por **AES-256-GCM** atrelado ao hardware no Linux (`~/.config/litecord/session.vault`) e **DPAPI** no Windows.
- ⌨️ **Comandos Slash Inteligentes**: Autocomplete em tempo real para bots (`/play`, `/skip`, etc.) com navegação por teclado e chips interativos.
- ⚡ **Modo DeepSleep (Sub-5 MB RAM)**: Reduz o consumo de RAM para **~3 MB a 5 MB** quando minimizado na bandeja.
- 🎙️ **Opus PLC (Ocultação de Perda de Pacotes)**: Evita vozes robóticas e estalos mesmo sob 100% de uso de CPU/GPU.
- 🌐 **7 Idiomas Nativos**: Detecção automática do idioma do sistema (Português, Inglês, Espanhol, Alemão, Francês, Russo e Japonês).

---

## ⚡ Benchmarks vs Discord Oficial

| Métrica | Discord Oficial (Electron) | **Litecord (Nativo Rust + Slint)** | Impacto nos Jogos |
| :--- | :--- | :--- | :--- |
| **Uso de CPU em Espera** | 1.5% - 4.5% | **0.00% - 0.02% (DeepSleep: 0.0%)** | 🚀 **150x mais leve em CPU** |
| **CPU em Canal de Voz** | 4.0% - 8.0% | **~0.1% - 0.3%** | ⚡ **Zero travamentos no jogo** |
| **CPU em Stream 1080p 60 FPS** | 8.0% - 16.0% | **~0.8% - 1.4%** | 🎮 **Gameplay 100% fluida** |
| **Uso de RAM (Bandeja/DeepSleep)** | 350 MB - 750 MB | **~3 MB - 5 MB** | 🌙 **99% mais leve em segundo plano** |
| **Uso de RAM (Janela Aberta)** | 500 MB - 900 MB | **~12 MB - 28 MB** | 💾 **Economiza até 850 MB de RAM** |
| **Tempo de Inicialização** | 4.5s - 9.0s | **< 150 ms** | ⏱️ **Abertura instantânea** |
| **Tamanho do Executável** | ~180 MB | **~8 MB Standalone** | 📦 **Código de máquina nativo puro** |

---

## 📦 Downloads & Instalação

### 🪟 Windows (10 / 11 x64)
- **Instalador Oficial**: Baixe o `Litecord-Setup-x64.exe` na [Página de Releases](https://github.com/Ak4ai/Litecord/releases).
- **Versão Portátil**: Baixe o `.zip`, extraia e execute `litecord.exe` direto.

### 🐧 Linux (Instalação em 1 Comando)
Cole no terminal:
```bash
curl -sSL https://raw.githubusercontent.com/Ak4ai/Litecord/main/install.sh | bash
```

---

## 🛠️ Compilando do Código-Fonte

```bash
# Clone o repositório
git clone https://github.com/Ak4ai/Litecord.git
cd Litecord

# Modo de desenvolvimento
cargo run

# Compilar versão final ultra-otimizada
cargo build --release --bin litecord
```

---

## 📄 Licença

Distribuído sob a licença **MIT**. Veja o arquivo [`LICENSE`](LICENSE) para mais detalhes.
