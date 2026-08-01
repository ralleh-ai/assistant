# Download a small ggml Whisper model for optional --features whisper e2e.
# Usage (PowerShell):
#   ./scripts/download-whisper-model.ps1
# Then:
#   $env:WHISPER_MODEL_PATH = (Resolve-Path .\models\ggml-tiny.en.bin)
#   cargo test -p ralleh-audio-core --features whisper -- --ignored whisper_e2e

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
Write-Host "Set WHISPER_MODEL_PATH to that path and run the ignored whisper_e2e test with --features whisper."
