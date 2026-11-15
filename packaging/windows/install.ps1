# Install jg for the current user on Windows (no admin rights needed).
#
#   powershell -ExecutionPolicy Bypass -File install.ps1
#   powershell -ExecutionPolicy Bypass -File install.ps1 -UpdateSource https://myserver.example/jg
#
# Running it again over an existing install updates jg.exe in place (settings are kept).
#
# Copies jg.exe to %LOCALAPPDATA%\Programs\jg and adds that folder to your user PATH.
# Later updates: `jg self-update` (reads update.source).
param(
    [string]$UpdateSource = ""
)
$ErrorActionPreference = "Stop"
$dest = Join-Path $env:LOCALAPPDATA "Programs\jg"
$exe = Join-Path $dest "jg.exe"
New-Item -ItemType Directory -Force -Path $dest | Out-Null
$previous = $null
if (Test-Path $exe) {
    $previous = (& $exe version) 2>$null
}
Copy-Item -Force (Join-Path $PSScriptRoot "jg.exe") $exe

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not ($userPath -split ";" | Where-Object { $_ -eq $dest })) {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$dest", "User")
    Write-Host "Added $dest to your PATH (open a new terminal to use it)."
}
if ($UpdateSource -ne "") {
    & $exe config --global update.source $UpdateSource | Out-Null
    Write-Host "update.source set to $UpdateSource"
}
if ($previous) {
    Write-Host "Updated: $previous -> $((& $exe version) 2>$null)"
    Write-Host "Your settings (%APPDATA%\jg\config) and repositories are unchanged."
    exit 0
}
& $exe version
Write-Host ""
Write-Host "Next steps:"
Write-Host '  jg config --global user.name  "Your Name"'
Write-Host '  jg config --global user.email "you@example.com"'
Write-Host '  # optional shortcuts (gs, ga, ...): add this line to your $PROFILE'
Write-Host '  jg shell-init powershell | Out-String | Invoke-Expression'
