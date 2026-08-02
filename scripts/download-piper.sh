#!/usr/bin/env bash
# Download Piper Linux x86_64 CLI + small English ONNX voice for PiperCliTts e2e.
# Usage:
#   ./scripts/download-piper.sh
#   export PIPER_CLI_PATH=... PIPER_MODEL_PATH=...
#   cargo test -p ralleh-audio-core -- --ignored piper_cli_e2e
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${ROOT}/tools/piper"
MODELS="${ROOT}/models"
URL="https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_linux_x86_64.tar.gz"
VOICE_BASE="https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/lessac/low"
ONNX="${MODELS}/en_US-lessac-low.onnx"
ONNX_JSON="${MODELS}/en_US-lessac-low.onnx.json"

mkdir -p "${OUT_DIR}" "${MODELS}"

EXE="$(find "${OUT_DIR}" -type f -name 'piper' 2>/dev/null | head -n 1 || true)"
if [[ -z "${EXE}" ]]; then
  ARCHIVE="$(mktemp /tmp/piper-XXXXXX.tar.gz)"
  echo "Downloading ${URL} ..."
  curl -L --fail --retry 3 -o "${ARCHIVE}" "${URL}"
  tar -xzf "${ARCHIVE}" -C "${OUT_DIR}"
  rm -f "${ARCHIVE}"
  EXE="$(find "${OUT_DIR}" -type f -name 'piper' | head -n 1)"
fi
if [[ -z "${EXE}" ]]; then
  echo "piper binary not found after extract" >&2
  exit 1
fi
chmod +x "${EXE}"

if [[ ! -f "${ONNX}" ]]; then
  echo "Downloading voice model ..."
  curl -L --fail --retry 3 -o "${ONNX}" "${VOICE_BASE}/en_US-lessac-low.onnx"
fi
if [[ ! -f "${ONNX_JSON}" ]]; then
  curl -L --fail --retry 3 -o "${ONNX_JSON}" "${VOICE_BASE}/en_US-lessac-low.onnx.json"
fi

echo "PIPER_CLI_PATH=${EXE}"
echo "PIPER_MODEL_PATH=${ONNX}"
