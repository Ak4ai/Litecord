# Litecord 🚀

![Rust](https://img.shields.io/badge/Language-Rust-orange.svg)
![UI](https://img.shields.io/badge/GUI-Slint-blue.svg)
![Audio](https://img.shields.io/badge/Audio-CPAL%20%7C%20Opus-green.svg)
![License](https://img.shields.io/badge/License-MIT-brightgreen.svg)

Um cliente desktop leve, ultra-rápido e de alto desempenho para o Discord, construído nativamente em **Rust** utilizando **Slint UI** para a interface gráfica, **Tokio** para networking assíncrono e **CPAL** + **Opus** para processamento de áudio em tempo real.

---

## ✨ Principais Funcionalidades

### 🎙️ 1. Salas de Voz & Pipeline de Áudio de Alta Fidelidade
- **Mixer de Áudio de Baixa Latência**: Amostragem de áudio em 48kHz com **Resampler Cúbico Hermite** para garantir reprodução suave sem ruídos ou estalos.
- **WebRTC Jitter Pre-Buffer (60ms)**: Bufferização adaptativa para estabilidade de voz durante variações de rede.
- **Decodificação Opus e Suporte ao Protocolo DAVE (E2EE)**: Filtragem de quadros de controle e decodificação transparente dos pacotes de áudio da voz do Discord.
- **Indicação Dinâmica de Fala (VAD)**: Barra de volume e contorno animado que reage em tempo real quando os participantes falam.

### 👑 2. Prioridade de Fala Personalizada (Speech Ducking Exclusivo)
Uma funcionalidade inovadora que permite definir prioridades de fala para cada participante na sala:
- **Controle de Prioridade na Interface (`[ - ] P:N [ + ]`)**: Ajuste individual para cada participante na chamada.
- **Atenuação Inteligente de Áudio (Speech Ducking)**: Quando múltiplos usuários falam simultaneamente, a voz de falantes com menor prioridade é atenuada progressivamente baseada na diferença de prioridade ($\text{Diferença} = \text{max\_priority} - \text{user\_priority}$):
  - **Diferença = 1** (ex: P1 vs P0): Volume atenuado para **50%**.
  - **Diferença = 2** (ex: P2 vs P0): Volume atenuado para **40%**.
  - **Diferença = 3** (ex: P3 vs P0): Volume atenuado para **30%**.
  - **Diferença = 5+**: Volume atenuado para o limite de proteção de **5% to 10%**.
- **Controle Independente de Volume (0% - 200%) & Mute**: Ajustes de volume individual permanecem independentes e multiplicativos.

### 🏷️ 3. Badges Reativas de Canais de Voz (Antes de Conectar)
- **Contador de Conectados no Servidor**: Badge com o número total de participantes ativos ao lado de cada canal de voz na barra lateral.
- **Carregamento Pré-Conexão**: As badges são populadas e atualizadas em tempo real via `GUILD_CREATE`, `READY_SUPPLEMENTAL` e `VOICE_STATE_UPDATE` do Gateway WebSocket antes de você entrar no canal de voz.

### 👥 4. Resolução 100% de Participantes Mutados & Bots
- **Suporte Total a Bots e Usuários Silenciados**: Identifica e resolve apelidos de servidor (Nicknames), Global Names e usernames para todos os participantes (incluindo bots como Lara, bots de música e usuários mutados).

### 🎨 5. Interface Gráfica Premium & Integração nativa com Windows
- **Barra de Título Escura Nativa (`#111214`)**: Integração via Win32 `DwmSetWindowAttribute` para combinar a cor da barra de título do Windows com o cabeçalho do app.
- **Ícone Personalizado**: Ícone do app configurado em `assets/app_icon.png`.
- **System Tray (Bandeja do Sistema)**: Minimiza e é gerenciado diretamente na bandeja.

---

## 🛠️ Requisitos e Compilação

### Requisitos:
- **Rust** (edição 2021 ou superior)
- **Cargo**

### 📦 Compilar e Executar em Modo de Desenvolvimento:
```bash
cargo run
```

### ⚡ Compilar para Produção (Release Otimizado):
```bash
cargo build --release
```
O executável final otimizado estará disponível em `target/release/litecord.exe`.

---

## 📂 Estrutura do Projeto

```text
Litecord/
├── assets/
│   └── app_icon.png           # Ícone oficial da aplicação
├── src/
│   ├── main.rs                # Ponto de entrada, ciclo de vida e binding de estado Slint UI
│   ├── gateway.rs             # Conexão WebSocket Gateway Discord, Voz CPAL/Opus e Ducking Engine
│   ├── http.rs                # Cliente HTTP REST (Canais, Servidores, Membros)
│   └── tray.rs                # Integração com a Bandeja do Sistema (Windows System Tray)
├── ui/
│   └── appwindow.slint        # Interface do Usuário responsiva Slint UI
├── Cargo.toml                 # Dependências Rust (Slint, Tokio, CPAL, Opus, Serde, Winapi)
└── README.md                  # Documentação do projeto
```

---

## 📄 Licença

Distribuído sob a licença MIT. Veja `LICENSE` para mais informações.
