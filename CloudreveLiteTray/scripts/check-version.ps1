param(
    [Parameter(Mandatory)]
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$ExpectedVersion
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $root 'Cargo.toml'

$metadataJson = & cargo metadata --manifest-path $manifest --locked --no-deps --format-version 1
if ($LASTEXITCODE -ne 0) {
    throw 'cargo metadata failed.'
}

$metadata = $metadataJson | ConvertFrom-Json
$versions = @($metadata.packages.version | Sort-Object -Unique)
if ($versions.Count -ne 1 -or $versions[0] -ne $ExpectedVersion) {
    throw "Cargo package versions do not all match ${ExpectedVersion}: $($versions -join ', ')"
}

$windowsVersion = "$ExpectedVersion.0"
$expectations = @(
    @{ Path = 'app/app.manifest'; Text = "version=`"$windowsVersion`"" }
    @{ Path = 'installer/setup.manifest'; Text = "version=`"$windowsVersion`"" }
    @{ Path = 'sparse/AppxManifest.xml'; Text = "Version=`"$windowsVersion`"" }
    @{ Path = 'app/app.rc'; Text = "VALUE `"ProductVersion`", `"$ExpectedVersion`"" }
    @{ Path = 'app/app.rc'; Text = "VALUE `"FileVersion`", `"$ExpectedVersion`"" }
    @{ Path = 'installer/setup.rc'; Text = "VALUE `"ProductVersion`", `"$ExpectedVersion`"" }
    @{ Path = 'installer/setup.rc'; Text = "VALUE `"FileVersion`", `"$ExpectedVersion`"" }
    @{ Path = 'installer/src/main.rs'; Text = 'reg_set(k, "DisplayVersion", env!("CARGO_PKG_VERSION"));' }
)

foreach ($expectation in $expectations) {
    $path = Join-Path $root $expectation.Path
    $content = Get-Content -LiteralPath $path -Raw
    if (-not $content.Contains($expectation.Text)) {
        throw "$($expectation.Path) does not contain expected version text: $($expectation.Text)"
    }
}

Write-Host "CloudreveLiteTray version metadata is aligned at $ExpectedVersion."
