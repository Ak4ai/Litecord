# 📚 Litecord Documentation & Wiki Hub

Bem-vindo à central de documentação e engenharia do **Litecord**!

Este diretório contém as especificações técnicas, planos arquiteturais, relatórios de auditoria e diretrizes de desenvolvimento da plataforma.

---

## 📑 Índice de Documentação

### 🚀 Arquitetura & Pipelines de Alta Performance
* **[P2P Video Streaming Architecture](P2P_VIDEO_STREAMING_PLAN.md)**:
  * Diagnóstico do sistema de transmissão Full HD (1080p @ 60 FPS).
  * Protocolo de sinalização via Gateway Discord e NAT Hole Punching (STUN RFC 5389 / Google / Cloudflare).
  * Fragmentação de NAL Units RFC 6184 para H.264 sobre UDP P2P de baixa latência.
  * Pipeline de aceleração por GPU (NVENC, AMD AMF, Intel/Windows Media Foundation MFT e fallback FFmpeg).

### 🛡️ Segurança, Criptografia & Auditorias
* **[Relatório de Auditoria de Segurança](SECURITY_AUDIT.md)**:
  * Análise aprofundada de superfície de ataque e modelos de ameaça.
  * Criptografia E2EE (Discord DAVE Protocol & MLS - Messaging Layer Security).
  * Armazenamento seguro de tokens com criptografia DPAPI no Windows.
  * Validação de memória e rotinas seguras em Rust nativo.
* **[Relatório de Auditoria Técnica de Código](CODE_AUDIT_REPORT.md)**:
  * Diagnóstico de concorrência, mapeamento de mutexes e mitigação de *mutex poisoning*.
  * Gerenciamento de ciclo de vida de threads e isolamento dinâmico de bibliotecas gráficas.
  * Otimizações de pipeline de áudio WASAPI e VAD de baixa latência.

### 👥 Governança & Comunidade
* **[Security Policy](../SECURITY.md)**: Diretrizes de reporte de vulnerabilidades e suporte de segurança.
* **[Contributing Guidelines](../CONTRIBUTING.md)**: Guia de desenvolvimento, padrões de código e submissão de Pull Requests.
* **[Code of Conduct](../CODE_OF_CONDUCT.md)**: Código de conduta para colaboradores e membros da comunidade.
* **[Changelog](../CHANGELOG.md)**: Histórico completo de versões, lançamentos e notas de atualização.

---

## 🏗️ Estrutura do Repositório

`	ext
Litecord/
├── src/                      # Código-fonte Rust nativo
│   ├── encoder/              # Motores de aceleração por hardware (NVENC, AMF, WMF, FFmpeg)
│   ├── gateway/              # Cliente Gateway Discord (WebSocket, REST, DAVE E2EE)
│   ├── p2p/                  # Protocolo P2P de vídeo/áudio e STUN NAT Hole Punching
│   ├── screen_capture/       # Captura de tela DXGI Desktop Duplication & GDI
│   ├── voice/                # Pipeline de áudio WASAPI, Loopback e Opus codec
│   └── main.rs               # Ponto de entrada e orquestração do Slint UI
├── ui/                       # Interface gráfica nativa Slint (GPU-accelerated)
│   └── appwindow.slint       # Design da UI, abas, sala de vídeo e componentes visuais
├── assets/                   # Ícones vetoriais SVG, assets gráficos e fontes
├── docs/                     # Documentação técnica e artigos da Wiki
└── Cargo.toml                # Dependências e manifesto do projeto
`
