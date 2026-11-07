# Apply the GM45 draw-path increment to C:\k8r\glue5-gm45-fallback
#
# Read-only with respect to history: it never checks out a branch, never
# commits, never resets and never touches files other than the two anchors
# below. Each file is backed up with a .gm45draw.bak suffix before the first
# edit, and every edit is idempotent, so running it twice is safe.
#
#   powershell -ExecutionPolicy Bypass -File .\apply-gm45-draw.ps1
#
param(
    [string]$Repo = "C:\k8r\glue5-gm45-fallback"
)

$ErrorActionPreference = "Stop"

$crate    = Join-Path $Repo "crates\glue5-gpu-d3d11"
$cargoToml = Join-Path $crate "Cargo.toml"
$libRs     = Join-Path $crate "src\lib.rs"

foreach ($path in @($cargoToml, $libRs)) {
    if (-not (Test-Path $path)) { throw "missing expected file: $path" }
}

function Backup-Once([string]$path) {
    $backup = "$path.gm45draw.bak"
    if (-not (Test-Path $backup)) { Copy-Item $path $backup }
}

# --- 1. Cargo.toml: the D3DCompile entry points live behind these features ---
Backup-Once $cargoToml
$toml = Get-Content $cargoToml -Raw
if ($toml -notmatch "Win32_Graphics_Direct3D_Fxc") {
    $anchor = '    "Win32_Graphics_Dxgi_Common",'
    if ($toml -notmatch [regex]::Escape($anchor)) { throw "Cargo.toml feature anchor not found" }
    $replacement = $anchor + "`r`n" +
        '    "Win32_Graphics_Direct3DCompiler",' + "`r`n" +
        '    "Win32_Graphics_Direct3D_Fxc",'
    $toml = $toml -replace [regex]::Escape($anchor), $replacement
    [System.IO.File]::WriteAllText($cargoToml, $toml, (New-Object System.Text.UTF8Encoding($false)))
    Write-Host "[edit] Cargo.toml: added Win32_Graphics_Direct3DCompiler + Win32_Graphics_Direct3D_Fxc"
} else {
    Write-Host "[skip] Cargo.toml already has the Direct3D compiler features"
}

# --- 2. lib.rs: register the new module next to the existing ones ---
Backup-Once $libRs
$lib = Get-Content $libRs -Raw
if ($lib -notmatch "mod graphics;") {
    $anchor = "mod ring;"
    if ($lib -notmatch [regex]::Escape($anchor)) { throw "lib.rs module anchor not found" }
    $lib = $lib -replace [regex]::Escape($anchor),
        ($anchor + "`r`n#[cfg(target_os = `"windows`")]`r`nmod graphics;")
    Write-Host "[edit] lib.rs: registered mod graphics"
} else {
    Write-Host "[skip] lib.rs already registers mod graphics"
}

if ($lib -notmatch "pub use graphics::") {
    $anchor = "pub use ring::backend;"
    if ($lib -notmatch [regex]::Escape($anchor)) { throw "lib.rs re-export anchor not found" }
    $exports = @(
        $anchor,
        '#[cfg(target_os = "windows")]',
        'pub use graphics::{',
        '    AdapterIdentity, ConstantBuffer, DrawProbeReport, GraphicsPipeline, IndexBuffer,',
        '    InputElement, RenderTarget, VertexBuffer, adapter_identity, draw, draw_indexed, pixel_at,',
        '    pixel_close, run_draw_probe,',
        '};'
    ) -join "`r`n"
    $lib = $lib -replace [regex]::Escape($anchor), $exports
    Write-Host "[edit] lib.rs: re-exported the graphics module"
} else {
    Write-Host "[skip] lib.rs already re-exports the graphics module"
}

[System.IO.File]::WriteAllText($libRs, $lib, (New-Object System.Text.UTF8Encoding($false)))

Write-Host ""
Write-Host "Next:"
Write-Host "  cd $Repo"
Write-Host "  cargo build -p glue5-gpu-d3d11 --example gm45_draw_probe"
Write-Host "  cargo test  -p glue5-gpu-d3d11"
Write-Host "  cargo run   -p glue5-gpu-d3d11 --example gm45_draw_probe --release"
