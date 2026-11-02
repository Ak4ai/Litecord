# Litecord — Roadmap de fluidez de UI e memória

**Data:** 2026-10-31  
**Base analisada:** `build/unified-local-release` em `1bdf120`  
**Binário medido:** `~/.local/bin/litecord` (release, `1bdf120`, sem `--features automation`)  
**Backend:** `winit-software` (padrão; `use_gpu_renderer: false`)  
**Janela:** maximizada em 2880×1620, tema escuro, HUD de hardware ligado  
**Escopo:** renderização, modelos Slint, pipeline de eventos, caches e ciclo de vida de memória.  
**Fora de escopo:** caminho `LTPV`, captura de microfone, DSP/AEC, filas de áudio e protocolo de voz. Esses caminhos não devem ser alterados por este roadmap.

> **Aviso sobre o working tree:** no momento da análise existiam mudanças não commitadas de outro agente em `src/main.rs`, `src/gateway/client.rs`, `src/native_video/*`, `src/screen_capture/*` e `ui/*.slint`. Os números de linha abaixo são do working tree em 2026-10-31; o diff não commitado desloca algumas linhas. O roadmap não altera nem reverte essas mudanças.

---

## 1. Resumo executivo

A fluidez do Litecord não está presa a um único gargalo. O padrão que aparece no código é **trabalho repetido e proporcional ao tamanho do histórico**, em vez de trabalho proporcional ao que mudou:

1. **A sidebar inteira é reconstruída** para atualizar o badge de não lidas de uma única linha, tanto ao receber mensagem (`src/ui/helpers.rs:180-198`, chamado em `src/main.rs:4815`) quanto ao clicar em um canal (`src/main.rs:4078-4084`).
2. **Cada mensagem nova copia e reagrupa todo o modelo de chat** (`src/main.rs:4861,4896-4900`), embora um append só possa mudar o agrupamento da última linha.
3. **Cada mensagem/entrada de canal agenda 7 scrolls extras** (`src/ui/helpers.rs:83-97`), sem verificar se o usuário está no fim; o estado `chat_is_at_bottom` existe mas não é usado para decidir (`ui/appwindow.slint:347,2979-2981`).
4. **A lista de chat não é virtualizada**: é um `ScrollView` com `VerticalLayout` sobre todo o modelo (`ui/components/chat_messages_view.slint:132,194`). O custo de layout cresce com todo o histórico carregado, e cada substituição do modelo recria as linhas.
5. **Imagens de anexo e emojis são lidas/decodificadas de forma síncrona na thread da UI** (`src/utils/attachment_cache.rs:278-320`, `src/utils/emoji_cache.rs:81-110`). Em canal com imagens isso aparece como travamento na primeira pintura ou ao voltar ao canal depois que o cache de 12 MB foi evictado.
6. **O gateway formata todas as mensagens**, inclusive de canais que o usuário não está vendo (`src/gateway/client.rs:1759-1825`). Em conta grande isso gera CPU e alocação desnecessários, que competem com a UI.

Na memória, o piso é o framebuffer do renderizador por software:

- 2880×1620×4 bytes = 17,8 MiB por buffer;
- com double buffering: **≈35,6 MiB** só de framebuffer;
- o restante é binário/bibliotecas, cena Slint, glifos, caches de imagem/avatar/emoji e heap retido pelo mimalloc.

Medição de uma inicialização limpa (sem voz, até 44 s; após esse ponto houve interação/entrada em voz no desktop e os dados foram descartados):

| Momento | RSS | PSS | Observação |
|---|---:|---:|---|
| t≈1 s | 135,8 MiB | 98,2 MiB | janela abrindo |
| t≈4 s | 172,6 MiB (pico HWM 180,1 MiB) | 134,6 MiB | READY + primeira pintura + ícones |
| t≈8 s | 173,7 MiB | 135,5 MiB | ainda caindo |
| t≈15 s | 169,3 MiB | 131,1 MiB | platô |
| t=20–44 s | 169,3 MiB | 131,3 MiB | ocioso, sem crescimento |

O `trim_process_memory()` de 8 s após o login (`src/main.rs:4754-4760`) coincidiu com uma variação de **≈0,5 MiB** entre amostras, não com uma queda grande. Ou seja: o “começa alto e cai” observado pelo usuário **não se reproduziu como queda de dezenas de MB** nesta execução Linux com renderizador por software. As hipóteses para o que ele viu estão na seção 4.3.

### Prioridades recomendadas

| Prioridade | Item | Tipo | Esforço | Impacto esperado |
|---|---|---|---|---|
| P0 | Atualização incremental da sidebar | quick win | S | Alto — elimina rebuild em rajada de mensagens |
| P0 | Agrupamento só da mensagem nova | quick win | S | Alto — remove O(n) por mensagem |
| P0 | Scroll pin debounced e condicional | quick win | S | Alto — remove 7 relayouts por mensagem e o “puxão” |
| P0 | Decodificação assíncrona de anexos/emojis | quick win/estrutural | M | Alto — primeira pintura de canal com imagem |
| P1 | Janela de mensagens / `ListView` virtualizado | estrutural | L | Alto — layout e memória deixam de crescer com o histórico |
| P1 | Não formatar mensagem de canal inativo no gateway | estrutural | M | Médio-alto em contas grandes |
| P1 | Limpeza de caches no logout/troca de conta | quick win | S–M | Médio — memória e privacidade |
| P2 | Política de animações/sombras no backend software | ajuste visual | S–M | Médio — hover/seleção mais leve em 4K |
| P2 | Remover/medir o trim periódico de 45 s | memória | S | Baixo/médio — evita pausas e dente de serra no RSS |

---

## 2. Como a UI funciona hoje (contexto)

### 2.1 Renderização

- O backend é escolhido antes de criar a janela: `SLINT_BACKEND=winit-software` quando o usuário não ativou GPU (`src/main.rs:119-124`). No Linux medido, `use_gpu_renderer: false`.
- O renderizador por software **tem renderização parcial** (regiões sujas), então um hover pequeno não repinta os 4,7 milhões de pixels. O problema não é “repintar tudo sempre”; é:
  - o framebuffer completo existe de qualquer forma (≈35,6 MiB);
  - substituições de modelo invalidam áreas grandes;
  - animações e `drop-shadow` custam rasterização na CPU;
  - qualquer trabalho síncrono na thread da UI atrasa o próximo frame.
- A UI tem 35 blocos `animate` no total: 20 em `ui/appwindow.slint`, 13 em `ui/components/channel_sidebar.slint`, 2 em `ui/components/settings_kit.slint`. Há 26 usos de `drop-shadow` e 29 `clip: true` em `ui/`.

### 2.2 Fluxo de dados

```
Gateway WebSocket
  → gateway/client.rs formata a mensagem (markdown, emoji, links, embeds, botões)
  → mpsc GatewayEvent
  → main.rs: loop de eventos
      → se canal ativo: invoke_from_event_loop(closure que monta ChatMessage e mexe no modelo)
      → se outro canal: atualiza badge da sidebar
```

O ponto importante: **a formatação acontece antes de saber se o canal está ativo**, e a parte de UI acontece inteira na thread do event loop do Slint (layout, criação de modelos, hidratação de avatar/invite, decodificação de imagem).

### 2.3 Cache de mensagens

- `MESSAGE_CACHE_CHANNELS = 8`, `MESSAGE_CACHE_FRESH_AFTER = 15 s` (`src/ui/data_loader.rs:285-287`).
- Ao reabrir um canal, o lote compacto é pintado imediatamente (`src/ui/data_loader.rs:1179-1200`); o REST só substitui se algo mudou.
- Isso já melhorou bastante a troca de canal. O que ainda atrapalha é o custo de **montar** esse lote: agrupamento, hidratação de avatares/invites e decodificação síncrona de anexos/emojis.

---

## 3. Achados detalhados

### F1 — Sidebar inteira é reconstruída para atualizar uma linha (Alta)

**Sintoma:** com o app visível, cada mensagem recebida em outro canal (e cada clique em canal de texto) pode causar um “tremidinho” na lista de canais; em servidor movimentado, a sidebar parece menos responsiva que o chat.

**Evidência:**

- `src/ui/helpers.rs:180-198` — `apply_channel_activity` copia todas as `ChannelItem` para um `Vec`, altera uma ou duas e chama `ui.set_channels(VecModel::from(channels))`.
- Chamada por mensagem: `src/main.rs:4815` dentro do closure `sidebar(1)`.
- Chamada ao clicar em canal de texto: `src/main.rs:4078-4084` (copia todas as linhas só para zerar o badge do canal aberto).
- O próprio arquivo já tem o padrão correto em `update_channel_voice_rows` (`src/ui/helpers.rs:203-239`): se as linhas são as mesmas, usa `set_row_data` apenas nas que mudaram.

**Por que custa:** substituir o modelo faz o Slint recriar as linhas visíveis da sidebar, reavaliar expressões e reiniciar animações/hover. O custo é O(nº de canais + membros de voz) por mensagem, não O(1). Em um servidor com 100+ canais e uma rajada de mensagens, isso vira uma sequência de rebuilds completos.

**Correção:** manter o `VecModel` atual e usar `set_row_data` para o canal afetado. Para DMs, mover a linha só quando a ordenação realmente mudar. Se a linha não existir (canal novo), aí sim reconstruir. Reutilizar a lógica de `update_channel_voice_rows`, preservando `unread_count` e a ordem atual.

**Risco:** baixo. Precisa preservar a semântica de não lidas, ordenação de DMs e o caso de canal que não está na lista visível (categoria colapsada).

**Ganho esperado:** elimina um rebuild completo por mensagem; medir com 20–50 mensagens/s em um servidor grande.

---

### F2 — Append de mensagem copia e reagrupa o modelo inteiro (Alta)

**Sintoma:** o chat fica progressivamente mais pesado conforme o usuário carrega histórico; cada mensagem nova tem um custo que cresce com o número de mensagens já na tela.

**Evidência:**

- `src/main.rs:4861` — `let mut current_msgs: Vec<ChatMessage> = ui.get_messages().iter().collect();`
- `src/main.rs:4896-4900` — `current_msgs.push(message); regroup_messages(&mut current_msgs); let mut appended = current_msgs.pop()...`
- `regroup_messages` (`src/ui/message_grouping.rs:82-129`) percorre todas as mensagens e faz `parse_local_time` (chrono) em cada uma para recalcular `day_label` e `show_header`.

**Por que custa:** para uma mensagem nova, o código copia o vetor inteiro (strings e `ModelRc` são clonados por referência, mas o vetor e os structs são refeitos) e reparseia o timestamp de todas as mensagens. O comentário no próprio código diz “um append nunca muda o agrupamento das linhas antigas”, mas a implementação ainda faz o trabalho O(n).

**Correção:** calcular `day_label`/`show_header` da mensagem nova a partir da última mensagem existente (autor, timestamp, dia) e dar `model.push(appended)`. Só cair no caminho completo se o modelo não for um `VecModel` ou se a última linha for inválida.

**Risco:** baixo; testar virada de dia, mesma janela de 5 minutos (`GROUP_WINDOW_SECONDS`), mensagens de sistema e mensagens sem timestamp.

**Ganho esperado:** append passa de O(n) para O(1); o ganho cresce com o histórico carregado.

---

### F3 — Scroll pin dispara 7 timers por mensagem e ignora se o usuário subiu (Alta)

**Sintoma:** ao receber mensagens, o chat “puxa” para o fim mesmo se o usuário estiver lendo o histórico; em rajada, há relayouts repetidos por até 1,5 s.

**Evidência:**

- `src/ui/helpers.rs:83-97` — `request_chat_scroll_to_bottom` agenda `15, 45, 100, 220, 500, 900, 1500 ms`, sem cancelar os anteriores.
- É chamado em append (`src/main.rs:4911`), troca de canal (`src/main.rs:4075`), carregamento de lote (`src/ui/data_loader.rs:1306`) e abertura de servidor (`src/ui/data_loader.rs:902`).
- `ui/components/chat_messages_view.slint:110-120` — todo `scroll_trigger` chama `scroll_to_bottom()` incondicionalmente.
- `chat_is_at_bottom` é calculado e armazenado (`ui/appwindow.slint:347`, `ui/components/chat_messages_view.slint:126-130`, `ui/appwindow.slint:2979-2981`), mas não é consultado antes do pin.

**Por que custa:** cada timer mexe em `viewport-y`, o que força atualização de layout/scroll e repintura da região do chat. Sete por mensagem, mais o `changed active_channel_id` (`ui/appwindow.slint:359-362`), dão 8 atualizações por troca de canal. Em chat movimentado, os timers se sobrepõem.

**Correção:** um único timer cancelável/debounced (ex.: 50–100 ms) por “intenção de pin”; só pin se `at_bottom` era verdadeiro antes da mudança (ou se o usuário acabou de enviar mensagem); cancelar ao detectar scroll manual para cima. Para o primeiro paint de canal, usar um `changed` de layout/modelo em vez de uma lista fixa de delays.

**Risco:** médio-baixo. É preciso manter o comportamento de “abrir canal com imagens já no fim” sem voltar ao problema de ficar no topo. Testar canal com imagens, embeds e altura variável.

**Ganho esperado:** reduz de 7–8 para 0–1 operações de scroll por evento; melhora percepção em rajada e elimina o “puxão”.

---

### F4 — Lista de chat não é virtualizada (Alta, estrutural)

**Sintoma:** quanto mais histórico carregado, mais lenta a troca de canal, o scroll e qualquer atualização de modelo.

**Evidência:**

- `ui/components/chat_messages_view.slint:132` — `msg_scroll := ScrollView { ... }`.
- `ui/components/chat_messages_view.slint:194` — `for msg in root.messages: VerticalLayout { ... }` dentro do `ScrollView`.
- Cada mensagem instancia condicionais para avatar, reply, header, links, `content_lines` (com `for line` e `for block`), embeds (outro `for line`/`for block`), botões, invites, anexos e links. É uma árvore grande por linha.
- O modelo cresce com “carregar anteriores” (`src/main.rs:3125-3132`): o código coleta o modelo atual, concatena o lote antigo, reagrupa, reidrata avatares/invites e substitui o modelo inteiro.

**Por que custa:** `ScrollView` não é virtualizado: todas as linhas existem na árvore, mesmo fora da viewport. O layout e a memória da cena crescem com o histórico. Substituir o modelo inteiro recria todas as linhas e perde âncoras de scroll.

**Correção estrutural (duas opções):**

1. **Janela deslizante (mais simples):** manter no máximo N mensagens no modelo (ex.: 200–300), remover as mais antigas ao paginar e oferecer “carregar anteriores” quando o usuário chega ao topo. Isso limita memória e layout, sem trocar o componente.
2. **`ListView` virtualizado:** migrar o `ScrollView + VerticalLayout` para `ListView` (ou repetidor virtualizado equivalente), que instancia apenas as linhas visíveis. É a solução com melhor escalabilidade, mas exige protótipo por causa de altura variável, separadores de dia, âncora no fim e carregamento de anteriores.

**Risco:** alto se feito de uma vez. Fazer atrás de flag, comparar com o caminho atual e validar: separadores de dia, `show_header`, anexos, invites, botões, replies, scroll para o fim e “carregar anteriores”.

**Ganho esperado:** o maior ganho estrutural de fluidez e o principal limite de memória do chat. A opção 1 já resolve a maior parte com risco bem menor.

---

### F5 — Decodificação síncrona de anexos na thread da UI (Alta)

**Sintoma:** abrir um canal com imagens, ou voltar a ele depois de um tempo, trava por alguns quadros; em canal com muitas imagens, a troca parece “em etapas”.

**Evidência:**

- `src/ui/helpers.rs:911-930` — `map_message_attachments` chama `att_cache.get_full_for_ui(&a.id, &a.filename)` para cada anexo.
- `src/utils/attachment_cache.rs:313-320` — `get_full_for_ui` cai em `get_full`.
- `src/utils/attachment_cache.rs:278-307` — `get_full` faz `std::fs::read` e `decode_bytes_to_rgba` de forma síncrona se não estiver na memória.
- O cache de UI tem `UI_IMAGE_CACHE_BYTES = 12 MiB` (`src/utils/attachment_cache.rs:169`); uma imagem 1024×1024 RGBA ocupa ≈4 MiB, então o cache guarda ~3 imagens grandes.
- O cache “full” tem 16 MiB (`src/utils/attachment_cache.rs:13`).

**Por que custa:** `image::load_from_memory` de WebP/JPEG grande pode custar dezenas de ms. Isso acontece dentro do closure que monta o modelo, antes de `ui.set_messages`, ou seja, bloqueia a thread que deveria estar desenhando.

**Correção:** nunca decodificar no caminho de pintura. No `map_message_attachments`, usar apenas o que está no cache de UI; se não estiver, colocar placeholder e agendar decodificação em `spawn_blocking`, com atualização de linha via `set_row_data` (o fluxo de `queue_attachment_update` já existe em `src/utils/attachment_cache.rs:103-167`). Guardar a geração do canal para não aplicar imagem de um canal que já foi trocado.

**Risco:** médio. Precisa evitar “piscar” placeholders, aplicar imagens fora de ordem e manter o scroll estável. Não mexer no caminho de vídeo/LTPV.

**Ganho esperado:** primeira pintura de canal com imagem deixa de bloquear; o decode vira trabalho de fundo coalescido.

---

### F6 — Emoji customizado é lido do disco de forma síncrona ao montar a mensagem (Alta/Média)

**Sintoma:** canais com muitos emojis customizados travam na primeira abertura ou depois que o cache é limpo.

**Evidência:**

- `src/ui/helpers.rs:878-908` — `map_message_lines` chama `emoji_mgr.get(&b.emoji_id)` para cada bloco de emoji.
- `src/utils/emoji_cache.rs:81-110` — se não está na memória, faz `std::fs::read` + `decode_bytes_to_rgba` no chamador (thread da UI).
- Ao chegar a 64 entradas, o cache faz `guard.clear()` (`src/utils/emoji_cache.rs:100-104`), então o próximo lote paga tudo de novo.

**Por que custa:** emoji é pequeno, mas o custo é por bloco e inclui I/O; uma mensagem com 20 emojis diferentes pode custar vários ms. O `clear()` total gera picos periódicos em vez de um LRU suave.

**Correção:** no caminho de pintura, só usar memória; se faltar, placeholder + `fetch_priority_async`. Trocar `clear()` por LRU (ex.: 128–256 entradas) e manter o decode em background.

**Risco:** baixo/médio; a geração de canal já existe (`active_generation`) e deve ser usada para descartar resultados antigos.

---

### F7 — `MessageUpdated` e `MessageDeleted` substituem o modelo inteiro (Média)

**Sintoma:** uma edição ou exclusão pode causar um flash/reflow no chat, principalmente com histórico grande.

**Evidência:**

- `src/main.rs:4926` — cópia de todo o modelo.
- `src/main.rs:4989-4992` — `regroup_messages` + `VecModel::from(current_msgs)` + `ui.set_messages`.
- `src/main.rs:5005-5011` — delete coleta, reagrupa, reidrata **todos** os avatares e invites e substitui o modelo.

**Por que custa:** edição/exclusão são eventos raros, mas o custo é O(n) e recria a lista. Em canal com muitas imagens, a reidratação de anexos pode inclusive voltar a decodificar.

**Correção:** localizar o índice, alterar a linha e usar `set_row_data`; recalcular apenas o agrupamento da linha alterada e da seguinte (por causa de `show_header`/separador de dia). Para delete, remover a linha do `VecModel` e ajustar a vizinha. Não reidratar o modelo inteiro.

**Risco:** médio-baixo; o agrupamento de fronteira é a parte delicada.

---

### F8 — Gateway formata mensagens de canais inativos (Média/Alta em contas grandes)

**Sintoma:** CPU alta e alocações constantes em servidores movimentados, mesmo sem o usuário olhar aqueles canais; pode gerar microtravadas por pressão de alocador.

**Evidência:**

- `src/gateway/client.rs:1759-1800` — em `MESSAGE_CREATE`, chama `format_discord_author` e `format_discord_message_parts` antes de enviar o evento.
- `src/gateway/client.rs:1801-1825` — o mesmo em `MESSAGE_UPDATE`.
- Só depois, em `src/main.rs:4785+`, o handler verifica se `channel_id == current_active_ch` e ignora a maior parte do payload.
- `format_discord_message_parts` (`src/gateway/formatting.rs:1070+`) faz parsing de markdown, emojis, links, embeds, botões e anexos.

**Por que custa:** o trabalho mais caro da mensagem é feito para todas as mensagens, e depois descartado para os canais que não estão abertos. Em conta com muitos servidores, isso é a maior parte do tráfego.

**Correção:** manter um `AtomicU64`/`RwLock` com o canal ativo (e talvez o guild ativo) acessível ao gateway. Para canal inativo, enviar um evento mínimo (`id`, `channel_id`, `author_id`, timestamp, talvez conteúdo cru) suficiente para badge/dedup; formatar só quando o canal for aberto (ou quando o handler realmente for pintar). Alternativa mais conservadora: formatar em `spawn_blocking` e manter o payload, mas isso ainda gasta CPU.

**Risco:** médio. O handler usa `content` para dedup e badges? Verificar. Não pode quebrar notificação, unread, reply ou cache. Fazer atrás de flag e comparar.

**Ganho esperado:** reduz CPU/alocação proporcionalmente ao tráfego de canais não vistos; indireto na fluidez.

---

### F9 — Atualização de voz reconstrói todos os canais (Média)

**Sintoma:** entrar/sair de voz ou mudar participantes pode causar atualização ampla da sidebar.

**Evidência:**

- `src/main.rs:5014-5031` — em `VoiceStatesUpdated`, copia `chans`, chama `build_ui_channels` para todos os canais e depois `update_channel_voice_rows`.
- `src/ui/helpers.rs:203-239` — se a contagem de linhas muda (entrou/saiu membro), substitui o modelo inteiro; se não, usa `set_row_data`.
- `src/main.rs:1998+` — loop de 250 ms calcula fingerprint de todos os canais/participantes e, quando muda, chama `build_ui_channels` de novo.

**Por que custa:** eventos de voz não são tão frequentes quanto mensagens, mas o loop de 250 ms roda sempre que a janela está visível e percorre todos os canais. A reconstrução ampla só é necessária quando muda a composição de linhas (membro entrou/saiu); mudança de “está falando”/streaming pode ser `set_row_data` na linha do membro.

**Correção:** separar “mudou composição” de “mudou estado visual”. Para fala/streaming, atualizar apenas as linhas `voice-member:*` afetadas. Para entrar/sair, inserir/remover a linha no `VecModel` em vez de reconstruir tudo. Reduzir o fingerprint para os canais de voz do guild ativo.

**Risco:** médio; a sidebar tem linhas virtuais de membros e ordem específica. Não tocar no estado de voz/áudio, só na apresentação.

---

### F10 — READY/READY_SUPPLEMENTAL: burst de parsing, alocação e logging (Média)

**Sintoma:** primeiro segundo após login com CPU alta, RSS subindo rápido e UI ocupada; a janela pode demorar a responder.

**Evidência:**

- `src/gateway/client.rs:1057-1132` — `handle_strict_ready` percorre usuários, guilds, emojis, voice states e canais, e envia um `GuildLoaded` por guild.
- `src/gateway/client.rs:1182-1338` — caminho alternativo de READY faz o mesmo com `serde_json::Value`.
- `src/gateway/client.rs:1339-1425` — `READY_SUPPLEMENTAL` itera guilds e loga cada voice state com `info!`.
- `src/main.rs:5034-5037` — cada `GuildLoaded` entra no `guilds_map`.

**Por que custa:** é um burst inevitável de dados, mas o parsing e o logging acontecem na mesma runtime que atende eventos; o `info!` por voz no startup pode ser centenas de linhas síncronas. O `GuildLoaded` já é usado para troca instantânea de servidor, então não remover sem medir.

**Correção:** reduzir logs por item para `debug!`/agregado; considerar `spawn_blocking` para o parsing pesado; evitar construir o `Value` completo no caminho não-estrito (o estrito já ajuda). Medir o tempo de READY e o pico de RSS antes/depois.

**Risco:** médio; READY é crítico para canais, emojis e voz. Não alterar a ordem necessária para o primeiro paint.

---

### F11 — Política de trim: 8 s, 45 s e “RSS cosmético” (Média)

**Evidência:**

- `src/main.rs:4754-4760` — um `trim_process_memory()` 8 s após `Connected`, com comentário dizendo que não é repetido periodicamente.
- `src/main.rs:2086-2094` (mudança não commitada no working tree) — **novo loop de 45 s** chamando `trim_process_memory()`.
- `src/ui/helpers.rs:998-1022` — Linux: `malloc_trim(0)` + `mi_collect(true)`.
- `src/main.rs:4579` e `4669` — trim ao sair da voz e ao minimizar para a bandeja.
- Windows: `K32EmptyWorkingSet` em `trim_process_memory` (`src/ui/helpers.rs:998-1010`).

**Por que importa:** `mi_collect(true)` força coleta; pode custar alguns ms e, depois, páginas voltam por page fault. O loop de 45 s cria uma serra de RSS e pode coincidir com animação/scroll. No Linux, `malloc_trim` só devolve páginas livres; não reduz memória viva (framebuffer, caches, modelo). No Windows, `K32EmptyWorkingSet` faz o RSS cair sem necessariamente reduzir commit — é cosmético e pode causar re-fault.

**Correção:** manter trim em eventos de mudança grande (pós-READY, ao minimizar/esconder, ao sair de voz, ao trocar de canal com histórico grande, logout), não em timer periódico. Se o loop de 45 s for mantido, medir a pausa com `perf`/timestamp e não rodar durante voz/streaming/animação. Não perseguir queda de RSS; medir PSS/Private_Dirty e memória viva.

**Risco:** baixo para remover o timer; o trim de 8 s pode ser mantido até haver medição comparativa.

---

### F12 — HUD de hardware atualiza propriedades a cada 1 s (Baixa/Média)

**Evidência:**

- `src/utils/hardware_monitor.rs:48-108` — a cada 1 s, lê `/proc/self/status` (Linux), formata textos e seta seis propriedades Slint (`hardware_cpu_text`, `hardware_ram_text`, `hardware_gpu_text`, `hardware_cpu_val`, `hardware_ram_val`, `hardware_gpu_val`).
- No Linux, CPU/GPU ficam em 0; só a RAM muda de vez em quando, mas os setters são chamados sempre.
- A HUD está ligada por padrão (`show_hardware_monitor: true` em `src/utils/video_settings.rs:47,116`).

**Por que custa:** acorda a thread da UI e pode invalidar o layout da titlebar a cada segundo; em repouso é pequeno, mas é um timer eterno que compete com scroll/animação.

**Correção:** só setar quando o valor mudou (comparar antes), atualizar a cada 2–5 s quando ocioso, pausar quando a janela estiver oculta/oclusa. Não usar `request_redraw` manual.

**Risco:** baixo.

---

### F13 — 35 animações, 26 sombras e 29 clips no backend software (Baixa/Média)

**Evidência:**

- `ui/components/channel_sidebar.slint:497-562` — hover/seleção com `animate background`/`border-color` de 120 ms em várias linhas; 13 animações no arquivo.
- `ui/appwindow.slint:1171,1239,1249,1257,1355,1366,1565,1610,1709,1754,1899,2877,2914,3193,3291,3514,3541,5276,5590,5626` — animações de guild rail, botões, menus, etc.
- `ui/appwindow.slint:2992,3137,3710,4137,4338,4521,5525,5835` e `ui/components/*.slint` — 26 `drop-shadow`, incluindo blurs de 10–24 px em modais, menus e painéis.
- 29 `clip: true` no total.

**Por que custa:** o renderizador por software rasteriza blur/sombra na CPU; `drop-shadow` de 24 px em painel grande é caro quando o painel abre/redimensiona. Animações de hover em 4K geram vários frames de trabalho local, mas podem ser percebidas como “peso” ao passar o mouse. Em renderização parcial, o custo é local, não o frame inteiro — por isso a prioridade é média/baixa, não alta.

**Correção:** no backend software, encurtar animações de hover (ou desligar `animate` de cor/fundo em linhas de lista), manter animação só para seleção; substituir `drop-shadow` de blur grande por uma imagem 9-slice pré-renderizada ou remover em painéis grandes; medir com `SLINT_DEBUG_PERFORMANCE`.

**Risco:** baixo visual; validar tema claro/escuro e modais.

---

### F14 — Caches e mapas sobrevivem ao logout/troca de conta (Média)

**Evidência:**

- Logout (`src/main.rs:3278-3330`) limpa token, guild map, canais, voz, filas, modelos de guild/canais, mas **não** limpa:
  - `avatar_cache` (memória + slots);
  - `emoji_cache` (memória);
  - `attachment_cache` (full + UI + preview);
  - `MESSAGE_CACHE` (`src/ui/data_loader.rs:408-437`);
  - `USER_NAMES` (`src/gateway/types.rs:647-661`);
  - `USER_AVATARS` (`src/gateway/types.rs:664-680`);
  - `GUILD_VOICE_STATES` (`src/gateway/types.rs:625+`);
  - `PRELOADED_GUILDS`, `LAST_CHANNEL_BY_GUILD`, `GUILD_REFRESHED_AT`, `OLDEST_MESSAGE_MAP`, `COLLAPSED_CATEGORIES`.
- `AttachmentCache::clear_memory` existe (`src/utils/attachment_cache.rs:323-330`) mas **não tem chamadores**.
- `reset_unread_and_dedup_state` (`src/ui/helpers.rs:67-72`) limpa só unread e dedup.

**Por que importa:** memória de até ~30 MB fica retida após trocar de conta; mensagens/canais da conta anterior podem reaparecer por cache antes do REST; `USER_NAMES` cresce sem limite (nomes de todos os usuários vistos em READY/voz).

**Correção:** criar `clear_session_caches()` chamado no logout e antes de login com outra conta; limpar caches de imagem/emoji/mensagem, mapas de nomes/avatares, voz e invites; manter apenas preferências e geometry. Adicionar cap/LRU em `USER_NAMES` (ex.: 4096) e limpar no logout.

**Risco:** baixo; validar que o primeiro paint pós-login continua funcionando (vai refazer downloads, mas é o comportamento correto).

---

### F15 — Pulso de redraw no startup (Baixa)

**Evidência:** `src/main.rs:366-386` — 8 `request_redraw()` a cada 250 ms no Linux, além do redraw inicial em `src/main.rs:316,368`.

**Por que custa:** pouco; serve para compensar first paint. Com o modelo atual, pode ser substituído por um único redraw após layout, ou removido se a janela já pinta corretamente. Não é prioridade.

---

### F16 — Troca de servidor/canal com transição suave e feedback elástico dos servidores (Baixa, UX)

**Evidência:** anteriormente, `paint_channel_messages` e a sidebar trocavam de golpe sem transição contínua; alternar entre 0% e 100% causava sensação de parpadeo ("blink"). Além disso, o indicador de servidor (pílula esquerda e raio do ícone) tinha curso muito curto (16px a 14px de radius) e sem animação de clique ou largura.

**Correção aplicada:**

- **Fade + Slide no Chat e Sidebar:**
  - `chat_view`: fade suave de `opacity: 0.35 → 1.0` combinado com micro-deslizamento vertical `y: 6px → 0px` em 280 ms (`ease-out`). Partir de 0.35 elimina totalmente o flash escuro ("blink").
  - `ChannelSidebar`: `VerticalBox` interno com `y: 6px → 0px` e `opacity: 0.35 → 1.0` em 300 ms (`ease-out`) apenas no primeiro carregamento do servidor (`sidebar_painted_guild`).
  - Cabeçalho: nome e prefixo (`#`/`@`) animam `opacity: 0.35 → 1.0` em 280 ms (`ease-out`).
  - Timer de transição reduzido para 12 ms para disparar exatamente no próximo frame sem reter quadros parados.
- **Pílula e ícones de servidores estilo Discord:**
  - Pílula lateral anima altura (`0px → 20px` em hover, `28px` pressionado, `40px` ativo), largura (`0px → 4px`) e opacidade em 220 ms (`ease-out`).
  - Ícone do servidor transiciona de círculo (`border-radius: 24px`) para squircle (`border-radius: 15px`) com feedback tátil de clique (`y: 1px`).
- **Otimização de sombras:**
  - Sombras grandes de overlays recalculadas com blur menor (4px–8px) com borda sutil, preservando 60 FPS no renderizador por software.

---

## 4. Mapa de memória

### 4.1 Orçamentos atuais

| Cache / estrutura | Limite | Onde | Observação |
|---|---:|---|---|
| Framebuffer software | ≈35,6 MiB | backend | 2880×1620×4×2; piso difícil de reduzir sem mudar resolução/backend |
| Anexos “full” | 16 MiB | `attachment_cache.rs:13` | imagens 1024² RGBA ≈4 MiB cada |
| Anexos na UI | 12 MiB | `attachment_cache.rs:169` | ~3 imagens 1024²; LRU |
| Previews de anexo | 30 entradas, clear-all | `attachment_cache.rs:359` | thumbnails 32×32; clear-all gera churn |
| Avatares | 128 × 40×40 RGBA | `avatar_cache.rs:17` | ~0,8 MiB de pixels + overhead |
| Slots de avatar da UI | 128 | `avatar_cache.rs:22` | `Image` handles na thread da UI |
| Emojis | 64 entradas, clear-all | `emoji_cache.rs` | pequenos, mas decode síncrono |
| Mensagens em cache | 8 canais × ~30 | `data_loader.rs:285` | compacto, bounded |
| Modelo de chat | sem limite | `ui/appwindow.slint` | cresce com “carregar anteriores” |
| `USER_NAMES` | sem limite | `gateway/types.rs:647` | nomes vistos em READY/voz |
| `USER_AVATARS` | 1024, clear-all | `gateway/types.rs:664` | hashes pequenos |
| Dedup de mensagens | 1000 ids | `helpers.rs:49-65` | bounded |

### 4.2 Onde os ~169 MiB de RSS provavelmente estão

Não foi feito breakdown por mapeamento nesta execução. Estimativa para orientar a investigação:

- **≈36 MiB** framebuffer;
- **≈40–55 MiB** binário, bibliotecas compartilhadas, pilhas de threads e runtime (RSS compartilhado eleva RSS, mas não PSS);
- **≈28 MiB** caches de anexo no pior caso + avatares/emoji;
- **≈? MiB** cena Slint, glifos, imagens decodificadas e buffers temporários do `image`;
- **≈? MiB** heap retido pelo mimalloc (páginas livres que o trim pode devolver).

O PSS de ~131 MiB é a métrica mais honesta para “quanto o processo realmente custa” quando há bibliotecas compartilhadas.

### 4.3 O que fazer com o “começa alto e cai”

Hipóteses, em ordem:

1. **Trim de 8 s** (`src/main.rs:4754-4760`) devolvendo páginas livres. Nesta medição Linux, a queda foi ≈0,5 MiB, não dezenas de MB.
2. **Loop de 45 s** não commitado (`src/main.rs:2086-2094`), que cria uma serra e pode ser o que o usuário está vendo em um build local mais novo.
3. **Minimizar para a bandeja** chama trim e esconde a janela (`src/main.rs:4669`); no Windows, `K32EmptyWorkingSet` derruba o working set de forma cosmética.
4. **Caches/imagens**: terminar o burst de decode e liberar buffers temporários pode derrubar alguns MB, mas o cache quente volta a crescer.
5. **Monitor do sistema** mostrando working set (Windows) ou RSS com páginas compartilhadas, que se comporta diferente de PSS.

**Recomendação:** não otimizar para a queda cosmética. Medir RSS, PSS, `Private_Dirty` e `Anonymous` com timestamp antes/depois de cada trim, e separar “memória viva” de “páginas livres”. A meta deve ser **platô estável e caches bounded**, não um número baixo logo após o login.

---

## 5. Plano por fases

### Fase 0 — Instrumentar (0,5–1 dia)

Sem mudar comportamento de release:

- Rodar com `SLINT_DEBUG_PERFORMANCE=refresh_lazy,console` para ver FPS/tempo de frame em release.
- Coletar `perf record -F 999 -g -p <pid>` durante: hover na guild rail, troca de canal, abertura de canal com imagens, rajada de mensagens, scroll de histórico.
- Coletar `/proc/<pid>/smaps_rollup` (PSS, Private_Dirty, Anonymous) e `status` (VmRSS/VmHWM) a cada 1 s, com timestamps, por 2–5 min.
- Registrar o instante de cada trim (log já existe para o de 8 s; adicionar timestamp se necessário).
- Medir o custo de `mi_collect(true)` com `perf`/`strace -c` em um build de teste; não no binário do usuário.

### Fase 1 — Quick wins (2–4 dias)

Ordem sugerida, cada uma independente:

1. **F1** — sidebar incremental (`set_row_data`).
2. **F2** — agrupamento só da mensagem nova.
3. **F3** — scroll pin único, debounced e condicional a `at_bottom`.
4. **F12** — HUD só quando o valor muda, cadência 2–5 s.
5. **F5/F6** — decode assíncrono de anexos/emoji no caminho de pintura.
6. **F11** — remover o trim periódico de 45 s ou medi-lo; manter trims por evento.
7. **F15** — remover o pulso de 8 redraws se a primeira pintura estiver estável.

**Critério de saída:** perfil de troca de canal e rajada mostra queda clara de tempo na thread da UI; sem regressão visual; sem mudanças em áudio/voz.

### Fase 2 — Estrutural (1–2 semanas)

1. **F4** — janela de mensagens (N=200–300) como primeiro passo; protótipo de `ListView` virtualizado depois, atrás de flag.
2. **F7** — update/delete incrementais.
3. **F8** — não formatar canal inativo no gateway (evento mínimo + formatação sob demanda).
4. **F9** — atualização de voz por linha, não por guild.
5. **F14** — `clear_session_caches()` no logout/troca de conta; LRU em `USER_NAMES`, emoji e previews.
6. **F13** — política de animações/sombras no backend software; testar GPU renderer como opção do usuário (não default).

**Critério de saída:** modelo de chat bounded; RSS/PSS estáveis após 10 min de uso; 20 trocas de canal sem crescimento monotônico; rajada de 100 mensagens sem picos de layout.

### Fase 3 — Validação e release (2–3 dias)

- Repetir todos os cenários com o binário release (sem `automation`).
- Comparar antes/depois: p95/p99 de frame, tempo de troca de canal, CPU ociosa, RSS/PSS, alocações (`heaptrack`/`massif` se necessário).
- Validar LTPV/áudio: entrar em voz, falar, ouvir, sair, trocar de canal, minimizar/restaurar; nenhuma alteração nesses caminhos.
- Rodar em 1080p e 4K; escala 1.0 e 1.4/1.5.
- Documentar números e manter o roadmap atualizado.

---

## 6. Protocolo de medição

### 6.1 Métricas

| Métrica | Como | Meta inicial |
|---|---|---|
| Frame time p50/p95/p99 | `SLINT_DEBUG_PERFORMANCE=refresh_lazy,console` + `perf` | p95 ≤16,7 ms, p99 ≤33 ms em 2880×1620 software |
| Tempo de troca de canal | instrumentação temporal no handler + log | ≤1 frame para cache quente; sem bloqueio >16 ms na UI |
| Rajada de mensagens | 10–50 msg/s em canal ativo | sem acúmulo de timers; p95 ≤33 ms |
| CPU ociosa | `perf stat`/`/proc` por 60 s | <1% de um núcleo |
| RSS/PSS | `smaps_rollup` 1 Hz | platô; sem crescimento >5% em 10 min |
| Memória após trim | antes/depois com timestamp | queda só de páginas livres; sem re-fault visível |
| Alocações | `heaptrack`/`massif` em build de teste | sem crescimento por mensagem/canal |

### 6.2 Cenários

1. Ocioso 60 s, janela visível, HUD ligado.
2. Hover na guild rail por 5 s; hover na sidebar por 5 s.
3. 20 trocas de canal alternando cache quente/frio.
4. Abrir canal com 10–30 imagens (anexos e embeds).
5. Receber 100 mensagens em rajada no canal ativo.
6. Receber mensagens em canais inativos (badge).
7. Rolar histórico até o topo e “carregar anteriores” 3×.
8. Editar e apagar mensagens no canal ativo.
9. Minimizar para tray, esperar 10 s, restaurar.
10. Entrar em voz, falar/ouvir 30 s, sair; repetir com tela compartilhada (somente validação de não regressão).
11. Trocar de conta (logout/login) e verificar caches/limpeza.
12. Redimensionar janela e alternar maximizado/restaurado.

### 6.3 Comandos úteis

```bash
# Série de memória (RSS + PSS + dirty + anon)
pid=$(pgrep -n litecord)
for i in $(seq 1 120); do
  rss=$(awk '/VmRSS/{print $2}' /proc/$pid/status)
  hwm=$(awk '/VmHWM/{print $2}' /proc/$pid/status)
  pss=$(awk '/^Pss:/{print $2}' /proc/$pid/smaps_rollup)
  dirty=$(awk '/^Private_Dirty:/{print $2}' /proc/$pid/smaps_rollup)
  anon=$(awk '/^Anonymous:/{print $2}' /proc/$pid/smaps_rollup)
  echo "$(date +%s) rss=$rss hwm=$hwm pss=$pss dirty=$dirty anon=$anon"
  sleep 1
done
```

```bash
# Perfil de CPU durante uma interação
perf record -F 999 -g -p $(pgrep -n litecord) -- sleep 30
perf report --stdio | head -80
```

```bash
# Métricas de frame do Slint (release)
SLINT_DEBUG_PERFORMANCE=refresh_lazy,console ./litecord
```

Para alocação, usar um build de teste separado (não substituir `~/.local/bin/litecord`) e `heaptrack`/`valgrind --tool=massif`; são lentos e servem para confirmar vazamento, não para medir frame time.

---

## 7. Riscos e guardrails

1. **Não tocar no caminho LTPV/áudio.** Nenhuma mudança em `native_video/*`, `gateway/voice*`, `audio/*`, `screen_capture/*`, filas de PCM, captura de mic, DSP/AEC ou relógio de heartbeat. As mudanças propostas são de UI, modelo, cache e política de memória.
2. **Ordem de eventos.** `invoke_from_event_loop` continua sendo a fronteira para a UI. Ao coalescer mensagens/scroll, preservar a ordem por canal e não atrasar eventos de voz.
3. **Geração de canal/imagem.** Decode assíncrono precisa de um `generation`/`channel_id` para não aplicar imagem ou emoji de um canal que já foi trocado.
4. **Virtualização.** `ListView` com altura variável e âncora no fim é a mudança de maior risco. Prototipar atrás de flag; manter o caminho antigo até validação.
5. **Caches.** Limpar cache não invalida `Image` já referenciada pelo modelo; seguro. Mas limpar no meio de um paint pode causar re-decode e “piscar”; limpar em logout/minimização, não durante scroll.
6. **Trim.** `mi_collect(true)` pode pausar; não chamar durante voz/streaming/animação. No Windows, `K32EmptyWorkingSet` é cosmético e pode causar re-fault; medir antes de manter.
7. **Outro agente no repo.** Não commitar mudanças não relacionadas; não reverter o diff existente; o roadmap é somente documentação. Se for implementar, criar branch a partir de `origin/main` (`fb2bfe9`) e não misturar com o trabalho local.
8. **Binário do usuário.** Não substituir `~/.local/bin/litecord` por build `--features automation`; testes de automação usam porta separada (`LITECORD_AUTOMATION_PORT`) e processo próprio.
9. **GPU renderer.** É opção de próximo start (`video_settings::toggle_use_gpu_renderer`, `src/main.rs:2461+`). Pode melhorar fluidez, mas aumenta VRAM/memória compartilhada e tem risco de driver; não virar default sem medição em Intel/Wayland/NVIDIA.

---

## 8. Perguntas em aberto

1. O “começa alto e cai” do usuário acontece em qual build? O loop de 45 s já está nesse binário? Qual a curva (RSS/PSS) e em qual momento?
2. Qual o tamanho típico do histórico carregado por canal? 200 mensagens é um limite aceitável para a janela deslizante?
3. O usuário usa 4K/escala 1.4–1.5 sempre? Vale testar GPU renderer como opção recomendada para 4K?
4. Quantos canais/guilds no pior caso? A sidebar incremental precisa suportar quantas linhas?
5. O comportamento de “sempre ir para o fim” é desejado ou o usuário prefere ficar no histórico quando está lendo? (O código já rastreia `at_bottom`, sugerindo que a intenção era condicional.)
6. Há requisito de privacidade para limpar caches na troca de conta? (Hoje não limpa.)

---

## 9. Arquivos mais relevantes

| Arquivo | Papel |
|---|---|
| `src/main.rs` | loop de eventos, handlers de mensagem/voz, callbacks de canal, trims, renderer |
| `src/ui/helpers.rs` | `apply_channel_activity`, `build_ui_channels`, `update_channel_voice_rows`, scroll pin, trim |
| `src/ui/data_loader.rs` | cache de mensagens, troca de canal, paint, guild cache |
| `src/gateway/client.rs` | parsing READY/GUILD_CREATE, formatação de mensagens, eventos |
| `src/gateway/formatting.rs` | parser de markdown/embeds/emoji/links |
| `src/gateway/types.rs` | `USER_NAMES`, `USER_AVATARS`, voice states |
| `src/utils/attachment_cache.rs` | decode de anexos, caches 16/12 MB, updates |
| `src/utils/emoji_cache.rs` | cache/decode de emojis |
| `src/utils/avatar_cache.rs` | cache de avatares, hidratação |
| `ui/components/chat_messages_view.slint` | lista de mensagens não virtualizada |
| `ui/components/channel_sidebar.slint` | linhas e animações da sidebar |
| `ui/appwindow.slint` | layout principal, animações, sombras, scroll |

---

## 10. Resumo de uma frase

**A maior melhora de fluidez virá de parar de reconstruir o mundo a cada mudança** — sidebar, modelo de chat e scroll — **e de tirar decode de imagem da thread da UI**; a maior melhora de memória virá de **limitar o modelo de chat e limpar caches no ciclo de sessão**, aceitando que o framebuffer de ~36 MiB é um piso do renderizador por software em 4K.

---

## 11. Status de implementação (2026-11-02)

Fases 1 e parte da Fase 2 aplicadas em `main` (commits `91eee39`, `f52b5b0` e o commit desta rodada), sem tocar em LTPV/áudio/mic/DSP.

| Item | Status | Arquivos |
|---|---|---|
| F1 — sidebar incremental (`set_row_data`, sem rebuild) | feito | `src/ui/helpers.rs` (`apply_channel_activity`, novo `clear_channel_unread`), `src/main.rs` (clique de canal) |
| F2 — append O(1) (agrupa só a última linha) | feito + teste | `src/ui/message_grouping.rs` (`group_appended_message` + teste que compara com `regroup_messages`), `src/main.rs` (`MessageCreated`) |
| F3 — scroll pin debounced (80 ms + 1 follow-up de 340 ms) e condicional | feito | `src/ui/helpers.rs`; `MessageCreated` só fixa se estava no fim ou é mensagem própria |
| F5 — decode de anexo fora da thread da UI | feito | `src/utils/attachment_cache.rs` (`get_full_for_ui` só memória, novo `hydrate_full_from_disk_async`), `src/ui/helpers.rs` |
| F6 — emoji fora da thread da UI | feito | `src/utils/emoji_cache.rs` (`get_cached`, `hydrate_from_disk_async`), `src/ui/helpers.rs` |
| F12 — HUD só quando o valor muda | feito | `src/utils/hardware_monitor.rs` |
| F7 — update/delete incrementais | feito + testes | `src/ui/message_grouping.rs` (`group_row`, `context_before`, `regroup_message_at` + testes de edição/remoção contra `regroup_messages`), `src/main.rs` (`MessageUpdated` via `set_row_data`, `MessageDeleted` via `remove` + `regroup_message_at`) |
| F14 — caches/mapas no ciclo de sessão | feito (parcial) | `clear_session` de avatar, `clear_memory` de emoji/anexo, `data_loader::clear_session_caches`, `gateway::clear_user_directory`/`clear_guild_voice_states`, `helpers::clear_session_caches` chamado em logout/troca/adicionar conta; `USER_NAMES` agora limitado a 4096 e limpo por inteiro no teto (como `USER_AVATARS`) |
| F15 — pulso de redraw no startup | feito | `src/main.rs`: de 8 redraws a cada 250 ms para 2 (300 ms e 800 ms), mantendo o redraw imediato |
| F9 — voz por linha | já existia | `src/ui/helpers.rs::update_channel_voice_rows` já usa `set_row_data` quando o conjunto de linhas é o mesmo; nada a mudar |
| F8 — não formatar canal inativo no gateway | feito + teste | `src/gateway/client.rs`: `GatewayClient` recebe o `Arc<Mutex<String>>` do canal ativo; `MESSAGE_CREATE`/`MESSAGE_UPDATE` de outro canal enviam evento mínimo (só ids) e não parseiam markdown/embeds/anexos. Teste `messages_for_the_open_channel_are_formatted_and_others_are_not` |
| F11 — trim periódico de 45 s | feito | `src/main.rs`: loop removido; os trims por evento (startup, troca de canal, esconder na bandeja, logout) continuam |
| F4 — janela de chat limitada | feito (janela, não virtualização) | `src/ui/message_grouping.rs` (`MAX_CHAT_ROWS = 300`, `MAX_CHAT_ROWS_WITH_HISTORY = 600`, `trim_chat_window`, `trim_chat_window_and_rebase` + teste), `src/main.rs`: appends em vivo podam só com o leitor no fim, movem o cursor de `OLDEST_MESSAGE_MAP` para a nova primeira linha e re-agrupam a linha 0; “carregar anteriores” cresce até 600 linhas e então esconde o botão, evitando crescimento sem limite na árvore não virtualizada |
| F13 — animações/sombras no backend software | feito (parcial) | `ui/components/channel_sidebar.slint` e `ui/appwindow.slint`: hover/seleção de 120 ms → 80 ms (23 animações). As `drop-shadow` de 10–24 px ficaram como estão: trocar por 9-slice ou removê-las exige validação visual claro/escuro |
| F16 — fade de entrada ao trocar de canal/servidor | feito | `ui/appwindow.slint` (`chat_enter_opacity`, `sidebar_enter_opacity`, `header_enter_opacity`, `ChatMessagesView`/textos com `animate opacity`), `ui/components/channel_sidebar.slint` (`content_opacity`), `src/ui/data_loader.rs` (`paint_channel_messages` anima só quando o canal pintado muda) |

**Validação:** `cargo check --bin litecord`, `cargo build --release --bin litecord` e `cargo test --bin litecord` (10 testes de agrupamento + 6 do gateway, incluindo o de formatação por canal) limpos, somente warnings pré-existentes. Smoke tests de 15–30 s com o binário release recém-compilado: login OK e nenhum panic; o RSS de platô variou entre ≈144 e ≈178 MiB conforme o canal em que a sessão abriu, então serve como sanidade e não como ganho medido.

**O que ainda falta para medir:** perfil controlado (`SLINT_DEBUG_PERFORMANCE`, `perf`) e um teste de rajada/20 trocas de canal. Nada disso foi feito ainda; os números acima são apenas de inicialização. F4 fica como janela de 300 linhas (não virtualização real: o `ListView` do Slint 1.17 assume altura fixa e as linhas do chat variam) e F13 ainda precisa de validação visual das sombras de 10–24 px.

**Observação:** durante o primeiro smoke test apareceu um panic de `RefCell already borrowed` no novo debounce de scroll; foi corrigido antes do build final (`slot.borrow()` mantido durante `borrow_mut()`). O binário em `~/.local/bin/litecord` foi sobrescrito por um build externo e coincide com `target/release/litecord`; a instância que já estava em execução continua com o código antigo até ser reiniciada.
