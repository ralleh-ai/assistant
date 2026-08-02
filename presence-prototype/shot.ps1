# Dev helper: rebuild, run, and screenshot the prototype. Not part of the crate.
# Wraps capture.ps1 so a visual iteration is one command instead of four.
param(
    [string]$Out = "capture.png",
    [switch]$SkipBuild,
    [int]$Settle = 12,
    # Keystrokes to send before capturing, e.g. "l" to toggle the loading
    # entity. Needed because the states worth looking at are behind the
    # prototype's dev keys and there is no other way to reach them from a script.
    [string]$Keys = "",
    # Seconds to wait after sending keys. An entity's own clock only runs while
    # it is showing, so this — not -Settle — is what reaches a later point in a
    # toggled entity's sequence (e.g. the loading plate's next resonance).
    [int]$Hold = 3
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$dev = Join-Path (Split-Path -Parent $root) "scripts\presence-dev.cmd"

Get-Process presence-runtime -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 1

if (-not $SkipBuild) {
    & $dev build --release
    if ($LASTEXITCODE -ne 0) { Write-Error "build failed"; exit 1 }
}

$env:PRESENCE_LOG_FPS = "1"
Start-Process -FilePath $dev -ArgumentList "run", "--release" `
    -RedirectStandardOutput (Join-Path $root "run.log") `
    -RedirectStandardError (Join-Path $root "run.err.log")

Start-Sleep -Seconds $Settle

if ($Keys) {
    Add-Type -AssemblyName Microsoft.VisualBasic
    Add-Type -AssemblyName System.Windows.Forms
    $proc = Get-Process -Name "presence-runtime" -ErrorAction Stop
    [Microsoft.VisualBasic.Interaction]::AppActivate($proc.Id)
    Start-Sleep -Milliseconds 400
    [System.Windows.Forms.SendKeys]::SendWait($Keys)
    # At minimum long enough for the presence fade to finish, or the capture
    # catches a half-faded entity and reads as a dim one.
    Start-Sleep -Seconds $Hold
}

powershell -ExecutionPolicy Bypass -File (Join-Path $root "capture.ps1") -Out $Out

Get-Content (Join-Path $root "run.err.log") | Select-String "fps" | Select-Object -Last 4
