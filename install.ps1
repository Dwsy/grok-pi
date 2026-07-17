# One-line install (Windows x64 PowerShell):
#   irm https://github.com/Dwsy/grok-pi/releases/latest/download/install.ps1 | iex
# Pin a version:
#   $env:GROK_PI_VERSION='v0.0.1'; irm https://github.com/Dwsy/grok-pi/releases/download/v0.0.1/install.ps1 | iex
# Optional env:
#   $env:GROK_PI_VERSION = 'v0.0.1'
#   $env:GROK_PI_INSTALL_DIR = "$env:LOCALAPPDATA\grok-pi\bin"
$ErrorActionPreference = 'Stop'

$repository = 'Dwsy/grok-pi'
$version = if ($env:GROK_PI_VERSION) { $env:GROK_PI_VERSION } else { 'latest' }
$installDir = if ($env:GROK_PI_INSTALL_DIR) {
    $env:GROK_PI_INSTALL_DIR
} else {
    Join-Path $env:LOCALAPPDATA 'grok-pi\bin'
}

$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
if ($arch -ne [System.Runtime.InteropServices.Architecture]::X64) {
    throw "Only Windows x64 is released (detected: $arch)."
}

$asset = 'grok-pi-windows-x86_64.zip'
if ($version -eq 'latest') {
    $url = "https://github.com/$repository/releases/latest/download/$asset"
} elseif ($version -match '^v') {
    $url = "https://github.com/$repository/releases/download/$version/$asset"
} else {
    throw "GROK_PI_VERSION must be 'latest' or a v-prefixed release tag (e.g. v0.0.1)."
}

New-Item -ItemType Directory -Force -Path $installDir | Out-Null
$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("grok-pi-install-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null

try {
    $archive = Join-Path $tempRoot $asset
    Write-Host "Downloading $asset ($version)..."
    Invoke-WebRequest -Uri $url -OutFile $archive
    Expand-Archive -Path $archive -DestinationPath $tempRoot -Force

    $binary = Join-Path $tempRoot 'grok-pi.exe'
    if (-not (Test-Path -LiteralPath $binary)) {
        throw 'archive did not contain grok-pi.exe'
    }

    $target = Join-Path $installDir 'grok-pi.exe'
    Copy-Item -LiteralPath $binary -Destination $target -Force
} finally {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ([string]::IsNullOrEmpty($userPath)) {
    [Environment]::SetEnvironmentVariable('Path', $installDir, 'User')
} elseif ($userPath -notlike "*$installDir*") {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$installDir", 'User')
}
if ($env:Path -notlike "*$installDir*") {
    $env:Path = "$env:Path;$installDir"
}

Write-Host ""
Write-Host "Installed $installDir\grok-pi.exe"
Write-Host 'Open a new terminal if this is the first install so PATH updates apply.'
Write-Host 'Install Pi with: npm install --global @earendil-works/pi-coding-agent'
Write-Host 'Run with: grok-pi --pi-bin pi --pi-cwd C:\path\to\project -- --no-session'
