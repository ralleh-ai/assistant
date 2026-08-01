# Download Piper Windows CLI + a small English ONNX voice for PiperCliTts e2e.
# Usage:
#   ./scripts/download-piper.ps1
#   $env:PIPER_CLI_PATH = (Resolve-Path .\tools\piper\piper\piper.exe)
#   $env:PIPER_MODEL_PATH = (Resolve-Path .\models\en_US-lessac-low.onnx)
#   cargo test -p ralleh-audio-core -- --ignored piper_cli_e2e

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$OutDir = Join-Path $Root "tools\piper"
$Models = Join-Path $Root "models"
$CliZipUrl = "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_windows_amd64.zip"
$VoiceBase = "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/lessac/low"
$Onnx = Join-Path $Models "en_US-lessac-low.onnx"
$OnnxJson = Join-Path $Models "en_US-lessac-low.onnx.json"

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
New-Item -ItemType Directory -Force -Path $Models | Out-Null

$Exe = Get-ChildItem -Path $OutDir -Recurse -Filter "piper.exe" -ErrorAction SilentlyContinue |
    Select-Object -First 1
if (-not $Exe) {
    $Zip = Join-Path $env:TEMP "piper_windows_amd64.zip"
    Write-Host "Downloading $CliZipUrl ..."
    Invoke-WebRequest -Uri $CliZipUrl -OutFile $Zip
    Expand-Archive -Path $Zip -DestinationPath $OutDir -Force
    $Exe = Get-ChildItem -Path $OutDir -Recurse -Filter "piper.exe" |
        Select-Object -First 1
}
if (-not $Exe) {
    throw "piper.exe not found after extract"
}

if (-not (Test-Path $Onnx)) {
    Write-Host "Downloading voice model ..."
    Invoke-WebRequest -Uri "$VoiceBase/en_US-lessac-low.onnx" -OutFile $Onnx
}
if (-not (Test-Path $OnnxJson)) {
    Invoke-WebRequest -Uri "$VoiceBase/en_US-lessac-low.onnx.json" -OutFile $OnnxJson
}

Write-Host "PIPER_CLI_PATH=$($Exe.FullName)"
Write-Host "PIPER_MODEL_PATH=$Onnx"
