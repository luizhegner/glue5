# Backlog: controles touch, HUD e benchmarks do GLUE 5

Itens registrados a pedido do usuário em 2026-10-28. Este documento é planejamento; não afirma que qualquer item já esteja implementado.

## Controles touchscreen

- Permitir editar os controles touch na própria tela, ajustando o layout para o uso do jogador.
- Considerar carregar e salvar presets XML, conforme o backlog existente.
- No celular do GTA V, permitir interação touchscreen direta: tocar no ícone visível do app para selecioná-lo/abri-lo e tocar nos controles da tela para atender/encerrar chamadas. Priorizar toque no alvo, não deslizar para navegar pela grade; manter gestos apenas onde fizerem sentido.
- No modo touchscreen, aumentar o tamanho visual do celular para facilitar leitura e toque, mantendo o retângulo de interação alinhado ao aparelho renderizado. Não alterar a escala para quem não usa touchscreen.
- Relato do usuário (2026-10-29): o celular do GTA abre e fecha repetidamente; o drive-by dispara sempre reto; o usuário quer poder editar a posição dos controles. Tratar o primeiro como possível repetição do botão de abrir, e o segundo como falta de mira direcional durante o tiro; validar no aparelho antes de considerar resolvidos.

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
- Correção inicial (2026-10-29): o botão de abrir o celular passou a emitir um único frame de `UP`, evitando repetir a ação enquanto o dedo continua pressionado; o teste cobre frames subsequentes sem soltar. O botão drive-by agora continua segurando L1+R1 enquanto um arrasto nele move a mira/câmera; testes cobrem Classic e Modern. Ainda não foi compilado nem validado no aparelho.
- Segunda correção (2026-10-30, após novo relato no aparelho): o pulso único não bastava — enquanto o celular ainda está subindo, o botão continua visível e qualquer segundo registro do mesmo toque (evento duplicado, segundo dedo ou retoque) emitia outro `UP`, guardando o celular de novo e reiniciando o ciclo abre/fecha. Agora o pulso vale até o jogo mostrar o celular aberto (ou 1,5 s, se ele não abrir): registros repetidos nesse intervalo não emitem nada. Além disso, com o celular aberto mas a posição ainda desconhecida, toques onde ele fica não caem mais em botões do layout (o Back o guardaria). Testes: `the_phone_button_pulses_once_until_the_phone_is_seen_out` e `with_the_phone_out_unplaced_a_tap_on_its_place_presses_no_button`. Ainda não compilado nem validado no aparelho; se o ciclo persistir, coletar o relatório de diagnóstico do app (ele registra as transições do pad touch: pulsos repetidos de `Up` apareceriam nele).
- Próximo passo: implementar o editor arrastável na tela com IDs semânticos estáveis, posições persistidas e a mesma geometria compartilhada por desenho, hitboxes e áreas ocupadas; depois adicionar importação/exportação de presets XML versionados e validados. Manter fallback para o layout padrão e não alterar os passes gráficos.

## Validação

- Tratar estes itens como backlog, sem presumir implementação ou resultados de teste.
- Validar cada mudança isoladamente e comparar execuções de benchmark apenas quando as condições forem equivalentes.
