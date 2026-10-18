# Créditos e componentes de terceiros

## Projeto de origem

**Kynn Voice é um fork de [danjaniell/noise-gator](https://github.com/danjaniell/noise-gator).** O histórico e a licença MIT original são preservados em [LICENSE](LICENSE). As alterações deste fork incluem a cadeia Windows/WASAPI, estabilização, marca Kynn Voice, interface nativa, preferências e inicialização por usuário.

## Processamento e infraestrutura

| Componente | Crédito / origem | Licença declarada / distribuição |
| --- | --- | --- |
| RNNoise | [Xiph.Org / Jean-Marc Valin e contribuidores](https://github.com/xiph/rnnoise) | BSD-3-Clause; veja os avisos preservados na implementação derivada |
| nnnoiseless 0.5.2 | [Joe Neeman e contribuidores](https://github.com/jneem/nnnoiseless) | BSD-3-Clause, código e rede padrão usados pelo modo Leve |
| DeepFilterNet / DeepFilterNet3 | [Hendrik Schröter e contribuidores](https://github.com/Rikorose/DeepFilterNet) | Projeto oferece MIT/Apache-2.0; modelo ONNX baixado do commit fixado pelo app e não incluído nos pacotes |
| ONNX Runtime | [Microsoft e contribuidores](https://github.com/microsoft/onnxruntime) | MIT; DLLs CPU baixadas da release oficial verificada e não incluídas nos pacotes |
| ort | [pyke.io e contribuidores](https://github.com/pykeio/ort) | Binding Rust; consulte a licença da versão fixada nos textos abaixo |
| CPAL, Rubato, ringbuf, windows-rs, winit, tray-icon e demais crates | Respectivos autores, discriminados em `Cargo.lock` e nos textos gerados | Licenças próprias, sem substituição pela licença MIT deste aplicativo |

Os textos de licença dos crates resolvidos para Windows, incluindo dependências de build, estão em [THIRD_PARTY_LICENSES.txt](THIRD_PARTY_LICENSES.txt). O arquivo é gerado do lockfile e dos fontes locais do registro Cargo com `packaging/generate-third-party-notices.ps1`, sem copiar caminhos pessoais. Inclui um conjunto conservador de dependências resolvidas; não é uma afirmação de que todos esses componentes estejam ligados ao executável final.

Os URLs, commits, versões e hashes dos downloads neurais/runtime são fixados em `src/models/mod.rs`. O app valida os arquivos antes de usá-los. Downloads de primeiro uso requerem conexão; inferência e processamento de voz acontecem localmente. As licenças dos assets/runtime continuam pertencendo a seus respectivos projetos.

## Dispositivo virtual externo

[VB-CABLE](https://vb-audio.com/Cable/) é desenvolvido pela **VB-Audio Software**, com termos próprios. Não faz parte da licença MIT do Kynn Voice. Na versão 0.1.5, o Setup inclui o Driver Pack 45 original para conveniência e só abre o instalador oficial depois que o usuário escolhe **App + VB-CABLE** e confirma o UAC. O pacote portátil não contém o driver. Ambas as opções incluem este aviso e os textos de licença de terceiros; a instalação segue os termos e as orientações da VB-Audio, inclusive a reinicialização recomendada pelo fabricante.

As marcas e nomes dos projetos pertencem aos respectivos titulares. Não há declaração de afiliação ou endosso ao fork.
