# GPU da transmissão no Windows

Em **Configurações → Vídeo e Interface → Codificador de vídeo**, o aplicativo lista
as placas encontradas nesta máquina, além de **Automático** e **CPU (OpenH264)**.
A escolha vale para a próxima transmissão; trocar uma preferência enquanto alguém
assiste não recria a captura nem o codificador da sessão atual.

## Escolha automática

O aplicativo cria o dispositivo D3D11 na placa ligada ao monitor capturado. Para uma
janela, usa o monitor onde ela está ao iniciar a transmissão. Em notebooks com a tela
na Intel, isso permite usar Quick Sync e deixar a NVIDIA disponível para o jogo.
Em um monitor conectado à NVIDIA, o automático pode usar NVENC na própria NVIDIA.
Mover a janela para outro monitor durante a sessão não troca o dispositivo.

Para capturar um monitor, tenta primeiro **DXGI Desktop Duplication**, que exige um
dispositivo da placa à qual esse monitor está conectado. Se não conseguir, tenta
**Windows Graphics Capture (WGC)** no mesmo dispositivo. Para capturar uma janela,
usa WGC. Essa exigência do DXGI está na [documentação da Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutput1-duplicateoutput).

O automático evita uma transferência entre placas na configuração usual; não é
garantia contra falhas de driver, troca de monitor, conteúdo protegido ou falta de
suporte do codificador. Se a captura com dispositivo explícito não iniciar, o
aplicativo recorre à captura anterior, incluindo a reserva por GDI. Se a conversão
ou codificação de texturas falhar, a sessão continua pelo caminho com cópia para a
CPU. Na AMD, texturas direto no AMF continuam restritas à seleção manual até haver
validação em uma placa real.

## Seleção manual

- **Intel:** usa Quick Sync por Media Foundation no dispositivo escolhido.
- **NVIDIA:** usa a API NVENC do driver no dispositivo escolhido.
- **AMD:** tenta AMF por textura; ainda precisa de validação em hardware AMD.
- **CPU (OpenH264):** mantém a captura compatível e codifica por software.

Quando a placa escolhida não é a do monitor, a captura usa WGC nessa placa. O
Windows pode transferir a imagem entre as GPUs. As etapas de conversão e codificação
permanecem no dispositivo de captura, sem que o aplicativo leia os quadros completos
de volta para a memória;
a miniatura local continua sendo lida em tamanho reduzido. Isso não significa que
as GPUs deixem de compartilhar banda ou energia com o jogo.

Durante a captura com dispositivo explícito, as configurações mostram o nome da
placa, **DXGI/WGC** e se o processamento permanece na GPU ou passou ao modo de
compatibilidade. A informação muda por eventos de início, reserva e parada, sem
timer de atualização quando o app está ocioso.

As preferências são gravadas como `gpu:VVVV:DDDD` (IDs PCI do fabricante e modelo).
Valores antigos continuam aceitos: `nvenc` e `ffmpeg` escolhem NVIDIA, `wmf` escolhe
Intel, `amf` escolhe AMD e `openh264` escolhe CPU. Não há regravação automática das
preferências. Se a placa salva não existir, ela aparece como indisponível na lista
e a captura usa o automático. Duas placas de modelo idêntico compartilham a mesma
opção; escolher uma instância específica desse modelo não está implementado.

## Validação local

Build com `--features automation`, worker local já iniciado em `127.0.0.1:8787`:

```powershell
cargo build --release --target-dir target --features automation -j 2
node scripts/perf/gpu_selection.cjs
# Opcional: comparar com uma cópia do executável integrado anterior.
node scripts/perf/gpu_selection.cjs --baseline=D:/caminho/litecord.exe --seconds=10
```

O roteiro usa as contas de teste Marido/Henrique somente no servidor
`Ricogames_ e MariaJulia_`, canal `fofocas`. Silencia a saída do aplicativo, não
captura áudio da tela e encerra somente os processos que iniciou. Uma janela do
Chrome mostra conteúdo animado real para a captura do monitor. Não publica o worker.

Verifica transmissão e decodificação reais com seleção automática, cada GPU
encontrada, CPU e uma GPU salva inexistente; captura de uma janela por WGC; troca
de preferência durante a sessão; limpeza do estado e tempo de resposta ao parar;
e as opções traduzidas em sete idiomas. Mede ciclos de
CPU de quem transmite e memória, gravando os resultados e as capturas das
configurações em `target/gpu-selection/`. O percentual é de **um núcleo de 3 GHz**,
como nos demais roteiros de desempenho, e não do processador inteiro.

O roteiro exige que cada GPU listada tenha codificação por textura funcional. Para
placas sem suporte, a reserva deve ser validada manualmente. Não cobre HDR, rotação
do monitor, troca de GPU por remoção física, perda de dispositivo, várias placas do
mesmo modelo, Linux ou comportamento dentro de um jogo específico.

## Resultado em 19/10/2026

Windows 11, i5-13450HX, 8 GB de RAM, Intel UHD ligada à tela 1920×1080 e RTX 3050
Laptop 6 GB. Duas contas na mesma máquina, captura real animada, pedido de 1080p60,
saída silenciada e aplicativo transmissor na bandeja. Amostras de oito segundos,
CPU contada em ciclos e normalizada para um núcleo de 3 GHz:

| Seleção | Captura observada | CPU de quem transmite | FPS recebidos |
|---|---|---:|---:|
| Automático | Intel / DXGI / Quick Sync | 14,2% | 59,9 |
| Intel manual | Intel / DXGI / Quick Sync | 15,7% | 59,9 |
| NVIDIA manual | NVIDIA / WGC / NVENC | 11,2% | 60,0 |
| GPU salva inexistente | Intel / DXGI / Quick Sync | 16,6% | 60,0 |
| CPU | captura compatível / OpenH264 | 76,6% | 58,1 |
| Janela, automático | Intel / WGC / Quick Sync | 15,0% | 59,7 |
| Sem transmitir, em chamada | — | 1,3% | 0 |
| Depois de parar a NVIDIA | — | 1,6% | 0 |

A cópia anterior do executável (identificada como `5efda85`) transmitiu com 16,1%
de um núcleo e aproximadamente 60 FPS. Em outra rodada da seleção nova, automático
ficou em 12,5% e Intel manual em 15,4%. São amostras curtas, com variação entre
rodadas; não demonstram ganho em jogos nem que uma GPU será sempre mais econômica.
O resultado principal é a escolha previsível da placa com os caminhos de hardware
funcionando. A NVIDIA pode disputar recursos e energia com o jogo.

Na rodada final, seis paradas responderam em cerca de 7–12 ms pelo controle local,
incluindo a parada da janela. Nenhuma imagem chegou depois de parar a transmissão
durante a amostra; o processamento por GPU foi desativado e o estado da captura foi
limpo. A alteração de preferência durante uma sessão permaneceu para a seguinte.
Os sete idiomas tiveram nomes das opções conferidos e capturas da interface salvas.

Passaram 85 testes unitários normais e seis testes de hardware executados
explicitamente (conversão, cores, escala, NVENC, Quick Sync e quadros-chave). A
fixture de navegador marcada como ignorada não foi executada nesta rodada.
Artefatos locais: `target/gpu-selection/2026-10-19T04-00-19-548Z` (comparação inicial)
e `target/gpu-selection/2026-10-19T04-09-29-350Z` (versão final). AMD e Linux continuam
sem validação local, assim como desempenho e latência durante um jogo pesado.
