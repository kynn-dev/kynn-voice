# Kynn Voice 0.1.3 — investigação de travamento e validação

Data: 10 de outubro de 2026. Host: Windows 11 x64, Ryzen 7 9700X, 32 GB de RAM. Este documento distingue observações, hipóteses e testes pendentes. Complementa o histórico em `docs/phase-1-audit.md`.

## Sintoma e evidência

O usuário relatou tela preta e perda de resposta ao abrir as configurações com League of Legends aberto; o áudio do jogo continuava e era necessário reiniciar o PC.

Foram encontrados registros Kernel-Power 41 / EventLog 6008 de encerramentos inesperados e relatórios WER `LiveKernelEvent` com códigos `1a8` e `1b8`. O Windows documenta esses códigos como eventos de diagnóstico de tela preta no subsistema de vídeo: [0x1A8](https://learn.microsoft.com/en-us/windows-hardware/drivers/debugger/bug-check-0x1a8--video-dxgkrnl-black-screen-livedump), [referência de live dumps](https://learn.microsoft.com/en-us/windows-hardware/drivers/debugger/kernel-live-dump-code-reference). Esses registros apoiam a investigação gráfica; não identificam sozinhos a chamada, o aplicativo ou o driver causador.

Neste host foram enumerados AMD Radeon Graphics e Radeon RX 570. A versão anterior criava um contexto OpenGL/glutin ao abrir as configurações. Interação desse contexto com jogo/driver é uma **hipótese**, ainda sem reprodução instrumentada do travamento. Não foi comprovado vazamento extremo de RAM ou saturação de CPU: a instância antiga observada consumia aproximadamente 117 MiB de working set em repouso.

O travamento completo com League of Legends não foi reproduzido nesta rodada. Não há confirmação definitiva de que o problema do jogo esteja resolvido.

## Correções no código

1. **Configurações nativas no Windows.** USER32/common controls substituem egui/glow/glutin nesse alvo. O grafo Windows e os módulos carregados foram examinados: não há OpenGL/glutin/glow no aplicativo Windows. A composição da área de trabalho continua sendo responsabilidade do Windows.
2. **Entrada padrão ou seleção manual.** Removida a escolha automática de um USB único. `None` significa o dispositivo de entrada padrão do Windows; a escolha manual guarda o ID do endpoint. Entradas virtuais são recusadas para impedir recaptura da soundboard. O app não muda o padrão do Windows.
3. **Enumeração fora da UI.** Uma thread mantém o catálogo de dispositivos, com no máximo 64 entradas e 64 saídas exibidas e uma solicitação de atualização pendente. Abrir a janela lê o cache, sem abrir/probar configurações de cada endpoint. Uma enumeração bloqueada não cria outras threads de enumeração.
4. **Controle de streams fora da UI.** Aplicar dispositivos e iniciar/parar usam um controlador de áudio e uma solicitação pendente coalescida. A UI consulta estado atômico. Destruição/reabertura WASAPI não ocorre na thread da janela.
5. **Admissão de worker.** Um DSP worker por processo. Se o encerramento exceder a espera de 100 ms e o worker for destacado, outra instância DSP não é admitida enquanto ele estiver vivo. O loader neural também tem admissão única e entrega por canal de capacidade 1.
6. **ONNX Runtime CPU.** As três sessões usam execução sequencial, intra-op/inter-op de um thread e spinning desabilitado, conforme a [documentação do ONNX Runtime](https://onnxruntime.ai/docs/performance/tune-performance/threading.html). Cinco frames de silêncio aquecem arenas/buffers no loader; o estado é resetado antes de entrar no fluxo real. RNNoise continua ativo durante o carregamento.
7. **Proteção de recursos.** Job Object do processo configura cota CPU de 15% da capacidade total em ciclos por intervalo e memória privada comprometida de 512 MiB. O código consulta os limites aplicados. Aos 384 MiB libera sessões neurais e usa RNNoise até reiniciar; aos 448 MiB pausa áudio. Trinta frames neurais consecutivos acima de 8 ms solicitam RNNoise. Um pico isolado é tolerado.
8. **Buffers e relógios de saída.** A solicitação `Fixed(480)` não é o tamanho observado das callbacks. A saída inicia/reinicia após um callback inteiro observado mais margem de 10 ms, com mínimo de 20 ms e máximo limitado pela fila. Um underrun real é contado e provoca novo prefill. Relógios independentes são acomodados por um controle PI suavizado da ocupação da fila e interpolação linear com duas amostras de estado. A razão é limitada a 0,998–1,002 (±0,2%); a correção em ppm aparece no log. Não aumenta filas nem encadeia supressores. Filas continuam fixas: 8 frames de captura, 3.840 amostras mono por saída e 7.680 amostras pendentes. Callbacks fazem apenas conversão/interpolação leve/filas: não carregam modelos, enumeram dispositivos, escrevem logs ou alocam novos buffers.
9. **Diagnóstico e encerramento.** CPU, memória privada/working set, tempos DSP, latência software, máximo de callback e perdas vão ao log a cada cinco segundos. Logs têm rotação de um arquivo anterior, com limite de 2 MiB por arquivo. O botão Sair e o menu encerram o controlador antes de destruir a pipeline. Desmarcar supressão mantém bypass A/B existente.

Limites de recursos usam [CPU rate control](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_cpu_rate_control_information) e [process memory limit](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_limit_information). A cota de CPU não representa 15% de um único núcleo nem garante que toda amostragem curta de tempo de CPU seja menor que 15%. O teto de memória bloqueia novas alocações; falha de alocação Rust pode encerrar o app. O limite é memória privada comprometida, não todo o working set. Essas proteções não limitam kernel, GPU ou drivers e não garantem recuperação de um defeito externo.

## Auditoria de build e dependências

- HEAD local e `origin/master` verificados em `8a5b67382a92e48a92d7dfc7344e9e8b0a0bfaff`, sem divergência naquele momento. O checkout já continha alterações da fase anterior; elas foram preservadas. Baseline copiado para `target/stability-audit/baseline`.
- Rust stable MSVC x64, MSVC 14.50.35717 e SDK 10.0.26100 já disponíveis. Nenhuma instalação de toolchain/driver ou alteração administrativa foi feita nesta rodada. Variáveis de build foram definidas apenas no processo.
- `build.rs` auditado: incorpora ícone com `winresource`. Build scripts pertinentes do grafo já auditados na fase 1; nenhum novo pacote ou versão de dependência foi introduzido aqui. O diff do lockfile contra o baseline muda somente a versão do próprio pacote de 0.1.2 para 0.1.3. Os builds usam `--locked`.
- Modelos/DLLs existentes mantêm origem, versões e SHA-256 fixados. `ort = 2.0.0-rc.10` com `load-dynamic`; ONNX Runtime CPU x64 1.30.0. O teste ONNX conferiu os assets e executou inferência real. Sem downloads de fontes alternativas.
- A versão antiga do executável foi preservada em `target/stability-audit/baseline/kynn-voice-0.1.2.exe`.

## Resultados de validação

| Verificação | Resultado | Evidência |
| --- | --- | --- |
| `cargo fmt --all --check` | PASS | `target/stability-audit/fmt.log` |
| `cargo check --all-targets --locked` | PASS | `check.log`; warnings de código legado não utilizado permanecem visíveis |
| `cargo test --locked` | PASS | 26 passaram, 0 falharam, 1 ignorado por exigir assets ONNX; `test.log` |
| Check sem DeepFilterNet | PASS | `check --all-targets --no-default-features --locked`; `check-rnnoise.log` |
| Testes sem DeepFilterNet | PASS | 26 passaram, 0 falharam; `test-rnnoise.log` |
| Teste ONNX ignorado, executado explicitamente | PASS | 200 frames de 10 ms após aquecimento; média 628 µs, p50 589 µs, p95 826 µs, máximo 1.142 µs; `test-onnx.log`. Build de testes, medição local sintética |
| Build release MSVC x64 | PASS | `build-release.log`; executável em `target/stability/release/kynn-voice.exe` |
| UI nativa do executável final | PASS | 50 criações/fechamentos, notificações de sliders, modo, bypass, EQ, entrada padrão, saída virtual preservada; zero streams nesse diagnóstico. Crescimento privado depois do aquecimento: 208.896 bytes (0,20 MiB). GDI 11→11 e USER 18→18; `ui-release.log` |
| Limites do executável final | PASS de configuração; RAM exercitada | Windows reportou cota CPU ENABLE/HARD_CAP de 15% em ciclos e limite privado de 512 MiB. Alocação controle de 1 MiB aceita; alocação de 576 MiB rejeitada. Carga CPU de dois segundos observou 17,64% por tempo, métrica diferente da cota de ciclos; `resource-release.log`. Não representa prova de teto de 15% por tempo |
| USB → processamento → VB-CABLE no executável final | PASS neste teste | 60 segundos por modo, três aplicações de entrada padrão com streams ativos, janela responsiva, zero underruns/drops, encerramento normal em menos de cinco segundos; `live-settings.stdout.log`, `live-settings-samples.json` |
| Setup 0.1.3 e payloads embutidos | PASS | Assinaturas de setup VB-Audio e catálogo Microsoft válidas, INF/SYS verificados pelo SignTool. `--verify-only` aguardado explicitamente terminou com código zero. Setup combinado sem assinatura própria; `installer-build.log` |
| League of Legends junto com abertura repetida das configurações | NOT_RUN | Exige sessão real do jogo e confirmação do usuário |
| Reconexão física USB | NOT_RUN | Não foi desconectado hardware do usuário |
| Soundboard + Discord/jogo | NOT_RUN | Rota preservada por implementação; não houve teste auditivo dessa mistura |
| Instalação normal / instalação de driver | NOT_RUN | Somente verificação de payload; sem UAC ou driver executado |

Os testes automatizados cobrem persistência/reset do RNNoise e DeepFilterNet, alinhamento seco/molhado, conversão/downmix, prefill de callbacks grandes e recuperação, resampling contínuo com buffers limitados, coalescência de solicitações, exclusão de worker duplicado e limiares de proteção. Uma simulação de cinco minutos por caso com ±1.000 ppm, pacotes de captura de 480 amostras e callbacks de 1.056 frames passou sem underruns/overruns e com razão limitada. Um teste de rampa verifica continuidade da interpolação.

### Métricas do teste real final

Testado aproximadamente de 12:21 a 12:24, horário de São Paulo. Captura `Microfone (USB PnP Sound Device)` → reprodução `CABLE Input (VB-Audio Virtual Cable)`. Ambos anunciaram 48.000 Hz, dois canais e F32. O maior callback de saída continuou em 1.056 frames, apesar da solicitação `Fixed(480)`.

Foram coletadas 117 amostras da UI/processo por modo, em intervalos de aproximadamente 500 ms. As medianas/p95 abaixo são das durações do último frame consultadas nesses instantes, não de todos os frames. Percentuais de CPU representam tempo do processo dividido pelos 16 processadores lógicos; médias/máximos se referem a essa amostragem. `live-summary.json` conserva o resumo.

| Modo | DSP mediana / p95 | CPU média / máxima | Memória privada mínima–máxima | Working set máximo | Underruns / drops entrada / saída |
| --- | --- | --- | --- | --- | --- |
| Leve | 33 / 54 µs | 0,080 / 0,760% | 4,730–4,801 MiB | 23,039 MiB | 0 / 0 / 0 |
| Equilibrado | 833 / 1.070 µs | 0,572 / 1,326% | 30,977–31,090 MiB | 55,359 MiB | 0 / 0 / 0 |
| Estúdio | 837 / 1.014 µs | 0,578 / 1,327% | 30,914–30,977 MiB | 55,305 MiB | 0 / 0 / 0 |

O máximo DSP cumulativo registrado pela instrumentação por frame foi 2.991 µs. A latência software consultada na UI variou de 23,469 a 47,214 ms; o máximo cumulativo instrumentado foi 48,583 ms. Há custo de prefill/compensação de relógios. Essas medidas excluem latência interna do driver/dispositivo e atraso algorítmico neural; não são uma medição acústica ponta a ponta. A memória não mostrou crescimento contínuo nessa janela curta. Até 15 threads foram observados; ao final havia 9. Não foi feito teste auditivo de timbre nem loopback independente da saída virtual nesta rodada.

### Artefatos entregues

- `target/release/kynn-voice.exe`: 6.244.352 bytes; SHA-256 `ff583896743e186a4eb70dd71ee5a8c71f0e2f72899c37fb95d55b14db535be4`. Mesmos bytes do executável testado em `target/stability/release`.
- `target/release/KynnVoice-Setup-0.1.3-x64.exe`: 7.623.168 bytes; SHA-256 `1a904b8714e771bee7bb078e9897c608b0c2193fd90ae934f89b654b7eb12bc1`. Hashes laterais `.sha256` disponíveis.
- Driver ZIP original: SHA-256 `b950e39f01af1d04ea623c8f6d8eb9b6ea5c477c637295fabf20631c85116bfb`. Só incorporado/verificado, não instalado.

## Falhas encontradas durante a própria validação

- Uma combinação inicial de flags `InitCommonControlsEx` falhou; usar somente `ICC_BAR_CLASSES` corrigiu a inicialização. Classes padrão são USER32. O teste de 50 janelas passou após essa correção.
- O primeiro harness enviou mensagens logo após encontrar o HWND, antes do término da abertura. Houve timeout. O harness passou a esperar o marcador de inicialização concluída e validar o controle correto. Não foi constatado deadlock persistente da UI nova.
- O harness tentou encerrar com `WM_QUIT`, que não encerrou o event loop winit. Agora aciona o botão normal Sair e exige código zero em até cinco segundos. Essa verificação passou nas rodadas seguintes.
- O teste novo de buffer inicialmente não compilou porque usava `unwrap()` de um erro sem `Debug`. Corrigido com asserção explícita; a suite passou.
- Uma tentativa de rebuild do release ocorreu com o mesmo executável aberto no teste e falhou com `Access denied (os error 5)`. O processo de teste encerrou e o build seguinte passou. Não foi modificada segurança/permissão para contornar a falha.
- Invocação direta de programas GUI no PowerShell pode voltar antes de terminar ou sem expor o exit code. Os diagnósticos e a verificação final do setup foram executados com `Start-Process`, espera explícita e leitura do código de saída. O builder do setup também foi corrigido para aguardar o verificador. Nenhum driver foi executado nessa verificação.
- O primeiro stream debug teve 480 frames ausentes em Leve, 1.920 acumulados em Equilibrado e 2.400 em Estúdio. Após prefill adaptado e warmup, Leve teve zero; houve 480 frames ausentes na passagem para DeepFilterNet. Um release preliminar de 30 segundos por modo também teve zero no Leve e 480 acumulados nos modos DeepFilter. Interface e encerramento passaram, mas esses testes de continuidade **falharam**. Logs/JSON originais foram preservados (`live-first-*`, `live-debug-corrected-*`, `live-release-pre-final-*`). Não se atribui essa falta automaticamente ao modelo ou ao clock sem evidência adicional.
- Um release de 60 segundos por modo ainda teve 480, 960 e 1.920 frames ausentes acumulados, com DSP máximo de 3.383 µs e zero drops de entrada/saída. Preservado em `live-release-without-drift-*`. A baixa carga e as perdas periódicas motivaram compensação de relógios; não foi medido o clock físico de cada dispositivo.
- A primeira compensação somente proporcional falhou na simulação a -1.000 ppm após 455 callbacks: a ocupação estacionária ficava baixa demais. O controle PI passou nos dois sentidos por cinco minutos, preservando a asserção de ausência de underruns/overruns.
- O primeiro diagnóstico de recursos comparava tempo de CPU com cota de ciclos e aceitou arbitrariamente três pontos de tolerância. Ele **falhou** ao observar 18,08% de tempo CPU contra limite de teste 18%. A comparação entre unidades diferentes foi removida; o teste agora exige e consulta as flags ENABLE/HARD_CAP e cota 15% no Windows, confirma rejeição real de alocação acima de 512 MiB e reporta o tempo observado sem afirmar teto de 15% nessa outra métrica. Não houve alteração da cota configurada. A rejeição original é registrada aqui; a medição por tempo não prova sozinha a cota em ciclos.

## Como validar o cenário do jogo

1. Encerre a versão antiga pelo menu da bandeja e execute o novo `kynn-voice.exe`. Abra as configurações com o jogo fechado e confirme a entrada padrão ou selecione o microfone USB e aplique.
2. Comece em Leve. Inicie League of Legends e, durante uma sessão de treino, abra/feche as configurações dez vezes. Confira se imagem, mouse e áudio continuam funcionando.
3. Teste Equilibrado e Estúdio, aplicação de entrada e bypass. Observe CPU/memória e os contadores. Deixe cada modo ligado por alguns minutos.
4. Caso reapareça tela preta, preserve `kynn-voice.log` e `kynn-voice.previous.log` em `%LOCALAPPDATA%/Kynn Voice/logs` e o horário. Correlacionar com novos eventos WER/Display é a próxima etapa; não é necessário mudar segurança ou dispositivos padrão.

Para repetir o teste controlado sem jogo, usando os endpoints deste host e um arquivo temporário:

```powershell
.\packaging\verify-live-settings.ps1 -AppPath .\target\release\kynn-voice.exe -SecondsPerMode 60
```

O script só opera a instância que ele cria; não altera padrões de áudio ou configuração principal. Nomes de endpoints precisam ser ajustados em outro PC. Não grava conteúdo de microfone.

## Próximos passos e limites

- Confirmar o cenário League com o usuário antes de declarar o travamento resolvido.
- Investigar qualquer underrun remanescente com cadência/ocupação das filas e relógios dos endpoints; os contadores permanecem expostos. Um teste curto sem perdas não comprova estabilidade diária.
- Validar desconexão/reconexão USB, mistura de soundboard, avaliação auditiva A/B e latência ponta a ponta. Latência callback→callback exclui buffers de driver/dispositivo e atraso algorítmico do supressor.
- Não foram avaliados backend APU, macOS/Linux, outros drivers/GPU ou PCs dos amigos. O setup combinado continua sem assinatura Authenticode própria; identidade e assinaturas do pacote VB-Audio permanecem originais.
