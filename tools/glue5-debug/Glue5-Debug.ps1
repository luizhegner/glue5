# GLUE 5 Android diagnostic helper (Windows PowerShell 5.1+, WinForms).
# Logs only the GLUE 5 process by PID; it does not clear logcat or run bugreport.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$script:Package = 'local.glue5'
$script:AdbPath = ''
$script:DeviceSerial = ''
$script:Capture = $null
$script:CaptureDir = ''
$script:LogPath = ''
$script:Pid = ''

function Invoke-Adb([string[]] $Arguments, [switch] $AllowFailure) {
    if ([string]::IsNullOrWhiteSpace($script:AdbPath) -or !(Test-Path -LiteralPath $script:AdbPath)) {
        throw 'Selecione adb.exe (Android Platform Tools).'
    }
    $output = & $script:AdbPath @Arguments 2>&1
    $code = $LASTEXITCODE
    $text = ($output | ForEach-Object { [string]$_ }) -join [Environment]::NewLine
    if ($code -ne 0 -and !$AllowFailure) { throw "adb terminou com código $code.`r`n$text" }
    return $text.Trim()
}

function Write-Status([string] $Text) {
    $statusLabel.Text = $Text
    $form.Refresh()
}

function Refresh-Devices {
    $deviceBox.Items.Clear()
    $text = Invoke-Adb @('devices', '-l')
    foreach ($line in ($text -split "`r?`n")) {
        if ($line -match '^([^\s]+)\s+device(?:\s|$)') {
            [void]$deviceBox.Items.Add($Matches[1])
        }
    }
    if ($deviceBox.Items.Count -gt 0) {
        $deviceBox.SelectedIndex = 0
        $script:DeviceSerial = [string]$deviceBox.SelectedItem
    }
    Write-Status ("Dispositivos ADB prontos: {0}. O serial fica apenas nesta sessão e não vai para o ZIP." -f $deviceBox.Items.Count)
}

function Get-SelectedDevice {
    if ($deviceBox.SelectedItem) { $script:DeviceSerial = [string]$deviceBox.SelectedItem }
    if ([string]::IsNullOrWhiteSpace($script:DeviceSerial)) { throw 'Conecte e selecione o aparelho primeiro.' }
    return $script:DeviceSerial
}

function Get-AppPid([string] $Serial) {
    $text = Invoke-Adb -Arguments @('-s', $Serial, 'shell', 'pidof', $script:Package) -AllowFailure
    $match = [regex]::Match($text, '\d+')
    if ($match.Success) { return $match.Value }
    return ''
}

function Safe-Write([string] $Path, [string] $Text) {
    [System.IO.File]::WriteAllText($Path, $Text, [System.Text.UTF8Encoding]::new($false))
}

function Read-AndroidMetadata([string] $Serial) {
    $api = Invoke-Adb @('-s', $Serial, 'shell', 'getprop', 'ro.build.version.sdk')
    $release = Invoke-Adb @('-s', $Serial, 'shell', 'getprop', 'ro.build.version.release')
    $abi = Invoke-Adb @('-s', $Serial, 'shell', 'getprop', 'ro.product.cpu.abi')
    $pkg = Invoke-Adb @('-s', $Serial, 'shell', 'dumpsys', 'package', $script:Package)
    $versionName = 'unknown'
    $versionCode = 'unknown'
    if ($pkg -match '(?m)^\s*versionName=([^\s]+)') { $versionName = $Matches[1] }
    if ($pkg -match '(?m)^\s*versionCode=(\d+)') { $versionCode = $Matches[1] }
    return @"
App: $($script:Package)
App version: $versionName
App version code: $versionCode
Android release: $release
Android API: $api
Primary ABI: $abi
Captured process PID: $($script:Pid)
Capture started (local time): $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss zzz')

The device serial, Wi-Fi address, pairing port, pairing code, account data, and full-device bugreport are intentionally omitted.
The app log may still contain file paths or other diagnostic values; review the archive before sharing it.
"@
}

function Start-Capture {
    if ($script:Capture -and !$script:Capture.HasExited) { throw 'A captura já está em andamento.' }
    $serial = Get-SelectedDevice
    $pidText = Get-AppPid $serial
    if (!$pidText) {
        Write-Status 'Abrindo GLUE 5 no aparelho…'
        [void](Invoke-Adb @('-s', $serial, 'shell', 'monkey', '-p', $script:Package, '-c', 'android.intent.category.LAUNCHER', '1'))
        for ($attempt = 0; $attempt -lt 20 -and !$pidText; $attempt++) {
            Start-Sleep -Milliseconds 500
            $pidText = Get-AppPid $serial
        }
    }
    if (!$pidText) { throw 'Não encontrei o processo local.glue5. Abra o app e tente de novo.' }

    $root = Join-Path ([Environment]::GetFolderPath('MyDocuments')) 'GLUE5-Debug'
    [void](New-Item -ItemType Directory -Path $root -Force)
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $script:CaptureDir = Join-Path $root "session-$stamp"
    [void](New-Item -ItemType Directory -Path $script:CaptureDir -Force)
    $script:Pid = $pidText
    $script:LogPath = Join-Path $script:CaptureDir 'glue5-logcat.txt'
    $stderrPath = Join-Path $script:CaptureDir 'adb-stderr.txt'
    Safe-Write $script:LogPath ''
    Safe-Write $stderrPath ''
    Safe-Write (Join-Path $script:CaptureDir 'session-info.txt') (Read-AndroidMetadata $serial)

    $captureArgs = @('-s', $serial, 'logcat', '-v', 'threadtime', "--pid=$pidText")
    $script:Capture = Start-Process -FilePath $script:AdbPath -ArgumentList $captureArgs `
        -RedirectStandardOutput $script:LogPath -RedirectStandardError $stderrPath `
        -WindowStyle Hidden -PassThru
    $startButton.Enabled = $false
    $stopButton.Enabled = $true
    Write-Status "Capturando somente local.glue5 (PID $pidText). Agora reproduza o erro no celular e depois clique em Parar e montar ZIP."
}

function Stop-Capture {
    if (!$script:CaptureDir) { throw 'Nenhuma sessão de captura foi criada.' }
    if ($script:Capture -and !$script:Capture.HasExited) {
        $script:Capture.Kill()
        [void]$script:Capture.WaitForExit(5000)
    }
    $serial = Get-SelectedDevice
    try {
        $crash = Invoke-Adb @('-s', $serial, 'logcat', '-b', 'crash', '-d', '-v', 'threadtime', "--pid=$($script:Pid)")
        Safe-Write (Join-Path $script:CaptureDir 'glue5-crash-buffer.txt') $crash
    } catch {
        Safe-Write (Join-Path $script:CaptureDir 'glue5-crash-buffer.txt') "Não foi possível ler o buffer crash: $($_.Exception.Message)"
    }

    if ($includeGameLog.Checked) {
        $remote = '/sdcard/Android/data/local.glue5/files/game.log'
        try {
            [void](Invoke-Adb @('-s', $serial, 'pull', $remote, (Join-Path $script:CaptureDir 'game.log')))
        } catch {
            Safe-Write (Join-Path $script:CaptureDir 'game-log-not-accessible.txt') 'O Android/ADB não permitiu ler game.log. O logcat do processo GLUE 5 foi mantido.'
        }
    }

    $readme = @"
GLUE 5 Android diagnostic bundle

Contents:
- glue5-logcat.txt: logcat captured only for the local.glue5 process PID shown in session-info.txt.
- glue5-crash-buffer.txt: crash buffer filtered to the same PID when available.
- adb-stderr.txt: adb capture errors, if any.
- game.log: included only when explicitly requested and readable by ADB.

No full-device bugreport, device serial, Wi-Fi IP, pairing port, or pairing code is included.
Application logs can contain paths or other diagnostic values. Review before sharing.
"@
    Safe-Write (Join-Path $script:CaptureDir 'README.txt') $readme
    $zip = "$($script:CaptureDir).zip"
    if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [System.IO.Compression.ZipFile]::CreateFromDirectory($script:CaptureDir, $zip)
    $stopButton.Enabled = $false
    $startButton.Enabled = $true
    Write-Status "Arquivo pronto: $zip — revise o conteúdo antes de enviar."
    [System.Windows.Forms.MessageBox]::Show("Pacote criado em:`r`n$zip`r`n`r`nRevise-o antes de compartilhar.", 'GLUE 5 Debug', 'OK', 'Information') | Out-Null
}

$form = New-Object System.Windows.Forms.Form
$form.Text = 'GLUE 5 Android Debug'
$form.StartPosition = 'CenterScreen'
$form.Size = New-Object System.Drawing.Size(790, 590)
$form.MinimumSize = New-Object System.Drawing.Size(790, 590)
$form.Font = New-Object System.Drawing.Font('Segoe UI', 9)

$label = { param($text, $x, $y, $w=150) $c = New-Object System.Windows.Forms.Label; $c.Text=$text; $c.Location=New-Object System.Drawing.Point($x,$y); $c.Size=New-Object System.Drawing.Size($w,24); return $c }
$button = { param($text, $x, $y, $w=130) $c=New-Object System.Windows.Forms.Button; $c.Text=$text; $c.Location=New-Object System.Drawing.Point($x,$y); $c.Size=New-Object System.Drawing.Size($w,30); return $c }

$group = New-Object System.Windows.Forms.GroupBox
$group.Text = 'ADB por Wi-Fi (Android: Opções do desenvolvedor → Depuração sem fio)'
$group.Location = New-Object System.Drawing.Point(12,12)
$group.Size = New-Object System.Drawing.Size(750,210)
$form.Controls.Add($group)

$group.Controls.Add((& $label 'adb.exe:' 16 32 70))
$adbBox = New-Object System.Windows.Forms.TextBox
$adbBox.Location=New-Object System.Drawing.Point(88,29); $adbBox.Size=New-Object System.Drawing.Size(520,25)
$group.Controls.Add($adbBox)
$browseButton = & $button 'Procurar…' 620 27 105
$group.Controls.Add($browseButton)

$group.Controls.Add((& $label 'IP do aparelho:' 16 68 110))
$ipBox=New-Object System.Windows.Forms.TextBox; $ipBox.Location=New-Object System.Drawing.Point(128,65); $ipBox.Size=New-Object System.Drawing.Size(150,25); $group.Controls.Add($ipBox)
$group.Controls.Add((& $label 'Porta de pareamento:' 294 68 145))
$pairPortBox=New-Object System.Windows.Forms.TextBox; $pairPortBox.Location=New-Object System.Drawing.Point(440,65); $pairPortBox.Size=New-Object System.Drawing.Size(70,25); $pairPortBox.Text=''; $group.Controls.Add($pairPortBox)
$group.Controls.Add((& $label 'Código:' 523 68 55))
$codeBox=New-Object System.Windows.Forms.TextBox; $codeBox.Location=New-Object System.Drawing.Point(580,65); $codeBox.Size=New-Object System.Drawing.Size(145,25); $codeBox.UseSystemPasswordChar=$true; $group.Controls.Add($codeBox)

$group.Controls.Add((& $label 'Porta ADB:' 16 104 85))
$connectPortBox=New-Object System.Windows.Forms.TextBox; $connectPortBox.Location=New-Object System.Drawing.Point(104,101); $connectPortBox.Size=New-Object System.Drawing.Size(70,25); $group.Controls.Add($connectPortBox)
$pairButton=& $button 'Parear' 190 98 100; $group.Controls.Add($pairButton)
$connectButton=& $button 'Conectar Wi-Fi' 300 98 125; $group.Controls.Add($connectButton)
$refreshButton=& $button 'Atualizar lista' 435 98 125; $group.Controls.Add($refreshButton)
$deviceBox=New-Object System.Windows.Forms.ComboBox; $deviceBox.Location=New-Object System.Drawing.Point(16,148); $deviceBox.Size=New-Object System.Drawing.Size(709,25); $deviceBox.DropDownStyle='DropDownList'; $group.Controls.Add($deviceBox)

$captureGroup=New-Object System.Windows.Forms.GroupBox
$captureGroup.Text='Captura focada no GLUE 5'
$captureGroup.Location=New-Object System.Drawing.Point(12,234)
$captureGroup.Size=New-Object System.Drawing.Size(750,142)
$form.Controls.Add($captureGroup)
$startButton=& $button 'Iniciar captura' 18 30 150; $captureGroup.Controls.Add($startButton)
$stopButton=& $button 'Parar e montar ZIP' 180 30 170; $stopButton.Enabled=$false; $captureGroup.Controls.Add($stopButton)
$includeGameLog=New-Object System.Windows.Forms.CheckBox
$includeGameLog.Text='Tentar incluir /sdcard/Android/data/local.glue5/files/game.log (opcional)'
$includeGameLog.Location=New-Object System.Drawing.Point(18,73); $includeGameLog.Size=New-Object System.Drawing.Size(690,24); $includeGameLog.Checked=$false; $captureGroup.Controls.Add($includeGameLog)

$privacy=New-Object System.Windows.Forms.Label
$privacy.Text='Não limpa o logcat. Não coleta bugreport do aparelho. O ZIP omite serial e endereço Wi-Fi; revise logs antes de enviar.'
$privacy.Location=New-Object System.Drawing.Point(18,104); $privacy.Size=New-Object System.Drawing.Size(715,25); $captureGroup.Controls.Add($privacy)

$statusLabel=New-Object System.Windows.Forms.Label
$statusLabel.Text='Informe o caminho de adb.exe e conecte o celular. O pareamento usa código temporário e não o salva.'
$statusLabel.Location=New-Object System.Drawing.Point(18,394); $statusLabel.Size=New-Object System.Drawing.Size(735,62); $statusLabel.BorderStyle='FixedSingle'; $statusLabel.Padding=New-Object System.Windows.Forms.Padding(8); $form.Controls.Add($statusLabel)
$instructions=New-Object System.Windows.Forms.Label
$instructions.Text='Fluxo: parear (IP, porta e código exibidos no celular) → conectar usando a porta de Depuração sem fio → iniciar captura → reproduzir o erro → parar. A captura começa antes da reprodução e segue apenas o processo local.glue5.'
$instructions.Location=New-Object System.Drawing.Point(18,470); $instructions.Size=New-Object System.Drawing.Size(735,55); $instructions.AutoSize=$false; $form.Controls.Add($instructions)

$browseButton.Add_Click({
    $dialog=New-Object System.Windows.Forms.OpenFileDialog
    $dialog.Title='Selecione adb.exe (Android SDK Platform Tools)'
    $dialog.Filter='adb.exe|adb.exe|Executáveis (*.exe)|*.exe|Todos os arquivos|*.*'
    if ($dialog.ShowDialog() -eq 'OK') { $adbBox.Text=$dialog.FileName; $script:AdbPath=$dialog.FileName }
})
$pairButton.Add_Click({
    try {
        $script:AdbPath=$adbBox.Text.Trim()
        $ip=$ipBox.Text.Trim(); $port=$pairPortBox.Text.Trim(); $code=$codeBox.Text.Trim()
        if (!$ip -or !$port -or !$code) { throw 'Preencha IP, porta de pareamento e código temporário mostrados em Depuração sem fio.' }
        Write-Status 'Pareando…'
        $result=Invoke-Adb @('pair', ($ip + ':' + $port), $code)
        $codeBox.Clear()
        Write-Status "Pareamento concluído: $result. O código foi descartado da interface. Agora informe a porta de Depuração sem fio e conecte."
    } catch { [System.Windows.Forms.MessageBox]::Show($_.Exception.Message, 'Erro ADB', 'OK', 'Error') | Out-Null }
})
$connectButton.Add_Click({
    try {
        $script:AdbPath=$adbBox.Text.Trim()
        $ip=$ipBox.Text.Trim(); $port=$connectPortBox.Text.Trim()
        if (!$ip -or !$port) { throw 'Preencha IP e porta de Depuração sem fio.' }
        Write-Status 'Conectando por Wi-Fi…'
        $result=Invoke-Adb @('connect', ($ip + ':' + $port))
        Refresh-Devices
        Write-Status "ADB: $result. O endereço não será incluído no pacote."
    } catch { [System.Windows.Forms.MessageBox]::Show($_.Exception.Message, 'Erro ADB', 'OK', 'Error') | Out-Null }
})
$refreshButton.Add_Click({
    try { $script:AdbPath=$adbBox.Text.Trim(); Refresh-Devices }
    catch { [System.Windows.Forms.MessageBox]::Show($_.Exception.Message, 'Erro ADB', 'OK', 'Error') | Out-Null }
})
$deviceBox.Add_SelectedIndexChanged({ if ($deviceBox.SelectedItem) { $script:DeviceSerial=[string]$deviceBox.SelectedItem } })
$startButton.Add_Click({ try { $script:AdbPath=$adbBox.Text.Trim(); Start-Capture } catch { [System.Windows.Forms.MessageBox]::Show($_.Exception.Message, 'Erro na captura', 'OK', 'Error') | Out-Null } })
$stopButton.Add_Click({ try { Stop-Capture } catch { [System.Windows.Forms.MessageBox]::Show($_.Exception.Message, 'Erro ao montar o pacote', 'OK', 'Error') | Out-Null } })
$form.Add_FormClosing({
    if ($script:Capture -and !$script:Capture.HasExited) {
        $answer=[System.Windows.Forms.MessageBox]::Show('A captura ainda está ativa. Encerrar sem montar o ZIP?', 'GLUE 5 Debug', 'YesNo', 'Warning')
        if ($answer -ne 'Yes') { $_.Cancel=$true; return }
        $script:Capture.Kill()
    }
})

$known = Get-Command adb.exe -ErrorAction SilentlyContinue
if ($known) { $adbBox.Text=$known.Source; $script:AdbPath=$known.Source }
elseif ($env:LOCALAPPDATA) {
    $sdkAdb=Join-Path $env:LOCALAPPDATA 'Android\Sdk\platform-tools\adb.exe'
    if (Test-Path -LiteralPath $sdkAdb) { $adbBox.Text=$sdkAdb; $script:AdbPath=$sdkAdb }
}
[void]$form.ShowDialog()
