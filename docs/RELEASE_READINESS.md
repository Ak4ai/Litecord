# Pendências para a próxima release

Auditoria de 17/10/2026: decisões registradas, plano, código e GitHub.
Este documento registra o estado; não autoriza publicação.

## Decisões vigentes

- Manter um único worker. Terminar as mudanças relacionadas a ele e publicar juntas.
- Testar a web por oferta/resposta manual ou worker local enquanto isso.
- Deixar VM/Linux e NAT simulado para o fim do plano.
- Não medir 1% low, conforme decisão registrada no plano.
- O lote de leveza da v1.2.1 foi separado das etapas maiores 4, 5, 6.2 e 7.
  Depois a web foi antecipada. As etapas maiores não devem ser confundidas com
  correções pequenas já concluídas nem consideradas prontas por estarem no plano.

## Já concluído

- Lote de leveza: renderizador por CPU como padrão com opção GPU, microfone em
  thread própria, atualização da interface por evento/visibilidade, correções de
  timers e dispositivos que continuavam ativos após sair da chamada.
- Fila de decodificação de 120 para 30 quadros, pedido de quadro-chave dirigido ao
  transmissor e espera crescente quando não há dispositivo de saída.
- Diagnóstico de rede com STUN, sinalização e relatório.
- Voz ponta a ponta e chamada com microfone real: PASS registrado nas medições
  anteriores; não foram repetidas nesta auditoria.
- Arredondamento de avatares/servidores no renderizador por software.
- Web: H.264 reaproveitado, áudio Opus, limite de três espectadores, chave por
  sessão, controles na interface, sinalização cifrada e modo manual de teste.
  Chrome validado localmente, com e sem worker; a publicação continua pendente.
- Botão de compartilhar na web reduzido a um globo de 32×32 px, com descrição e
  contagem de espectadores ao passar o mouse e rótulo de acessibilidade.

## Implementações maiores ainda pendentes

- [ ] **Etapa 4 — Receber vídeo com a GPU.** O receptor nativo ainda usa OpenH264
  por software. Falta decodificação por hardware e apresentação do vídeo pela GPU,
  avaliando o custo das cópias e preservando a interface leve por CPU.
- [ ] **Etapa 5 — Assistir sob demanda no protocolo nativo.** Receber/decodificar
  somente após clicar em assistir; enviar só a espectadores que solicitaram;
  esconder/parar de assistir deve interromper a recepção.
- [ ] Na mesma etapa: chave nativa por sessão, ajuste de bitrate que possa voltar
  a subir e seja reiniciado a cada transmissão, e expiração de espectadores.
  A chave aleatória da web não corrige o caminho nativo, que ainda deriva a chave
  do ID do canal.
- [ ] Planejar a transição do protocolo nativo e as mudanças do worker juntas.
  Definir compatibilidade/atualização necessária antes de alterar `MIN_PROTOCOL`;
  atualizar o worker pode desconectar temporariamente WebSockets existentes.
- [ ] **Etapa 6.2 — Prioridade de threads não relacionadas a áudio.** Comparar
  antes/depois com jogo rodando; só adotar se houver ganho sem prejudicar vídeo.

## Validações pendentes

- [ ] Transmissão real entre redes distintas, tanto nativa quanto web. STUN local
  e teste de NAT não comprovam esse cenário. Sem TURN, algumas redes não conectam.
- [ ] VM/Linux: executar atalhos globais e NAT simulado, deixados para o fim.
- [ ] Compilar e testar o estado atual no Linux. O CI anterior não cobre as novas
  mudanças de WebRTC/Opus/interface ainda locais.
- [ ] Medir custo de transmissão web com captura real, resolução/FPS alvo e
  um/três espectadores. As amostras da fonte sintética não provam 1080p60 real.
- [ ] Teste prolongado de 2–3 h para verificar memória/CPU; ainda não executado.
- [ ] Hardware AMD e outros navegadores: continuam sem validação.
- [ ] Medição final de leveza após as últimas mudanças, usando `scripts/perf/`.

O par de teste na nuvem era a **opção 2** do diagnóstico e ficou pendente; é uma
ferramenta para testar outras redes, não uma correção já autorizada/implementada.
A comparação direta com TeamSpeak também não foi feita nem é requisito definido.

## Fechamento da release no GitHub

- [ ] Revisar `CHROME_VERSION` e `CLIENT_BUILD_NUMBER` em
  `src/utils/client_identity.rs` com valores verificados, sem inventá-los.
- [ ] Consolidar e versionar as mudanças locais, revisar o diff e executar CI
  Linux/Windows para o commit final, incluindo os pacotes de distribuição.
- [ ] Atualizar versão em `Cargo.toml`, `Cargo.lock`, `installer.iss` e rótulos de
  download; completar data, testes e texto final do CHANGELOG.
- [ ] Revisar afirmações de desempenho dos READMEs: os números de CPU/RAM vêm
  deste PC; as promessas de zero perda de FPS não foram verificadas em jogos.
- [ ] Publicar o worker uma vez, junto da transição planejada; validar sinalização
  nativa e web após o deploy.
- [ ] Gerar artefatos de release **sem a feature `automation`**.
- [ ] Preparar/postar a issue dos bugs do Slint e o comentário da #27 solicitando
  teste em AMD. Os [rascunhos](RELEASE_COMMUNICATION_DRAFTS.md) foram
  preservados neste repositório a partir do scratchpad temporário; revisar textos,
  métricas e link do build antes de postar. Nada foi postado nesta auditoria.
- [ ] Criar tag/release somente após o fechamento e a validação dos artefatos.

Estado verificado no GitHub nesta auditoria: a última release continua sendo
`v1.2.0-beta` (12/10). O CI da branch `perf/idle-lightness` passou no commit
`8b5933fb70a85756ae401e3c84ac2866c167858b`; esse resultado é anterior ao HEAD
local `871e2dd` e às mudanças ainda não commitadas.

Referências: [plano](../plan.md), [changelog](../CHANGELOG.md),
[testes e uso da transmissão web](WEB_SHARING.md).
