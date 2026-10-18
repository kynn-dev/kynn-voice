# Kynn Voice 0.1.4 — preferências e inicialização com o Windows

Data: 18 de outubro de 2026. Validação local no Windows 11 x64 do usuário, Ryzen 7 9700X, 32 GB de RAM. Este relatório complementa [STABILITY_REPORT.md](STABILITY_REPORT.md); não substitui os testes pendentes de uso prolongado, jogo e reconexão física.

## Escopo e diagnóstico reproduzido

O código anterior salvava configurações apenas ao sair normalmente. Um encerramento forçado podia perder as mudanças da sessão. Além disso, o controle nativo de clareza sempre era reconstruído em 100%, mesmo com EQ salvo. Não existia opção de inicialização com o Windows.

Antes da correção, `packaging/verify-persistence.ps1 -ExpectLegacyFailure` confirmou na versão 0.1.3 que as escolhas não eram gravadas durante o uso; após encerramento forçado, modo Leve e redução 75% anteriores retornaram. O teste usou configuração isolada, entrada inexistente e somente o processo criado pelo teste.

O checkout já continha a implementação da fase 1 e o trabalho de estabilidade, sem commit. Esses arquivos foram preservados. Antes de editar, HEAD, referência local e `git ls-remote` de `origin/master` apontavam para `8a5b67382a92e48a92d7dfc7344e9e8b0a0bfaff`, sem divergência. `origin` ainda é o upstream `danjaniell/noise-gator`; não houve push nem publicação. Cópias dos arquivos afetados antes desta tarefa estão em `target/persistence-audit/baseline`.

## Implementação

- Salvamento automático com comparação de snapshots em thread própria: consulta a cada 100 ms, debounce de 300 ms, espera máxima de dois segundos durante ajustes contínuos e retry de dois segundos em falhas. Nenhum acesso a arquivo foi acrescentado às callbacks de áudio ou aos handlers da janela.
- Arquivo temporário exclusivo no mesmo diretório, `sync_all` e substituição por `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)` no Windows. A versão anterior válida vai para `config.previous.toml`; principal inválido não substitui backup válido. Leitura recupera o backup quando necessário.
- Persistência de dispositivos aplicados, modo, bypass/supressão, ganhos, gate/sensibilidade, EQ/clareza, autogain e intenção de manter o áudio ligado ou pausado. Campos avançados de autogain não expostos na interface são preservados. Escolhas ainda não aplicadas no seletor de dispositivos não são ativadas nem salvas como ativas.
- Ao sair, a intenção de processamento é salva antes de parar a pipeline. **Sair** preserva essa escolha; **Parar** deixa o próximo início pausado. Rótulos e posições dos sliders são reconstruídos a partir dos valores salvos, sem reaplicar presets por cima do EQ.
- Caixa **Iniciar com o Windows**, inicialmente desativada. Worker próprio lê e modifica somente `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, valor `Kynn Voice`. Comando com executável e configuração absolutos entre aspas; sem shell, elevação, tarefa agendada ou serviço. Inicialização só consulta o cadastro, sem habilitá-lo automaticamente.
- Limite conservador de 260 unidades UTF-16 incluindo terminador. Caminhos incompatíveis e erros de salvamento/registro são mostrados e registrados. A preferência de bloqueio do Gerenciador de Tarefas não é alterada.
- Revisão independente identificou risco de perder a alteração de inicialização ao sair imediatamente. Corrigido com flush e confirmação de leitura, limitados a dois segundos. Revisão final do diff incremental não apontou outro problema acionável; foi revisão estática, distinta dos testes abaixo.

Mantidos RNNoise/DeepFilterNet, backend CPU, limites de recursos e UI nativa anteriores. Não foram instalados drivers, mudados dispositivos padrão ou alteradas configurações de segurança. A saída da soundboard continua sendo roteada diretamente ao dispositivo virtual, depois do processamento do microfone.

## Dependências e build

Toolchain existente Rust/Cargo 1.99.0, MSVC x64 14.50.35717 e Windows SDK 10.0.26100.0. Nenhuma instalação foi necessária. `build.rs` foi inspecionado: gera os recursos do ícone pelo `winresource`, sem nova execução de scripts externos. Em relação ao baseline desta tarefa, `Cargo.lock` mudou somente a versão do pacote raiz de 0.1.3 para 0.1.4. Nenhum novo crate nem atualização de pacote; foram habilitados recursos de Registry/FileSystem no crate `windows` já existente. Comandos Cargo usaram `--locked` quando aplicável.

## Resultados executados

Logs e configurações de teste: `target/persistence-audit/`. Os testes nativos usam arquivos isolados; não substituem o perfil normal do usuário.

| Verificação | Resultado | Evidência |
| --- | --- | --- |
| `cargo fmt --all --check` | PASS | `fmt.log` |
| `cargo check --all-targets --locked` | PASS | `check.log` |
| `cargo check --all-targets --no-default-features --locked` | PASS | `check-rnnoise.log` |
| `cargo test --locked` | PASS | 38 passaram, zero falhas, dois opt-in ignorados na execução padrão; `test-full.log` |
| `cargo test --no-default-features --locked` | PASS | 38 passaram, zero falhas, um opt-in ignorado; `test-rnnoise.log` |
| Teste opt-in de registro HKCU isolado | PASS | Roundtrip REG_SZ, rejeição de REG_DWORD e remoção idempotente, somente subchave temporária de testes; `test-startup-registry.log` |
| Teste opt-in ONNX de continuidade | PASS | 200 frames de 10 ms após dez de aquecimento. Média 570 µs, p50 551 µs, p95 677 µs, máximo 1.061 µs; estimativa de 5,7% de um núcleo a 100 FPS. Build de testes, medição sintética; `test-onnx.log` |
| Build debug e release | PASS | `cargo build --locked`; `cargo build --release --locked --target-dir target/persistence`; logs de build |
| Persistência pela interface no release final | PASS | Alterações salvas com app aberto; encerramento forçado e reabertura restauraram modo, bypass, ganhos, gate, EQ, clareza, autogain e ID de entrada. Estado pausado permaneceu sem streams. Encerramento normal código zero; `persistence-release.log` |
| Inicialização pela interface no release final | PASS | Marcar e sair imediatamente, confirmar comando salvo; reabrir, checkbox restaurado; desmarcar e sair imediatamente, confirmar remoção. Caminho de configuração com espaços e Unicode. Execução desse comando sem `--settings` abriu a bandeja e restaurou estado pausado, sem abrir janela nem streams; `startup-ui-release.log` |
| UI nativa no release final | PASS | 50 criações/fechamentos com controles exercitados, zero streams. Memória privada após aquecimento 2.998.272 → 2.981.888 bytes; GDI 3→3, USER 5→5; `ui-smoke.stdout.log` |
| Áudio real USB → VB-CABLE no release final | PASS neste teste | 20 segundos medidos por modo, três aplicações da entrada padrão, RNNoise e DeepFilterNet ativos, nenhum módulo OpenGL detectado e saída normal em até cinco segundos. Zero underruns/drops; `live-release.log` e subdiretório `live/` |
| Novo logon/reinício real do Windows | NOT_RUN | Não reiniciamos nem encerramos a sessão do usuário. Cadastro e simulação do comando foram testados separadamente |
| League of Legends e abertura repetida de configurações | NOT_RUN | Não reproduzido com jogo aberto nesta tarefa |
| Desconexão física USB, avaliação auditiva A/B, soundboard + Discord/jogo e longa duração | NOT_RUN | Permanecem gates de aceitação reais |
| Instalação/atualização do aplicativo instalado e driver | NOT_RUN | Entrega portátil; instalação anterior preservada |

### Áudio e recursos observados nesta versão

Captura `Microfone (USB PnP Sound Device)` e saída `CABLE Input (VB-Audio Virtual Cable)`, WASAPI a 48.000 Hz, dois canais F32 nos endpoints e processamento interno mono. Callbacks de saída chegaram a 1.056 frames apesar da solicitação de 480. Foram amostrados 39 pontos por modo, aproximadamente a cada 500 ms. CPU abaixo é tempo do processo dividido pelos 16 processadores lógicos; não equivale à cota de ciclos do Job Object nem mede todo o sistema.

| Modo | CPU média / máxima | Memória privada mín.–máx. | Working set máximo | Underruns / drops entrada / saída |
| --- | --- | --- | --- | --- |
| Leve | 0,09% / 0,38% | 4,57–4,57 MiB | 21,89 MiB | 0 / 0 / 0 |
| Equilibrado | 0,40% / 0,95% | 31,70–31,71 MiB | 55,09 MiB | 0 / 0 / 0 |
| Estúdio | 0,37% / 0,96% | 31,61–31,71 MiB | 55,09 MiB | 0 / 0 / 0 |

Latência registrada é de software entre callbacks, não ponta a ponta; não inclui todos os buffers físicos nem o atraso algorítmico neural. Testes curtos e métricas sintéticas não comprovam estabilidade prolongada ou compatibilidade em outras máquinas.

## Falhas e restrições encontradas

- Primeira execução dos testes no sandbox: 33 passaram, três falharam e dois foram ignorados. As falhas eram `Access denied` (0x80070005) na substituição de arquivos temporários pelo Windows. Os sete testes de configuração passaram fora desse sandbox; depois as suites completas passaram no mesmo contexto do usuário. A proteção não foi desabilitada e os testes não foram enfraquecidos.
- O sandbox também impediu leitura de HKCU Run. Validação classificada como bloqueada até a execução autorizada fora dele. Uma solicitação de execução expirou na revisão automática antes de iniciar. A versão instalada antiga estava rodando; o teste recusou prosseguir. O usuário a encerrou por **Sair**, e só então o teste final passou.
- Avisos de hardlinks do incremental recusados com fallback para cópia e helpers anteriores não utilizados continuam visíveis nos logs.
- Perda de energia/kill antes do próximo autosave pode perder a última fração de segundo de escolhas. I/O bloqueado pode exceder o limite de saída e preservar somente o último salvamento concluído. Backup anterior não substitui backup externo.
- Marcar a caixa confirma cadastro e read-back; o Gerenciador de Tarefas ou políticas do Windows podem impedir a execução real. A entrada temporária do teste foi removida: o usuário precisa habilitar a opção no executável definitivo.

## Entrega local

`target/release/kynn-voice.exe` e `target/release/KynnVoice-0.1.4-windows-x64.exe` são cópias idênticas do binário release que passou nos testes. Tamanho: 6.326.784 bytes. SHA-256:

```text
33f70427348a189299cb51fc49fc05a3492bf71ed2ba96f7fadb701f0856c119
```

O ZIP portátil inclui o aplicativo, licença MIT upstream e instruções. Checksums acompanham os arquivos. Não contém driver, DLL/modelo ONNX nem configuração pessoal. RNNoise funciona com o executável; os modos DeepFilterNet usam o cache existente ou os downloads oficiais fixados e conferidos pelo app. O VB-CABLE precisa estar instalado separadamente. O aplicativo não tem assinatura Authenticode própria. O instalador combinado antigo 0.1.3 não foi atualizado nem deve ser confundido com esta entrega 0.1.4.

Próximo teste de aceitação: colocar o executável em pasta definitiva, habilitar a opção e, num reinício escolhido pelo usuário, confirmar bandeja, parâmetros e rota de áudio. A publicação proposta está em [PUBLICATION_PLAN.md](PUBLICATION_PLAN.md), sem ação remota executada.
