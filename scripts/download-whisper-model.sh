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
# Supply-chain pin (M6): SHA-256 of ggml-tiny.en.bin as published in the
# Hugging Face Xet pointer for ggerganov/whisper.cpp@main. A model that
# hashes to anything else — CDN compromise, MITM despite TLS, a silently
# re-uploaded artifact — is rejected and deleted rather than fed to the
# STT engine. Update this in lockstep if the pinned model is ever moved.
EXPECTED_SHA256="921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f"

verify_sha256() {
  local file="$1" expected="$2" actual
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "${file}" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "${file}" | awk '{print $1}')"
  else
    echo "error: no sha256sum/shasum available to verify ${file}" >&2
    return 1
  fi
  if [[ "${actual}" != "${expected}" ]]; then
    echo "error: checksum mismatch for ${file}" >&2
    echo "  expected ${expected}" >&2
    echo "  actual   ${actual}" >&2
    rm -f "${file}"
    return 1
  fi
  echo "Verified SHA-256 ${actual}"
}

mkdir -p "${OUT_DIR}"
if [[ -f "${OUT_FILE}" ]]; then
  echo "Already present: ${OUT_FILE}"
  # Still verify — a stale/corrupt/tampered cached file must not pass.
  verify_sha256 "${OUT_FILE}" "${EXPECTED_SHA256}"
  exit 0
fi

echo "Downloading ${URL} ..."
# `--proto '=https' --tlsv1.2` refuses any downgrade to plaintext HTTP
# (including via a redirect), so the transport can't be silently
# stripped before the checksum is even computed.
curl -L --fail --proto '=https' --tlsv1.2 --retry 3 -o "${OUT_FILE}" "${URL}"
verify_sha256 "${OUT_FILE}" "${EXPECTED_SHA256}"
echo "Saved ${OUT_FILE}"
