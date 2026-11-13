[CmdletBinding()]
param(
    [string]$OutputDirectory = ''
)

$ErrorActionPreference = 'Stop'

$toolRoot = Split-Path -Parent $PSScriptRoot
$repoRoot = (Resolve-Path (Join-Path $toolRoot '..\..')).Path
$manifest = Join-Path $toolRoot 'Cargo.toml'
$fixture = Join-Path $toolRoot 'tests\fixtures\storage_read_vs.wgsl'

if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    if ($env:RUNNER_TEMP) {
        $OutputDirectory = Join-Path $env:RUNNER_TEMP 'gm45-shader-audit'
    } else {
        $OutputDirectory = Join-Path $repoRoot '.artifacts\gm45-shader-audit'
    }
}
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null

Write-Output '=== Toolchain ==='
rustc --version
cargo --version

Write-Output '=== Build Naga HLSL probe ==='
cargo build --locked --release --manifest-path $manifest
if ($LASTEXITCODE -ne 0) {
    throw "Naga probe build failed with exit code $LASTEXITCODE"
}

$probeExe = Join-Path $toolRoot 'target\release\naga-hlsl-probe.exe'
if (-not (Test-Path -LiteralPath $probeExe -PathType Leaf)) {
    throw "Expected probe executable was not created: $probeExe"
}

$hlsl = Join-Path $OutputDirectory 'storage_read_vs.hlsl'
$dxbc = Join-Path $OutputDirectory 'storage_read_vs.vs_4_0.cso'
$assembly = Join-Path $OutputDirectory 'storage_read_vs.vs_4_0.asm'
$fxcLog = Join-Path $OutputDirectory 'storage_read_vs.fxc.log'
$reportPath = Join-Path $OutputDirectory 'report.json'
$summaryPath = Join-Path $OutputDirectory 'report.md'

Write-Output '=== WGSL -> HLSL ==='
& $probeExe $fixture 'vs' $hlsl
if ($LASTEXITCODE -ne 0) {
    throw "WGSL to HLSL conversion failed with exit code $LASTEXITCODE"
}

Write-Output '=== Locate FXC in the installed Windows SDK ==='
$programFilesX86 = [Environment]::GetFolderPath('ProgramFilesX86')
$kitsRoot = Join-Path $programFilesX86 'Windows Kits\10\bin'
if (-not (Test-Path -LiteralPath $kitsRoot -PathType Container)) {
    throw "Windows 10 SDK bin directory not found: $kitsRoot"
}

$fxc = Get-ChildItem -LiteralPath $kitsRoot -Filter 'fxc.exe' -File -Recurse -ErrorAction SilentlyContinue |
    Where-Object { $_.Directory.Name -eq 'x64' -and $_.Directory.Parent.Name -match '^10\.0\.' } |
    Sort-Object { [version]$_.Directory.Parent.Name } -Descending |
    Select-Object -First 1

if (-not $fxc) {
    throw "fxc.exe for x64 was not found under $kitsRoot"
}
Write-Output "FXC: $($fxc.FullName)"

Write-Output '=== Compile as vs_4_0 (optimizations disabled so resource loads remain visible) ==='
$fxcArgs = @('/nologo', '/Ges', '/Od', '/T', 'vs_4_0', '/E', 'main', '/Fo', $dxbc, $hlsl)
$fxcOutput = & $fxc.FullName @fxcArgs 2>&1
$fxcExit = $LASTEXITCODE
$fxcOutput | Out-String | Set-Content -LiteralPath $fxcLog -Encoding UTF8
if ($fxcExit -ne 0) {
    $fxcOutput | ForEach-Object { Write-Output $_ }
    throw "FXC vs_4_0 compilation failed with exit code $fxcExit"
}
if (-not (Test-Path -LiteralPath $dxbc -PathType Leaf)) {
    throw 'FXC reported success but did not create the DXBC file.'
}

Write-Output '=== Disassemble DXBC and inspect resource registers ==='
$dumpOutput = & $fxc.FullName '/dumpbin' '/Fc' $assembly $dxbc 2>&1
$dumpExit = $LASTEXITCODE
if ($dumpOutput) { $dumpOutput | ForEach-Object { Write-Output $_ } }
if ($dumpExit -ne 0 -or -not (Test-Path -LiteralPath $assembly -PathType Leaf)) {
    throw "FXC DXBC disassembly failed (exit $dumpExit)"
}

$asmText = [System.IO.File]::ReadAllText($assembly).Replace("`r", '')
$hasT0 = $asmText -match '(?m)^dcl_resource_raw t0$'
$hasT1 = $asmText -match '(?m)^dcl_resource_raw t1$'
$hasT32 = $asmText -match '(?m)^dcl_resource_raw t32$'
$hasT33 = $asmText -match '(?m)^dcl_resource_raw t33$'
$rawLoadCount = ([regex]::Matches($asmText, '(?m)^ld_raw\s')).Count

if ($hasT32 -and $hasT33) {
    $mappingStatus = 'known-mismatch'
    $mappingMessage = 'The current probe emits logical storage bindings 32/33 as native t32/t33; the GLUE5 convention expects t0/t1.'
    Write-Output "::warning::$mappingMessage"
} elseif ($hasT0 -and $hasT1) {
    $mappingStatus = 'logical-slots'
    $mappingMessage = 'The DXBC uses the expected native SRV slots t0/t1.'
} else {
    $mappingStatus = 'unexpected'
    $mappingMessage = "Could not identify the expected SRV pair. t0=$hasT0 t1=$hasT1 t32=$hasT32 t33=$hasT33"
    Write-Output "::warning::$mappingMessage"
}

$hlslHash = (Get-FileHash -LiteralPath $hlsl -Algorithm SHA256).Hash
$dxbcHash = (Get-FileHash -LiteralPath $dxbc -Algorithm SHA256).Hash
$asmHash = (Get-FileHash -LiteralPath $assembly -Algorithm SHA256).Hash
$report = [ordered]@{
    fixture = 'storage_read_vs.wgsl'
    tool = 'Naga HLSL probe'
    nagaVersion = '30.0.1'
    shaderProfile = 'vs_4_0'
    fxc = $fxc.FullName
    fxcExitCode = $fxcExit
    mappingStatus = $mappingStatus
    mappingMessage = $mappingMessage
    hasRawSrvT0 = $hasT0
    hasRawSrvT1 = $hasT1
    hasRawSrvT32 = $hasT32
    hasRawSrvT33 = $hasT33
    rawLoadInstructionCount = $rawLoadCount
    hlslSha256 = $hlslHash
    dxbcSha256 = $dxbcHash
    assemblySha256 = $asmHash
}
$report | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $reportPath -Encoding UTF8

$summary = @"
# GM45 shader compiler audit

- WGSL fixture: $fixture
- Naga: 30.0.1
- FXC profile: vs_4_0
- FXC: $($fxc.FullName)
- FXC exit code: $fxcExit
- Resource mapping status: $mappingStatus
- Result: $mappingMessage
- Raw load instructions in DXBC: $rawLoadCount

| Artifact | SHA-256 |
|---|---|
| HLSL | $hlslHash |
| DXBC | $dxbcHash |
| FXC assembly | $asmHash |

This validates parsing, HLSL generation, FXC compilation and DXBC resource-slot inspection.
It does not validate D3D11 device creation, GM45 driver acceptance, resource binding,
Draw execution or readback. Those remain hardware tests on the Inspiron 1545.
"@
$summary | Set-Content -LiteralPath $summaryPath -Encoding UTF8

if ($env:GITHUB_STEP_SUMMARY) {
    Get-Content -LiteralPath $summaryPath | Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY
}

Write-Output '=== RESULT ==='
Get-Content -LiteralPath $summaryPath
Write-Output "Artifacts: $OutputDirectory"
