# Builds target\release\robopaste.exe and packs it into installer\out\robopaste-<version>.msi.
# Needs the WiX v4 dotnet tool: dotnet tool install --global wix
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$wix = Join-Path $env:USERPROFILE '.dotnet\tools\wix.exe'
if (-not (Test-Path $wix)) { throw "wix.exe not found at $wix" }

Push-Location $root
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

    $version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"(.+)"').Matches[0].Groups[1].Value
    $outDir = Join-Path $PSScriptRoot 'out'
    New-Item -ItemType Directory -Force $outDir | Out-Null
    $msi = Join-Path $outDir "robopaste-$version.msi"

    Push-Location $PSScriptRoot
    try {
        & $wix build robopaste.wxs -arch x64 -o $msi
        if ($LASTEXITCODE -ne 0) { throw 'wix build failed' }
    } finally { Pop-Location }

    Write-Output $msi
} finally { Pop-Location }
