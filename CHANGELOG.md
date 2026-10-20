# Changelog

All notable changes to **Litecord** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [v1.2.1-beta] - não publicada — Leveza para quem joga (#27)

> **Atualização obrigatória.** As transmissões passam a ser enviadas só para quem pede para assistir, com uma chave sorteada a cada transmissão (protocolo 3). Versões anteriores não conseguem assistir nem ser assistidas por esta, então o servidor de sinalização passa a pedir a atualização.

### ⚡ Parado de verdade na bandeja
- **Laço infinito da interface com a janela escondida**: qualquer animação que começasse com o app na bandeja (alguém entrando num canal de voz, um contador de não lidas mudando) fazia o Slint pedir quadros para sempre, ~130 vezes por segundo, até a janela ser reaberta. A causa está no backend winit do Slint (também na 1.18.1); o Litecord passa a usar uma cópia local da versão 1.17.1 com a correção (`vendor/`).
- **A interface é desenhada pela CPU por padrão.** Sem contexto de GPU, o driver de vídeo não fica com threads acordando ao lado do jogo e a memória privada cai pela metade. A renderização pela GPU continua disponível em Configurações → Vídeo e Interface (vale ao reiniciar).
- Deixaram de acordar periodicamente: atalhos globais (liam o teclado a cada 20 ms; agora esperam a tecla), ícone da bandeja (120 ms) e sua dica (500 ms), barra do popout (300 ms), monitor de hardware escondido, recepção P2P, decodificador e sinalização fora de chamada, governador do timer, faxina de memória a cada 60 s.
- Com a janela escondida, mensagens, contadores de voz e ordem das DMs só são anotados; ao restaurar, tudo é aplicado de uma vez e o canal aberto é recarregado (antes isso só acontecia durante uma chamada).
- A conexão com o Discord lê só o tipo de cada evento e descarta sem interpretar os que o Litecord não usa (presenças, digitação, reações de outros servidores...).
- Resultado (Windows 11, fora de chamada): na bandeja, de 338 para **0,9 acordadas por segundo** e 0,00% de CPU; janela aberta em 0,08% de CPU; ~10 MB de RAM na bandeja e 16–25 MB com a janela aberta.

### 🎙️ Voz e chamadas
- **Chamadas não caem mais quando o Discord pede reconexão.** O Litecord reconectava abrindo uma sessão nova, e cerca de 1m45s depois o Discord tirava a conta da chamada (e encerrava a transmissão) porque a sessão que tinha entrado nela havia sumido. Isso acontece sozinho em chamadas longas, sem nenhuma falha de rede. Agora a sessão é retomada: num teste derrubando a conexão dos dois lados no meio de uma transmissão, ela voltou em 2 s e ninguém saiu da chamada.
- **Envio de voz no ritmo do microfone** em vez de um timer de 20 ms: o timer de 1 ms do Windows não é mais usado durante chamadas. No teste de ponta a ponta, o maior intervalo entre pacotes de fala caiu de 131 ms para 22 ms.
- **Microfone fechado ao sair da chamada**: ele continuava capturando até fechar o app, com o indicador de microfone do Windows ligado.
- **Entrar num canal não congela mais a interface**: o microfone é aberto numa thread própria (com headset sem fio isso podia levar segundos).
- O painel de voz só é redesenhado quando alguém entra, sai ou muda de volume; o medidor do microfone roda a 15 FPS e só com as configurações abertas, e ruído abaixo do limiar de voz não acorda mais a interface.
- Sem dispositivo de saída de áudio, a chamada procura de novo com espera crescente (até 5 s) e avisa uma vez, em vez de a cada meio segundo.
- O timer de 1 ms e a saída de áudio de transmissões ficavam ligados para sempre depois da primeira chamada; agora são liberados ao sair.

### 📡 Transmissão e sinalização
- **Escolha da GPU no Windows.** O automático usa a placa ligada ao monitor capturado, deixando a dedicada disponível para o jogo quando a tela está na integrada. As configurações também permitem escolher Intel, NVIDIA ou AMD presentes no computador, ou CPU; mostram a placa e o método realmente usados e explicam quando a imagem pode atravessar entre GPUs. A escolha vale na próxima transmissão e uma placa salva que não existe mais volta ao automático.
- **Transmitir sem a CPU tocar na imagem.** A tela capturada fica na GPU: a própria placa ajusta o tamanho, converte as cores e entrega ao codificador, sem as duas viagens GPU → memória → GPU por quadro. Quem transmite 1080p60 com alguém assistindo passou de 45–49% para **11–19% de um núcleo** (NVENC ou Quick Sync); a imagem chega com as mesmas cores da tela (diferença máxima de 1 nível). O NVENC passou a ser usado direto pela API do driver, sem FFmpeg, e notebooks só com Intel usam o Quick Sync. Se algo falhar, a transmissão volta sozinha ao caminho anterior.
- As transmissões do Media Foundation (Quick Sync) passam a declarar as cores no fluxo; navegadores e o Litecord adivinhavam a matriz e um dos dois via cores levemente erradas. Um quadro que demorava mais que o esperado saía grudado ao seguinte e o receptor perdia um dos dois.
- **Assistir sob demanda.** Uma transmissão nova aparece com o selo AO VIVO e o botão **Assistir**, como no Discord; nada é recebido antes disso. Quem transmite só envia para quem pediu, e sem ninguém assistindo não codifica nada e captura a tela só no ritmo da própria pré-visualização: **de 25–33% para 5% de um núcleo**. Esconder a transmissão, fechar ou deixar o Litecord na bandeja ou minimizado para de receber; ao voltar, a imagem reaparece em 0,1–0,45 s.
- **Chave por transmissão.** Antes, vídeo e áudio eram cifrados com uma chave derivada do ID do canal, e qualquer pessoa que o conhecesse podia entrar na sala de sinalização, passar a receber a transmissão e decifrá-la; o receptor também aceitava imagens sem criptografia. Agora cada transmissão sorteia uma chave, entregue só a quem pediu para assistir e está no canal de voz, cifrada para ele (X25519). Detalhes e limites em `docs/WATCH_ON_DEMAND.md`.
- **Qualidade que volta a subir.** A taxa só caía quando havia perda e nunca voltava. Agora começa cheia a cada transmissão, cai 20% com perda acima de 8% e sobe 0,5 Mbps a cada 5 s sem perda, seguindo o pior espectador do momento; quem sai deixa de contar.
- Fora de quem assiste e de quem transmite, ninguém troca heartbeats na chamada; o broadcast na rede local só acontece com o servidor de sinalização fora do ar.
- **Compartilhar na web**: link com chave por sessão, vídeo H.264 e áudio da transmissão em Opus por WebRTC, até três espectadores sem login. A página e a sinalização usam o worker existente; a mídia vai diretamente aos navegadores. Encerrar a web, parar a transmissão ou sair da chamada desconecta os espectadores. Sem TURN: redes restritivas podem não conectar. A atualização do worker precisa ser publicada antes do uso pela internet.
- Fora de chamada a recepção P2P dorme por completo (sem heartbeat nem broadcast); em chamada sem transmissão, acorda uma vez por segundo.
- Depois de sair de uma chamada, a sinalização voltava a entrar na sala antiga e o filtro de privacidade aceitava pacotes dela.
- A sinalização acorda por evento em vez de checar a cada 500 ms, e o keepalive passou de 15 s para 40 s.
- Avisos de quadro descartado viraram um resumo por segundo.
- **Sem ninguém assistindo e com a pré-visualização escondida, a captura da tela para por completo** (a sessão do Windows Graphics Capture é fechada e a duplicação DXGI deixa de pedir quadros): quem transmite fica em 1,7–2,2% de um núcleo com o Litecord na bandeja. O codificador é aberto assim que a captura começa, e a primeira imagem chega 0,24–0,54 s depois de alguém clicar em Assistir.
- **O áudio da transmissão vai em Opus** (64 kbit/s) em vez de PCM sem compressão (768 kbit/s).
- **Só quem transmite pode encerrar a própria transmissão.** O aviso de fim agora leva uma prova feita com a chave da transmissão; antes, qualquer pessoa que soubesse o ID do canal e o de um usuário podia derrubar a transmissão dele para todos com um único pacote. Um anúncio de presença dizendo que alguém parou, ou um aviso do servidor de sinalização de que ele saiu, também são ignorados enquanto o vídeo dele continua chegando com a chave certa.
- Navegadores assistindo pela web ouviam o áudio da transmissão com o volume errado e distorcido: o codificador Opus mono da biblioteca usada está quebrado. O áudio passa a ser codificado em estéreo (com os dois canais iguais).

### 🩺 Diagnóstico de rede
- Configurações → Rede & Proxy ganhou um **diagnóstico de rede**: testa se a transmissão com alguém de outra rede consegue conectar direto (tipo de NAT pelo STUN de três provedores, UDP bloqueado, servidor de sinalização) e copia um relatório para colar numa issue. O relatório esconde os dois últimos números do IP público.

### 🎞️ Assistir transmissões
- **No Windows, a transmissão é decodificada e exibida pela GPU sem passar pela CPU.** O decodificador da placa (Intel, NVIDIA ou AMD) recebe o vídeo direto, sem Media Foundation, e a imagem vai para a tela numa camada do Windows por baixo da interface, com os controles da transmissão desenhados por cima. Assistir uma transmissão 1080p a 60 FPS passou de **68% para 8% de um núcleo** (em tela cheia, de 106% para 8%). Vale para a janela principal e para a janela destacada.
- **Nada é decodificado quando nenhuma janela mostra a transmissão** (Litecord na bandeja ou minimizado durante um jogo): 2–3% de um núcleo, contra 53% antes. Ao voltar, a imagem reaparece em cerca de 0,1 s (0,5 s saindo da bandeja).
- Quando a GPU não pode decodificar a transmissão (máquina virtual, driver sem decodificador H.264, vídeo entrelaçado), ela continua pela CPU como antes, agora convertida já no tamanho em que é exibida (de 68% para 46% de um núcleo). O mesmo vale para quem usa a renderização da interface pela GPU, que não usa as camadas.
- As cores das transmissões feitas com NVENC estavam declaradas como BT.709 de faixa completa, mas o Litecord as converte em BT.601 de faixa limitada; quem confia na declaração (navegadores assistindo pela web, o decodificador da GPU) via os pretos acinzentados. A declaração foi corrigida, e o receptor trata as transmissões de versões antigas.
- **Transmitir com o codificador do Media Foundation** (escolhido à mão em Configurações → Vídeo) não funcionava: ele gravava um arquivo MP4 e mandava os bytes novos, e o cabeçalho com os parâmetros do vídeo só existe quando o arquivo termina. Agora o codificador da placa (Intel Quick Sync, NVIDIA ou AMD) é usado diretamente, com quadro-chave sob pedido.
- A etiqueta de qualidade voltou a mostrar a resolução da transmissão (mostrava o tamanho em que ela era desenhada).
- **Esconder e voltar a assistir não acumula mais memória, e esconder devolve tudo.** O dispositivo da GPU era soltado antes de tudo o que tinha sido criado nele (camadas, swap chain, processador de vídeo), e por isso nunca era destruído: cada vez que se voltava a assistir deixava 30–50 MB para trás, e em 10 ciclos o receptor ia de 154 para 686 MB. Além disso, a janela principal religava as camadas de uma transmissão que acabara de ser escondida. Agora tudo é destruído na ordem certa, esperando o compositor do Windows soltar a árvore de camadas: ao esconder, a memória volta de 174 para 66 MB em pouco mais de um segundo (56 MB antes de assistir), e em 15 ciclos de esconder e assistir fica entre 187 e 193 MB. Abrir e fechar o popout também deixou de acumular (~10 MB por vez antes).
- **Menos memória em geral**: o alocador passa a reservar memória só quando ela é usada. Com uma transmissão 1080p60, quem assiste usa 164–190 MB em vez de 228–247 MB e quem transmite 271 MB em vez de 310 MB, com a mesma CPU; fora de chamada o app começa com 40 MB em vez de 55 MB.
- A fila de decodificação caiu de 120 para 30 quadros (0,5 s): quando o receptor atrasa, a imagem não fica mais até 2 s atrás da transmissão. Engasgos de até meio segundo passam sem descartar nada.
- Quando quadros são descartados, o pedido de quadro-chave agora vai para **quem transmite** (ia para o próprio codificador), e o receptor espera esse quadro em vez de decodificar imagens corrompidas.

### 💬 Interface
- **Servidores na mesma ordem e pastas do Discord**, e servidores e canais podem ser reordenados arrastando (a ordem fica salva neste computador). Contribuição de @stickyfran (#39).
- **Abrir um servidor ficou instantâneo.** O chat esperava a lista de membros e os comandos de bots, que não são necessários para ler: agora as mensagens começam a carregar junto com a lista de canais (de ~1,1 s para ~1 ms depois dela), e membros e comandos vêm em segundo plano. Um servidor já visitado mostra os canais e o chat na hora, e a resposta do Discord só corrige a lista. Canais e permissões são buscados ao mesmo tempo. Trocar de servidor antes de o anterior terminar de carregar não deixa mais os canais do anterior na tela, e um canal aberto durante o carregamento não é trocado pelo primeiro canal. Ideia de @stickyfran (#38).
- **Barra do usuário como a do Discord**: avatar com indicador de status, nome de exibição e @usuário. Contribuição de @stickyfran (#40).
- **Mensagens de bots, enquetes e markdown**: mensagens só com embeds, imagens ou links deixaram de aparecer como "[Conteudo especial]"; enquetes mostram a pergunta e as opções; títulos, citações, listas, texto riscado e links em negrito são exibidos; datas do Discord (`<t:...>`) seguem o fuso local e o idioma do app. Contribuição de @stickyfran (#37).
- Avatares e ícones de servidor são arredondados na imagem carregada, corrigindo o recorte quadrado do renderizador por software.
- Canais de servidor não pulam mais para o topo da lista a cada mensagem (só DMs são ordenadas por atividade).
- Atualizar contadores de voz não desfaz mais a ordem das DMs nem apaga as não lidas.
- Com a renderização pela CPU (o novo padrão), a janela voltava da bandeja ou de minimizada transparente, mostrando só o que se mexia (os cards da chamada, os contadores de CPU e RAM). O Windows apaga o conteúdo de uma janela escondida e não avisa o renderizador; agora ela é redesenhada inteira ao voltar.
- A borda do card da transmissão fica por cima do vídeo: antes a imagem cobria a borda onde encostava, e os cantos do vídeo saíam da borda arredondada.

### 🐧 Linux
- Atalhos globais esperam eventos do teclado (`poll` no evdev) em vez de ler as teclas a cada 20 ms. Ainda não testado num Linux real.

### 🧪 Como esta versão foi testada
- **Ferramentas novas** (`scripts/perf/`, só em builds de teste): contagem de acordadas por thread, custo parado e em chamada, e testes de ponta a ponta que rodam duas contas lado a lado no mesmo PC.
- **Voz de ponta a ponta**: uma fala de referência atravessa o Discord e é comparada com o que o remetente capturou: mediana de semelhança 0,99, 2 de 283 trechos danificados, 0,2% de perda; um pacote a cada 20 ms (máximo 21 ms). No ritmo antigo, por timer, o maior intervalo chegou a 131 ms.
- **Em chamada com microfone real**: CPU de 13,6% para 0,6% de um núcleo com a janela aberta e de 4,0% para 0,4% na bandeja.
- **Receptor lento**: engasgos simulados de 300 ms passam sem descarte; de 800 ms, um único pedido de quadro-chave e volta a 60 FPS no segundo seguinte.
- **Transmissão**: abre na hora no outro cliente, 1080p a 60 FPS. CPU do receptor contada em ciclos (a contagem de tempo do Windows deixa de fora threads que acordam por pouco tempo, como uma de 60 FPS): 68% de um núcleo antes, 46% com a conversão no tamanho exibido, 34% decodificando pela GPU e copiando de volta, **8% com as camadas**; em tela cheia, 103–110% pela CPU contra 8%. Na bandeja ou minimizado, 2–3%.
- **Decodificador da GPU**: comparado quadro a quadro com o OpenH264 e o ffmpeg em placas Intel e NVIDIA, com vídeos de x264 (vários perfis, quadros B, várias referências e fatias), NVENC, Quick Sync, Media Foundation e OpenH264 configurado como no app: todos idênticos. Ao perder pacotes, para de mostrar imagens até o próximo quadro-chave, sem mostrar imagem corrompida.
- Enquanto alguém assiste pela GPU, o receptor usa cerca de 80 MB a mais de memória; o driver de vídeo guarda parte dela depois (medido isolado: até ~100 MB comprometidos, fora da memória em uso).
- 70 testes unitários (incluindo a contagem de quadros de referência do H.264 com vídeos sintéticos e a negociação da transmissão pela web) e 6 testes do worker. A correção da animação de rolagem no core do Slint tem teste no próprio crate; a do laço com a janela escondida foi verificada medindo o app (132 → 0 acordadas/s).
- Medido só neste PC (Intel e NVIDIA); falta AMD.

## [v1.2.0-beta] - 2026-10-12 — Nova sinalização P2P, chat agrupado, fotos de perfil & instalação Linux corrigida

> **Atualização necessária para transmitir tela pela internet.** O servidor de sinalização passou a aceitar só o protocolo desta versão; clientes v1.1.0 ou anteriores continuam com voz, chat e transmissão na rede local, mas não encontram pares pela internet até atualizar.
>
> **Required update for screen sharing over the internet.** The signaling server now only accepts this version's protocol; v1.1.0 and older clients keep voice, chat and LAN streaming, but cannot find peers over the internet until they update.

---

### 📡 Sinalização P2P e Transmissão de Tela
- **Presença orientada a eventos**: o cliente anunciava presença a cada 2 s durante toda a chamada, o que esgotava o servidor de sinalização. Agora anuncia ao entrar, quando outro participante entra, quando a transmissão começa ou para e quando o endereço de rede muda.
- **Servidor de sinalização** (`cloudflare/`): o código passou a fazer parte do repositório, usa hibernação de WebSocket e avisa a sala quando alguém entra ou sai. O ID do usuário que saiu chegava corrompido (número JSON acima de 2^53) e agora é enviado como texto.
- **Fim da transmissão**: quadros ainda em trânsito depois de parar reabriam a visualização congelada no último quadro, sem nada que a fechasse.
- **Receptores lentos**: um receptor sem capacidade de decodificar podia pedir vários quadros-chave por segundo e derrubar a taxa de quadros do transmissor; os pedidos ficam limitados a um por segundo dos dois lados.
- **Logs**: o registro de cada pacote recebido (93% de um log de 48 MB) virou um resumo a cada 10 s; heartbeat e endereço público só são registrados quando mudam.
- Reconexão ao servidor de sinalização com espera crescente (2 s a 60 s) e endereço configurável por `LITECORD_SIGNALING_URL`.

### 💬 Chat & Fotos de Perfil
- **Mensagens agrupadas** (PR #26, por **Starzinho — [@starzynhobr](https://github.com/starzynhobr)**): mensagens consecutivas do mesmo autor são agrupadas, com separadores de dia e avatares persistentes; as linhas são atualizadas de forma incremental, sem recriar a lista. Todo o desenho e a implementação do agrupamento e da hierarquia de avatar/nome são dele.
- **Fotos de perfil em mais lugares** (#25): lista de DMs, membros de voz na barra lateral e painel de voz. Reaproveita o cache de avatares existente (miniaturas 40x40, limite de memória e de disco) e não faz requisições novas.

### ⚡ Desempenho e Segundo Plano (#27)
- O monitor de hardware só consulta o contador de GPU com a janela visível e o HUD ligado; na bandeja amostra CPU/RAM a cada 10 s.
- O perfilador de threads do sistema passou a ser opcional (`LITECORD_CPU_PROFILER=1`).
- A resolução de timer de 1 ms do Windows só é mantida durante chamadas de voz e streams; a saída de áudio de streams só é aberta quando há áudio chegando.
- A thread de recepção P2P reduz o polling quando está ociosa.
- Medido em benchmark sintético: custo em idle caiu de +2,0% para +1,2% sobre o baseline; em voz ficou em ~+5,3% (v1.1.0-beta: +6,0%). Efeito em jogos reais ainda não medido.

### 🪪 Identificação do cliente (#30)
- REST, Gateway e login por QR declaravam três ambientes diferentes (Electron/Chrome 120, Chrome 127 e Chrome 130), sempre como Windows e `pt-BR`. Um único módulo (`utils/client_identity`) passa a gerar o `User-Agent`, as propriedades do Gateway e um cabeçalho `X-Super-Properties` correspondente a partir das mesmas constantes, como cliente web no Chrome, com o sistema operacional e o idioma reais.
- Campos opcionais que os clientes oficiais geram a cada abertura (`launch_signature`, identificadores de sessão) ficaram de fora de propósito.

### 🌍 Idiomas (#29)
- Cerca de 190 textos que estavam fixos em português (login/QR, configurações, compartilhamento de tela, contas, atualizações e mensagens de erro) agora seguem o idioma escolhido.
- Inglês e português completos; espanhol, alemão, francês, russo e japonês entram como tradução de máquina, a revisar por falantes nativos.

### 🐧 Linux
- **install.sh no Fedora/Nobara** (#28): os pacotes passam a ser instalados um a um, com os nomes corretos; antes uma falha numa instalação atômica do `dnf` deixava tudo sem instalar, em silêncio.
- **AppImage Anylinux** (#21, #28): workflow reescrito conforme o guia (container Arch, `quick-sharun`, OpenGL/PipeWire/GTK/GStreamer empacotados). O AppImage gerado pelo GitHub Actions foi validado numa VM Fedora 43 KDE Wayland (abre e inicia).

### 🔧 Dependências
- `futures-util` 0.3.34 (PR #15), `windows-sys` 0.61.2 (PR #17), `x25519-dalek` 3.0.0 (PR #14), `sha2` 0.11 (PR #13) e `openh264` 0.9 (PR #16).

### 🧪 Como esta versão foi testada
Testes feitos em 11 e 12/10/2026, com Windows 11 (RTX 3050, NVENC) de um lado e uma VM Fedora 43 KDE Wayland (sem GPU, 2 vCPUs) do outro, em contas diferentes.

- **Testes automáticos**: 28 testes unitários (tabelas de idioma, formato de data por idioma, agrupamento de mensagens, identidade do cliente).
- **Servidor de sinalização**: 16 testes de protocolo contra o servidor em produção (sala, entrada e saída de participantes, repasse sem eco, keepalive, isolamento entre salas, clientes antigos estacionados).
- **Consumo do servidor de sinalização** (medido na Cloudflare): de ~960 GB-s para menos de 0,01 GB-s a cada 15 minutos; uma sala com dois clientes conectados ficou em milésimos de GB-s.
- **Transmissão de tela Windows ↔ Linux**: nos dois sentidos, com início, parada e reinício em seguida; sessões contínuas de 12 e de 7 minutos, sem crescimento de memória e com o envio do Windows estável em 60 FPS.
- **Custo de CPU ao transmitir no Windows**: 92% de um núcleo com OpenH264 (CPU), ~1–2% com NVENC depois da redução de logs.
- **Tamanho do log**: de 48 MB para 0,27 MB numa sessão equivalente.
- **Login**: token salvo, detecção de token e QR code (QR validado no Linux), com a identidade nova, sem erro de autenticação.
- **Voz**: chamadas em servidor entre os dois sistemas.
- **Linux**: `install.sh` e o AppImage gerado pelo GitHub Actions abrem e conectam no Fedora 43 KDE Wayland.
- **Idiomas**: troca de idioma conferida em telas reais em alemão, russo e japonês.
- Recurso de compilação opcional `automation` (desligado por padrão e ausente dos binários publicados), usado para dirigir a interface nesses testes: captura da janela, cliques, teclado e leitura de estado, apenas em `127.0.0.1`.

### ⚠️ Não validado
- Pares em redes diferentes (os dois lados do teste saíam pelo mesmo IP público) e mais de dois participantes na mesma sala.
- Linux com placa de vídeo: a VM renderiza e decodifica por software, então os números de desempenho no Linux (memória, FPS de recepção) não representam uma máquina real.
- Login por QR no Windows com a identidade nova, voz em DM e uso prolongado do `openh264` 0.9.
- Efeito em jogos reais das mudanças de #27 (só medido em benchmark sintético).
- As traduções de espanhol, alemão, francês, russo e japonês não foram revisadas por falantes nativos.

### 🙏 Créditos
- **Starzinho ([@starzynhobr](https://github.com/starzynhobr))** — autor da PR #26 (mensagens agrupadas, separadores de dia e avatares persistentes).
- Quem reportou os problemas corrigidos nesta versão: #21, #25, #27, #28, #29 e #30.

---

## [v1.1.0-beta] - 2026-09-23 — Mensagens Diretas (DMs), Chamadas de Voz Privadas & AnyLinux AppImage

Esta versão introduz suporte completo e nativo a **Mensagens Diretas (DMs)**, **Chamadas de Voz Privadas (DM Voice Calls)**, contadores e badges de mensagens não lidas com sincronização em tempo real e o empacotamento universal **AppImage AnyLinux (distro-agnostic)** via GitHub Actions.

---

### 💬 Mensagens Diretas (DMs) & Conversas em Grupo
- **Navegação e Interface de DMs**:
  - Novo botão de DMs no topo da barra lateral de servidores com ícone vetorial dedicado (`assets/dm-chat.svg`).
  - Lista lateral com suporte a canais individuais (1:1) e grupos (Group DMs).
  - Avatares com iniciais coloridas para cada usuário/grupo, status de presença e nomes atualizados.
- **Sincronização Gateway em Tempo Real**:
  - Carregamento de canais privados via REST API com atualização dinâmica de mensagens via Gateway Discord v10.
  - Reordenação automática da lista ao receber novas mensagens.

### 📞 Chamadas de Voz Privadas em DMs (Ring & Call Management)
- **Chamada de Voz em 1 Clique**:
  - Botão de ligação telefônica (`assets/phone.svg`) posicionado diretamente no cabeçalho da DM ativa.
- **Modal de Chamada Recebida (Incoming Call Modal)**:
  - Notificação visual animada sobreposta informando nome e avatar do remetente da ligação.
  - Botões dedicados de **Atender** (verde) e **Recusar** (vermelho, `assets/phone-slash.svg`).
  - Prevenção de loop/auto-chamada: ao iniciar uma ligação, o originador ingressa direto no canal de voz sem disparar o modal para si mesmo.
  - Feedback sonoro de toques e chamada atendida/encerrada.

### 🔴 Badges de Mensagens Não Lidas & Sincronização Precisa
- **Badge Global Agregada**:
  - Contador exibido sobre o ícone de DMs na barra de servidores indicando o total de mensagens não lidas em todas as conversas.
- **Badges Individuais por DM**:
  - Badge vermelha no formato pill com contagem individual ao lado do nome da conversa.
  - Formatação inteligente para números altos (`99+`).
  - Alinhamento geométrico e centralização óptica idênticos à badge de contagem de usuários de canais de voz.
- **Desduplicação Inteligente de Mensagens**:
  - Cache de controle para prevenir contagens espúrias decorrentes de múltiplos disparos de eventos `MESSAGE_CREATE`.
  - Zeramento automático e remoção imediata da badge ao selecionar e visualizar a conversa.

### 🎨 Tema Claro (Light Theme), Participantes em Canais de Voz & Filtro de Permissões (por @starzynhobr — PRs #22, #23)
- **Tema Claro (Light Theme) Nativo**:
  - Seletor de Tema Claro nas Configurações do aplicativo com persistência automática no arquivo de configurações do usuário.
  - Centralização de tokens de cores semânticos no módulo `AppTheme` do Slint para consistência visual.
- **Visualização de Participantes em Salas de Voz**:
  - Exibição direta dos usuários presentes em canais de voz na barra lateral antes de conectar.
  - Linhas compactas com avatar de iniciais e nome do usuário.
  - Clique na linha do participante conecta automaticamente à sala de voz correspondente.
  - Mecanismo de impressão digital (*fingerprinting*) para detectar entrada/saída de membros sem recarregamento desnecessário.
- **Filtro de Permissões de Canais (`VIEW_CHANNEL`)**:
  - Cálculo local de permissões do Discord respeitando a precedência oficial (dono/administrador, roles e sobrescritas de canal/membro).
  - Canais inacessíveis são automaticamente ocultados da barra lateral e categorias vazias são removidas, eliminando a poluição visual.
- **Aprimoramentos de Clareza Visual e UX**:
  - Superfícies mais planas e limpas nos canais e mensagens, eliminando o efeito de caixas excessivas.
  - Cursores de ponteiro (*hand pointer*) aplicados a botões e controles interativos.
  - Barra de rolagem nativa arrastável (`ScrollView`) na barra lateral de canais.
  - Modularização de componentes Slint em arquivos dedicados (`channel_sidebar.slint`, `chat_messages.slint`, `settings.slint`, `icon_button.slint`).

### 🐧 AnyLinux Universal AppImage & CI/CD
- **Distribuição Universal para Linux**:
  - Geração automatizada do pacote `Litecord-x86_64.AppImage` no GitHub Actions através do `quick-sharun`.
  - Execução garantida em qualquer distribuição Linux x86_64 moderna ou antiga (Ubuntu, Debian, Fedora, Arch Linux, Void, etc.), sem conflitos de `glibc`.
  - Todas as bibliotecas dinâmicas compartilhadas incluídas diretamente no bundle (incluindo `libxdo.so.3`, `libayatana-appindicator3`, `libfontconfig`, `libasound`).
  - Integração de desktop com arquivo `.desktop` (`assets/litecord.desktop`) e ícone oficial em alta resolução (`assets/litecord.png`).

---

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
