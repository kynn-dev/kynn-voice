# Kynn Voice 0.1.5 · Windows x64

Processamento local do microfone com RNNoise e DeepFilterNet, interface nativa do Windows e saída para VB-CABLE. Fork de [danjaniell/noise-gator](https://github.com/danjaniell/noise-gator), distribuído sob MIT, com os avisos de terceiros preservados.

## Nesta versão

- Setup Windows `KynnVoice-Setup-0.1.5-x64.exe`, com instalação do aplicativo no perfil do usuário.
- Escolha explícita entre **App + VB-CABLE**, **Somente o app** e **Cancelar**.
- Na opção **App + VB-CABLE**, o Setup valida o arquivo original do VB-Audio, solicita UAC e inicia o instalador oficial do VB-CABLE. A instalação deve ser seguida de reinicialização do Windows conforme orientação do fabricante.
- A opção **Somente o app** não instala driver nem altera o dispositivo de áudio padrão. O ZIP portátil e o EXE avulso continuam disponíveis para quem já tem o VB-CABLE ou prefere instalação manual.
- Arquivo `.sha256` acompanha o Setup. O Setup não possui assinatura Authenticode própria; o hash é uma conferência de integridade.
- O aviso [VB-CABLE-NOTICE.txt](packaging/VB-CABLE-NOTICE.txt) identifica a origem do Driver Pack 45, a VB-Audio e o caráter donationware.
- O Setup e o ZIP portátil incluem a licença MIT e os avisos/textos de licença das dependências. O Setup instala esses documentos junto do aplicativo e verifica os hashes antes de mostrar as opções.

## Download e uso

Use o [Setup da release](https://github.com/kynn-dev/kynn-voice/releases/download/v0.1.5/KynnVoice-Setup-0.1.5-x64.exe) para a instalação guiada. Antes de escolher **App + VB-CABLE**, confira a origem e os hashes publicados; o instalador oficial só é iniciado depois da escolha e da confirmação UAC. Se o UAC for cancelado, o app é mantido e o driver não é iniciado.

O [ZIP portátil](https://github.com/kynn-dev/kynn-voice/releases/download/v0.1.5/KynnVoice-0.1.5-windows-x64-portable.zip) não contém o driver. Extraia-o numa pasta permanente e abra `kynn-voice.exe`; o EXE avulso é equivalente. Em qualquer opção, selecione `CABLE Output (VB-Audio Virtual Cable)` como microfone no Discord, jogo ou gravador. Uma soundboard deve enviar efeitos diretamente para `CABLE Input`, fora da captura e do processamento neural.

O arquivo `SHA256SUMS.txt` cobre os artefatos portáteis. O Setup tem um arquivo `.sha256` separado. A instalação não muda dispositivos padrão do Windows. DeepFilterNet baixa modelos e runtime oficiais fixados e verifica os hashes no primeiro uso; RNNoise funciona sem esses downloads.

## Validação e limites

Os testes da fase anterior no host Windows 11 / Ryzen 7 9700X incluíram build release, 38 testes padrão e 38 sem DeepFilterNet, opt-in de ONNX/registro, persistência após encerramento forçado, cadastro de inicialização pela UI, 50 ciclos de janela e áudio USB → VB-CABLE por 20 segundos em cada modo, sem underruns/drops nessa medição.

O empacotamento local do Setup foi reconstruído para incluir e validar os avisos de terceiros, além do download oficial do Driver Pack 45, conferência SHA-256 e verificação de assinatura do payload durante o build. A instalação real em um Windows limpo, a aceitação UAC, a reinicialização pedida pelo fabricante, o novo logon, a sessão com League of Legends, o uso prolongado, a reconexão física USB e a avaliação auditiva com soundboard/Discord continuam **NOT_RUN** para esta versão. Esses artefatos não comprovam compatibilidade geral, assinatura do Setup ou desempenho de ponta a ponta.

Detalhes dos testes anteriores: [STARTUP_PERSISTENCE_REPORT.md](https://github.com/kynn-dev/kynn-voice/blob/master/STARTUP_PERSISTENCE_REPORT.md) e [STABILITY_REPORT.md](https://github.com/kynn-dev/kynn-voice/blob/master/STABILITY_REPORT.md).
