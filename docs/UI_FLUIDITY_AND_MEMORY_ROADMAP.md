# Litecord — Roadmap de Fluidez de UI e Memória (Auditado)

**Última auditoria:** 2026-11-03  
**HEAD auditado:** `8918b21` (`main`)  
**Binário de produção:** `~/.local/bin/litecord` (release, sem `--features automation`)  
**Backend padrão:** `winit-software` (`use_gpu_renderer: false`) em 2880×1620 (escala 1.5)  
**Escopo permitido:** renderização Slint, modelos de UI, pipeline de eventos do chat/sidebar, caches em RAM/disco e ciclo de vida de sessão.  
**Escopo proibido (INTOCÁVEL):** caminho `LTPV`/transmissão P2P (`src/native_video/*`, `src/screen_capture/*` de captura/codificação), captura de microfone, DSP/AEC (`src/audio/*`), filas PCM e protocolo de voz (`src/gateway/voice.rs`).

---

## 1. Diagnóstico da Auditoria: por que o projeto parecia perder o rumo

Ao auditar o código atual contra a versão original deste documento (de 2026-10-31), ficaram claros quatro problemas de condução:

1. **O documento misturava passado com presente:** as seções 1 a 4 descreviam bugs e limites como se ainda existissem (`MESSAGE_CACHE_CHANNELS = 8`, `MAX_MEMORY_AVATARS = 128`, `EmojiCache` de 64 entradas com `clear()`, modelo de chat sem limite), quando na verdade **F1 a F17 já foram implementados**.
2. **Otimizações especulativas sem medição prévia (Fase 0 pulada):** tentou-se alterar `MAX_CHAT_ROWS`, `guilds_map` ou inventar virtualização manual em Slint sem antes rodar uma única medição controlada de frame time (`SLINT_DEBUG_PERFORMANCE`) ou de alocação.
3. **Risco de regressão funcional por micro-otimizações de baixo retorno:**
   - Tentar virtualizar manualmente `ScrollView` em Slint 1.17 (sem suporte nativo a alturas variáveis em `ListView`) causa saltos de scroll (*scroll jump*) ao carregar imagens/embeds.
   - Podar `guilds_map` em RAM economiza ~100 KB em uma conta de 25 servidores, mas introduz latência de rede (REST round-trip) ao clicar em um servidor. **Não compensa.**
4. **Falta de uma sequência fechada de finalização:** em vez de fechar e validar o que já foi feito, abriram-se novas frentes (`automation.rs`, scripts extras, mudanças em `MAX_CHAT_ROWS`).

---

## 2. Estado Real do Código Hoje (`HEAD 8918b21`)

### 2.1. O que JÁ ESTÁ FEITO e funcionando em `main`

| ID | Área | O que foi feito no código | Status |
| :--- | :--- | :--- | :---: |
| **F1** | Sidebar | Atualização incremental por linha (`set_row_data`) em `apply_channel_activity` e `clear_channel_unread` (`src/ui/helpers.rs`). Não reconstrói mais a sidebar a cada mensagem. | ✅ Concluído |
| **F2** | Chat (Append) | `group_appended_message` (`src/ui/message_grouping.rs`) calcula agrupamento em O(1) olhando apenas a última linha. | ✅ Concluído + Testes |
| **F3** | Chat (Scroll) | `request_chat_scroll_to_bottom` (`src/ui/helpers.rs`) usa 2 timers canceláveis (`80 ms` + `340 ms`) em vez de 7 timers soltos, e respeita se o usuário subiu o histórico. | ✅ Concluído |
| **F4** | Chat (Janela) | Janela deslizante limitada (`MAX_CHAT_ROWS = 150` ao vivo, `MAX_CHAT_ROWS_WITH_HISTORY = 300` paginando histórico) com rebase automático do cursor `OLDEST_MESSAGE_MAP`. | ✅ Concluído + Testes |
| **F5** | Anexos | `get_full_for_ui` nunca lê disco na thread da UI; `hydrate_full_from_disk_async` decodifica em `spawn_blocking` e atualiza com debounce de 120 ms. | ✅ Concluído |
| **F6** | Emojis | `get_cached` na pintura; `hydrate_from_disk_async` e download decodificam em `spawn_blocking`; `queue_emoji_refresh` agrupa atualizações em 60 ms com fast-path sem alocação (`uses_any`). | ✅ Concluído + Testes |
| **F7** | Chat (Edit/Del) | `MessageUpdated` e `MessageDeleted` atualizam/removem apenas a linha afetada e reagrupam a vizinha (`regroup_message_at`). | ✅ Concluído + Testes |
| **F8** | Gateway | `GatewayClient` conhece o canal ativo; `MESSAGE_CREATE`/`MESSAGE_UPDATE` de canais inativos não fazem parsing de markdown/embeds/anexos. | ✅ Concluído + Testes |
| **F9** | Sidebar (Voz) | Loop de 250 ms separa mudança estrutural (entrou/saiu da call) de mudança efêmera (quem está falando), atualizando só a linha `voice-member:*` via `update_voice_member_rows`. | ✅ Concluído |
| **F11** | Memória (Trim) | Removido o loop periódico de 45 s; mantidos apenas os trims orientados a evento (8 s pós-login, minimizar para bandeja, sair de voz, logout). | ✅ Concluído |
| **F12** | HUD Hardware | `hardware_monitor.rs` só envia `invoke_from_event_loop` quando o texto de RAM/CPU realmente muda e dorme quando a janela está oculta na bandeja. | ✅ Concluído |
| **F13** | Animações | Hover/seleção da sidebar e botões reduzidos para 80 ms; sombras de overlays reduzidas (4–12 px, os modais de 24 px caíram para 12 px). | ✅ Concluído |
| **F14** | Sessão / Logout | `clear_session_caches` limpa avatares, emojis, anexos, mensagens, `CACHED_DM_ITEMS`, `SNAPSHOT_GUILDS`, `channel_names`, `invite_cards` e `onboarding`. | ✅ Concluído |
| **F15** | Startup Redraw | Reduzido de 8 redraws para 2 redraws (300 ms e 800 ms) no Linux. | ✅ Concluído |
| **F16** | UX Transições | Transição suave de entrada (`opacity: 0.35 -> 1.0`, 280 ms `ease-out`) ao trocar de canal/servidor e pílulas animadas estilo Discord na guild rail. | ✅ Concluído |
| **F17** | Guild Icons & I/O | Ícones de servidor decodificados fora da thread da UI (`decode_guild_icons` + cache `.rgba` escalado); removido `preload_guild_members` de 1000 membros; `save_user_directory_cache` com debounce de 2 s em `spawn_blocking`. | ✅ Concluído |
| **F18** | Caches em disco/RAM | `prune_disk_emojis` (2048 PNGs, em `spawn_blocking`); `USER_NAMES`/`USER_AVATARS` despejam metade no teto em vez de zerar (preservando o próprio usuário). | ✅ Concluído + Testes |
| **F19** | Fechamento | Teste da invariante de rebase/paginação da janela de 150 linhas; 7 modais com blur 24 px → 12 px; build release medido. | ✅ Concluído |

### 2.2. Orçamentos de Memória Atuais (Reais no Código Hoje)

| Estrutura / Cache | Limite Atual | Arquivo | Estado |
| :--- | :--- | :--- | :--- |
| Framebuffer software (2880×1620) | ≈ 35,6 MiB (fixo) | `winit-software` | Piso físico da resolução em CPU |
| Anexos decodificados (`full_cache` + `UI_IMAGES`) | 12 MiB / 8 MiB (sincronizados) | `src/utils/attachment_cache.rs` | Teto real unificado de ~12 MiB |
| Avatares em RAM (`MemoryStore`) | 256 × 40×40 RGBA (~1,6 MiB) | `src/utils/avatar_cache.rs:17` | LRU com poda em disco (4096 arquivos) |
| Emojis em RAM (`EmojiCache`) | 512 entradas (~4,5 MiB máx) | `src/utils/emoji_cache.rs:27` | LRU em RAM |
| Mensagens em cache (`MESSAGE_CACHE`) | 16 canais × ~50 msgs | `src/ui/data_loader.rs:430` | Compacto em RAM |
| Linhas instanciadas no chat | 150 (vivo) / 300 (histórico) | `src/ui/message_grouping.rs:16,24` | Limitado com rebase de cursor |
| Diretório de usuários (`USER_NAMES` / `USER_AVATARS`) | 8192 nomes / 2048 avatares | `src/gateway/types.rs:763,779` | Bounded |
| Medição real do binário atual (`6bac5e68`) | **~237–250 MiB** (canal c/ imagens) / **~30–41 MiB** (bandeja/DeepSleep) | Processo release | 0 panics |

---

## 3. O que REALMENTE falta fazer (Plano Passo a Passo Fechado)

Para terminar este ciclo com engenharia limpa — **sem inventar complexidade, sem quebrar o scroll e sem tocar em áudio/voz** — restam apenas **4 passos objetivos, com arquivos, linhas, código e comandos de validação exatos**.

---

### Passo 1 — Garantir rebase, reagrupamento da linha 0 e paginação na janela deslizante (`MAX_CHAT_ROWS = 150`)

#### 1.1. Contexto e Objetivo
Em `src/ui/message_grouping.rs:16,24`, a janela de chat está definida com `MAX_CHAT_ROWS = 150` (ao vivo) e `MAX_CHAT_ROWS_WITH_HISTORY = 300` (paginando histórico).
Embora exista o teste básico `trim_chat_window_drops_the_oldest_rows_and_reports_the_new_first_id` (`src/ui/message_grouping.rs:549`), ele testa apenas `trim_chat_window` isolado. Falta testar e blindar a função completa `trim_chat_window_and_rebase` (`src/ui/message_grouping.rs:61-76`), garantindo duas invariantes críticas:
1. **Invariante visual da linha 0:** Se a nova primeira linha (índice 0 após o corte) era uma continuação agrupada da mensagem que foi removida (`show_header == false` ou sem `day_label`), `regroup_message_at(messages, 0)` **obrigatoriamente** precisa promovê-la a cabeçalho (`show_header = true` e `day_label` preenchido), senão o topo do chat fica órfão sem autor nem data.
2. **Invariante do cursor de paginação:** `OLDEST_MESSAGE_MAP` para aquele `channel_id` deve ser atualizado exatamente para o `id` da nova linha 0, ignorando linhas de sistema cujo `id` seja vazio (`""`).

#### 1.2. Arquivos e Linhas Exatas
- `src/ui/message_grouping.rs` (linhas `61-76` e bloco `#[cfg(test)]` a partir da linha `548`).
- `src/main.rs` (linhas `5063-5071` em `MessageCreated` e `5163-5171` em `MessageUpdated`, que já chamam `ui.set_has_more_older_messages(true)` quando `trim_chat_window_and_rebase` retorna `true`).

#### 1.3. Instruções de Implementação
1. Abrir `src/ui/message_grouping.rs` e inspecionar `trim_chat_window_and_rebase` (linhas `61-76`).
2. No módulo `#[cfg(test)] mod tests` no final de `src/ui/message_grouping.rs` (após a linha `563`), adicionar o teste `trim_and_rebase_promotes_new_first_row_to_header_and_updates_cursor`:
   - Criar 4 mensagens consecutivas do mesmo autor (`author_id = "1"`) dentro do mesmo minuto (`2026-09-24T12:00:00Z` a `12:00:03Z`) com IDs `"10"`, `"11"`, `"12"`, `"13"`.
   - Rodar `regroup_messages(&mut rows)` e confirmar que apenas a linha 0 (`"10"`) tem `show_header == true` e `day_label != ""`, enquanto `"11"`, `"12"`, `"13"` têm `show_header == false` e `day_label == ""`.
   - Colocar as linhas num `Rc<VecModel<ChatMessage>>` e `ModelRc`, chamar `trim_chat_window(&vec_model, 2)` + `regroup_message_at(&model, 0)` e atualizar `get_oldest_message_map()`.
   - Verificar com `assert_eq!`:
     - `model.row_count() == 2`;
     - A nova linha 0 tem `id == "12"`, `show_header == true` e `day_label != ""` (foi promovida a cabeçalho);
     - A nova linha 1 (`"13"`) permanece agrupada com `show_header == false` e `day_label == ""`.

#### 1.4. Comando de Verificação e Critério de Aceite
```bash
cargo test --offline --bin litecord message_grouping::tests -- --nocapture
```
- **Aceite:** 100% dos testes de `message_grouping` passando sem warnings novos.

---

### Passo 2 — Poda em disco do cache de emojis (`prune_disk_emojis`) e Evicção Parcial em `USER_NAMES` / `USER_AVATARS`

#### 2.1. Contexto e Objetivo
Há dois pequenos vazamentos de longo prazo (em disco e em RAM):
1. **Disco (`src/utils/emoji_cache.rs`):** O `AvatarCache` (`src/utils/avatar_cache.rs:170-200`) limita o diretório de disco a 4096 arquivos (`prune_disk_avatars`), mas o `EmojiCache` (`~/.cache/Litecord/cache/emojis/*.png`) faz `std::fs::write(&file_path, &bytes)` (linha `288`) e nunca poda arquivos antigos.
2. **RAM (`src/gateway/types.rs`):** `register_user_name` (linhas `763-774`) e `register_user_avatar` (linhas `779-793`) fazem `map.clear()` quando chegam a `8192` / `2048` entradas. Zerar o `HashMap` inteiro de uma vez apaga o nome do próprio usuário logado e dos membros dos canais de voz ativos naquele segundo.

#### 2.2. Arquivos e Linhas Exatas
- `src/utils/emoji_cache.rs` (linhas `18-22`, `286-298` e bloco `#[cfg(test)]` nas linhas `380-420`).
- `src/gateway/types.rs` (linhas `760-795`).

#### 2.3. Instruções de Implementação

**Parte A — `src/utils/emoji_cache.rs`:**
1. Definir a constante de limite de disco logo abaixo de `MAX_MEMORY_EMOJIS` (linha `20`):
   ```rust
   const MAX_DISK_EMOJIS: usize = 2048;
   ```
2. Criar a função `prune_disk_emojis(dir: &std::path::Path, keep: usize)`:
   - Ler `std::fs::read_dir(dir)`.
   - Filtrar apenas arquivos regulares cujo nome termine em `.png`.
   - Coletar `(entry.path(), metadata.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH))`.
   - Se `files.len() > keep`, ordenar por data de modificação crescente (`files.sort_by_key(|(_, modified)| *modified)`) e remover os `files.len() - keep` mais antigos com `std::fs::remove_file`.
3. No método `fetch_priority_async` (linha `288`), mover a escrita `std::fs::write(&file_path, &bytes)` e a chamada `prune_disk_emojis(&disk_dir, MAX_DISK_EMOJIS)` para dentro do `tokio::task::spawn_blocking` que já faz o `decode_bytes_to_rgba`, evitando I/O síncrono no worker async do Tokio.
4. Adicionar um teste unitário `disk_cache_prunes_oldest_png_files_beyond_limit` em `src/utils/emoji_cache.rs` criando um diretório temporário em `std::env::temp_dir()`, gravando 5 arquivos `.png` com timestamps distintos, chamando `prune_disk_emojis(&dir, 3)` e validando que sobraram apenas os 3 mais novos.

**Parte B — `src/gateway/types.rs`:**
1. Em `register_user_name` (linhas `765-774`): quando `map.len() >= MAX_USER_NAMES && !map.contains_key(&user_id)`, em vez de `map.clear()`, preservar o ID do usuário logado (`let my_id = get_my_user_id();`) e remover apenas metade da tabela:
   ```rust
   if map.len() >= MAX_USER_NAMES && !map.contains_key(&user_id) {
       let my_id = get_my_user_id();
       let mut drop_count = MAX_USER_NAMES / 2;
       map.retain(|&k, _| {
           if k == my_id || drop_count == 0 {
               true
           } else {
               drop_count -= 1;
               false
           }
       });
   }
   ```
2. Aplicar exatamente a mesma retenção de metade (`MAX_USER_AVATARS / 2`, preservando `my_id`) em `register_user_avatar` (linhas `785-793`).
3. Adicionar um teste unitário em `src/gateway/types.rs` verificando que ao inserir `MAX_USER_NAMES + 1` entradas o mapa reduz para `~MAX_USER_NAMES / 2 + 1` (não zero) e que `get_my_user_id()` nunca é removido.

#### 2.4. Comando de Verificação e Critério de Aceite
```bash
cargo test --offline --bin litecord emoji_cache::tests -- --nocapture
cargo test --offline --bin litecord gateway::types -- --nocapture
```
- **Aceite:** Testes de poda em disco e retenção parcial de diretório passando; zero I/O de disco fora de `spawn_blocking` no download de emojis.

---

### Passo 3 — Redução de `drop-shadow-blur` nos Modais para o Renderizador por Software (Fechamento de F13)

#### 3.1. Contexto e Objetivo
O renderizador padrão do Litecord é `winit-software` (CPU). Um `drop-shadow-blur: 24px` em um modal grande força o rasterizador por software a calcular um blur Gaussiano de raio 24 px ao redor de todo o perímetro do cartão. Como todos esses modais já possuem `border-width: 1px` e um `Rectangle` de scrim escuro por trás (`AppTheme.surface-scrim` / `AppTheme.backdrop`), reduzir o blur de `24px` para `12px` mantém a separação visual idêntica nos temas Dark, Light e AMOLED, mas reduz a área do kernel de blur em **75%**.

#### 3.2. Arquivos e Linhas Exatas (Todos os 7 pontos do código)
1. `ui/components/settings_view.slint`:
   - **Linha 134:** `drop-shadow-blur: 24px;` → alterar para `drop-shadow-blur: 12px;` (cartão principal da janela de Configurações).
2. `ui/components/onboarding_view.slint`:
   - **Linha 50:** `drop-shadow-blur: 24px;` → alterar para `drop-shadow-blur: 12px;` (cartão do modal de Onboarding de servidor).
3. `ui/appwindow.slint`:
   - **Linha 3882:** `drop-shadow-blur: 24px;` → alterar para `drop-shadow-blur: 12px;` (modal de Troca/Adição de Conta).
   - **Linha 4309:** `drop-shadow-blur: 24px;` → alterar para `drop-shadow-blur: 12px;` (modal de Confirmação de Logout).
   - **Linha 4510:** `drop-shadow-blur: 24px;` → alterar para `drop-shadow-blur: 12px;` (modal de Compartilhamento Web / Link).
   - **Linha 4693:** `drop-shadow-blur: 24px;` → alterar para `drop-shadow-blur: 12px;` (modal de Seleção de Tela / Câmera para Transmissão).
   - **Linha 5697:** `drop-shadow-blur: 24px;` → alterar para `drop-shadow-blur: 12px;` (modal de Atualização Disponível).

#### 3.3. Comando de Verificação e Critério de Aceite
```bash
grep -rn "drop-shadow-blur" ui/
cargo check --offline --bin litecord
```
- **Aceite:** Nenhum `drop-shadow-blur` acima de `12px` em todo o diretório `ui/` e `cargo check` concluindo com código `0`.

---

### Passo 4 — Validação Final, Build de Produção e Fechamento do Ciclo

#### 4.1. Contexto e Objetivo
Garantir que todos os passos compilam juntos sem warnings novos, que a suíte inteira de testes passa, que o binário de produção `~/.local/bin/litecord` está atualizado (sem `--features automation`) e que as métricas reais de memória (RSS e PSS) em janela ativa e em DeepSleep estão documentadas.

#### 4.2. Sequência Exata de Execução
1. **Conferir limpeza da árvore Git:**
   - Garantir que `src/automation.rs` não foi modificado e que não há arquivos não rastreados (`git status --short`).
2. **Rodar toda a suíte de testes:**
   ```bash
   cargo test --offline --bin litecord
   ```
   - Todos os testes (165+ testes) devem passar com `0 failed`.
3. **Compilar o binário Release de produção:**
   - Verificar antes se não há outro `cargo`/`rustc` rodando (`pgrep -x "cargo|rustc"`).
   ```bash
   cargo build --release --offline --bin litecord
   ```
4. **Instalar e reiniciar a instância do usuário:**
   ```bash
   kill $(pgrep -x litecord) 2>/dev/null || true
   cp target/release/litecord ~/.local/bin/litecord
   sha256sum target/release/litecord ~/.local/bin/litecord
   setsid ~/.local/bin/litecord >/tmp/opencode/litecord-final.log 2>&1 &
   ```
5. **Coletar métricas reais pós-login (após o trim de 8 s):**
   - Ler `VmRSS`, `VmHWM` de `/proc/<pid>/status` e `Pss`, `Private_Dirty` de `/proc/<pid>/smaps_rollup`.
   - Verificar que `grep -ciE "panic|panicked" /tmp/opencode/litecord-final.log` retorna `0`.
6. **Atualizar a tabela da Seção 2.1 deste roadmap** marcando `F13`, `F14` e `F17` como 100% concluídos e fazer um único commit final limpo.

---

## 4. O que NÃO faremos (Decisões de Arquitetura para não perder o rumo)

1. **NÃO faremos virtualização manual com spacers no `ScrollView` do Slint 1.17:** sem suporte nativo do motor de layout do Slint a itens de altura variável no `ListView`, qualquer tentativa de trocar linhas fora da tela por retângulos de altura estimada gera *scroll drift* e pulos visuais quando há imagens/embeds. A janela limitada (`MAX_CHAT_ROWS = 150`) + fast-paths sem alocação já resolve o custo de CPU/RAM sem quebrar a rolagem.
2. **NÃO podaremos `guilds_map` em RAM para contas normais:** a lista de `ChannelData` de 20–50 servidores ocupa ~100–250 KB em RAM e permite que o clique em qualquer servidor mostre seus canais em `0 ms`. Descartar isso economizaria kilobytes irrelevantes em troca de atraso visível de rede.
3. **NÃO tocaremos em `LTPV`, `voice.rs`, `audio/*` nem `screen_capture/*`:** o pipeline de voz e streaming está estável e fora do escopo de UI/memória.

---

## 5. Resultado Final (2026-11-03, `HEAD` após este fechamento)

### 5.1. Validação

| Item | Resultado |
|---|---|
| `cargo check --bin litecord` | 0 erros, apenas warnings pré-existentes |
| `cargo test --bin litecord` | **167 passaram**, 0 falharam, 2 ignorados |
| `cargo build --release --bin litecord` | OK |
| Release instalado | `~/.local/bin/litecord` (sha256 `047e9432…`) |
| Smoke test em execução | 0 panics, 0 erros no log, UI renderizando chat, sidebar e anexos |
| Memória (janela aberta, ~45 s pós-login) | RSS **211 MiB**, PSS **188 MiB**, `Private_Dirty` **102 MiB**, `Anonymous` **102 MiB** |

### 5.2. Escopo entregue nesta rodada de fechamento

- **F18:** poda do cache de emojis em disco (2048 PNGs, feita junto do decode em `spawn_blocking`) e despejo parcial (metade) dos diretórios `USER_NAMES`/`USER_AVATARS` em vez de zerá-los — com testes unitários.
- **F19:** teste da invariante de rebase da janela de 150 linhas (a nova primeira linha é promovida a cabeçalho e o cursor de paginação acompanha) e redução dos 7 `drop-shadow-blur` de 24 px para 12 px nos modais.

### 5.3. Métricas que ficam como débito (não bloqueiam o release)

- Perfil de frame time com `SLINT_DEBUG_PERFORMANCE=refresh_lazy,console` e `perf record` durante troca de canal, rajada e scroll — não executado.
- Comparação A/B software vs GPU renderer em 1080p/4K — não executada.
- Teste de 20 trocas de canal e rajada de 100 mensagens com o binário de produção — não executado.

Esses três itens são de **medição**, não de código, e podem ser feitos quando houver um ambiente controlado.
