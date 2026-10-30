# Protótipo de UI (parallax)

Um protótipo, **só paisagem** (o app não tem menu retrato): `index.html`, o shell
novo com a cena em parallax contínuo e rotação automática entre cenas.

- **PARALLAX** (canto inferior esquerdo): distância/tamanho/altura do foco,
  amplitude, velocidade, intervalo e desvanecer da troca de cenas,
  largura/opacidade/desfoque do painel. Em **COMPARE O BLUR**, alterne entre
  Balanced, Frosted, Cinematic e Fluted sem mover o painel; Frosted abre como predefinição.
  Fluted simula as ranhuras verticais e a refração característica do vidro canelado. O personagem é sempre colocado
  na área que o menu deixa livre.
- Clique nos traços no topo da arte (ou no botão da pasta) para trocar de cena.
- **Carregar pasta `glue5-scenes`**: cada **pasta** dentro dela é uma cena (um
  momento com seu personagem); usa todas, em rotação. `scenes.json` com
  `rotate.seconds` / `rotate.order` e `title` por cena é opcional. As cenas não
  dependem do modo de jogo.

Nada aqui é usado pelo app: é protótipo de avaliação.

- **PARALLAX** (canto inferior direito): amplitude, desfoque por profundidade,
  véu sobre a arte, visualizar camadas, alternar paisagem/retrato e **carregar a
  sua pasta `glue5-scenes`** (a mesma do app: `scenes.json` + PNGs).
- Arraste a pasta `glue5-scenes` sobre a página (desktop) ou use o botão.
- Sem arquivos, as camadas de exemplo são desenhadas inline (SVG).

O contrato de importação é o que já existe em `crates/glue5/src/ui/scenes.rs`:
`glue5-scenes/scenes.json`, camadas com `depth`, fundo sem `height`, figuras com
`height`/`x`/`bottom`, `focus`, `tint`, `amp`, `period`, `panel_side`.
