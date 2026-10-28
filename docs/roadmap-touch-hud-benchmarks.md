# Backlog: controles touch, HUD e benchmarks do GLUE 5

Itens registrados a pedido do usuário em 2026-10-28. Este documento é planejamento; não afirma que qualquer item já esteja implementado.

## Controles touchscreen

- Permitir editar os controles touch na própria tela, ajustando o layout para o uso do jogador.
- Considerar carregar e salvar presets XML, conforme o backlog existente.
- No celular do GTA V, permitir interação touchscreen direta: tocar no ícone visível do app para selecioná-lo/abri-lo e tocar nos controles da tela para atender/encerrar chamadas. Priorizar toque no alvo, não deslizar para navegar pela grade; manter gestos apenas onde fizerem sentido.
- No modo touchscreen, aumentar o tamanho visual do celular para facilitar leitura e toque, mantendo o retângulo de interação alinhado ao aparelho renderizado. Não alterar a escala para quem não usa touchscreen.

## HUD e renderização

- Melhorar a responsividade do HUD do GTA V para diferentes tamanhos de tela, densidades e orientações, incluindo escala, legibilidade e áreas de toque.
- Investigar a separação da composição/renderização do HUD em relação ao passe 3D, para que escala e posição do HUD possam ser tratadas independentemente.
- Antes de qualquer mudança nos passes, mapear alvos, ordem de composição, profundidade, clears e demais dependências; não presumir que redirecionar ou separar o HUD seja seguro.

## Benchmarks do GLUE 5

- Planejar opções de benchmark próprias do GLUE 5, distintas do benchmark embutido no GTA V.
- Usar cenários controlados e repetíveis e registrar as condições e os resultados de cada execução.
- Manter ativo o log da execução do GLUE 5 durante todo o benchmark; não ampliar a captura para logs de outros processos.
- Cobrir pelo menos testes de efeitos visuais e de renderização do mapa.

## Progresso técnico: benchmark próprio do GLUE 5 (2026-10-28)

- Adicionado `benchmark::glue5` como base isolada do benchmark embutido no GTA V: cenários tipados para efeitos visuais e renderização do mapa, condições de execução e registros JSONL de início, cada frame e encerramento.
- O gravador faz `flush` após cada registro para preservar os frames já capturados em caso de falha e rejeita tempos não finitos/negativos. Ainda não está ligado a um botão, a cenários executáveis ou à coleta ao vivo; portanto nenhum benchmark foi executado.
- Próxima etapa: conectar o gravador a um executor controlado que leia os tempos/census do GLUE 5, mantenha o log ativo durante todo o run e grave condições/resultados em local próprio.

## Mapeamento inicial do código (2026-10-28)

- O editor de layout ainda não existe. `Settings` guarda apenas perfil Classic/Modern, tamanho, opacidade e preferências de interação; não há posições customizadas nem importação/exportação XML.
- `Touch::buttons()` monta controles a partir do contexto do jogador e dos prompts dinâmicos. `Touch::draw()`, `touch_finger()` e `occupied()` dependem dessa geometria; qualquer editor deve usar a mesma posição resolvida para renderizar, acertar hitboxes e evitar sobreposições.
- As posições padrão são âncoras `At` relativas à safe area e à escala da tela. O app configura o objeto `Touch` durante o jogo; a página de controles atual não oferece uma prévia editável.
- Implementação inicial em andamento: toques no aparelho agora enviam clique do mouse na coordenada tocada, sem aceitar também o app que estava previamente selecionado; swipes direcionais continuam como alternativa. Ainda precisa ser validado no GTA se a tela principal do Scaleform aceita esses cliques e se chamadas podem ser atendidas/encerradas assim.
- Em touch ativo, o código aproxima a posição 3D do aparelho para projetá-lo em 1,25×, preservando o centro e restaurando a posição original quando o touch termina ou o aparelho é guardado. A geometria de toque usa a mesma posição ampliada. A mudança ainda não foi validada no dispositivo.
- Próximo passo: validar os toques no jogo; depois projetar IDs semânticos estáveis e XML versionado/validado para o editor arrastável, sem alterar os passes gráficos. A geometria dos controles deve continuar compartilhada por desenho, entrada e cálculo de áreas ocupadas.

## Validação

- Tratar estes itens como backlog, sem presumir implementação ou resultados de teste.
- Validar cada mudança isoladamente e comparar execuções de benchmark apenas quando as condições forem equivalentes.
