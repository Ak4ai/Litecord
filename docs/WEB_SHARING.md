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

## Captura real e matriz de navegadores

Com o worker local iniciado e os navegadores já instalados, use uma build de
release isolada para medir o transmissor. O script abre a conta de teste indicada,
seleciona pelo nome somente o servidor `Ricogames_ e MariaJulia_` e o canal
`fofocas`, e encerra apenas o processo que iniciou. A saída de áudio fica muda.

```powershell
cargo build --target-dir target --release --bin litecord --features automation --offline
node scripts/perf/web_share_real.cjs --account=Marido --seconds=20 --rounds=2 --quiet-builds=1
```

A captura usa o desktop real do Windows a 1080p60, sem áudio, com o app na bandeja.
Uma janela própria dimensionada para o monitor principal mostra movimento constante;
ela é capturada pelo caminho normal do app, sem injetar frames. Não é um jogo e
seu custo não representa todo conteúdo possível. Chrome headless recebe o vídeo
durante as medições; Edge e Firefox também verificam a reprodução real ao final.

Cada rodada mede: transmissão sem web, web sem espectadores, com um espectador e
com três. Há uma última amostra sem web para observar deriva. CPU é porcentagem de
**um núcleo lógico**, memória é working set e private em MiB. O script chama os
coletores existentes `process_sample.py` e `wakeups.ps1`, registra resolução/FPS
recebidos e salva dados em `target/web-share-real/<data>/`. Falha do coletor de
acordadas fica registrada separadamente. Feche outras compilações e mantenha o
conteúdo da tela igual ao comparar amostras.

`--quiet-builds=1` aguarda até dez minutos pelo fim de compiladores antes de iniciar
o app e antes de cada amostra. Se uma compilação começa durante a coleta, o script
preserva a amostra e repete a fase (até três tentativas). PIDs de compiladores no
início/fim ficam no JSON. FPS usa `framesDecoded` do WebRTC: o Firefox 158 retorna
zero em `getVideoPlaybackQuality().totalVideoFrames` mesmo decodificando normalmente.

O teste sintético, que inclui áudio Opus não silencioso, aceita cada navegador
instalado, sem baixar outro navegador nem usar o perfil pessoal:

```powershell
node scripts/perf/web_share_e2e.cjs target/debug/deps/litecord-<hash>.exe --browser=chrome
node scripts/perf/web_share_e2e.cjs target/debug/deps/litecord-<hash>.exe --browser=edge
node scripts/perf/web_share_e2e.cjs target/debug/deps/litecord-<hash>.exe --browser=firefox
```

O Firefox instalado usa o canal `moz-firefox` do Playwright (WebDriver BiDi).
O app anuncia o IPv4 da rede local, em vez de depender de candidato `127.0.0.1`.
Os resultados sintéticos ficam em `target/web-share-e2e/<navegador>/`; não servem
para estimar o custo da captura real. Nenhum desses scripts publica o worker.

### Resultados de 18/10/2026 e pendências

No Windows, passaram os testes completos com Chrome **155.0.8059.84**, Edge
**155.0.4283.62** e Firefox **158.0**: H.264, Opus não silencioso, chave errada,
três espectadores, recusa do quarto, liberação da vaga, encerramento e regressão
da sinalização nativa. Nos três, o par ICE selecionado foi host/host e o candidato
remoto não era loopback. Esses testes usam a fonte sintética de 640×360.

A captura real foi executada com a build de release do código `6146e6e`, em um
Core i5-13450HX (16 processadores lógicos), 8 GB de RAM e NVIDIA RTX 3050 Laptop
6 GB, usando NVENC, desktop de 1920×1080, alvo de 60 FPS e 8 Mbps, sem áudio e com
o app na bandeja. Chrome recebeu 1920×1080 com três espectadores a **59,3–60,0
FPS** nas amostras estáveis; Edge também reproduziu a captura real em 1080p.

Foram coletadas duas rodadas de 20 segundos por fase. A tabela abaixo preserva
o intervalo observado; **é uma série exploratória, com interferência de
compilações, e não uma comparação controlada do custo adicional da web**:

| Captura real | CPU (% de um núcleo) | Working set (MiB) | Private (MiB) |
| --- | ---: | ---: | ---: |
| Web desligada | 50,78–60,00 | 79,08–93,23 | 517,01–543,71 |
| Web, zero espectadores | 52,73–53,83 | 83,67–118,47 | 528,04–581,50 |
| Web, um espectador | 57,89–60,86 | 91,83–125,03 | 544,07–606,32 |
| Web, três espectadores | 48,51–58,28 | 98,63–119,73 | 558,13–611,48 |

Os dados estão em `target/web-share-real/2026-10-18T03-11-31-686Z/`. Durante a
compilação concorrente, uma amostra com um espectador teve recepção média de
33,87 FPS; a fase seguinte voltou a aproximadamente 60 FPS nos três. Memória é
o valor ao fim de cada amostra, e o processo acumula caches entre as fases. Não
se pode interpretar os números menores de CPU com três espectadores como ganho.

A repetição sem compiladores (`2026-10-18T03-17-47-905Z`) obteve uma amostra sem
web a 60 FPS, mas foi interrompida às **00:20:45, horário de Brasília**, por uma
desconexão da conta de teste do canal `fofocas`. O app encerrou captura e web; os
dados posteriores a essa desconexão são inválidos para a comparação. O coletor
agora valida chamada, captura e quantidade de espectadores antes/depois da amostra.

**Falta concluir a repetição controlada com a conta reservada por cerca de quatro
minutos e a captura real no Firefox.** O teste sintético do Firefox já passou;
o contador corrigido usa `framesDecoded`, pois o contador do elemento de vídeo
retornava zero. Não foi realizado teste entre redes distintas, nem deploy.

## Roteiro entre redes diferentes, após publicação autorizada

1. No computador transmissor, abra o diagnóstico em **Rede & Proxy → Testar rede**
   antes e durante a chamada. Guarde o relatório, horário e tipo de conexão.
2. Inicie uma captura real com resolução/FPS anotados, habilite o compartilhamento
   web e envie o link completo em particular ao aparelho de teste.
3. No celular, desligue o Wi-Fi e confirme que está em 4G/5G. Abra o link em HTTPS.
   Anote navegador/versão, operadora e tempo até aparecer a primeira imagem.
4. Observe por pelo menos dois minutos. Confira movimento contínuo, tela cheia,
   rotação e, se a captura incluir áudio, o botão **Ativar som**. Grave pausas,
   atraso percebido, perda de áudio e o número de espectadores mostrado no app.
5. Abra mais dois espectadores, tente um quarto, feche um e tente novamente.
   Confirme o aviso de lotação e a liberação da vaga. Encerre a web no app e
   confirme o aviso de encerramento nos aparelhos.
6. Repita em outra rede Wi-Fi e, se possível, invertendo a rede do transmissor.
   Anote cada combinação separadamente: uma conexão local bem-sucedida não valida
   NAT entre redes diferentes.

Se falhar, colete: horário, mensagem exata da página, estado/contagem de espectadores
do app, relatório de diagnóstico de rede, versões, tipos de rede e se funciona na
mesma LAN. No computador receptor, `chrome://webrtc-internals`,
`edge://webrtc-internals` ou `about:webrtc` ajudam a registrar estados ICE/DTLS,
tipo do par selecionado (host/srflx), codec, bytes recebidos e quadros decodificados.
Não publique o link/chave, tokens ou SDP bruto; esses dados podem conter acesso à
sessão ou endereços. Relatórios do app já mascaram endereços públicos.

Página indisponível aponta primeiro para HTTPS/rota/deploy. Acesso recusado pede
um link novo da sessão atual. ICE falhando sem bytes de mídia sugere bloqueio UDP
ou NAT incompatível; o diagnóstico distingue alguns desses casos, mas não garante
que dois NATs sejam compatíveis. Há somente conexão direta com STUN, sem TURN.
Bytes chegando sem quadros decodificados pedem inspeção do codec e dos keyframes.

## Checklist para publicar o worker único

- Reunir todas as mudanças planejadas para o worker e revisar a integração na
  branch escolhida pelo responsável. A integração não faz parte destes scripts.
- Repetir testes Rust, `npm test` em `cloudflare/`, os três navegadores com worker
  local e as amostras reais. Registrar commit, números e pendências antes da release.
- Conferir `worker.js`, `web-share.js`, página importada, regra HTML, binding `ROOMS`
  e classe existente `SignalingRoom`. Não acrescentar migration de classe nova
  ao worker existente. Manter `/ws` nativo v2 e `/health` funcionando.
- Guardar a versão atualmente publicada e o procedimento de rollback. Escolher
  uma janela para a reconexão de WebSockets; links web ativos podem encerrar e
  precisar ser recriados. Comunicar esse efeito aos participantes.
- Pedir autorização para o deploy do conjunto revisado. Só então executar, da
  pasta `cloudflare/`, `npx wrangler deploy` **uma vez**. Push, tag e release também
  dependem de autorização própria; não são feitos por estes testes.
- Verificar `/health`, `/watch`, reconexão da sinalização nativa e o roteiro 4G/5G
  acima. Se necessário, reverter a versão do worker e desabilitar o compartilhamento
  web enquanto se investiga.
- Publicar somente a build normal, sem `automation`, após a validação acordada.

O teste entre redes diferentes continua pendente até esse deploy e a execução do
roteiro pelo usuário. Nenhuma publicação é necessária para validar o caminho local.

Validação local em 17/10/2026: modo manual passou no Chrome com o worker de teste
desligado, incluindo vídeo H.264 e áudio Opus não silencioso. Também passaram os
46 testes Rust e os quatro testes de roteamento do worker. O caminho automático
foi validado anteriormente com três espectadores e regressão do protocolo nativo.
