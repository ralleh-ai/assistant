# Download a small ggml Whisper model for optional whisper e2e.
# Usage (PowerShell):
#   ./scripts/download-whisper-model.ps1
#   ./scripts/download-whisper-cli.ps1   # Windows-friendly CLI path
# Then either:
#   $env:WHISPER_CLI_PATH / WHISPER_MODEL_PATH → whisper_cli_e2e (no cargo feature)
#   or --features whisper + WHISPER_MODEL_PATH → whisper_rs_e2e (Linux/bindgen)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$OutDir = Join-Path $Root "models"
$OutFile = Join-Path $OutDir "ggml-tiny.en.bin"
$Url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin"

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
if (Test-Path $OutFile) {
    Write-Host "Already present: $OutFile"
    exit 0
}

Write-Host "Downloading $Url ..."
Invoke-WebRequest -Uri $Url -OutFile $OutFile
Write-Host "Saved $OutFile"
Write-Host "Prefer WhisperCliStt e2e on Windows (see download-whisper-cli.ps1)."
