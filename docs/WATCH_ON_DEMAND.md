# Assistir sob demanda (protocolo 3)

Quem transmite só envia para quem está assistindo, e só codifica enquanto alguém
assiste (no app ou pelo navegador). Quem está na chamada sem assistir não recebe
nada da transmissão.

## Como era (protocolo 2)

- O transmissor mandava cada quadro e o áudio da transmissão para **todos** os pares
  que se anunciaram na sala de sinalização nos últimos 60 s, assistindo ou não.
- Vídeo, áudio e presença eram cifrados com uma chave derivada do **ID do canal**.
  O ID não é segredo: qualquer pessoa que o conhecesse podia entrar na sala do
  worker, anunciar presença, passar a receber a transmissão e decifrá-la. A troca
  X25519 que existia era calculada e nunca usada.
- A taxa só diminuía (perda > 8% → −20%), nunca voltava a subir e não era
  reiniciada entre transmissões.
- Em chamada, todos trocavam heartbeats a cada 1,5 s e faziam broadcast na rede local.

## Como fica

### Pedido de assistir

- Uma transmissão nova aparece **sem vídeo**, com o botão **Assistir** (como no
  Discord). Assistir, fechar a transmissão e "esconder" passam a significar começar
  ou parar de receber.
- Enquanto quem assiste quer a transmissão **e alguma janela pode mostrá-la**, o app
  manda `OP_WATCH` ao transmissor uma vez por segundo, pelo socket P2P. Na bandeja,
  minimizado ou com a transmissão escondida, o pedido para; ao voltar, recomeça e o
  transmissor gera um quadro-chave na hora.
- `OP_WATCH` leva a chave pública X25519 de quem assiste. O transmissor só aceita
  quem está **no mesmo canal de voz do Discord** (estado de voz do gateway).
- O pedido vale por 4 s. Sem renovação, quem assiste sai da lista. `OP_UNWATCH`
  encerra na hora.

### Chave por transmissão

- Cada transmissão sorteia uma chave AES-256 nova. Vídeo, FEC e áudio da transmissão
  são cifrados com ela, uma vez por pacote, para todos os espectadores.
- O transmissor responde a cada pedido com `OP_WATCH_ACK`: sua chave pública e a
  chave da transmissão cifrada com a chave do par (X25519 + SHA-256, a mesma dos dois
  lados). Só quem fez o pedido consegue abri-la.
- Quem não pediu, ou foi recusado, não recebe pacotes e não teria como decifrá-los.

**Limite:** o Discord não oferece um jeito de um cliente provar a outro quem é. Um
atacante que conheça o ID do canal ainda pode fingir ser alguém **que está no canal
de voz** e pedir a transmissão em nome dele. O transmissor associa cada pessoa à
primeira chave pública que aparecer na transmissão e recusa outra chave para o
mesmo ID. Isso fecha o caso comum (quem nem está na chamada), mas não é uma prova de
identidade.

### Taxa

- Reiniciada no valor configurado a cada transmissão.
- Decidida pelo **pior espectador** com relatório nos últimos 3 s. Quem sai deixa de
  contar quando o pedido expira.
- Perda acima de 8%: −20% (mínimo 3 Mbps). Perda abaixo de 2% por 5 s seguidos:
  +0,5 Mbps, até o valor configurado.

### Temporizadores

- Sem espectador: o transmissor não codifica nem envia vídeo e áudio, e a captura
  cai para o ritmo da pré-visualização local (4 quadros por segundo). O anúncio do
  transmissor (a cada 1,5 s) continua, para que quem chega possa encontrá-lo.
- Os heartbeats entre pares ficam só onde há transmissão: entre quem assiste e quem
  transmite (o próprio `OP_WATCH`). O broadcast na rede local só acontece quando a
  sinalização está fora do ar.
- O relatório de perda só vai para transmissões assistidas.

### Navegador

A transmissão pela web já é sob demanda: o vídeo só é copiado para a sessão WebRTC
com espectadores conectados. Agora ela também conta como espectador para o
codificador: com só o navegador assistindo, o app codifica e não envia nada pelo P2P.

## Compatibilidade

Versões antigas mandam para todos com a chave do canal e não sabem pedir. Elas não
conseguem assistir a quem usa o protocolo 3, e vice-versa. Por isso a versão que
traz o protocolo 3 é **atualização obrigatória**: o worker passa a exigir `v=3`
(`MIN_PROTOCOL = 3`) e estaciona as versões antigas, como na 1.2.0. O app antigo
para de achar pares pela internet até atualizar.

Publicar o worker antes da versão nova deixaria todos sem transmissão até a
atualização; o worker deve ir junto com a release.

## Medido (Windows 11, 1080p60, duas contas no mesmo PC)

`scripts/perf/watch_on_demand.ps1`:

| Situação | Quem assiste recebe | CPU de quem transmite |
|---|---|---|
| Ninguém assistindo | nada | 4,5–5,9% de um núcleo (antes: 25–33%, capturando a 60 FPS) |
| Assistindo | ~60 FPS | 45–49% |
| Transmissão escondida | nada | 5–6% |
| Quem assiste na bandeja | nada | 5–6% |

A primeira imagem chega 0,2–0,7 s depois de **Assistir**; ao voltar da bandeja ou
mostrar de novo, 0,1–0,45 s (antes 0,12 s, porque o vídeo continuava chegando).
Quem assiste gasta 1,3–1,5% de um núcleo na bandeja (antes 2,2%), porque nada chega.
Com só um navegador assistindo, o app codifica normalmente (Chrome, Edge e Firefox
recebem 1080p60 da captura real); sem ninguém, nativo ou web, a captura fica em 4 FPS.

## Limites que continuam

- A presença na sinalização (endereços IP e "estou transmitindo") ainda é cifrada
  com a chave do canal: quem souber o ID do canal e entrar na sala do worker vê os
  endereços de quem está nela, mas não recebe nem decifra a transmissão.
- `OP_STOP` não é autenticado: alguém na mesma rede poderia fazer a transmissão de
  outra pessoa sumir da tela de quem assiste até o próximo anúncio.
- Sem TURN: redes que bloqueiam UDP ou NAT simétrico nos dois lados não conectam.

## Testes

- Unitários: derivação da chave do par (igual dos dois lados), embrulho da chave da
  transmissão, expiração dos pedidos, recusa de quem não está no canal ou troca de
  chave, ajuste de taxa.
- Ponta a ponta (`scripts/perf/watch_on_demand.ps1`): sem assistir, o transmissor não
  codifica e o receptor não recebe nada; ao assistir, a imagem chega; ao esconder,
  para; ao voltar, a imagem reaparece; e a regressão de `stream_e2e.ps1`.
