# Preparação do fork público Kynn Voice

Data: 18 de outubro de 2026. **Preparação autorizada pelo usuário: fork público `kynn-dev/kynn-voice`, créditos preservados e candidato 0.1.5 com Setup opcional para VB-CABLE.** A pré-release 0.1.4 publicada continua sendo somente do aplicativo. O estado remoto efetivo pode ser consultado no [repositório](https://github.com/kynn-dev/kynn-voice) e na [release anterior](https://github.com/kynn-dev/kynn-voice/releases/tag/v0.1.4).

## Candidato

- Nome: `kynn-dev/kynn-voice`, fork nativo de [danjaniell/noise-gator](https://github.com/danjaniell/noise-gator), mantendo histórico, arquivo LICENSE e atribuição MIT do autor original.
- Versão candidata: **v0.1.5, pré-release Windows x64 / CPU**, com Setup, executável portátil, ZIP, `SHA256SUMS.txt` e sidecar `.sha256` do Setup.
- O Setup instala o app no perfil do usuário e apresenta **App + VB-CABLE**, **Somente o app** e **Cancelar**. A primeira opção só inicia o instalador oficial do VB-Audio depois da confirmação UAC; a instalação deve ser seguida de reinicialização do Windows conforme orientação do fabricante.
- Notas devem apontar o [relatório de inicialização/persistência](STARTUP_PERSISTENCE_REPORT.md) e o [relatório de estabilidade](STABILITY_REPORT.md), distinguindo testes no host de alegações gerais de compatibilidade e deixando a instalação real como não testada.
- O app e o Setup são sem assinatura Authenticode própria. Hash fornece uma referência de integridade; não equivale à identificação por certificado.

## Antes de publicar

1. Conta `kynn-dev` confirmada pelo conector e pelo GitHub CLI já autenticado; publicação solicitada diretamente pelo usuário. Preservar o repositório original como `upstream` e não enviar alterações a ele.
2. Basear esta atualização na release publicada 0.1.4. O estado inicial da branch estava limpo e igual a `origin/master`; incluir apenas as mudanças de versão, empacotamento e documentação deste Setup. Excluir `target/`, logs de teste, configurações pessoais e IDs de endpoint do pacote público.
3. Conferir avisos de terceiros para crates, RNNoise/modelo, DeepFilterNet/modelos e ONNX Runtime/DLLs, com referências às versões fixadas e termos dos distribuíveis. Não pressupor que a licença MIT do aplicativo cobre todos os assets.
4. Na distribuição 0.1.5, oferecer o Setup e manter o ZIP portátil/EXE avulso. O Setup embute o Driver Pack 45 original, sem alteração, e preserva [origem, identificação e aviso donationware](packaging/VB-CABLE-NOTICE.txt). A opção do driver é voluntária e exige UAC; quem preferir não instalar pode escolher **Somente o app** ou usar o pacote portátil. O aviso do Setup explica que o Windows 10/11 pode selecionar o driver como dispositivo padrão após instalar; o usuário deve confirmar a saída física após reiniciar. O Setup não restaura nem altera defaults automaticamente. A licença MIT cobre o código do app; os termos da VB-Audio são próprios.
5. Concluir teste de novo logon/reinício, abrir configurações com League of Legends, sessão prolongada, reconexão física e avaliação auditiva. Os que não forem concluídos precisam constar explicitamente das limitações da pré-release.
6. CI Windows com Rust 1.99.0, formatação, check e testes nas configurações padrão e RNNoise independente. Fluxo manual para uma tag existente baixa o Driver Pack 45 do endereço oficial, verifica o SHA-256, compila o Setup e gera somente um draft com o Setup, o sidecar `.sha256` e os artefatos portáteis. O workflow não foi executado nesta preparação; não tratar CI como teste de dispositivos físicos.

## Fluxo de publicação

Manter o fork pessoal como `origin` e o projeto original como `upstream`, sem enviar alterações ao upstream. Criar PR no fork, aguardar CI, revisar o merge, criar a tag 0.1.5 e executar o workflow manual. Conferir os artefatos e hashes do draft antes de torná-lo público. Não atribuir ao upstream original a autoria das alterações locais.

## Uso do candidato local

Colocar `kynn-voice.exe` em pasta permanente, abrir somente uma cópia e selecionar **Iniciar com o Windows** nas configurações. A preferência aponta para aquela localização. O aplicativo instalado antigo não foi sobrescrito nesta tarefa; seu atalho pode continuar abrindo 0.1.3. Para comparar versões, encerrar uma pela bandeja antes de abrir a outra.

O Setup pode instalar o app sem driver ou, por escolha explícita, iniciar o instalador oficial do VB-CABLE. O app-only e o pacote portátil não instalam driver nem alteram o áudio padrão. Para soundboard, manter `Microfone USB → Kynn Voice → dispositivo virtual → consumidor`, com efeitos da soundboard enviados diretamente ao cabo virtual, fora da captura neural. A disponibilidade do roteamento não representa teste auditivo de todos os programas consumidores.
