<#
.SYNOPSIS
Builds the release DLL and copies it into an ME3 mod folder.

.DESCRIPTION
Only the DLL is replaced. SekiroCameraFixes.ini and SekiroCameraFixes.log in the
target folder are left alone. The game must be closed, because Windows locks a
loaded DLL.

.PARAMETER Destination
Folder to copy the DLL into. Defaults to the folder ME3 Manager made for this mod
in the "sekiro-default" profile.
#>
param(
    [string]$Destination = "$env:LOCALAPPDATA\garyttierney\me3\config\profiles\sekiro-mods\Sekiro Camera Fixes"
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

cargo build --release --locked
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

New-Item -ItemType Directory $Destination -Force | Out-Null
try {
    Copy-Item target\release\sekiro_camera_fixes.dll $Destination -Force
} catch [System.IO.IOException] {
    throw "Could not replace the DLL in $Destination. Is Sekiro still running?"
}
Write-Host "Installed to $Destination"
