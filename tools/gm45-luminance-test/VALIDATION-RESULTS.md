# GM45 — Validação do fallback de luminância e preview D3D11

## Ambiente

O protótipo foi compilado com o fxc do Windows SDK e executado no adaptador físico Intel Mobile 4 Series do Inspiron 1545.

- Adaptador: Mobile Intel(R) 4 Series Express Chipset Family
- PCI vendor/device: 8086:2A42
- Driver: 8.15.10.2702 (WDDM 1.1)
- D3D feature level obtido: 10_0
- Perfis usados: vs_4_0 e ps_4_0

## Suporte de formatos observado

- R32_FLOAT: CheckFormatSupport = 0x16f5fb. Criado como render target + shader resource e usado nas duas passagens.
- R11G11B10_FLOAT: CheckFormatSupport = 0x6f3f3. A criação de Texture2D e SRV foi bem-sucedida; o shader também leu essa textura nos testes.
- RGBA32_FLOAT: CheckFormatSupport = 0x16f1fb. Usado na primeira rodada de controle.

## Teste com o formato de entrada capturado

Fonte: DXGI_FORMAT_R11G11B10_FLOAT
Intermediário: R32_FLOAT com RTV + SRV
Saída: R32_FLOAT RTV 1×1
Redução: duas passagens ps_4_0, 16×16 por tile.

| Entrada | Grade parcial | GPU | CPU | Erro absoluto | Resultado |
|---|---:|---:|---:|---:|---|
| 180×81 | 12×6 | 0.330611885 | 0.330614299 | 2.41398811e-06 | PASS |
| 240×108 | 15×7 | 0.330624372 | 0.330627054 | 2.68220901e-06 | PASS |
| 255×241 | 16×16 | 0.330651790 | 0.330655813 | 4.02331352e-06 | PASS |

A tolerância usada foi max(0.00005, abs(CPU) * 0.0005). A fonte inclui amostras NaN e +Inf para exercitar a rejeição de luminâncias não finitas. Os valores de CPU são calculados a partir das amostras já quantizadas para o formato R11G11B10 usado na textura.

## Primeira rodada de controle

A mesma redução também passou com entrada RGBA32_FLOAT:

| Entrada | Grade parcial | GPU | CPU | Erro absoluto | Resultado |
|---|---:|---:|---:|---:|---|
| 180×81 | 12×6 | 0.332606435 | 0.332608849 | 2.41398811e-06 | PASS |
| 240×108 | 15×7 | 0.332618624 | 0.332621276 | 2.65240669e-06 | PASS |
| 255×241 | 16×16 | 0.332646400 | 0.332650363 | 3.96370888e-06 | PASS |

## Preview D3D11 em janela

Foi criado o executável independente gm45_preview.exe. Ele abre uma janela Win32 com swap chain D3D11 de hardware e apresenta três painéis:

1. textura sintética animada no formato R11G11B10_FLOAT;
2. textura intermediária R32_FLOAT com contribuições de luminância por tile;
3. valor final de luminância global 1×1, expandido para visualização.

O FPS e a média de referência da CPU aparecem no título e em uma sobreposição dentro da imagem D3D11. O hardware e o feature level aparecem na linha inferior.

Uma captura da sessão no GM45 registrou 77,6 FPS. Trata-se da taxa de atualização deste demonstrador (geração/upload da entrada, duas passagens de redução, três painéis e apresentação), não do FPS do GTA V. O demonstrador não carrega recursos do jogo nem consome frames do GTA V.

A compilação do alvo gm45_preview foi concluída com sucesso. Após a última alteração de apresentação, o CTest foi executado novamente:

- gm45_luminance_rgba32 — Passed
- gm45_r11_format — Passed
- gm45_luminance_r11g11b10 — Passed

Resultado: 3/3 testes, 0 falhas.

## Arquivos

- shaders/luminance_fallback.hlsl — VS e duas passagens de redução.
- shaders/preview_display.hlsl — shaders de apresentação e overlay.
- gm45_preview.cpp — janela nativa D3D11, swap chain, painéis e FPS.
- main.cpp — harness de readback D3D11 com entrada RGBA32_FLOAT.
- r11_format_probe.cpp — verificação de suporte e criação de SRV R11G11B10_FLOAT.
- r11_luminance_test.cpp — harness com entrada R11G11B10_FLOAT.
- CMakeLists.txt — compilação fxc, executáveis e CTest.
- README.md — instruções de build e execução.

## Limites da validação

Isto valida o algoritmo de luminância, os recursos/formatos das duas passagens e uma pequena apresentação D3D11 no GM45. Não valida o renderer inteiro do GLUE5, o gerenciamento real de estados/recursos da engine, nem o desempenho de uma cena do GTA V. Também não implementa fallback genérico para outros compute shaders. A família tiled_lighting::DepthDownsampleCS_Default continua fora do escopo.

O preview utiliza entrada sintética gerada pela CPU e atualizada em uma textura de hardware; ele não lê um frame de jogo. O backend D3D11 ainda não foi conectado ao GpuRing do GLUE5.

Os arquivos fonte e documentação estão em tools/gm45-luminance-test e docs/gm45-d3d11-backend-plan.md, na branch de pesquisa. Os intermediários de build e a captura de desktop ficam dentro do diretório build/ ignorado pelo Git.
