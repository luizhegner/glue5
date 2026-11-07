# GM45 — incremento de `Draw` real (aplicar + compilar)

Adiciona ao backend D3D11 o caminho vertical que faltava: compilar `vs_4_0`/`ps_4_0`
no dispositivo real, bind de pipeline, `Draw` e `DrawIndexed`, render target com
viewport, e readback para CPU com verificação de pixels.

**O que este pacote NÃO faz:** não altera `d3d11.rs`, `executor.rs` nem `ring.rs`;
não mexe em Git (sem add, checkout, reset, clean, commit); não toca nos 7 arquivos
modificados nem nos 7 não rastreados do seu checkout.

## Estrutura publicada (branch `arena/3448d280-glue5`)

| Caminho | Papel | Origem |
| --- | --- | --- |
| `crates/glue5-gpu-d3d11/src/graphics.rs` | caminho de render completo | **recuperado do commit** |
| `crates/glue5-gpu-d3d11/examples/gm45_draw_probe.rs` | harness de aceitação | **recuperado do commit** |
| `patches/gm45-draw/apply-gm45-draw.ps1` | instalador | **recuperado do commit** |
| `crates/glue5-gpu-d3d11/Cargo.toml` | +`Win32_Graphics_Direct3D_Fxc` (`Direct3D` ja presente) | editado no lugar pelo instalador |
| `crates/glue5-gpu-d3d11/src/lib.rs` | +`mod graphics;` e reexports | editado no lugar pelo instalador |

Só existem **três** arquivos para recuperar. `Cargo.toml` e `lib.rs` já existem no
seu checkout (o crate é *untracked*) e são editados pelo instalador — não são
recuperados.

---

## 0. Estado inicial (somente leitura)

```powershell
cd C:\k8r\glue5-gm45-fallback

git rev-parse --abbrev-ref HEAD      # research/gm45-compute-fallbacks
git rev-parse HEAD                   # guarde este hash: ele NAO pode mudar

$before = @(git status --porcelain)
"modificados: " + @($before | Where-Object { $_ -like ' M *' }).Count   # esperado 7
"untracked  : " + @($before | Where-Object { $_ -like '?? *' }).Count   # esperado 7
"staged     : " + @(git diff --cached --name-only).Count                # esperado 0
```

Se `staged` não for 0, pare e resolva antes: o procedimento não deve mexer no
index e você precisa de uma linha de base limpa.

## 1. Buscar a branch remota do Arena

```powershell
git fetch origin arena/3448d280-glue5
```

Não faz checkout de branch, não altera a árvore, não altera o index.

## 2. Capturar o hash do commit recuperado

```powershell
$c = git rev-parse FETCH_HEAD
$c
```

Anote o valor. Todos os passos seguintes usam `$c` e nada mais.

## 3. Recuperar os três arquivos — com proteção contra sobrescrita

O comando abaixo **verifica cada destino antes de escrever**: se o arquivo já
existir com conteúdo diferente do commit, o script para e não sobrescreve nada.

```powershell
$paths = @(
  'crates/glue5-gpu-d3d11/src/graphics.rs',
  'crates/glue5-gpu-d3d11/examples/gm45_draw_probe.rs',
  'patches/gm45-draw/apply-gm45-draw.ps1'
)

$todo = @()
foreach ($p in $paths) {
    if (-not (Test-Path -LiteralPath $p)) {
        "[absent]    $p"
        $todo += $p
        continue
    }
    # hash dos bytes crus do arquivo local vs blob do commit
    $local  = git hash-object --no-filters -- $p
    $blob   = git rev-parse "${c}:$p"
    if ($local -eq $blob) {
        "[identical] $p"
    } else {
        "[DIFFERENT] $p"
        throw "O arquivo $p ja existe com conteudo diferente do commit. Abortando preservando o original. Mova-o para fora do repositorio e rode de novo."
    }
}

# git restore --source=... --worktree NAO altera o index (ao contrario de
# `git checkout <commit> -- <path>`, que deixaria os arquivos staged).
if ($todo.Count -gt 0) {
    git restore --source=$c --worktree -- @todo
}

# conferencia final: os tres batem com o commit e nada foi staged
foreach ($p in $paths) {
    $local = git hash-object --no-filters -- $p
    $blob  = git rev-parse "${c}:$p"
    if ($local -ne $blob) { throw "falha ao recuperar $p" }
    "[ok] $p"
}
"staged: " + @(git diff --cached --name-only).Count   # DEVE continuar 0
```

Requer git ≥ 2.23 (`git restore`). Em versões mais antigas, use o equivalente
abaixo — que também não deixa nada staged:

```powershell
git checkout $c -- @todo
git restore --staged -- @todo        # ou: git reset -q -- @todo
```

## 4. Rodar o instalador — pelo caminho correto

O script foi recuperado dentro do repositório, em `patches\gm45-draw\`:

```powershell
# simulação: não grava nada, só mostra o plano
powershell -ExecutionPolicy Bypass -File .\patches\gm45-draw\apply-gm45-draw.ps1 -Repo (Get-Location).Path -DryRun

# aplicação real
powershell -ExecutionPolicy Bypass -File .\patches\gm45-draw\apply-gm45-draw.ps1 -Repo (Get-Location).Path
```

O instalador:

* modifica **somente** `crates\glue5-gpu-d3d11\Cargo.toml` e `crates\glue5-gpu-d3d11\src\lib.rs`;
* verifica as duas features **individualmente** (`Win32_Graphics_Direct3D`, que
  fornece `ID3DBlob`/`ID3DInclude`, e `Win32_Graphics_Direct3D_Fxc`, que fornece
  `D3DCompile`), inserindo apenas as que faltarem — normalmente só a segunda;
* recusa `Win32_Graphics_Direct3DCompiler`: essa feature **não existe** em
  `windows` 0.62.2 e faria o cargo falhar;
* exige `src\graphics.rs` presente antes de registrar `mod graphics;`;
* aborta se alguma âncora estiver ausente ou ambígua;
* aborta se `mod graphics;`/`pub use graphics::{` já existirem com conteúdo diferente do esperado;
* preserva BOM e quebras de linha de cada arquivo;
* é idempotente (segunda execução não duplica nada);
* grava backups em `%TEMP%\gm45draw-<arquivo>-<timestamp>.bak`, nunca dentro do repositório;
* não executa git.

## 5. Verificação do estado

```powershell
git rev-parse HEAD                 # IDENTICO ao hash guardado no passo 0
"staged: " + @(git diff --cached --name-only).Count    # 0

$after = @(git status --porcelain)
"modificados: " + @($after | Where-Object { $_ -like ' M *' }).Count   # 7
"untracked  : " + @($after | Where-Object { $_ -like '?? *' }).Count   # 8 (o 8o e patches/)

# os 7 modificados continuam exatamente os mesmos
Compare-Object $before $after | Where-Object { $_.SideIndicator -eq '<=' }   # vazio

# conteúdo aplicado
Select-String -LiteralPath crates\glue5-gpu-d3d11\Cargo.toml -Pattern '"Win32_Graphics_Direct3D"','"Win32_Graphics_Direct3D_Fxc"'
Test-Path (Select-String -LiteralPath crates\glue5-gpu-d3d11\Cargo.toml -Pattern 'Direct3DCompiler')   # False
Select-String -LiteralPath crates\glue5-gpu-d3d11\src\lib.rs -Pattern 'mod graphics;','pub use graphics::'

# backups gravados
Get-ChildItem $env:TEMP -Filter 'gm45draw-*.bak' | Select-Object Name,Length,LastWriteTime
```

O único item novo esperado em `git status` é `?? patches/`. `crates/glue5-gpu-d3d11/`
já era untracked antes, então nada muda lá. Nada fica staged.

## 6. Compilar, testar e rodar o probe

O crate entra no workspace automaticamente: o `Cargo.toml` da raiz declara
`members = ["crates/*"]` e `[workspace.package]`, então `version.workspace`/
`edition.workspace` resolvem e `-p glue5-gpu-d3d11` funciona.

```powershell
cargo build -p glue5-gpu-d3d11 --examples
cargo test  -p glue5-gpu-d3d11                      # 13 testes puros, sem GPU
cargo run   -p glue5-gpu-d3d11 --example gm45_draw_probe --release
cargo test  -p glue5-gpu-d3d11 -- --ignored         # o mesmo probe como teste
```

Saída esperada no GM45:

```text
== GLUE5 D3D11 draw probe (GM45 / FL10_0) ==
[ok] hardware device opened (no WARP fallback)
     feature level: Level10_0
[ok] adapter: vendor=0x8086 device=0x2a42 software=false "Mobile Intel(R) 4 Series ..."
     identity: Mobile Intel 4 Series (GM45, PCI 8086:2A42)
[ok] render target: 64x64 R8G8B8A8_UNORM
[ok] stage 1 (SV_VertexID, Draw): centre=[64, 128, 191, 255] corner=[0, 0, 0, 255]
[ok] stage 2 (VB+IB+IL+cbuffer, DrawIndexed): centre=[128, 64, 32, 255] corner=[0, 0, 0, 255]
PASS: real Draw reached the render target and the pixels matched
```

O probe é **estrito por padrão**: exige PCI `8086:2A42` e Feature Level 10_0.
Em outra máquina use `--allow-any-adapter` (ainda recusa adaptador de software).

## Rollback

```powershell
# restaurar os dois arquivos editados a partir dos backups impressos pelo instalador
Copy-Item -LiteralPath "$env:TEMP\gm45draw-Cargo.toml-<timestamp>.bak" -Destination crates\glue5-gpu-d3d11\Cargo.toml -Force
Copy-Item -LiteralPath "$env:TEMP\gm45draw-lib.rs-<timestamp>.bak"     -Destination crates\glue5-gpu-d3d11\src\lib.rs   -Force

# remover os arquivos recuperados (nenhum deles existia antes)
Remove-Item -LiteralPath crates\glue5-gpu-d3d11\src\graphics.rs
Remove-Item -LiteralPath crates\glue5-gpu-d3d11\examples\gm45_draw_probe.rs
Remove-Item -Recurse -Force patches\gm45-draw

git status --porcelain      # volta a 7 M + 7 ??
```

## O que não está validado

O `graphics.rs` **não foi compilado**: o ambiente onde ele foi produzido não tem
toolchain Rust nem acesso a `crates.io`, e o relay até o Windows é somente
leitura. As assinaturas foram conferidas uma a uma contra os bindings gerados de
`windows 0.62.2` (commit `d468916ac` de `microsoft/windows-rs`), mas verificação
estática não substitui o compilador.

Se o `cargo build` falhar, cole a saída completa: o módulo é autocontido e a
correção fica restrita a ele.
