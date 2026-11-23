<#
.SYNOPSIS
Builds the release DLL and puts everything a player needs into one folder, plus a zip of it.

.DESCRIPTION
Creates, relative to the repository root:
  dist\SekiroCameraFixes\                 sekiro_camera_fixes.dll, SekiroCameraFixes.me3, README.md
  dist\SekiroCameraFixes-v<version>.zip   that folder, zipped, ready to upload to a release
The version comes from Cargo.toml. Any previous dist folder is replaced.
Works in Windows PowerShell 5.1 and PowerShell 7.
#>
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

cargo build --release --locked
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

$version = (cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages[0].version
$folder = 'dist\SekiroCameraFixes'
$zip = "dist\SekiroCameraFixes-v$version.zip"

if (Test-Path dist) { Remove-Item dist -Recurse -Force }
New-Item -ItemType Directory $folder | Out-Null
Copy-Item target\release\sekiro_camera_fixes.dll, SekiroCameraFixes.me3, README.md $folder
if (Test-Path LICENSE) { Copy-Item LICENSE $folder }
Compress-Archive -Path $folder -DestinationPath $zip

Write-Host "Folder: $folder"
Write-Host "Zip:    $zip"
