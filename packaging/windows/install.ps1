# Install jg for the current user on Windows (no admin rights needed).
#
#   powershell -ExecutionPolicy Bypass -File install.ps1
#   powershell -ExecutionPolicy Bypass -File install.ps1 -UpdateSource https://myserver.example/jg
#
# Copies jg.exe to %LOCALAPPDATA%\Programs\jg and adds that folder to your user PATH.
# Later updates: `jg self-update` (reads update.source).
param(
    [string]$UpdateSource = ""
)
$ErrorActionPreference = "Stop"
$dest = Join-Path $env:LOCALAPPDATA "Programs\jg"
New-Item -ItemType Directory -Force -Path $dest | Out-Null
Copy-Item -Force (Join-Path $PSScriptRoot "jg.exe") (Join-Path $dest "jg.exe")

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not ($userPath -split ";" | Where-Object { $_ -eq $dest })) {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$dest", "User")
    Write-Host "Added $dest to your PATH (open a new terminal to use it)."
}
if ($UpdateSource -ne "") {
    & (Join-Path $dest "jg.exe") config --global update.source $UpdateSource | Out-Null
    Write-Host "update.source set to $UpdateSource"
}
& (Join-Path $dest "jg.exe") version
Write-Host ""
Write-Host "Next steps:"
Write-Host '  jg config --global user.name  "Your Name"'
Write-Host '  jg config --global user.email "you@example.com"'
Write-Host '  # optional shortcuts (gs, ga, ...): add this line to your $PROFILE'
Write-Host '  jg shell-init powershell | Out-String | Invoke-Expression'
