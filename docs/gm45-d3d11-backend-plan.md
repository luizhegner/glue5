# GM45 / D3D11 — arquitetura e plano de integração

Status: investigação inicial; ainda não existe backend D3D11 integrado ao GLUE5.

## 1. O que já foi provado no hardware

O teste isolado em tools/gm45-luminance-test foi executado no Intel Mobile 4 Series / GM45 (PCI 8086:2A42), driver 8.15.10.2702, D3D feature level 10_0.

- O par de shaders que substitui os dois estágios de média de luminância compila como vs_4_0 / ps_4_0.
- O GM45 aceita R11G11B10_FLOAT como Texture2D + SRV.
- O GM45 aceita R32_FLOAT como RTV + SRV.
- Os dois estágios executaram no hardware e passaram os testes comparando leitura GPU com CPU, incluindo bordas parciais e valores não finitos.
- O harness tem CMake/CTest e reproduz o teste com fxc do Windows SDK.

Relatório detalhado: tools/gm45-luminance-test/VALIDATION-RESULTS.md.

Shaders originais observados na captura:

| Família compute | Hash/programa observado | Recurso de entrada | Recurso de saída |
|---|---|---|---|
| avgLuminanceCompute::LuminanceDownsampleInitCS | 9b8e0c6c85e065da | t3 | u1 |
| avgLuminanceCompute::LuminanceDownsampleCS | 7a1e45a48ffaaf08 | t2 | u1 |

O primeiro estágio soma cada tile 16×16 e divide a contribuição pela área completa da imagem. O segundo estágio soma a grade de contribuições; não divide novamente. O segundo estágio só cobre grades de até 16×16 no padrão registrado.

## 2. Arquitetura atual identificada no código

- crates/glue5-runtime/src/gpu.rs define GpuRing e o protocolo de comandos D3D11-style. O host fornece GpuBackend, uma closure que recebe GpuRing.
- crates/glue5-runtime/src/imports.rs chama a closure quando o engine inicia o worker via wgpu_start_worker.
- crates/glue5/src/app.rs cria uma wgpu::Instance, uma wgpu::Surface/Screen e atualmente entrega glue5_gpu::backend(gpu_settings, Some(screen)).
- crates/glue5-gpu/src/lib.rs seleciona Vulkan fora do ecossistema Apple e Metal nos sistemas Apple.
- crates/glue5-gpu/Cargo.toml ativa wgpu com std, parking_lot, metal, vulkan e wgsl; não há backend D3D11 habilitado.
- crates/glue5-gpu/src/exec.rs guarda Device, Texture, CommandEncoder, RenderPass, bind groups e demais objetos WGPU no executor principal.
- crates/glue5-gpu/src/draw.rs resolve os programas e pipelines de compute e emite dispatch_workgroups.
- crates/glue5-gpu/src/ops.rs cria ComputePipeline WGPU a partir de Program/WGSL.
- crates/glue5-gpu/src/shader.rs carrega os programas traduzidos a partir dos arquivos WGSL indexados pelo hash.

Consequência: o protocolo é D3D11-style, mas o executor não é D3D11. Adicionar um shader HLSL isolado não faz o GLUE5 abrir o adaptador GM45.

## 3. O que uma integração real exige

Uma implementação nativa precisa de um executor D3D11 que consuma o mesmo GpuRing. A função GpuBackend fornece um ponto de entrada útil, mas a aplicação desktop também precisa de uma rota de inicialização/apresentação que não dependa de wgpu::Surface/Screen.

Áreas necessárias:

1. **Seleção e inicialização do backend**
   - manter Vulkan/Metal como caminho atual;
   - adicionar caminho Windows/D3D11 opt-in ou selecionado por capability;
   - criar device, immediate/deferred context se apropriado, swap chain e associação com a janela SDL;
   - não inicializar wgpu Surface no caminho D3D11.

2. **Modelo de objetos e recursos**
   - mapear IDs do protocolo para ID3D11Texture2D, buffers, views, shaders e estados;
   - implementar formatos DXGI, bind flags, mips, arrays, MSAA, uploads e readbacks;
   - respeitar hazards SRV/RTV/UAV e desvincular views antes de reutilizar o recurso;
   - tratar contadores e recursos que não tenham equivalência direta.

3. **Pipeline de shaders**
   - converter/compilar os programas VS/PS atuais para perfis compatíveis com FL10_0;
   - definir como o binding/layout/constant packing WGSL atual será mapeado para D3D11;
   - validar as instruções e builtins usados pelos shaders reais, não apenas pelos dois shaders de luminância;
   - não assumir que todos os programas aceitam ps_4_0/vs_4_0 sem transformação.

4. **Comandos do ring**
   - suportar os comandos de criação/destruição, uploads, shader/layout/state, SRV/UAV, draw, cópia, resolve, clear, queries, readback, dispatch e present necessários pelo jogo;
   - manter semântica de ordenação e sincronização do ring;
   - reportar falhas sem deixar o engine preso esperando comandos já consumidos.

5. **Compute fallback allowlist**
   - interceptar somente hashes e variantes explicitamente testados;
   - reconhecer dimensões e formatos de entrada/saída;
   - rotear os dois shaders de luminância para as passagens de pixel shader;
   - para qualquer compute sem fallback validado, registrar diagnóstico claro e não produzir silenciosamente dados incorretos.

## 4. Sequência recomendada

### Milestone A — fechado
Teste independente do fallback de luminância no GM45, com CMake/CTest e relatório reprodutível.

### Milestone B — parcial, preview independente criado

A primeira fatia visual já executa no GM45, ainda fora do GLUE5 runtime:

- cria device D3D11 de hardware e swap chain nativa em janela Win32;
- cria fonte R11G11B10_FLOAT como SRV e recursos R32_FLOAT como RTV/SRV;
- executa os dois shaders ps_4_0 por draws full-screen;
- apresenta entrada, grade parcial e valor final em três painéis;
- exibe FPS, média de referência da CPU, adaptador e feature level;
- os harnesses separados fazem readback para comparar GPU com CPU.

O executável está em tools/gm45-luminance-test/gm45_preview.cpp. A captura usa uma entrada sintética animada, não uma imagem do GTA V.

Ainda falta extrair a inicialização, os recursos e a execução de comandos para um módulo Windows reutilizável e conectá-lo ao protocolo GpuRing. Portanto isto é um protótipo visual D3D11, não o backend GLUE5.

### Milestone C — protocolo mínimo
Implementar e testar criação de objetos, upload de buffer/textura, views, estados e draw simples por comandos GpuRing. Comparar a sequência de comandos com o executor WGPU.

### Milestone D — shaders e jogo
Resolver conversão de shaders reais, então acrescentar compute allowlist. Só depois testar cenas reais e ampliar o fallback para tiled_lighting::DepthDownsampleCS_Default, que é outro algoritmo e usa operações de redução mais complexas.

## 5. Riscos e limites conhecidos

- O teste de luminância não mede desempenho em uma cena do GTA V.
- As validações não exercitam o gerenciamento de recursos e estados da engine.
- O segundo grupo de compute observado no log, tiled_lighting::DepthDownsampleCS_Default, segue fora do escopo.
- A compatibilidade dos demais shaders traduzidos em WGSL com perfis SM4 ainda está por investigar.
- O backend D3D11 não deve ser misturado com as structs do executor WGPU por meio de casts ou objetos compartilhados sem ownership explícito.
- O fallback deve ser limitado por hash, variante, dimensões, formatos e binding contract. Não se deve inferir que compute arbitrário pode ser reproduzido por pixel shaders.

## 6. Regra de isolamento

Esta branch é de pesquisa. Não alterar integrate/ui-radio, main, nem diag/gpu-workload-s21. Não mudar o app de diagnóstico para depender do backend D3D11. Cada milestone deve ter seu teste independente e resultado documentado antes de avançar para o seguinte.
