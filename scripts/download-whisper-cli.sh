#!/usr/bin/env bash
# Download whisper.cpp Linux x64 CLI + JFK sample for WhisperCliStt e2e.
# Usage:
#   ./scripts/download-whisper-cli.sh
#   ./scripts/download-whisper-model.sh
#   export WHISPER_CLI_PATH=... WHISPER_MODEL_PATH=...
#   cargo test -p ralleh-audio-core -- --ignored whisper_cli_e2e
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${ROOT}/tools/whisper"
URL="https://github.com/ggml-org/whisper.cpp/releases/download/v1.8.7/whisper-bin-ubuntu-x64.tar.gz"
SAMPLE_URL="https://github.com/ggml-org/whisper.cpp/raw/master/samples/jfk.wav"
SAMPLE_OUT="${ROOT}/models/jfk.wav"

mkdir -p "${OUT_DIR}" "${ROOT}/models"

CLI="$(find "${OUT_DIR}" -type f -name 'whisper-cli' 2>/dev/null | head -n 1 || true)"
if [[ -z "${CLI}" ]]; then
  ARCHIVE="$(mktemp /tmp/whisper-bin-XXXXXX.tar.gz)"
  echo "Downloading ${URL} ..."
  curl -L --fail --retry 3 -o "${ARCHIVE}" "${URL}"
  tar -xzf "${ARCHIVE}" -C "${OUT_DIR}"
  rm -f "${ARCHIVE}"
  CLI="$(find "${OUT_DIR}" -type f -name 'whisper-cli' | head -n 1)"
fi
if [[ -z "${CLI}" || ! -x "${CLI}" ]]; then
  # Some archives ship without +x
  CLI="$(find "${OUT_DIR}" -type f -name 'whisper-cli' | head -n 1)"
  chmod +x "${CLI}"
fi
if [[ -z "${CLI}" ]]; then
  echo "whisper-cli not found after extract" >&2
  exit 1
fi

if [[ ! -f "${SAMPLE_OUT}" ]]; then
  echo "Downloading jfk.wav sample ..."
  curl -L --fail --retry 3 -o "${SAMPLE_OUT}" "${SAMPLE_URL}"
fi

echo "WHISPER_CLI_PATH=${CLI}"
echo "Sample WAV=${SAMPLE_OUT}"
