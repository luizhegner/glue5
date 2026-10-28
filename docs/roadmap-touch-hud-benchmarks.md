# Backlog: controles touch, HUD e benchmarks do GLUE 5

Itens registrados a pedido do usuário em 2026-10-28. Este documento é planejamento; não afirma que qualquer item já esteja implementado.

## Controles touchscreen

- Permitir editar os controles touch na própria tela, ajustando o layout para o uso do jogador.
- Considerar carregar e salvar presets XML, conforme o backlog existente.
- No celular do GTA V, permitir interação por toque direto nos apps: abrir um app tocando nele, atender chamadas e encerrá-las por toque, sem depender apenas dos controles virtuais.

## HUD e renderização

- Melhorar a responsividade do HUD do GTA V para diferentes tamanhos de tela, densidades e orientações, incluindo escala, legibilidade e áreas de toque.
- Investigar a separação da composição/renderização do HUD em relação ao passe 3D, para que escala e posição do HUD possam ser tratadas independentemente.
- Antes de qualquer mudança nos passes, mapear alvos, ordem de composição, profundidade, clears e demais dependências; não presumir que redirecionar ou separar o HUD seja seguro.

## Benchmarks do GLUE 5

- Planejar opções de benchmark próprias do GLUE 5, distintas do benchmark embutido no GTA V.
- Usar cenários controlados e repetíveis e registrar as condições e os resultados de cada execução.
- Manter ativo o log da execução do GLUE 5 durante todo o benchmark; não ampliar a captura para logs de outros processos.
- Cobrir pelo menos testes de efeitos visuais e de renderização do mapa.

## Validação

- Tratar estes itens como backlog, sem presumir implementação ou resultados de teste.
- Validar cada mudança isoladamente e comparar execuções de benchmark apenas quando as condições forem equivalentes.
