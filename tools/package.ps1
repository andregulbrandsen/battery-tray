# Packages a built exe into target\dist: <name>.zip (exe, README, LICENSE) and SHA256SUMS.txt.
# Used by the GitHub Actions build and usable locally:
#   cargo build --release; pwsh -File tools\package.ps1 -Version 0.1.0
param(
    [Parameter(Mandatory)][string]$Version,
    [string]$Exe = 'target\release\battery-tray.exe',
    [string]$OutDir = 'target\dist'
)
$ErrorActionPreference = 'Stop'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    $name = "battery-tray-$Version-windows-x64"
    if (Test-Path $OutDir) { Remove-Item $OutDir -Recurse -Force }
    $stage = Join-Path $OutDir $name
    New-Item -ItemType Directory -Force $stage | Out-Null
    Copy-Item $Exe, 'README.md', 'LICENSE' $stage

    $zip = Join-Path $OutDir "$name.zip"
    Compress-Archive -Path $stage -DestinationPath $zip
    $zipHash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    $exeHash = (Get-FileHash (Join-Path $stage 'battery-tray.exe') -Algorithm SHA256).Hash.ToLower()
    # Standard "hash  path" lines, so `sha256sum -c SHA256SUMS.txt` works after extracting.
    [IO.File]::WriteAllText((Join-Path $OutDir 'SHA256SUMS.txt'), "$zipHash  $name.zip`n$exeHash  $name/battery-tray.exe`n")
    Remove-Item $stage -Recurse -Force

    Get-ChildItem $OutDir -File | ForEach-Object { '{0}  {1} bytes' -f $_.Name, $_.Length }
}
finally {
    Pop-Location
}
