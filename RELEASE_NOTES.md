# Kynn Voice 0.1.4 · Windows x64

Processamento local do microfone com RNNoise e DeepFilterNet, interface nativa do Windows e saída para VB-CABLE. Fork de [danjaniell/noise-gator](https://github.com/danjaniell/noise-gator), distribuído sob MIT, com os devidos avisos de terceiros.

## Nesta versão

- Escolhas salvas automaticamente e restauradas ao reabrir, incluindo clareza, dispositivos aplicados e estado ligado/pausado.
- Opção **Iniciar com o Windows**, por usuário, sem administrador, com confirmação de gravação mesmo ao sair imediatamente após marcar ou desmarcar.
- UI nativa sem OpenGL; seleção de entrada padrão ou explícita; gerenciamento de streams em segundo plano e proteções de CPU/memória da fase de estabilidade.
- Leve: RNNoise. Equilibrado: DeepFilterNet. Estúdio: DeepFilterNet com DSP vocal. Um supressor neural por vez; bypass para comparar.

## Download e uso

Baixe o ZIP portátil e extraia numa pasta permanente. Ele inclui executável, instruções e licenças. O EXE avulso também está disponível; os avisos de terceiros correspondentes acompanham a release. Verifique o arquivo baixado com `Get-FileHash -Algorithm SHA256` e `SHA256SUMS.txt`.

Encerre a versão antiga pela bandeja antes de abrir. A instalação anterior e seus atalhos não são atualizados por esses arquivos. As escolhas existentes em `%APPDATA%\noise-gator\config.toml` continuam sendo usadas.

O [VB-CABLE oficial](https://vb-audio.com/Cable/) deve ser instalado separadamente. Esta release não inclui driver. RNNoise está incluído no app. DeepFilterNet usa modelos/runtime oficiais baixados e verificados por SHA-256 no primeiro uso; depois o processamento usa o cache e roda localmente na CPU. O aplicativo não tem assinatura Authenticode própria.

## Validação e limites

Testes no host Windows 11 / Ryzen 7 9700X: build release, 38 testes padrão e 38 sem DeepFilterNet, opt-in de ONNX/registro, persistência após encerramento forçado, cadastro de inicialização pela UI, 50 ciclos de janela e áudio USB → VB-CABLE por 20 segundos em cada modo, sem underruns/drops nessa medição.

**Pré-release:** novo logon real do Windows, sessão com League of Legends, uso prolongado, reconexão física USB e avaliação auditiva com soundboard/Discord ainda estão pendentes. Os testes locais não comprovam compatibilidade geral ou resolução definitiva do travamento relatado com o jogo. Detalhes em [STARTUP_PERSISTENCE_REPORT.md](https://github.com/kynn-dev/kynn-voice/blob/master/STARTUP_PERSISTENCE_REPORT.md) e [STABILITY_REPORT.md](https://github.com/kynn-dev/kynn-voice/blob/master/STABILITY_REPORT.md).
