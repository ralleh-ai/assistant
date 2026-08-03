# Phase 0 manual capture: overlay corner rain, replace crossfade, idle after dismiss.
param(
    [string]$OutDir = "docs\screenshots"
)
$ErrorActionPreference = "Stop"
$Repo = Split-Path $PSScriptRoot -Parent
$Proto = Join-Path $Repo "presence-prototype"
$Bin = Join-Path $Proto "target\debug\presence-runtime.exe"
if (-not (Test-Path $Bin)) {
    Write-Error "Build first: scripts\presence-dev.cmd build -p presence-runtime"
}
$ShotDir = Join-Path $Repo $OutDir
New-Item -ItemType Directory -Force -Path $ShotDir | Out-Null
$Capture = Join-Path $Proto "capture.ps1"

$env:PRESENCE_STDIN_IPC = "1"
$env:PRESENCE_STDOUT_IPC = "1"

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $Bin
$psi.UseShellExecute = $false
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.WorkingDirectory = $Proto
$p = [System.Diagnostics.Process]::Start($psi)

function Send-Cmd($json) {
    $p.StandardInput.WriteLine($json)
    $p.StandardInput.Flush()
}

$overlay = '{"version":2,"payload":{"kind":"present_scene","id":"precipitation","params":{"density":0.85,"wind":0.12},"disposition":"overlay","placement":{"anchor":"bottom_right","offset":[0,0],"scale":0.42},"ttl_ms":120000}}'
$replace = '{"version":2,"payload":{"kind":"present_scene","id":"precipitation","params":{"density":0.9,"wind":0},"disposition":"replace","placement":{"anchor":"center","offset":[0,0],"scale":0.65},"ttl_ms":120000}}'
$dismiss = '{"version":2,"payload":{"kind":"dismiss_scene","id":"precipitation"}}'

Send-Cmd $overlay
Start-Sleep -Seconds 4
Push-Location $Proto
& $Capture -Out (Join-Path $ShotDir "p0-overlay-rain-corner.png")

Send-Cmd $dismiss
Start-Sleep -Seconds 2
Send-Cmd $replace
Start-Sleep -Seconds 4
& $Capture -Out (Join-Path $ShotDir "p0-replace-rain.png")

Send-Cmd $dismiss
Start-Sleep -Seconds 3
& $Capture -Out (Join-Path $ShotDir "p0-idle-after-dismiss.png")
Pop-Location

$p.Kill()
Write-Host "Saved screenshots to $ShotDir"
