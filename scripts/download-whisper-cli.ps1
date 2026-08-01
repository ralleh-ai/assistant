# Download whisper.cpp Windows x64 CLI for WhisperCliStt e2e (no bindgen).
# Usage:
#   ./scripts/download-whisper-cli.ps1
#   ./scripts/download-whisper-model.ps1
#   $env:WHISPER_CLI_PATH = (Resolve-Path .\tools\whisper\Release\whisper-cli.exe)
#   $env:WHISPER_MODEL_PATH = (Resolve-Path .\models\ggml-tiny.en.bin)
#   cargo test -p ralleh-audio-core -- --ignored whisper_cli_e2e

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$OutDir = Join-Path $Root "tools\whisper"
$Marker = Join-Path $OutDir "Release\whisper-cli.exe"
$Url = "https://github.com/ggml-org/whisper.cpp/releases/download/v1.8.7/whisper-bin-x64.zip"
$SampleUrl = "https://github.com/ggml-org/whisper.cpp/raw/master/samples/jfk.wav"
$SampleOut = Join-Path $Root "models\jfk.wav"

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $Root "models") | Out-Null

if (-not (Test-Path $Marker)) {
    $Zip = Join-Path $env:TEMP "whisper-bin-x64.zip"
    Write-Host "Downloading $Url ..."
    Invoke-WebRequest -Uri $Url -OutFile $Zip
    Expand-Archive -Path $Zip -DestinationPath $OutDir -Force
} else {
    Write-Host "Already present: $Marker"
}

if (-not (Test-Path $SampleOut)) {
    Write-Host "Downloading jfk.wav sample ..."
    Invoke-WebRequest -Uri $SampleUrl -OutFile $SampleOut
} else {
    Write-Host "Already present: $SampleOut"
}

Write-Host "WHISPER_CLI_PATH=$Marker"
Write-Host "Sample WAV=$SampleOut"
