# CloudreveLiteTray full build: app + shell DLL + cert + MSIX + setup exe.
# Usage:  pwsh -NoProfile -File build.ps1 [-FreshCert]
param([switch]$FreshCert)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$bin = Join-Path $root 'target\release'
$certDir = Join-Path $root 'build\cert'
$sparse = Join-Path $root 'sparse'
$payload = Join-Path $root 'installer\payload'
$dist = Join-Path $root 'dist'

function Step($m) { Write-Host "==> $m" -ForegroundColor Cyan }

function Find-WindowsSdkBin {
    $sdkRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    $versions = Get-ChildItem -LiteralPath $sdkRoot -Directory -ErrorAction Stop |
        Where-Object Name -Match '^\d+\.\d+\.\d+\.\d+$' |
        Sort-Object { [version]$_.Name } -Descending

    foreach ($version in $versions) {
        $candidate = Join-Path $version.FullName 'x64'
        if ((Test-Path -LiteralPath (Join-Path $candidate 'signtool.exe')) -and
            (Test-Path -LiteralPath (Join-Path $candidate 'makeappx.exe'))) {
            return $candidate
        }
    }

    throw 'A Windows 10/11 SDK containing signtool.exe and makeappx.exe is required.'
}

function Export-PublicCertificate($PfxPath, $Password, $CerPath) {
    $securePassword = ConvertTo-SecureString -String $Password -Force -AsPlainText
    $pfxData = Get-PfxData -FilePath $PfxPath -Password $securePassword
    $certificate = @($pfxData.EndEntityCertificates |
        Where-Object Subject -EQ 'CN=CloudreveLiteTray' |
        Select-Object -First 1)
    if ($certificate.Count -ne 1) {
        throw 'Signing PFX does not contain the CloudreveLiteTray leaf certificate.'
    }
    Export-Certificate -Cert $certificate[0] -FilePath $CerPath -Force | Out-Null
}

$SDKBIN = Find-WindowsSdkBin

# 1. Rust artifacts ------------------------------------------------------------
Step 'building app + shell (cargo release)'
cargo build --release --locked -p cloudreve-tray -p cloudreve-shell
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

# 2. Code signing certificate --------------------------------------------------
New-Item -ItemType Directory -Force -Path $certDir | Out-Null
$pfx = Join-Path $certDir 'CloudreveLiteTray.pfx'
$cer = Join-Path $certDir 'CloudreveLiteTray.cer'
$pwFile = Join-Path $certDir 'pfxpassword.txt'
$secretPfx = $env:CLOUDREVE_TRAY_SIGNING_PFX_BASE64
$secretPassword = $env:CLOUDREVE_TRAY_SIGNING_PASSWORD
$hasSecretPfx = -not [string]::IsNullOrWhiteSpace($secretPfx)
$hasSecretPassword = -not [string]::IsNullOrWhiteSpace($secretPassword)
if ($hasSecretPfx -xor $hasSecretPassword) {
    throw 'Both CLOUDREVE_TRAY_SIGNING_PFX_BASE64 and CLOUDREVE_TRAY_SIGNING_PASSWORD are required.'
}

if ($hasSecretPfx) {
    if ($FreshCert) {
        throw '-FreshCert cannot be combined with a signing certificate from the environment.'
    }
    Step 'loading release signing certificate from environment'
    [IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($secretPfx.Trim()))
    Set-Content -LiteralPath $pwFile -Value $secretPassword -Encoding ascii -NoNewline
    Export-PublicCertificate $pfx $secretPassword $cer
} elseif ($FreshCert -or -not (Test-Path -LiteralPath $pfx)) {
    Step 'creating self-signed code signing certificate (CN=CloudreveLiteTray)'
    if (-not (Test-Path -LiteralPath $pwFile)) {
        $pw = [Convert]::ToBase64String((1..24 | ForEach-Object { Get-Random -Max 256 }))
        Set-Content -LiteralPath $pwFile -Value $pw -Encoding ascii -NoNewline
    }
    $pw = Get-Content -LiteralPath $pwFile -Raw
    $sec = ConvertTo-SecureString -String $pw -Force -AsPlainText
    $cert = New-SelfSignedCertificate -Type Custom -Subject 'CN=CloudreveLiteTray' `
        -KeyUsage DigitalSignature -FriendlyName 'CloudreveLiteTray Build Signing' `
        -CertStoreLocation 'Cert:\CurrentUser\My' -KeyExportPolicy Exportable `
        -NotAfter (Get-Date).AddYears(5) `
        -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
    Export-PfxCertificate -Cert $cert -FilePath $pfx -Password $sec | Out-Null
    Export-Certificate -Cert $cert -FilePath $cer | Out-Null
    Write-Host "    thumbprint: $($cert.Thumbprint)"
} elseif (-not (Test-Path -LiteralPath $cer)) {
    Export-PublicCertificate $pfx (Get-Content -LiteralPath $pwFile -Raw) $cer
}

# 3. Sign the binaries ----------------------------------------------------------
$signingPassword = Get-Content -LiteralPath $pwFile -Raw
Step 'signing exe + dll'
& "$SDKBIN\signtool.exe" sign /fd SHA256 /f $pfx /p $signingPassword "$bin\CloudreveTray.exe"
if ($LASTEXITCODE -ne 0) { throw 'signtool exe failed' }
& "$SDKBIN\signtool.exe" sign /fd SHA256 /f $pfx /p $signingPassword "$bin\cloudreve_shell.dll"
if ($LASTEXITCODE -ne 0) { throw 'signtool dll failed' }

# 4. MSIX sparse package --------------------------------------------------------
Step 'packing sparse MSIX'
$msix = Join-Path $bin 'CloudreveShell.msix'
New-Item -ItemType Directory -Force -Path (Join-Path $sparse 'Assets') | Out-Null
Copy-Item (Join-Path $root 'assets\Square150x150Logo.png') (Join-Path $sparse 'Assets\') -Force
Copy-Item (Join-Path $root 'assets\Square44x44Logo.png') (Join-Path $sparse 'Assets\') -Force
& "$SDKBIN\makeappx.exe" pack /d $sparse /p $msix /o /nv
if ($LASTEXITCODE -ne 0) { throw 'makeappx failed' }
Step 'signing MSIX'
& "$SDKBIN\signtool.exe" sign /fd SHA256 /f $pfx /p $signingPassword $msix
if ($LASTEXITCODE -ne 0) { throw 'signtool msix failed' }

# 5. Installer payload ----------------------------------------------------------
Step 'assembling installer payload'
New-Item -ItemType Directory -Force -Path $payload | Out-Null
Copy-Item "$bin\CloudreveTray.exe" $payload -Force
Copy-Item "$bin\cloudreve_shell.dll" $payload -Force
Copy-Item $msix $payload -Force
Copy-Item $cer "$payload\CloudreveLiteTray.cer" -Force

# 6. Installer -------------------------------------------------------------------
Step 'building setup exe'
cargo build --release --locked -p crl-setup
if ($LASTEXITCODE -ne 0) { throw 'cargo build installer failed' }
New-Item -ItemType Directory -Force -Path $dist | Out-Null
$setup = Join-Path $dist 'CloudreveLiteTray-Setup.exe'
Copy-Item "$bin\CloudreveLiteTray-Setup.exe" $setup -Force
& "$SDKBIN\signtool.exe" sign /fd SHA256 /f $pfx /p $signingPassword $setup
if ($LASTEXITCODE -ne 0) { throw 'signtool setup failed' }

Write-Host ''
Write-Host 'Build complete:' -ForegroundColor Green
Get-Item $setup, "$payload\CloudreveTray.exe", "$payload\cloudreve_shell.dll", "$payload\CloudreveShell.msix" |
    Select-Object Name, @{n='SizeKB';e={[math]::Round($_.Length/1KB,1)}} | Format-Table -AutoSize
Write-Host "Installer: $setup"
