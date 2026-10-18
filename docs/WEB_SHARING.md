# Transmissão por link no navegador

Durante uma transmissão, use **Compartilhar na web → Copiar link**. Quem tem o link
assiste sem conta do Discord. **Encerrar web** desconecta os navegadores e mantém a
transmissão nativa; parar de transmitir ou sair da chamada também encerra a web.
O som é o áudio do computador/janela selecionado na transmissão, sem adicionar o
microfone ou capturar a chamada por um caminho separado. Se a captura de áudio do
computador incluir a saída de uma chamada, esse som também pode entrar nela.

O primeiro acesso inicia o vídeo sem som para respeitar a reprodução automática dos
navegadores. Use **Ativar som**. Há controles de volume, reprodução e tela cheia.
Até três espectadores podem assistir ao mesmo tempo, com uma cópia do upload por
espectador. O link muda a cada sessão e para de funcionar quando ela termina.

## Arquitetura

- Litecord ↔ Litecord continua com o protocolo existente.
- `str0m` envia o H.264 do codificador existente por uma trilha WebRTC. Não há segundo
  codificador de vídeo nem WebCodecs na página; o navegador decodifica um `<video>`.
- O loopback existente alimenta um codificador Opus mono compartilhado, criado
  somente enquanto há espectadores web. Suporta entrada de 44,1 e 48 kHz.
- O worker existente serve `/watch` e roteia SDP em `/web/ws`. Nenhum áudio/vídeo
  passa pelo worker. Não é necessário hospedar uma segunda página no GitHub Pages.
- A chave AES-256-GCM fica no fragmento `#` do link. A sinalização é criptografada
  antes do envio. O token de proprietário é independente dessa chave; conhecer o
  link de espectador não permite registrar um transmissor naquela sala.
- Antes de reservar uma vaga, o worker verifica uma prova derivada da chave e da
  sala. A prova é `SHA-256("litecord-web-viewer-v1\0" || chave de 32 bytes || sala
  ASCII)`; o transmissor registra somente o hash SHA-256 dessa prova. A chave AES
  não vai ao worker. Uma prova ausente ou incorreta é recusada com status 403 sem
  registrar socket na sala ou repassar uma oferta ao transmissor. A capacidade é
  conferida novamente após o hash assíncrono para respeitar o limite sob concorrência.
- O endereço público é descoberto por STUN no próprio socket WebRTC. Uma conexão
  após um período sem espectadores refaz essa descoberta, sem heartbeat STUN ocioso.
- Desligado: sem socket, thread ou timer de WebRTC. Ligado sem espectadores: só a
  conexão de sinalização, com keepalive de 40 segundos; nenhum frame é copiado e
  nenhum áudio é codificado. Os contadores da interface são atualizados por evento.
- Filas limitadas evitam bloquear captura/codificação; perda local de um frame
  invalida as referências e o envio espera outro quadro-chave.

## Validação local (Windows)

Compile os testes, inicie o worker em outro terminal e rode o teste de navegador:

```powershell
cargo test --release --lib screen_capture::web_share::tests --no-run
cd cloudflare
npx wrangler dev --local --ip 127.0.0.1 --port 8787
```

Na raiz, passe o executável de testes informado pelo Cargo:

```powershell
node scripts/perf/web_share_e2e.cjs target/release/deps/litecord-<hash>.exe
```

O teste usa Chrome headless, Playwright e Python, uma fonte H.264 sintética de
640×360 a aproximadamente 12,5 FPS e uma senoide Opus de 440 Hz. Não usa contas,
microfone ou captura de tela. Confere decodificação de vídeo/áudio, chave errada,
três espectadores, recusa do quarto, reconexão de espectador, encerramento e link
inválido. Artefatos e amostras de CPU/memória vão para `target/web-share-e2e/`.
As amostras incluem a geração/codificação sintética: não são um benchmark de jogo
nem uma estimativa do custo de 1080p60.

Após a autenticação antes da admissão, passaram **48 testes Rust** (uma fixture
ignorada por padrão), **seis testes do worker** e o fluxo completo no Chrome com
worker local: H.264/Opus, chave errada, três espectadores, recusa do quarto,
substituição de espectador e encerramento. A regressão da sinalização nativa
também passou. As tentativas com prova ausente, errada ou malformada não registram
espectador, e dez admissões simultâneas mantêm o limite de três.

```powershell
cargo test --release --lib screen_capture::web_share::tests -- --skip browser_fixture
cd cloudflare
npm test
```

Para testar o aplicativo com o worker local, defina
`LITECORD_WEB_SIGNALING_URL=ws://127.0.0.1:8787` ao iniciar uma build de teste. A
automação `web_share_start` retorna imediatamente com `pending`; consulte `state`
até `stream.web_url` estar disponível, ou `stream.web_error` indicar a falha.

## Publicação e limites

### Parada e reinício da sessão

A parada retira os canais da sessão da interface e sinaliza o encerramento sem
esperar a thread ou a rede. Cada início cria socket, canais, codificador e estado
próprios; a limpeza de uma sessão anterior não altera a sessão atual. O fechamento
do WebSocket tem limite de 50 ms em segundo plano.

Validação local em 17/10/2026: com um servidor que não confirma o fechamento,
`stop()` retornou em **0,040 ms**. O teste também manteve uma nova sessão com três
espectadores após a limpeza da anterior. Passaram **47 testes Rust** (uma fixture
de navegador ignorada por padrão) e os **quatro testes do worker**. Este teste de
parada não substitui a validação de captura real e dos navegadores.

O servidor em produção precisa receber `cloudflare/worker.js`, `web-share.js`, a
página e a regra de módulos HTML do `wrangler.toml`. O Wrangler empacota a página
por importação. Nenhuma migration ou nova classe de Durable Object é necessária:
as salas web usam o prefixo `web:` na classe existente e attachments que sobrevivem
à hibernação. As rotas nativas `/ws` e `/health` continuam disponíveis.

Compatibilidade do protocolo não significa ausência de interrupção no deploy: a
[Cloudflare informa que atualizar o código reinicia os Durable Objects e desconecta
os WebSockets](https://developers.cloudflare.com/durable-objects/best-practices/websockets/).
O cliente nativo tenta reconectar. Levar isso em conta ao escolher o momento da publicação.

Decisão: manter um único worker e publicar suas mudanças juntas, depois de terminar
o trabalho relacionado a ele. Por enquanto, não publicar. A variável
`LITECORD_WEB_SIGNALING_URL` permite usar um worker local apenas para os testes web,
sem alterar o endereço da sinalização nativa.

```powershell
cd cloudflare
npx wrangler deploy
```

Não há TURN. NAT simétrico ou redes que bloqueiam UDP podem impedir a conexão;
STUN não garante conectividade em qualquer rede. Uma queda da sinalização encerra
a sessão; gere um novo link para reconectar. Falta validar uma transmissão real
entre redes distintas, outros navegadores, Linux e o custo em resolução/FPS de jogo.

## Troca manual de oferta/resposta, sem worker

Este é um modo de teste, ativado explicitamente. A transmissão continua saindo do
codificador real do app. STUN descobre endereços, mas nenhum servidor faz sinalização.

Com o executável de teste já compilado em `target/web-share-e2e/litecord-web-test.exe`:

```powershell
powershell -File scripts/perf/start_web_manual.ps1
```

Para recompilar essa build manualmente:

```powershell
cargo build --release --bin litecord --features automation
$env:LITECORD_WEB_MANUAL = '1'
$env:LITECORD_AUTOMATION_PORT = '39091'
.\target\release\litecord.exe
```

Inicie a transmissão e clique em **Compartilhar na web → Copiar link**. Abra o link
local `file:///…#manual` e execute o comando abaixo no terminal:

```powershell
powershell -File scripts/perf/web_share_manual.ps1 -Port 39091
```

Se a área de transferência ainda não tiver uma oferta válida, o script aguarda:
selecione todo o texto do primeiro campo do navegador e copie; volte ao terminal
e pressione Enter. Assim, copiar o próprio comando não faz o teste falhar. O script
substitui a área de transferência pela resposta. Cole-a no segundo campo do
navegador e clique em **Conectar** e depois **Ativar som**. Use Q para cancelar.

A automação só escuta em
localhost. Para testar outro computador, ele pode abrir a mesma `watch.html#manual`
e enviar sua oferta como texto; quem roda o Litecord devolve a resposta. O SDP
contém endereços de rede: troque-o em particular. Isso não contorna NAT simétrico
nem bloqueio de UDP. Sem a sinalização, o encerramento remoto só é percebido pelo
timeout do WebRTC, e fechar uma aba pode demorar para liberar a vaga.

O teste automático do mesmo fluxo dispensa copiar e colar à mão:

```powershell
node scripts/perf/web_share_e2e.cjs target/release/deps/litecord-<hash>.exe --manual
```

Não distribuir builds com `automation`. O modo normal continua usando o worker único.

Validação local em 17/10/2026: modo manual passou no Chrome com o worker de teste
desligado, incluindo vídeo H.264 e áudio Opus não silencioso. Também passaram os
46 testes Rust e os quatro testes de roteamento do worker. O caminho automático
foi validado anteriormente com três espectadores e regressão do protocolo nativo.
