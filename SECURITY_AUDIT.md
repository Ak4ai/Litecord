# Relatório de Auditoria de Segurança Pré-Release (Security Audit)

**Data**: Setembro de 2026  
**Repositório**: Litecord (`Ak4ai/Litecord`)  
**Branch**: `dev`  
**Escopo**: Análise de vulnerabilidades, superfície de ataque, armazenamento de credenciais, processamento de mídia/rede e recomendações de hardening para versões de produção.

---

## 1. Resumo Executivo

O **Litecord** é um cliente leve e nativo para Discord construído em **Rust** e **Slint GUI**. A auditoria de segurança pré-release avaliou a postura de segurança do aplicativo, com foco em:
1. Armazenamento e manipulação de credenciais / tokens.
2. Superfície de ataque em comparação a clientes baseados em Electron / Web.
3. Tratamento de links externos e esquemas de URI do sistema operacional.
4. Processamento de anexos, imagens e custom emojis (riscos de DoS / OOM / Decompression Bombs).
5. Segurança de rede, áudio e voz em tempo real (RTC / WebRTC / DAVE E2EE).
6. Mecanismo de atualização automática (Self-Updater).
7. Interação com APIs nativas e FFI (Linux X11/Wayland/PulseAudio e Windows Win32/WASAPI/D3D11).

### Status da Auditoria
- **Nível Geral de Risco**: **Muito Baixo**.
- **Vulnerabilidades Críticas / RCE**: Nenhuma encontrada.
- **Mitigações Aplicadas Imediatamente**: Implementadas proteções contra *Decompression Bombs* em caches de imagens/emojis e correção de leitura de buffer no profiler do Windows.
- **Conclusão**: O sistema demonstra excelente postura defensiva e está apto para lançamento de release após as blindagens aplicadas.

---

## 2. Análise Arquitetural e Superfície de Ataque

### 2.1. Rust + Slint vs. Electron / Chromium
A maioria dos clientes alternativos ou o cliente oficial utiliza Electron (Node.js + Chromium). O Litecord elimina por completo essa classe de vulnerabilidades:
- **Ausência de Engine Web/DOM**: Não há renderização de HTML/CSS livre, eliminando vetores de **Cross-Site Scripting (XSS)**.
- **Ausência de Interpretador JavaScript**: Ataques de prototype pollution, `eval()`, code injection via JS ou bridging inseguro de `nodeIntegration` não existem no Litecord.
- **Memory Safety**: O código é compilado com as garantias estritas de segurança de memória e concorrência do compilador Rust (`rustc`), prevenindo *use-after-free*, *double-free* e a grande maioria dos *buffer overflows*.

### 2.2. Gerenciamento e Armazenamento de Tokens
- **Windows**:
  - Os tokens de sessão são cifrados usando a API nativa **Windows DPAPI** (`CryptProtectData`), vinculando as chaves criptográficas à conta do usuário no sistema operacional.
  - Apenas processos rodando sob o mesmo usuário local do Windows conseguem decifrar a sessão.
- **Linux**:
  - Armazenamento em diretório com permissão POSIX estrita `0700` (`~/.config/litecord/`) e arquivo `0600` (`session.vault`).
  - Cifragem autenticada usando **AES-256-GCM**, com derivação de chave via PBKDF2/SHA256 utilizando identificador da máquina (`/etc/machine-id` ou `/var/lib/dbus/machine-id`), UID do usuário e salt estático.
- **Zero-Token Logging**:
  - Filtros em todas as camadas de log e `println!`/`eprintln!` garantem que credenciais e tokens de autenticação jamais sejam gravados em disco (`litecord_app.log`) ou impressos no terminal.
  - Purga atômica no logout: o arquivo de token é deletado e removido de memória ao encerrar a sessão.

---

## 3. Vulnerabilidades Auditadas e Mitigações Implementadas

Durante a auditoria foram identificados e mitigados os seguintes pontos de atenção:

### 3.1. Mitigação de Decompression Bombs em Anexos
- **Arquivo**: `src/attachment_cache.rs`
- **Cenário**: Imagens manipuladas com dimensões desproporcionais (ex: 50.000 x 50.000 pixels com poucos bytes comprimidos) poderiam forçar a alocação de gigabytes de memória RAM durante o decode, provocando *Out of Memory (OOM)* ou congelamento da máquina do usuário.
- **Ação Implementada**: Adicionada verificação prévia de dimensões (`ImageReader::into_dimensions()`) antes da decodificação integral:
  - Limite máximo de **8192px** para largura e altura individual.
  - Limite máximo de **25.000.000 pixels** (25 Megapixels) no produto largura x altura.
  - Arquivos que ultrapassam o limite são rejeitados de forma segura sem sobrecarregar a memória.

### 3.2. Mitigação de Emojis Maliciosos
- **Arquivo**: `src/emoji_cache.rs`
- **Cenário**: Servidores com emojis customizados contendo dimensões infladas poderiam causar degradação severa de performance no chat.
- **Ação Implementada**: Inserida validação que rejeita emojis com largura ou altura superior a **1024px** antes do decode completo na GPU/CPU.

### 3.3. Bounds Check no Leitor de Threads do Windows
- **Arquivo**: `src/cpu_profiler.rs`
- **Cenário**: A função que recupera nomes de threads no Windows (`GetThreadDescription`) utilizava um cast de comprimento sem conferir o limite superior do buffer estático de 256 bytes, possibilitando overread em casos atípicos.
- **Ação Implementada**: Adicionado clamp explícito `len < 256` antes de iterar ou indexar o buffer, garantindo total segurança no processamento de strings nativas.

### 3.4. Restrição de Protocol Handlers em Links Externos
- **Arquivo**: `src/main.rs` (`open_url`)
- **Cenário**: Abertura de links arbitrários via chat poderia acionar handlers perigosos do SO (`file://`, `ms-msdt:`, `powershell:`, `calculator:`, `cmd:`).
- **Status**: Verificado que o Litecord impõe verificação estrita:
  ```rust
  if !url.starts_with("http://") && !url.starts_with("https://") {
      log::warn!("URL recusada por esquema inseguro: {}", url);
      return;
  }
  ```
  Isso bloqueia qualquer tentativa de exploração de handlers do sistema operacional.

---

## 4. Segurança de Comunicações, Voz e Vídeo

1. **Transporte de Dados Discord**:
   - Conexões Gateway e REST utilizam TLS 1.3 / 1.2 com `rustls` ou backend nativo seguro.
   - Comunicação estritamente direta com os servidores oficiais da Discord (`gateway.discord.gg`, `discord.com`). Não há proxies intermediários, analytics nem telemetria externa.
2. **Criptografia de Voz e Transmissão (SRTP & DAVE E2EE)**:
   - Pacotes de mídia são protegidos com `aead_aes256_gcm_rtpsize`.
   - Suporte ao protocolo **DAVE** (Discord Audio/Video End-to-End Encryption) baseado no padrão IETF MLS (Messaging Layer Security - RFC 9420), garantindo que somente os participantes da chamada consigam decodificar os frames de áudio e vídeo.
3. **Captura de Tela e Fallbacks**:
   - Permissões de captura no Linux seguem os portais de segurança do Wayland (`xdg-desktop-portal`) com consentimento explícito do usuário, além do X11 Shared Memory (`xshm`).
   - No Windows, utiliza DirectX Desktop Duplication API (DXGI) ou GDI com isolamento de contexto de dispositivo.

---

## 5. Mecanismo de Atualização (Self-Updater)

- **Validação de Origem**: O instalador e auto-updater conferem a URL de download antes de qualquer requisição. Somente assets provenientes do repositório oficial no GitHub (`https://github.com/Ak4ai/Litecord/releases/download/...`) são aceitos.
- **Integridade de Execução**: Não há execução de scripts externos não assinados ou injeção de comandos via shell para atualização.

---

## 6. Recomendações de Hardening Contínuo (Próximos Passos)

Para versões futuras após o lançamento desta release, recomenda-se:
1. **Isolamento de Diretório Temporário no Linux**:
   - Mudar o download temporário de atualizações de `/tmp` para o diretório de execução exclusivo do usuário (`$XDG_RUNTIME_DIR/litecord` ou via `mkdtemp` com permissões `0700`), prevenindo ataques de corrida de links simbólicos (symlink race) em sistemas multiusuário compartilhados.
2. **Windows DLL Search Order**:
   - Chamar `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32)` logo na inicialização (`main`) no Windows para anular qualquer possibilidade de DLL Side-Loading ou DLL Hijacking caso o executável seja rodado de diretórios de download não confiáveis.
3. **Invocação de Utilitários de Som**:
   - Garantir que todas as chamadas ao `pactl` ou utilitários de áudio passem argumentos como fatias isoladas (`std::process::Command::args`) e nunca através de subshell (`sh -c`).

---

## 7. Conclusão da Auditoria

O Litecord apresenta uma arquitetura sólida, segura e moderna. As proteções implementadas eliminam vetores comuns de negação de serviço e manipulação de memória. O código em `dev` está aprovado e pronto para a preparação de release.
