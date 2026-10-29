# Roteiro: de "GTA V rodando no Android" a "GTA V para Android"

> A pergunta que guia este repositório: **se a Rockstar portasse GTA V para Android, como ele seria?**
> Este documento traduz a resposta em fatias pequenas, cada uma com critério de pronto, para ir
> montando de pouco em pouco. O que já existe está marcado com ✅; o que falta, com o módulo onde
> vive.

Alvo de desenvolvimento e de teste: **Galaxy S21+ 5G (Exynos 2100, Mali-G78 MP14, 8 GB, 1080×2400
@48–120 Hz, Android 15)**. Revisão técnica que sustenta as prioridades: [android-review.md](android-review.md).

---

## O princípio

Um port de console para celular não é "o jogo com botões na tela". O que a Rockstar faz — e o que
separa um port de um emulador — são cinco coisas:

| Pilar | O que significa | Onde estamos |
|---|---|---|
| **1. Boot de console** | Logos, dicas enquanto carrega, menu único com abas, "Continuar" que sempre funciona | ✅ página inicial + benchmark; ❌ sem slots/saves visíveis, sem "continuar de onde parei" |
| **2. Controles que somem** | Poucos botões contextuais, editor de layout, *aim assist*, haptics | ✅ 2.1 k linhas de toque com testes e haptics Android implementados; ❌ sem editor arrastável; haptics ainda sem validação no S21+ |
| **3. Desempenho previsível** | Presets por aparelho, resolução dinâmica, *governor* térmico, 30/40/60 estáveis | ✅ frame pacing, FSR, escala de render; ❌ sem perfil de aparelho, sem termal Android |
| **4. Nada de "PC na tela"** | HUD legível, cutout respeitado, textos grandes, tudo alcançável com o polegar | ✅ safe area no UI; ❌ testes só com telas de iPhone |
| **5. Sessões curtas** | Salva sozinho, reinicia missão no checkpoint, retoma em < 2 s | ❌ maior lacuna atual |

---

## Fase 0 — Fundação Android-only (2–3 semanas)

Objetivo: um repositório que só sabe compilar Android, e um aparelho que não derruba o jogo.

- [x] **A4 (código)** permissões + multicast lock restrito à busca/Servers/Online; validação LAN no S21+ pendente.
- [x] **A2 (código)** perfil `Auto/Low/Normal`; `Auto` usa `MemTotal < 8500 MB` e há escolha manual. Medição do `MemTotal` e validação no S21+ pendentes.
- [x] **A8 (código)** Voltar = pausa (`SDL_ANDROID_TRAP_BACK_BUTTON` + `AC_BACK`); manter controles visíveis também foi corrigido.
- [x] **A3** instruções recomendam Find files ou `adb push`, sem copiar para `Android/data` pelo app Files.
- [ ] **A5/A13** `minSdk 30` e verificação de 16 KB no build.
- [x] **A14** `density` em `configChanges`.
- [ ] Remoção em 4 fases dos resíduos (§5 da revisão): `mise`/CI → desktop → Apple → constantes
      `MOBILE`/`TOUCH`/`TV`.
- [ ] `README.md` com uma plataforma só.

**Pronto quando:** `mise run android` num clone limpo gera um APK só-Android, e no S21+ o jogo
sobrevive a: voltar do background, girar, mudar zoom de tela, apertar Voltar, e a uma sessão de
15 min (sem morte por memória).

---

## Fase 1 — Perfil de dispositivo e governador térmico (2 semanas)

Esta é a fase que faz o S21+ **parecer** que roda bem, antes de qualquer melhoria visual.

**1.1 `DeviceProfile`** — novo módulo `crates/glue5-runtime/src/device.rs`:

```rust
pub struct Profile {
    pub name: &'static str,      // "galaxy-s21-plus-exynos2100"
    pub render_lines: u32,       // altura alvo do back buffer (720 hoje p/ todos os celulares)
    pub max_fps: u32,            // 40 no S21+ (>30 estável, metade de um painel de 120)
    pub low_memory: bool,        // 8 GB → provavelmente sim
    pub texture_budget_mb: u32,  // quanto de textura cabe sem o lmkd entrar
    pub game_preset: Preset,     // o que escrever no settings.xml do jogo na 1ª execução
}
```

Detecção por `wgpu::AdapterInfo` (`device`/`vendor`/`name`) + `system_ram()`, com tabela de 5–10
aparelhos e um perfil `generic`. O jogador pode sobrepor tudo em Settings › Video (o perfil só decide
o padrão). **Critério:** no S21+, primeira execução já sai com 40 fps, escala de render 0.6 e sombras
médias, sem o jogador tocar em nada.

**1.2 Governador térmico** (A9): `PowerManager` via JNI → escada `LIGHT/MODERATE/SEVERE/CRITICAL`
que mexe em `render_scale` e `max_fps` (e, no `SEVERE`, em uma opção do jogo). Mostrar no overlay e
logar. **Critério:** 20 minutos de jogo sem queda abaixo de 30 fps e sem tela preta.

**1.3 Taxa de quadros** (A10): `FIXED_SOURCE` + estratégia `ALWAYS`; testar 30/40/60 no painel
48–120 Hz do S21+ com `dumpsys SurfaceFlinger`.

---

## Fase 2 — "Boot de console" (3–4 semanas)

A porta de entrada é 90% da impressão. Hoje há uma página inicial competente; falta o que faz o
jogador sentir que comprou um jogo.

- [ ] **Fluxo de abertura**: logo → "carregando arquivos" com dicas rotativas (as cenas já existem em
      `assets/ui/scenes/`) → menu com abas `JOGAR · MAPA · ESTATÍSTICAS · AMIGOS · CONFIGURAÇÕES`,
      como o menu de pausa do GTA V, e não como uma página de configuração de emulador.
- [ ] **Continuar**: cartão grande com o último save, nome do personagem, tempo de jogo e onde ele
      estava; um "Jogar" que sempre retoma.
- [ ] **Slots de save** (3, como os jogos da série): `save_slot`, timestamp, screenshot do momento
      (o readback de miniatura já existe — `hooks/readback.rs`), "sobrescrever?".
- [ ] **Checkpoint / reiniciar missão**: o mais importante da fase. O motor já tem falha de missão
      exposta (`stats.rs`, `gamelog.rs`); o app precisa de um botão "Reiniciar do checkpoint" no
      menu de pausa e no *game over*, sem voltar à tela de carregamento.
- [ ] **Salvamento automático** a cada N minutos e ao entrar/sair de veículo, com indicador discreto.

**Pronto quando:** matar o app no meio de uma missão e abrir de novo leva ao mesmo ponto em menos de
5 s, com um toque.

---

## Fase 3 — Controles (3 semanas)

- [ ] **Haptics Android** (A7) — o item de maior impacto por linha de código. Toque nos botões,
      *rumble* do motor, respeitando o ajuste do sistema.
- [ ] **Editor de layout**: arrastar cada botão (o `touch.rs` já posiciona por `At::{Left,Right,Mid}`),
      salvar em `glue5.json` por contexto (a pé / dirigindo / avião), com "restaurar padrão".
- [ ] **Três perfis como na série**: `Simplificado` (o atual "Modern"), `Completo` (o "Classic") e
      `Personalizado`.
- [ ] **Botões contextuais**: nada de botão fixo que não faz nada; o sistema de `prompts.rs` já
      alimenta isso — falta esconder/mostrar com animação em vez de piscar.
- [ ] **Aim assist por toque** calibrado para o S21+ (o `TouchAim` já mapeia para os modos do jogo) e
      **aceleração automática** opcional em veículos.
- [ ] **Testes com telas Android** (A15): 914×411 com *punch-hole*, 20:9, barra de gestos.

**Pronto quando:** um jogador que nunca viu GTA V dirige, atira e entra num carro sem abrir ajuda.

---

## Fase 4 — Desempenho "de estúdio" (4 semanas)

- [ ] **A6**: `StoreOp::Discard` onde o motor não relê — a maior vitória de banda num Mali, sem mudar
      um pixel.
- [ ] **A11**: cache em disco de texturas transcodificadas; depois ASTC pré-cozido no build.
- [ ] **Resolução dinâmica**: alvo de 30/40/60 fps com `render_scale` variando 0.4–0.9 em passos de
      0.05 e histerese de 1 s (o `present.rs` já aceita escala em runtime).
- [ ] **Pré-compilação de shaders na instalação** (o `prepare.rs` já compila as pipelines conhecidas):
      mover para "depois de instalar, antes de jogar", com barra de progresso, e cache no disco.
- [ ] **Orçamento de memória**: medir pico por cena; se passar do perfil, baixar mip bias antes de o
      `lmkd` decidir.
- [ ] **A16**: cache de pipeline inclui a versão do driver.

**Pronto quando:** 30 minutos de jogo no S21+ com frame time p95 abaixo de 33 ms e sem morte por
memória, em missão de perseguição com polícia.

---

## Fase 5 — O que faz sentir "GTA V" (contínuo)

- HUD escalável e reposicionável (o minimapa do jogo é o que mais sofre em 6.7").
- Roda de armas e de rádio com pré-visualização (parcialmente pronto).
- **Modo foto / compartilhar** pela folha de compartilhamento do Android (`FileProvider` já existe).
- Legendas e nomes de menu em pt-BR (hoje a UI do app é em inglês; o jogo tem os textos).
- **Sessões curtas**: retomar em < 2 s do background, inclusive depois de o sistema matar o processo
  (SDL recria a Activity — precisa do estado em disco, não em memória).
- **Online sem servidores** com o lock de LAN (A4) e um "entrar rápido" no amigo visto por último.

---

## Fase 6 — Distribuição

- APK por perfil de aparelho, `versionCode` derivado de `VERSION_CODE` (A12), assinatura própria.
- Build no GitHub restrito a Android (`docs/github-build.md`).
- **Não** perseguir Google Play enquanto houver `MANAGE_EXTERNAL_STORAGE` e chave de debug: o
  caminho natural é sideload/apk próprio, como os builds pessoais de hoje.

---

## Ordem sugerida (uma coisa por vez)

```
Fase 0 (fundação)  →  Fase 1.1 (perfil)  →  Fase 1.2 (termal)
                   →  Fase 3 (haptics + editor de layout)
                   →  Fase 2 (saves + checkpoint)
                   →  Fase 4 (banda + dinâmica)
                   →  Fase 5/6
```

Haptics e editor de layout vêm antes dos saves porque são baratos e mudam a percepção do produto
inteiro; saves e checkpoint vêm antes da performance porque sem eles o jogo não é jogável em
sessões curtas, por mais rápido que esteja.
