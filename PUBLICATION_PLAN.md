# Preparação do fork público Kynn Voice

Data: 18 de outubro de 2026. **Publicação autorizada pelo usuário: fork público `kynn-dev/kynn-voice`, créditos preservados e pré-release 0.1.4 somente do aplicativo.** O estado remoto efetivo pode ser consultado no [repositório](https://github.com/kynn-dev/kynn-voice) e na [release](https://github.com/kynn-dev/kynn-voice/releases/tag/v0.1.4).

## Candidato

- Nome: `kynn-dev/kynn-voice`, fork nativo de [danjaniell/noise-gator](https://github.com/danjaniell/noise-gator), mantendo histórico, arquivo LICENSE e atribuição MIT do autor original.
- Versão candidata: **v0.1.4, pré-release Windows x64 / CPU**, com executável portátil, ZIP e `SHA256SUMS.txt`.
- Notas devem apontar o [relatório de inicialização/persistência](STARTUP_PERSISTENCE_REPORT.md) e o [relatório de estabilidade](STABILITY_REPORT.md), distinguindo testes no host de alegações gerais de compatibilidade.
- O executável é sem assinatura Authenticode própria. Hash fornece uma referência de integridade; não equivale à identificação por certificado.

## Antes de publicar

1. Conta `kynn-dev` confirmada pelo conector e pelo GitHub CLI já autenticado; publicação solicitada diretamente pelo usuário. Preservar o repositório original como `upstream` e não enviar alterações a ele.
2. Preparar o commit com todas as alterações autorizadas acumuladas da fase 1 e revisão do diff. O checkout contém trabalho anterior sem commit: a publicação não deve tratá-lo como baseline limpo nem descartá-lo. Excluir `target/`, logs de teste, configurações pessoais, IDs de endpoint e dados locais do pacote público.
3. Conferir avisos de terceiros para crates, RNNoise/modelo, DeepFilterNet/modelos e ONNX Runtime/DLLs, com referências às versões fixadas e termos dos distribuíveis. Não pressupor que a licença MIT do aplicativo cobre todos os assets.
4. Na primeira distribuição pública, oferecer **somente o aplicativo**, com VB-CABLE como pré-requisito externo e [link oficial](https://vb-audio.com/Cable/). A licença MIT permite a publicação do código do app com os avisos preservados; os termos da VB-Audio são próprios. O setup combinado 0.1.3 permanece um artefato histórico separado e não entra na release pública.
5. Concluir teste de novo logon/reinício, abrir configurações com League of Legends, sessão prolongada, reconexão física e avaliação auditiva. Os que não forem concluídos precisam constar explicitamente das limitações da pré-release.
6. CI Windows com Rust 1.99.0, formatação, check e testes nas configurações padrão e RNNoise independente. Fluxo manual de release compila e gera somente um draft, com nomes Kynn Voice e hashes. Não tratar CI como teste de dispositivos físicos.

## Fluxo de publicação

Criar o fork, manter o original como `upstream` e usar o fork pessoal como `origin`, sem reescrever o histórico do upstream. Criar branch `codex/...`, commit revisado e PR no fork. Publicar a pré-release somente depois dos checks relevantes, com arquivos revisados e checksums. Não atribuir ao upstream original a autoria das alterações locais.

## Uso do candidato local

Colocar `kynn-voice.exe` em pasta permanente, abrir somente uma cópia e selecionar **Iniciar com o Windows** nas configurações. A preferência aponta para aquela localização. O aplicativo instalado antigo não foi sobrescrito nesta tarefa; seu atalho pode continuar abrindo 0.1.3. Para comparar versões, encerrar uma pela bandeja antes de abrir a outra.

O app-only não instala driver nem altera o áudio padrão. Para soundboard, manter `Microfone USB → Kynn Voice → dispositivo virtual → consumidor`, com efeitos da soundboard enviados diretamente ao cabo virtual, fora da captura neural. A disponibilidade do roteamento não representa teste auditivo de todos os programas consumidores.
