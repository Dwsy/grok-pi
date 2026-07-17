$ErrorActionPreference = 'Stop'

$repository = 'Dwsy/pi-grok-build'
$version = if ($env:GROK_PI_VERSION) { $env:GROK_PI_VERSION } else { 'latest' }
$installDir = if ($env:GROK_PI_INSTALL_DIR) { $env:GROK_PI_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'grok-pi\bin' }

if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne [System.Runtime.InteropServices.Architecture]::X64) {
    throw 'Only Windows x64 is released.'
}

if ($version -eq 'latest') {
    $url = "https://github.com/$repository/releases/latest/download/grok-pi-windows-x86_64.zip"
} elseif ($version -match '^v') {
    $url = "https://github.com/$repository/releases/download/$version/grok-pi-windows-x86_64.zip"
} else {
    throw "GROK_PI_VERSION must be 'latest' or a v-prefixed release tag."
}

New-Item -ItemType Directory -Force -Path $installDir | Out-Null
$archive = Join-Path $env:TEMP 'grok-pi-windows-x86_64.zip'
Invoke-WebRequest -Uri $url -OutFile $archive
Expand-Archive -Path $archive -DestinationPath $installDir -Force

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -notlike "*$installDir*") {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$installDir", 'User')
}
$env:Path += ";$installDir"

Write-Host "Installed $installDir\grok-pi.exe"
Write-Host 'Install Pi with: npm install --global @earendil-works/pi-coding-agent'
Write-Host 'Run with: grok-pi --pi-bin pi --pi-cwd C:\path\to\project -- --no-session'
