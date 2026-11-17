$ErrorActionPreference = 'Stop'

# This test writes installer registrations on a disposable Windows Actions host.
if ($env:GITHUB_ACTIONS -ne 'true') {
    throw 'Run this installer smoke test only on a disposable GitHub Actions runner.'
}

$root = Split-Path -Parent $PSScriptRoot
$setup = Join-Path $root 'dist\CloudreveLiteTray-Setup.exe'
$payload = Join-Path $root 'installer\payload'
$sandbox = Join-Path $env:RUNNER_TEMP ('cloudreve-tray-update-' + [guid]::NewGuid().ToString('N'))
$originalLocal = $env:LOCALAPPDATA
$originalRoaming = $env:APPDATA
$parent = $null
$installer = $null
$names = @('CloudreveTray.exe', 'cloudreve_shell.dll', 'CloudreveShell.msix', 'CloudreveLiteTray.cer', 'CloudreveLiteTray-Setup.exe')

try {
    $env:LOCALAPPDATA = Join-Path $sandbox 'local'
    $env:APPDATA = Join-Path $sandbox 'roaming'
    $installDir = Join-Path $env:LOCALAPPDATA 'CloudreveLiteTray'
    $configDir = Join-Path $env:APPDATA 'CloudreveLiteTray'
    New-Item -ItemType Directory -Path $installDir, $configDir -Force | Out-Null

    foreach ($name in $names) {
        Set-Content -LiteralPath (Join-Path $installDir $name) -Value 'previous-installation-fixture' -Encoding ascii
    }
    $config = Join-Path $configDir 'config.json'
    @{
        server = 'https://example.invalid'
        api_key_enc = 'smoke-test-placeholder'
        expire_days = 14
        downloads = 20
        private = $true
        auto_update = $false
    } | ConvertTo-Json | Set-Content -LiteralPath $config -Encoding utf8
    $configHash = (Get-FileHash -LiteralPath $config).Hash

    $certificate = Join-Path $payload 'CloudreveLiteTray.cer'
    Import-Certificate -FilePath $certificate -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
    Import-Certificate -FilePath $certificate -CertStoreLocation Cert:\LocalMachine\TrustedPublisher | Out-Null
    $runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
    Remove-ItemProperty -LiteralPath $runKey -Name CloudreveLiteTray -ErrorAction SilentlyContinue

    $parentInfo = [Diagnostics.ProcessStartInfo]::new()
    $parentInfo.FileName = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $parentInfo.UseShellExecute = $false
    $parentInfo.CreateNoWindow = $true
    foreach ($argument in @('-NoProfile', '-NonInteractive', '-Command', 'Start-Sleep -Seconds 2')) {
        $parentInfo.ArgumentList.Add($argument)
    }
    $parent = [Diagnostics.Process]::Start($parentInfo)
    $installer = Start-Process -FilePath $setup -ArgumentList @('--update', [string]$parent.Id) -WindowStyle Hidden -PassThru
    if (-not $installer.WaitForExit(90000)) { throw 'Unattended update did not finish within 90 seconds.' }
    if ($installer.ExitCode -ne 0) { throw "Unattended update failed: $($installer.ExitCode)" }

    foreach ($name in $names) {
        $source = if ($name -eq 'CloudreveLiteTray-Setup.exe') { $setup } else { Join-Path $payload $name }
        if ((Get-FileHash -LiteralPath (Join-Path $installDir $name)).Hash -ne (Get-FileHash -LiteralPath $source).Hash) {
            throw "Installed payload differs: $name"
        }
    }
    if ((Get-FileHash -LiteralPath $config).Hash -ne $configHash) { throw 'Update changed user configuration.' }
    if (Get-ItemProperty -LiteralPath $runKey -Name CloudreveLiteTray -ErrorAction SilentlyContinue) {
        throw 'Update unexpectedly enabled autostart.'
    }
    $backup = @(Get-ChildItem -LiteralPath (Join-Path $installDir 'backups') -Directory -Filter 'update-*')
    if ($backup.Count -ne 1) { throw 'Expected one complete pre-update backup.' }
    foreach ($name in $names) {
        if ((Get-Content -LiteralPath (Join-Path $backup[0].FullName $name) -Raw).Trim() -ne 'previous-installation-fixture') {
            throw "Backup does not contain the previous file: $name"
        }
    }
    $running = @(Get-CimInstance Win32_Process -Filter "Name='CloudreveTray.exe'" | Where-Object ExecutablePath -EQ (Join-Path $installDir 'CloudreveTray.exe'))
    if ($running.Count -ne 1) { throw 'Updated tray process is not running.' }
    Write-Host 'Unattended update passed: payload, restart, backup, configuration and autostart.'
}
finally {
    if ($installer -and -not $installer.HasExited) { Stop-Process -Id $installer.Id -ErrorAction SilentlyContinue }
    if ($parent -and -not $parent.HasExited) { Stop-Process -Id $parent.Id -ErrorAction SilentlyContinue }
    if (Test-Path -LiteralPath (Join-Path $sandbox 'local\CloudreveLiteTray')) {
        Get-CimInstance Win32_Process -Filter "Name='CloudreveTray.exe'" |
            Where-Object ExecutablePath -EQ (Join-Path $sandbox 'local\CloudreveLiteTray\CloudreveTray.exe') |
            ForEach-Object { Stop-Process -Id $_.ProcessId -ErrorAction SilentlyContinue }
    }
    $env:LOCALAPPDATA = $originalLocal
    $env:APPDATA = $originalRoaming
}
