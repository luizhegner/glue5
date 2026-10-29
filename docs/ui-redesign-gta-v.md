# GLUE 5 — reformulação da interface (referência: menu do GTA V)

Status: **implementado** (§0). A prévia visual que fez a proposta:
`docs/images/ui-proposal-play.png` e `ui-proposal-settings.png` (2400 × 1080 = o S21+
em paisagem; **não há modo retrato**) e `ui-proposal-scenes.png` (as quatro cenas
em rotação). Protótipo navegável, no preview desta sessão:
`docs/ui-proposal/index.html` — shell + cena rotativa + carregador da pasta
`glue5-scenes`.

Escopo: **só o GLUE 5.** O GTA V é referência de linguagem visual, nada do jogo
é tocado. Alvo de proporção: **S21+ 5G (Exynos 2100 / Mali-G78), 1080 × 2400 px,
~394 dpi**, **só paisagem** (§3: sem modo retrato).

---

## 0. Estado: implementado

O shell descrito aqui está no código (`crates/glue5/src/ui/`): `shell.rs` é o
chrome único (cabeçalho, abas, lista, descrição, rodapé, cena atrás), `theme.rs`
traz Manrope e os tokens (§2), `widgets.rs` as linhas (38 pt, seleção branca com
texto preto, 1 px), SETTINGS tem as **nove categorias** de §5 e a tela inicial é
a aba **PLAY** — `menu.rs`, depois de a antiga `pages.rs::start_page` (cards,
ícones, barra inferior) ser **removida**, com os seus testes movidos para a aba
(eles agora desenham `menu::show`, não a página morta). Implementado também: música abafada (§6), o suporte aos efeitos sonoros
(`docs/menu-sfx.md`; os arquivos são do jogador) e a **rotação das cenas a cada
9 s** de §3.1 — independente do modo, `rotate.seconds`/`rotate.order` no
`scenes.json`, e a figura padrão colocada dentro da área de arte (74 % da
largura com o painel à direita), nunca sob o painel.

## 1. O que existe hoje e por que parece "conglomerado"

| Peça | Onde | Hoje |
|---|---|---|
| Sistema visual | `ui/theme.rs` | paleta azul-preto + laranja `#FF7A3D`, cantos 14–22 px, **Barlow + Barlow Condensed** |
| Tela inicial | `ui/pages.rs::start_page` → `ui/menu.rs` | era título de 40 px + 4 *cards* de modo + cards de status + 5 botões de ícone + **barra inferior** própria (`bottom_rail`); hoje é a aba PLAY (§4), e a página antiga, os *cards* de status (`info`, `card`, `game_files`, `sharing_card`, `last_benchmark`, `last_failure`) e o cartão de mods da tela inicial foram **removidos** |
| Configurações | `ui/pages.rs::sheet()` | **janela flutuante central**, 4 abas (VIDEO/CONTROLS/GAME/DEBUG), cards com borda |
| Amigos, Servidores, Mods, Créditos | `ui/{friends,servers,mods,credits}.rs` | a mesma **janela flutuante**, cada uma com abas próprias |
| Fundo | `ui/stage.rs` | gradiente noturno + linhas diagonais animadas + arcos laranja |

Dois desenhos convivendo (tela inicial × janela flutuante) e cada página com seu
próprio cabeçalho. **A janela flutuante sai por completo** — nenhuma tela do GLUE 5
vai flutuar sobre outra: o menu ocupa a tela inteira, como no GTA V.

---

## 2. Referências

### Anatomia do menu do GTA V (fontes primárias)

- **Estrutura real:** `pausemenu.xml` do jogo (build 3725, 3.842 linhas) mapeado em
  [gta5-under-the-hood — *The pause menu*](https://github.com/Goodvibes74/gta5-under-the-hood/blob/main/docs/ui/pause-menu.md):
  154 telas, **23 barras de abas**, 408 botões, 214 opções, **162 `PREF_*`**.
  Menu de pausa do modo história = **10 abas**; Online = 9.
  As **configurações são 16 painéis** (Gamepad 22 linhas, Keyboard 22, Audio 24,
  Camera 12, Display 18, **Graphics 38**, Advanced Graphics 8, Voice Chat 16,
  Notifications 8, Saving and Startup 3, ...). Ou seja: o jogo usa **muitas
  categorias curtas e nomeadas**, não 4 blocos gigantes.
- **Mapa de telas** (MAP / BRIEF / STATS / SETTINGS / GAME / ONLINE) e hierarquia de
  informação em [HNK MEDIA — *GTA 5 UI/UX Design*](https://hnkmedia.com/case-studies/gta-5-ui-ux-design/).
- **Screenshot de referência** (menu de pausa no PC): cabeçalho escuro translúcido
  com título à esquerda e jogador à direita; **faixa de abas** em caixa alta logo
  abaixo, aba ativa em retângulo quase branco com texto escuro, inativas escuras,
  **‹ ›** nas pontas; painel de conteúdo com borda fina; **rodapé com os comandos**.
- **Anatomia dos menus de script** ([RAGENativeUI](https://github.com/alexguirre/RAGENativeUI/wiki/Menus-Overview)):
  banner → subtítulo em barra preta → lista → **item selecionado branco com texto
  preto** → **painel de descrição embaixo**. É o padrão que uso nas telas que o jogo
  original não tem (Mods, Servidores).
- **Interaction Menu** (screenshot do jogo): cabeçalho "INTERACTION MENU 1/16",
  linhas de 1 px de separação, valor à direita (`‹ Eclipse Towers, 40 ›`), descrição
  do item selecionado fixa no pé da caixa.

### Tipografia

- GTA V usa **Chalet London 1960** (itens, legendas, HUD) e **Chalet Comprimé
  Cologne 1960** (título do menu, créditos): [Fonts In Use](https://fontsinuse.com/uses/55702/grand-theft-auto-v),
  [GTA Wiki](https://gta.fandom.com/wiki/Fonts), [GTAMods](https://gtamods.com/wiki/Fonts_(GTA_V)).
- **Chalet é comercial** (House Industries) e não existe WOFF livre no projeto —
  não posso embarcá-lo. Os mockups usam **Manrope** (OFL): geométrica, largura
  normal, a mais próxima do esqueleto do Chalet London entre as fontes livres.
  Alternativas: **Archivo** (tem largura condensada, lembra o Chalet Comprimé) e
  **Inter** (mais neutra). Se você tiver o Chalet com licença, uso o seu arquivo.

### O que **não** vem do arena.ai

A linhagem "arena/Claude" que está no repo é exatamente o visual atual (laranja,
cantos 14–22 px, cards, janela flutuante). Das referências de como o arena.ai se
desenha hoje ([Fudge — arena.ai design](https://fudge.design/share/arena.ai-design))
aproveito só três princípios que, por acaso, são também os do GTA V: **borda de
1 px, cantos pequenos/quase retos, lista densa com um único acento raro e seleção
branca**. O resto do look atual é descartado.

---

## 3. O desenho novo: um único shell, em tela cheia

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ GLUE 5                                  STORY MODE · 2 MODS · READY  [avatar] │ 48 pt
├──────────────────────────────────────────────────────────────────────────────┤
│ ▮PLAY▮  SETTINGS  FRIENDS  SERVERS  MODS  CREDITS                            │ 38 pt
├──────────────────────────────────────────────────────────────────────────────┤
│  STORY MODE                              Resume from your saves  ›           │
│  GTA ONLINE                              Freemode with friends   ›           │
│ █SANDBOX                               │ The whole map, no missions  › █      │  ← linha 38 pt
│  BENCHMARK                               Guided flight, then results ›       │  │
│  ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄   ◀ rolagem ──┐                              │
│  █ PLAY SANDBOX                                       A █                    │
│                                          │ descrição do item selecionado      │
│                                          │ (título + 2–3 linhas + detalhe)    │
├──────────────────────────────────────────────────────────────────────────────┤
│ (A) Select   (B) Back   (Y) Settings             GLUE 5 0.4 · EXYNOS 2100    │ 36 pt
└──────────────────────────────────────────────────────────────────────────────┘
```

Regras:

- **Fundo:** a **cena em parallax** do jogador (§3.1) no lugar do gradiente com
  arcos laranja e linhas diagonais. Sem blur em runtime: o desfoque das camadas de
  fundo é feito **na arte** ou por uma cópia reduzida da textura (custo zero no
  Mali-G78); o `scene`/`stage` continua sendo a camada de trás.
- **Opacidade do painel: 86 %** (`rgba(9,10,12,.86)`) com `backdrop-filter:
  blur(6px) saturate(110%)` — **desfocado e meio opaco escuro**, como pedido;
  ajustável de 55 % a 100 % em DISPLAY (o desfoque de 0 a 16 px). O suficiente
  para o texto ficar firme **e** o parallax aparecer atrás do menu como
  profundidade — mesmo quando a arte não cabe inteira na área livre. Em
  DISPLAY ficam "Opacidade do menu" e "Desfoque do fundo" (0–16 px; 0 = sem
  `backdrop-filter`, para o Mali).

### 3.1 Parallax: contínuo, sem sensores, alternando entre cenas

O app já tem o contrato pronto (`ui/scenes.rs`: `glue5-scenes/`, camadas PNG com
transparência, `depth`, `focus`, `tint`, `amp`/`period`, `panel_side`, decodificação
fora da thread da UI, no máximo 3 cenas em memória). O que muda:

| Hoje | Proposto |
|---|---|
| Camadas “respiram” num seno lento (amp 20/6 pt; períodos 47/61 s) | **Movimento contínuo e suave**, sem ponteiro e **sem giroscópio**: dois senos fora de fase, período **23 s** (horizontal) e **31 s** (vertical), amplitude **22 pt** na camada mais próxima, escalada por `depth`. A figura em foco tem ainda uma “respiração” de 0,35 % a cada 17 s |
| Sem troca de cena | **Rotação automática a cada 9 s**, com o desvanecer de **300 ms que já existe** (`FADE_SECS` no `scenes.rs`) — o movimento não para durante a troca. Traços indicadores no topo da área de arte |
| Cena presa ao modo escolhido (`scenes::key(mode)`) | **A rotação não depende do modo de jogo.** As cenas são “momentos” (Franklin ao entardecer, Trevor à noite, um carro no neon…), em ordem fixa, iguais nas quatro abas; trocar de aba não troca a cena |
| `story/online/sandbox/benchmark` | Usa **todas as pastas** de `glue5-scenes/` como cenas, na ordem de `rotate` no `scenes.json` (ou por nome). As pastas podem ter qualquer nome |
| Fundo do menu é o gradiente | A cena ocupa **só a área de arte** (§4) |

**O foco — resolvido por construção, sem opções.** A área de arte é o que sobra à
direita do painel (38 % da largura em paisagem). A camada do foco (o personagem ou o
objeto) é posicionada **dentro dessa área**, não da tela: margem de 2,5 % da largura
da área, largura de 60 % da área, ancorada na base. Não há modo nem chave para
ligar: o personagem **sempre** aparece inteiro. Em números do S21+ (2400 px de
largura, painel de 62 %):

| | |
|---|---|
| painel | 0 – 62 % |
| figura em foco | **63,0 % – 84,6 %** |
| área livre | até 100 % |
| parte da figura escondida | **0 %** |

Para comparar: se a arte ocupasse a tela toda e a figura ficasse centrada, ela iria
de 20 % a 80 % e **70 % dela ficaria sob o painel**. É por isso que a arte passa a
existir só na área livre.

O `scenes.json` ganha duas chaves opcionais (nada quebra sem elas):

```json
{
  "rotate": { "seconds": 9, "order": ["franklin", "trevor", "carro", "michael"] },
  "scenes": { "franklin": { "title": "FRANKLIN · ENTARDECER" } }
}
```

Sem `rotate`, vale a ordem alfabética das pastas; `seconds` ausente = 9 s.

E a pasta aceita o nome antigo: `glue5-assets/` é procurada como **alias** quando
não existe `glue5-scenes/`. Nada é embarcado nem baixado; sem a pasta, o app desenha
o fundo (como hoje).

**Sobre a transição:** fica só o desvanecer simples que o app já faz (300 ms, com a
opacidade cruzada entre a cena que entra e a que sai) e o fade de 80 ms ao trocar de
aba. **Sem desfoque de transição** — a ideia foi descartada.

### As abas (menu de pausa)

| Aba | Conteúdo | Vem de |
|---|---|---|
| **PLAY** | os 4 modos como linhas; a selecionada expande *inline* (Online: nome + lobby + entrar) e vira o botão PLAY; à direita, o estado (pasta, cópia, servidor, mods) em pares rótulo/valor | `menu.rs::show` |
| **SETTINGS** | sub-faixa de categorias (§5) + lista + descrição | `pages.rs::settings_items` |
| **FRIENDS** | sub-abas AMIGOS / PEDIDOS / CONVITES / ENTRAR + o seu código | `friends.rs` |
| **SERVERS** | EM USO / NA REDE / POR ENDEREÇO / ESTE DISPOSITIVO | `servers.rs` |
| **MODS** | sub-abas MENUS / MODS / STUDIO / LOG | `mods.rs` |
| **CREDITS** | lista rolável + licença | `credits.rs` |

A tela inicial deixa de ter barra inferior e botões de ícone: **ela é a aba PLAY**.
`Home`/`Esc` abre o menu na aba PLAY; dentro dele `B`/`Esc` volta ao jogo (a mesma
máquina de navegação de `Ui::{back, show}`, adaptada). A engrenagem, o botão de mods
e o chip do servidor **continuam flutuando sobre o jogo** — são HUD, não menu; só
ganham o ícone/cor novos.

---

## 4. Proporções para o S21+ (o ponto que você levantou)

Medidas em pontos, com a escala que o app já calcula (`layout_scale`); entre
parênteses o que dá em px no S21+.

| Item | Paisagem (915 × 394 pt) |
|---|---|
| Largura útil do shell | **90,5 %** da safe area (24 pt de cada lado), painel em 62 % disso |
| Cabeçalho | 48 pt (126 px) |
| Faixa de abas | 1 linha, 38 pt |
| Linha de lista | **38 pt** (~10 mm, alvo de toque) |
| Painel de descrição | coluna à direita, 41 % da largura do shell |
| Rodapé | 36 pt |
| Conteúdo mínimo garantido | lista ≥ 60 % da altura |
| Margem interna | 18 pt |
| Cena (parallax) | área de arte, **38 %** da largura (≈ 348 pt), começando na borda do painel |
| Foco (figura) | 60 % da área de arte, margem de 2,5 % → ~63 %–85 % da tela, **nunca sob o painel** |

- **Nada de janela flutuante**: fim de margens de 6 pt, largura máxima de 718 pt e
  altura máxima de 558 pt (as do `sheet()` atual) — era isso que espremia o
  conteúdo no celular e obrigava a rolar dentro de uma caixinha.
- **Só paisagem.** O app trava a orientação em paisagem no Android; não existe
  layout retrato para o menu. Em janelas de desktop estreitas, o painel desce para
  78 % da largura e a descrição continua à direita (nada de coluna dupla virando
  lista).
- Áreas seguras (notch, barra de gestos) respeitadas em ambos: o shell desenha
  dentro de `ctx.content_rect()` e estende o fundo até a borda física.
- Texto em pt fixo com escala: 13,5 pt ≈ 35 px ≈ 3,2 mm de altura de maiúscula —
  legível a distância de leitura do celular; os títulos (21 pt) só existem no
  cabeçalho.
- Sem animações de transição entre abas (só um fade de 80 ms): troca instantânea,
  como o menu do jogo. Sem movimento contínuo no fundo.

---

## 5. Configurações: de 4 para 9 categorias

Mesma `Settings` (nenhuma migração de `glue5.json`), só reorganizada. (R) reinicia
o jogo · (P) próximo início · (A) só Android · (D) só desktop.

| Categoria | Linhas |
|---|---|
| **GRAPHICS** | Escala de resolução (com a resolução resultante na descrição) · Upscaler · Nitidez (FSR) · Geração de quadros · VSync · Limite de FPS · Baixa memória (R) · **Restaurar padrões gráficos** (última linha, isolada) |
| **DISPLAY** | Modo de janela (D) · Tela e taxa detectadas (só leitura) · Reduzir movimento |
| **AUDIO** | Volume da música do menu · **Abafar com o menu** (liga/desliga) · Mudo ao perder foco |
| **CONTROLS** | Controles de toque (Auto/On/Off) · Layout (Modern/Classic) · Direção (Stick/Arrows) · Tamanho · Opacidade · Velocidade da câmera · Aim assist · Fire (Auto/Manual) · Háptica |
| **GAMEPLAY** | Modo padrão · Nome online · Entrar no lobby automaticamente · Mods (abre a aba MODS) |
| **FILES** | Origem (Pasta/Servidor, P) · Pasta do jogo · Servidor em uso (abre SERVERS) · Baixar o jogo inteiro (P) · Compartilhar esta pasta (D) · Limpar cópia local · Importar save / recursos (A) |
| **NETWORK** | Amigos (abre FRIENDS) · Notificações de amigos · Entrar em amigo por chave |
| **ADVANCED** | Overlay de desempenho · Detalhe · Canto · GLUE 5 Studio (ligar, LAN, código, abrir) |
| **DIAGNOSTICS** | Captura de log · Salvar captura · Marcadores de congelamento · Relatório de importação/GPU · Créditos · Versão · Sair do jogo |

Nenhuma opção nova de comportamento: é a mesma lista de `settings_items`
(`ui/pages.rs`), redistribuída, cada uma com uma frase de descrição que hoje é
`on_hover_text` (no celular, hover não existe — por isso vira texto na tela).

---

## 6. Áudio: abafado ao abrir o menu, desabafado ao fechar

O arquivo **não muda** (`assets/audio/pausemenu.mp3`, o mesmo do `pausemenu.m4a`),
o volume não muda, nada é recodificado. Muda só o que é feito com o som que já sai
do mixer:

- **A música deixa de parar/fazer fade.** Hoje (`audio.rs`) ela vai a ganho 0 em
  1200 ms e é pausada quando qualquer página abre, e volta em 1800 ms. Isso sai.
  A faixa passa a tocar **contínua** na tela inicial e **com o menu aberto por cima**.
- **Um filtro no fim da cadeia:** `MIX_SetPostMixCallback` (SDL_mixer 3, disponível
  em `sdl3::mixer::sys`), que recebe as amostras já mixadas e pode alterá-las.
  Ali entra um **low-pass biquad de 2ª ordem**: corte de 20 kHz → **750 Hz** em
  ~450 ms ao abrir, e de volta em ~700 ms ao fechar. Ganho apenas **−3 dB**.
  É "som abafado", como um paredão entre você e o alto-falante — não é mudo.
- Interpolação do corte por blocos de 64 amostras (estável, sem estalo); custo
  ~1 operação por amostra, irrelevante no Exynos 2100.
- Estados: `normal` (tela inicial, menu fechado) → `abafado` (menu aberto) →
  `parado` (jogo iniciado ou app em segundo plano — o filtro abre antes de parar).
- Em **AUDIO** ficam "Volume da música do menu" e "Abafar com o menu" (desligado =
  a música continua sem filtro com o menu aberto).
- Só a *tela inicial* conta como menu para o efeito. Abrir a engrenagem durante o
  jogo não abafa nada, porque nesse momento a música já não toca.

---

## 7. O que **não** muda

- `pausemenu.mp3` / `pausemenu.m4a` e o áudio do jogo (`audio_update`, mixer do engine).
- Hooks do jogo, GPU, stream, mods, benchmark, rede, importação (`hooks/*`,
  `crates/glue5-gpu`, `glue5-runtime`, `glue5-stream`) e o `settings.rs` (schema).
- Controles de toque dentro do jogo e a lógica de todas as páginas: muda só a
  apresentação. Os testes de layout existentes em `ui/pages.rs` são adaptados à
  estrutura nova, não apagados.

## 8. Arquivos (estimativa) e ordem de implementação

1. `ui/theme.rs` (tokens novos, fonte, métricas) + `assets/ui/*` (fonte + OFL) +
   `ui/stage.rs` (cena + véu no lugar dos arcos) + `ui/scenes.rs` (alvo do
   ponteiro/giroscópio, `blur` por camada) + `paths.rs` (alias `glue5-assets`).
2. `ui/shell.rs` **novo**: cabeçalho + faixas de abas + lista + descrição + rodapé.
3. `ui/menu.rs` reescrito (aba PLAY); `sheet()` removido de `ui/pages.rs`.
4. SETTINGS em 9 categorias.
5. FRIENDS / SERVERS / MODS / CREDITS no shell.
6. `src/audio.rs` (post-mix low-pass) + 3 linhas em `src/app.rs`.
7. `docs/design.md`, `assets/ui/README.md`, README de áudio.

## 9. Verificação

- **Não há toolchain Rust neste sandbox** (sem `cargo`/`rustc`, e Rust não é
  instalável pelos hosts liberados aqui): o `cargo check`/CI do repo é o juiz.
  Escrevo com cuidado e, se o CI apontar algo, corrijo na sequência.
- O que deu para fazer aqui: os dois mockups acima, nas medidas reais do S21+.

---

## Protótipo (esta sessão)

`docs/ui-proposal/index.html` — shell novo + cena rotativa, com o painel
**PARALLAX**: distância/tamanho/altura do foco (com o número de onde ele cai), amplitude,
velocidade, intervalo e duração da troca de cenas, largura/opacidade/desfoque do
painel, e o botão
**carregar pasta** que lê a sua `glue5-scenes` (`scenes.json` + PNGs) pelo mesmo
contrato do app. Sem arquivos, quatro cenas de exemplo em SVG.

## Ajuste após teste no aparelho

O desfoque pedido é **estático no fundo**, não uma transição de blur. No menu
inicial, a primeira camada PNG é suavizada uma vez ao carregar; as figuras
continuam nítidas. Sobre o jogo, um filtro fixo é aplicado ao quadro do jogo
antes da interface, enquanto o menu está aberto, e a apresentação normal volta
quando ele fecha. O filtro usa o quadro original também quando há um upscaler
temporal; seu desempenho e aparência devem ser conferidos no S21+.

## Referências (resumo)

1. [gta5-under-the-hood — *The pause menu* (pausemenu.xml, frontend.xml)](https://github.com/Goodvibes74/gta5-under-the-hood/blob/main/docs/ui/pause-menu.md)
2. [HNK MEDIA — *GTA 5 UI/UX Design*](https://hnkmedia.com/case-studies/gta-5-ui-ux-design/)
3. [RAGENativeUI — *Menus Overview* (anatomia dos menus do GTA V)](https://github.com/alexguirre/RAGENativeUI/wiki/Menus-Overview)
4. [Fonts In Use — *Grand Theft Auto V*](https://fontsinuse.com/uses/55702/grand-theft-auto-v) · [GTA Wiki — Fonts](https://gta.fandom.com/wiki/Fonts) · [GTAMods — Fonts (GTA V)](https://gtamods.com/wiki/Fonts_(GTA_V))
5. [Fudge — *arena.ai design* (o que descartar e os 3 princípios aproveitados)](https://fudge.design/share/arena.ai-design)
6. [SDL_mixer 3 — `MIX_SetPostMixCallback`](https://github.com/libsdl-org/SDL_mixer/blob/main/include/SDL3_mixer/SDL_mixer.h) (efeito de abafado)
7. [Google Fonts — Manrope (OFL)](https://github.com/google/fonts/tree/main/ofl/manrope) · [Archivo](https://github.com/google/fonts/tree/main/ofl/archivo)
