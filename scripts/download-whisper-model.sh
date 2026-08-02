#!/usr/bin/env bash
# Download a small ggml Whisper model for optional e2e.
# Usage:
#   ./scripts/download-whisper-model.sh
#   export WHISPER_MODEL_PATH="$(pwd)/models/ggml-tiny.en.bin"
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${ROOT}/models"
OUT_FILE="${OUT_DIR}/ggml-tiny.en.bin"
URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin"

mkdir -p "${OUT_DIR}"
if [[ -f "${OUT_FILE}" ]]; then
  echo "Already present: ${OUT_FILE}"
  exit 0
fi

echo "Downloading ${URL} ..."
curl -L --fail --retry 3 -o "${OUT_FILE}" "${URL}"
echo "Saved ${OUT_FILE}"
