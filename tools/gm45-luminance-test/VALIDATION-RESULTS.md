# GM45 — Validação do fallback de luminância

## Resultado

O protótipo HLSL foi compilado com o fxc do Windows SDK e executado no adaptador físico Intel Mobile 4 Series do Inspiron 1545.

- Adaptador: Mobile Intel(R) 4 Series Express Chipset Family
- PCI vendor/device: 8086:2A42
- Driver: 8.15.10.2702 (WDDM 1.1)
- D3D feature level obtido: 10_0
- Perfis compilados: vs_4_0 e ps_4_0

## Suporte de formatos observado

- R32_FLOAT: CheckFormatSupport = 0x16f5fb. Criado como render target + shader resource e usado nas duas passagens.
- R11G11B10_FLOAT: CheckFormatSupport = 0x6f3f3. A criação de Texture2D e SRV foi bem-sucedida; o shader também leu essa textura nos testes abaixo.
- RGBA32_FLOAT: CheckFormatSupport = 0x16f1fb. Usado na primeira rodada de teste.

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

## Arquivos

- luminance_fallback.hlsl — os três entry points do fallback.
- VS_Fullscreen.cso, PS_LuminanceTile.cso, PS_LuminanceFinal.cso — bytecode compilado.
- main.cpp — harness D3D11 com entrada RGBA32_FLOAT.
- r11_format_probe.cpp — verificação de suporte e criação de SRV R11G11B10_FLOAT.
- r11_luminance_test.cpp — harness que testa o formato R11G11B10_FLOAT.
- CMakeLists.txt — targets dos testes.

## Limites da validação

Isto valida o algoritmo de luminância e os recursos/formatos usados por essas duas passagens no GM45. Não valida o renderer inteiro do GLUE5, nem o gerenciamento real de estados/recursos da engine, nem o desempenho final em uma cena do GTA V. Também não implementa fallback genérico para outros compute shaders. A família tiled_lighting::DepthDownsampleCS_Default continua fora do escopo.

Os testes foram criados em C:\k8r\gm45-luminance-test, fora dos worktrees do GLUE5. Nenhuma branch ou arquivo do repositório foi modificado nesta etapa.
