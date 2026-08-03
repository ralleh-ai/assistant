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
# Supply-chain pin (M6): SHA-256 of ggml-tiny.en.bin as published in the
# Hugging Face Xet pointer for ggerganov/whisper.cpp@main. Keep in lockstep
# with the value in download-whisper-model.sh.
$ExpectedSha256 = "921E4CF8686FDD993DCD081A5DA5B6C365BFDE1162E72B08D75AC75289920B1F"

function Confirm-Sha256($File, $Expected) {
    $actual = (Get-FileHash -Path $File -Algorithm SHA256).Hash
    if ($actual -ne $Expected) {
        Remove-Item -Force $File
        throw "Checksum mismatch for $File`n  expected $Expected`n  actual   $actual"
    }
    Write-Host "Verified SHA-256 $actual"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
if (Test-Path $OutFile) {
    Write-Host "Already present: $OutFile"
    # Still verify — a stale/corrupt/tampered cached file must not pass.
    Confirm-Sha256 $OutFile $ExpectedSha256
    exit 0
}

Write-Host "Downloading $Url ..."
Invoke-WebRequest -Uri $Url -OutFile $OutFile
Confirm-Sha256 $OutFile $ExpectedSha256
Write-Host "Saved $OutFile"
Write-Host "Prefer WhisperCliStt e2e on Windows (see download-whisper-cli.ps1)."
