# GM45 — incremento de `Draw` real (aplicar + compilar)

Este pacote adiciona o caminho vertical que faltava no backend D3D11: compilar
`vs_4_0`/`ps_4_0` no dispositivo real, bind de pipeline, `Draw` e `DrawIndexed`,
render target com viewport, e readback para CPU com verificação de pixels.

Nada aqui altera `d3d11.rs`, `executor.rs` ou `ring.rs`. O único contato com
código existente são **duas linhas de âncora** (`Cargo.toml` ganha as features
do `D3DCompile`; `lib.rs` registra e reexporta o módulo novo).

## Arquivos

| Arquivo | Situação |
| --- | --- |
| `crates/glue5-gpu-d3d11/src/graphics.rs` | **novo** — todo o caminho de render |
| `crates/glue5-gpu-d3d11/examples/gm45_draw_probe.rs` | **novo** — harness de aceitação |
| `crates/glue5-gpu-d3d11/Cargo.toml` | +2 features (`Direct3DCompiler`, `Direct3D_Fxc`) |
| `crates/glue5-gpu-d3d11/src/lib.rs` | +`mod graphics;` e o `pub use` |

## 1. Buscar os arquivos novos (sem trocar de branch)

```powershell
cd C:\k8r\glue5-gm45-fallback
git status --porcelain          # deve continuar 7 M + 7 ??
git fetch origin arena/3448d280-glue5
git checkout FETCH_HEAD -- crates/glue5-gpu-d3d11/src/graphics.rs crates/glue5-gpu-d3d11/examples/gm45_draw_probe.rs
```

`git checkout <tree-ish> -- <paths>` só escreve esses dois caminhos, que hoje
não existem na sua árvore (o crate é *untracked*). Não muda `HEAD`, não mexe no
branch `research/gm45-compute-fallbacks`, não toca nos 7 modificados.

## 2. Costurar as duas âncoras

```powershell
powershell -ExecutionPolicy Bypass -File .\apply-gm45-draw.ps1
```

O script faz backup (`.gm45draw.bak`), é idempotente e falha alto se a âncora
não existir. Se preferir editar à mão:

* `Cargo.toml` — dentro do bloco `features = [ ... ]` do `windows`:
  ```toml
      "Win32_Graphics_Direct3DCompiler",
      "Win32_Graphics_Direct3D_Fxc",
  ```
* `lib.rs` — depois de `mod ring;` e depois de `pub use ring::backend;`:
  ```rust
  #[cfg(target_os = "windows")]
  mod graphics;
  #[cfg(target_os = "windows")]
  pub use graphics::{
      AdapterIdentity, ConstantBuffer, DrawProbeReport, GraphicsPipeline, IndexBuffer,
      InputElement, RenderTarget, VertexBuffer, adapter_identity, draw, draw_indexed, pixel_at,
      pixel_close, run_draw_probe,
  };
  ```

## 3. Compilar, testar, rodar

```powershell
cargo build -p glue5-gpu-d3d11 --examples
cargo test  -p glue5-gpu-d3d11                 # 9 testes puros, sem GPU
cargo run   -p glue5-gpu-d3d11 --example gm45_draw_probe --release
cargo test  -p glue5-gpu-d3d11 -- --ignored    # o mesmo probe como teste
```

Saída esperada no GM45:

```text
== GLUE5 D3D11 draw probe (GM45 / FL10_0) ==
[ok] hardware device opened (no WARP fallback)
     feature level: Level10_0
[ok] adapter: vendor=0x8086 device=0x2a42 software=false "Mobile Intel(R) 4 Series ..."
     identity: Mobile Intel 4 Series (GM45, PCI 8086:2A42)
[ok] render target: 64x64 R8G8B8A8_UNORM
[ok] stage 1 (SV_VertexID, Draw):      centre=[64, 128, 191, 255] corner=[0, 0, 0, 255]
[ok] stage 2 (VB+IB+IL+cbuffer, DrawIndexed): centre=[64, 128, 191, 255]
PASS: real Draw reached the render target and the pixels matched
```

## 4. Se não compilar

Esta é a parte honesta: o código foi escrito contra a API da crate (li
`Cargo.toml`, `lib.rs`, a struct `D3D11Device` com `raw_device()`/
`immediate_context()`, `readback_texture`, `TextureBindings`) mas **não foi
compilado** — o ambiente de onde ele foi escrito não tem toolchain Rust nem
alcance até `crates.io`, e o relay até o Windows é somente leitura.

As assinaturas com maior chance de divergência, todas isoladas:

1. `D3DCompile` — se a sua windows-rs devolver `HRESULT` em vez de
   `Result<()>`, o compilador aponta a linha em `compile_shader`
   (há um comentário no local exato).
2. `OMSetRenderTargets` / `VSSetConstantBuffers` / `IASetVertexBuffers` —
   forma slice (`Some(&[...])`) vs ponteiro; cada um está numa função de bind
   única (`bind_pipeline`, `RenderTarget::bind`).
3. `ClearRenderTargetView` — `&color` vs `*const [f32;4]`.

Cole o `cargo build` completo (com `RUSTFLAGS` padrão) que eu corrijo e devolvo
o arquivo pronto — o ciclo é rápido porque o módulo inteiro é meu, não é
cirurgia no código auditado.
