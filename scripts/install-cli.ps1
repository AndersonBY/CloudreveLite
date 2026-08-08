param(
    [Parameter(Mandatory = $true)]
    [string]$Server,

    [Parameter(Mandatory = $true)]
    [string]$PairingCode,

    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'CloudreveLite\bin')
)

$ErrorActionPreference = 'Stop'

$architecture = switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture) {
    'X64' { 'amd64' }
    'Arm64' { 'arm64' }
    default { throw "Unsupported Windows architecture: $_" }
}

$asset = "cloudreve-cli_windows_$architecture.zip"
$releaseBase = 'https://github.com/AndersonBY/CloudreveLite/releases/latest/download'
$tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("cloudreve-cli-" + [System.Guid]::NewGuid().ToString('N'))

try {
    New-Item -ItemType Directory -Path $tempDir -Force | Out-Null
    $archive = Join-Path $tempDir $asset
    $checksums = Join-Path $tempDir 'SHA256SUMS'
    Invoke-WebRequest -UseBasicParsing -Uri "$releaseBase/$asset" -OutFile $archive
    Invoke-WebRequest -UseBasicParsing -Uri "$releaseBase/SHA256SUMS" -OutFile $checksums

    $escapedAsset = [Regex]::Escape($asset)
    $checksumLine = Select-String -LiteralPath $checksums -Pattern "^([a-fA-F0-9]{64})\s+\*?$escapedAsset$" | Select-Object -First 1
    if ($null -eq $checksumLine) {
        throw "No checksum was published for $asset"
    }
    $expected = $checksumLine.Matches[0].Groups[1].Value.ToLowerInvariant()
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $archive).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "Checksum verification failed for $asset"
    }

    $expanded = Join-Path $tempDir 'expanded'
    Expand-Archive -LiteralPath $archive -DestinationPath $expanded -Force
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $executable = Join-Path $InstallDir 'cloudreve-cli.exe'
    Copy-Item -LiteralPath (Join-Path $expanded 'cloudreve-cli.exe') -Destination $executable -Force

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $pathEntries = @($userPath -split ';' | Where-Object { $_ })
    if ($pathEntries -notcontains $InstallDir) {
        $updatedPath = (@($pathEntries) + $InstallDir) -join ';'
        [Environment]::SetEnvironmentVariable('Path', $updatedPath, 'User')
    }
    if (($env:Path -split ';') -notcontains $InstallDir) {
        $env:Path = "$InstallDir;$env:Path"
    }

    $connectArgs = @('connect', '--server', $Server, '--pairing-code', $PairingCode)
    & $executable @connectArgs
    if ($LASTEXITCODE -ne 0) {
        throw "cloudreve-cli connect failed with exit code $LASTEXITCODE"
    }
}
finally {
    if (Test-Path -LiteralPath $tempDir) {
        Remove-Item -LiteralPath $tempDir -Recurse -Force
    }
}
