Kynn Voice 0.1.5 - Windows x64 / CPU (pacote portatil)
https://github.com/kynn-dev/kynn-voice
Fork de danjaniell/noise-gator. Veja LICENSE e os avisos de terceiros incluídos.

Extraia numa pasta permanente e abra kynn-voice.exe.
Encerre versões anteriores por Sair no ícone da bandeja antes de abrir esta.
Abra Configurações pela bandeja, ou execute com --settings.
Marque Iniciar com o Windows se desejar início no logon. Se mover o executável,
desmarque e marque novamente usando a nova cópia.
As escolhas são salvas em %APPDATA%\noise-gator\config.toml.
O backup válido anterior fica em config.previous.toml.

Este pacote portatil nao inclui nem instala o VB-CABLE. Se quiser a instalacao guiada
com a opcao explicita do driver, use KynnVoice-Setup-0.1.5-x64.exe na release.
VB-CABLE: https://vb-audio.com/Cable/
Entrada do app: microfone físico. Saída do app: CABLE Input.
Discord/jogo: selecionar CABLE Output como microfone.
Soundboard: enviar os efeitos diretamente ao CABLE Input, fora do supressor.
Este pacote não instala driver e não altera dispositivos padrão do Windows.

RNNoise está incluído. DeepFilterNet baixa modelos/DLLs oficiais fixados e
verificados na primeira utilização; depois usa o cache e processa localmente.
Não há serviço de processamento de voz em nuvem.

Candidato pré-release sem assinatura Authenticode própria.
Testes neste host: 38 testes por configuração, persistência/interface/registro,
50 ciclos de janela e áudio 20 segundos por modo sem perdas nessa medição.
Ainda pendentes: logon real, League of Legends, longa duração, reconexão física
e avaliação auditiva da rota com soundboard/Discord. Veja o relatório no GitHub.
