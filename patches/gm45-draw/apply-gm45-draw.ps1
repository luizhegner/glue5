<#
.SYNOPSIS
    Instala o incremento "draw path" do GM45 num checkout existente do GLUE5.

.DESCRIPTION
    Edita exatamente dois arquivos do crate glue5-gpu-d3d11:

      * Cargo.toml  -> garante as features Win32_Graphics_Direct3D e
                       Win32_Graphics_Direct3D_Fxc (cada uma verificada por si,
                       nunca uma presumindo a outra); remove a feature
                       inexistente Win32_Graphics_Direct3DCompiler, se ela
                       tiver sobrado de uma versao anterior deste script;
      * src/lib.rs  -> registra `pub mod graphics;` (publico, porque o example
                       importa `glue5_gpu_d3d11::graphics::...`) e o bloco
                       `pub use graphics::{...}`.

    Garantias (Windows PowerShell 5.1 e PowerShell 7):

      * NAO executa git. Nem add, nem checkout, nem reset, nem clean, nem commit.
      * Nao cria, remove ou renomeia nenhum outro arquivo do repositorio.
      * Exige que src/graphics.rs ja exista: registrar o modulo sem o arquivo
        quebraria o build.
      * Calcula as duas edicoes e valida tudo ANTES de gravar qualquer coisa;
        se Cargo.toml ou lib.rs estiverem fora do esperado, nada e escrito.
      * Exige que cada ancora apareca exatamente uma vez; ancora ambigua ou
        ausente aborta a instalacao.
      * Se um trecho ja existir com conteudo inesperado, aborta em vez de
        sobrescrever.
      * Preserva codificacao (BOM) e quebras de linha de cada arquivo.
      * Idempotente: rodar duas vezes nao duplica nada.
      * Backups em %TEMP% (ou -BackupDir), nunca dentro do repositorio.
      * -DryRun mostra o que faria, sem tocar em nada.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\apply-gm45-draw.ps1 -Repo C:\k8r\glue5-gm45-fallback

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\apply-gm45-draw.ps1 -Repo C:\k8r\glue5-gm45-fallback -DryRun
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Repo,

    [string]$BackupDir = $env:TEMP,

    [switch]$DryRun
)

$ErrorActionPreference = "Stop"

# --------------------------------------------------------------------------
# helpers
# --------------------------------------------------------------------------

function Fail {
    param([string]$Message)
    throw ("apply-gm45-draw: " + $Message)
}

function Read-SourceFile {
    param([string]$Path)
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $hasBom = $false
    $offset = 0
    if (($bytes.Length -ge 3) -and ($bytes[0] -eq 0xEF) -and ($bytes[1] -eq 0xBB) -and ($bytes[2] -eq 0xBF)) {
        $hasBom = $true
        $offset = 3
    }
    $text = [System.Text.Encoding]::UTF8.GetString($bytes, $offset, $bytes.Length - $offset)
    $eol = "`n"
    if ($text.Contains("`r`n")) { $eol = "`r`n" }
    [pscustomobject]@{
        Path = $Path
        Text = $text
        Bom  = $hasBom
        Eol  = $eol
    }
}

function Save-SourceFile {
    param($File, [string]$Text)
    $encoding = New-Object System.Text.UTF8Encoding($File.Bom)
    [System.IO.File]::WriteAllText($File.Path, $Text, $encoding)
}

function Backup-File {
    param([string]$Path)
    $name = [System.IO.Path]::GetFileName($Path)
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $target = Join-Path $BackupDir ("gm45draw-" + $name + "-" + $stamp + ".bak")
    Copy-Item -LiteralPath $Path -Destination $target -Force
    Write-Host ("[backup] " + $Path + " -> " + $target)
    return $target
}

function Count-Literal {
    param([string]$Text, [string]$Needle)
    $count = 0
    $index = $Text.IndexOf($Needle, [System.StringComparison]::Ordinal)
    while ($index -ge 0) {
        $count = $count + 1
        $index = $Text.IndexOf($Needle, $index + $Needle.Length, [System.StringComparison]::Ordinal)
    }
    $count
}

function Require-SingleAnchor {
    param([string]$Text, [string]$Needle, [string]$Where)
    $count = Count-Literal -Text $Text -Needle $Needle
    if ($count -eq 0) {
        Fail ("ancora ausente em " + $Where + ": " + $Needle)
    }
    if ($count -gt 1) {
        Fail ("ancora ambigua em " + $Where + " (" + $count + " ocorrencias de " + $Needle + "); abortando sem alterar nada")
    }
}

# --------------------------------------------------------------------------
# 1. pre-condicoes - nada e lido para escrita antes daqui
# --------------------------------------------------------------------------

if (-not (Test-Path -LiteralPath $Repo -PathType Container)) {
    Fail ("repositorio nao encontrado: " + $Repo)
}
$Repo = (Resolve-Path -LiteralPath $Repo).Path

$crateDir  = Join-Path $Repo "crates\glue5-gpu-d3d11"
$cargoPath = Join-Path $crateDir "Cargo.toml"
$libPath   = Join-Path $crateDir "src\lib.rs"
$gfxPath   = Join-Path $crateDir "src\graphics.rs"
$probePath = Join-Path $crateDir "examples\gm45_draw_probe.rs"

if (-not (Test-Path -LiteralPath $crateDir -PathType Container)) {
    Fail ("crate nao encontrado: " + $crateDir)
}
foreach ($required in @($cargoPath, $libPath)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        Fail ("arquivo esperado ausente: " + $required)
    }
}
if (-not (Test-Path -LiteralPath $gfxPath -PathType Leaf)) {
    Fail "src\graphics.rs ausente. Recupere-o do commit da branch primeiro (passo 4 do APPLY.md); registrar 'mod graphics;' sem o arquivo quebra o build."
}
if (-not (Test-Path -LiteralPath $probePath -PathType Leaf)) {
    Write-Warning "examples\gm45_draw_probe.rs ausente; o crate compila mas o example nao existira"
}

$cargo = Read-SourceFile -Path $cargoPath
$lib   = Read-SourceFile -Path $libPath

if ((Count-Literal -Text $cargo.Text -Needle "glue5-gpu-d3d11") -eq 0) {
    Fail ("Cargo.toml nao parece ser o manifesto do crate glue5-gpu-d3d11: " + $cargoPath)
}

Write-Host ("[info] repositorio : " + $Repo)
Write-Host ("[info] Cargo.toml  : BOM=" + $cargo.Bom + " EOL=" + $(if ($cargo.Eol -eq "`r`n") { "CRLF" } else { "LF" }))
Write-Host ("[info] src/lib.rs  : BOM=" + $lib.Bom + " EOL=" + $(if ($lib.Eol -eq "`r`n") { "CRLF" } else { "LF" }))
if ($DryRun) { Write-Host "[info] modo DRY-RUN: nenhum arquivo sera gravado" }

# --------------------------------------------------------------------------
# 2. calcular a edicao de Cargo.toml (as duas features, uma por uma)
# --------------------------------------------------------------------------

# Features exigidas pelo caminho de render, verificadas UMA A UMA:
#   Win32_Graphics_Direct3D      -> ID3DBlob, ID3DInclude, D3D_SHADER_MACRO
#   Win32_Graphics_Direct3D_Fxc  -> D3DCompile e as flags D3DCOMPILE_*
# Ambas existem no manifesto publicado de windows 0.62.2 (Fxc implica Direct3D).
# ATENCAO: "Win32_Graphics_Direct3DCompiler" NAO existe em 0.62.2; pedi-la faria o
# cargo falhar com "does not have these features".
$featureNames = @("Win32_Graphics_Direct3D", "Win32_Graphics_Direct3D_Fxc")

$bogusFeature = "Win32_Graphics_Direct3DCompiler"
$cargoUpdated = $cargo.Text
if ((Count-Literal -Text $cargoUpdated -Needle ('"' + $bogusFeature + '"')) -gt 0) {
    # Residuo de uma versao anterior deste instalador. A feature nao existe no
    # manifesto de windows 0.62.2; remove-la e o conserto, e o backup ja cobre
    # a reversao.
    $bogusLine = '    "' + $bogusFeature + '",' + $cargo.Eol
    if ((Count-Literal -Text $cargoUpdated -Needle $bogusLine) -ne 1) {
        Fail ("Cargo.toml contem " + $bogusFeature + " em formato inesperado. Remova a linha manualmente e rode de novo. Nada foi alterado.")
    }
    $cargoUpdated = $cargoUpdated.Replace($bogusLine, "")
    Write-Host ("[plan] Cargo.toml: - " + $bogusFeature + " (nao existe em windows 0.62.2)")
}

$missingFeatures = @()
foreach ($feature in $featureNames) {
    if ((Count-Literal -Text $cargoUpdated -Needle ('"' + $feature + '"')) -eq 0) {
        $missingFeatures = $missingFeatures + $feature
    }
}

if ($missingFeatures.Count -eq 0) {
    Write-Host "[skip] Cargo.toml: as duas features D3D exigidas pelo render ja estao presentes"
} else {
    $anchor = '    "Win32_Graphics_Dxgi_Common",'
    Require-SingleAnchor -Text $cargoUpdated -Needle $anchor -Where "Cargo.toml"
    $insertLines = @()
    foreach ($feature in $missingFeatures) {
        $insertLines = $insertLines + ('    "' + $feature + '",')
    }
    $cargoUpdated = $cargoUpdated.Replace($anchor, ($anchor + $cargo.Eol + ($insertLines -join $cargo.Eol)))
    Write-Host ("[plan] Cargo.toml: + " + ($missingFeatures -join ", "))
}

# --------------------------------------------------------------------------
# 3. calcular a edicao de src/lib.rs
# --------------------------------------------------------------------------

$cfgLine     = '#[cfg(target_os = "windows")]'
$moduleLine  = "pub mod graphics;"
$moduleBlock = $cfgLine + $lib.Eol + $moduleLine

$libUpdated = $lib.Text
$moduleCount = Count-Literal -Text $libUpdated -Needle $moduleLine
$privateModuleCount = Count-Literal -Text $libUpdated -Needle "mod graphics;"
if ($privateModuleCount -ne $moduleCount) {
    # Residuo de uma versao anterior: 'mod graphics;' privado impede o example
    # de importar glue5_gpu_d3d11::graphics::.... Promove para publico.
    $privateBlock = $cfgLine + $lib.Eol + "mod graphics;"
    if ((Count-Literal -Text $libUpdated -Needle $privateBlock) -ne 1) {
        Fail "src/lib.rs declara 'mod graphics;' privado, mas sem o '#[cfg(target_os = \"windows\")]' esperado na linha anterior. Ajuste manualmente para 'pub mod graphics;'. Nada foi alterado."
    }
    $libUpdated = $libUpdated.Replace($privateBlock, $moduleBlock)
    $moduleCount = Count-Literal -Text $libUpdated -Needle $moduleLine
    Write-Host "[plan] src/lib.rs: mod graphics -> pub mod graphics"
}
if ($moduleCount -eq 0) {
    Require-SingleAnchor -Text $libUpdated -Needle "mod ring;" -Where "src/lib.rs"
    $libUpdated = $libUpdated.Replace("mod ring;", "mod ring;" + $lib.Eol + $moduleBlock)
    Write-Host "[plan] src/lib.rs: + pub mod graphics"
} elseif ($moduleCount -eq 1) {
    if ((Count-Literal -Text $libUpdated -Needle $moduleBlock) -ne 1) {
        Fail "src/lib.rs ja contem 'pub mod graphics;' mas sem '#[cfg(target_os = \"windows\")]' imediatamente antes. Conteudo inesperado: abortando sem sobrescrever."
    }
    Write-Host "[skip] src/lib.rs: mod graphics ja registrado corretamente"
} else {
    Fail ("src/lib.rs contem 'pub mod graphics;' " + $moduleCount + " vezes; esperado no maximo 1. Abortando.")
}

$exportAnchor = "pub use ring::backend;"
$exportBlockLines = @(
    $cfgLine,
    'pub use graphics::{',
    '    AdapterIdentity, ConstantBuffer, DrawProbeReport, GraphicsPipeline, IndexBuffer,',
    '    InputElement, ProbeVertex, RasterizerState, RenderTarget, VertexBuffer, adapter_identity,',
    '    draw, draw_indexed, pixel_at, pixel_close, run_draw_probe, signed_area_twice,',
    '};'
)
$exportCount = Count-Literal -Text $libUpdated -Needle 'pub use graphics::{'
if ($exportCount -eq 0) {
    Require-SingleAnchor -Text $libUpdated -Needle $exportAnchor -Where "src/lib.rs"
    $libUpdated = $libUpdated.Replace($exportAnchor, ($exportAnchor + $lib.Eol + ($exportBlockLines -join $lib.Eol)))
    Write-Host "[plan] src/lib.rs: + pub use graphics::{...}"
} elseif ($exportCount -eq 1) {
    if ((Count-Literal -Text $libUpdated -Needle "DrawProbeReport") -eq 0) {
        Fail "src/lib.rs ja contem 'pub use graphics::{' mas sem DrawProbeReport; conteudo inesperado. Abortando sem sobrescrever."
    }
    Write-Host "[skip] src/lib.rs: reexport de graphics ja presente"
} else {
    Fail ("src/lib.rs contem 'pub use graphics::{' " + $exportCount + " vezes; esperado no maximo 1. Abortando.")
}

# --------------------------------------------------------------------------
# 4. nada a fazer? / dry-run
# --------------------------------------------------------------------------

$cargoChanged = ($cargoUpdated -ne $cargo.Text)
$libChanged   = ($libUpdated -ne $lib.Text)

if (-not $cargoChanged -and -not $libChanged) {
    Write-Host ""
    Write-Host "[ok] nada a fazer: o incremento ja esta instalado"
    return
}
if ($DryRun) {
    Write-Host ""
    Write-Host ("[dry-run] gravaria: Cargo.toml=" + $cargoChanged + " lib.rs=" + $libChanged)
    return
}

# --------------------------------------------------------------------------
# 5. backup e gravacao (so agora, com tudo validado)
# --------------------------------------------------------------------------

if (-not (Test-Path -LiteralPath $BackupDir -PathType Container)) {
    New-Item -ItemType Directory -Path $BackupDir -Force | Out-Null
}

$cargoBackup = $null
$libBackup   = $null
if ($cargoChanged) { $cargoBackup = Backup-File -Path $cargoPath }
if ($libChanged)   { $libBackup   = Backup-File -Path $libPath }

if ($cargoChanged) { Save-SourceFile -File $cargo -Text $cargoUpdated }
if ($libChanged)   { Save-SourceFile -File $lib   -Text $libUpdated }

# --------------------------------------------------------------------------
# 6. verificacao final relendo o disco
# --------------------------------------------------------------------------

$cargoFinal = Read-SourceFile -Path $cargoPath
$libFinal   = Read-SourceFile -Path $libPath

foreach ($feature in $featureNames) {
    $count = Count-Literal -Text $cargoFinal.Text -Needle ('"' + $feature + '"')
    if ($count -ne 1) {
        Fail ("verificacao falhou: feature " + $feature + " aparece " + $count + " vezes em Cargo.toml (esperado 1)")
    }
}
if ((Count-Literal -Text $libFinal.Text -Needle $moduleBlock) -ne 1) {
    Fail "verificacao falhou: bloco 'pub mod graphics' (com cfg) nao aparece exatamente uma vez em src/lib.rs"
}
if ((Count-Literal -Text $libFinal.Text -Needle 'pub use graphics::{') -ne 1) {
    Fail "verificacao falhou: 'pub use graphics::{' nao aparece exatamente uma vez em src/lib.rs"
}
foreach ($symbol in @("AdapterIdentity", "DrawProbeReport", "run_draw_probe")) {
    if ((Count-Literal -Text $libFinal.Text -Needle $symbol) -eq 0) {
        Fail ("verificacao falhou: " + $symbol + " nao aparece no reexport de src/lib.rs")
    }
}

Write-Host ""
Write-Host "[ok] instalado e verificado"
Write-Host ("     Cargo.toml : " + (($cargoFinal.Text -split "`n").Count) + " linhas")
Write-Host ("     src/lib.rs : " + (($libFinal.Text -split "`n").Count) + " linhas")
Write-Host ""
Write-Host "Proximos passos:"
Write-Host ("  cd " + $Repo)
Write-Host "  cargo build -p glue5-gpu-d3d11 --examples"
Write-Host "  cargo test  -p glue5-gpu-d3d11"
Write-Host "  cargo run   -p glue5-gpu-d3d11 --example gm45_draw_probe --release"
Write-Host ""
Write-Host "Rollback:"
if ($null -ne $cargoBackup) { Write-Host ("  Copy-Item -LiteralPath '" + $cargoBackup + "' -Destination '" + $cargoPath + "' -Force") }
if ($null -ne $libBackup)   { Write-Host ("  Copy-Item -LiteralPath '" + $libBackup   + "' -Destination '" + $libPath   + "' -Force") }
