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

# Supply-chain hardening (M6). Piper's release assets don't publish a
# stable SHA-256 next to the tarball, so instead of pinning a value that
# could silently rot we (a) force HTTPS-only transport so no redirect can
# strip TLS, and (b) print the computed digest of the executable archive
# and enforce it when `EXPECTED_PIPER_SHA256` is set — which is how an
# enterprise/CI runner pins the binary it actually vetted.
report_and_verify_sha256() {
  local file="$1" expected="${2:-}" actual
  actual="$(sha256sum "${file}" | awk '{print $1}')"
  echo "SHA-256(${file##*/}) = ${actual}"
  if [[ -n "${expected}" && "${actual}" != "${expected}" ]]; then
    echo "error: checksum mismatch for ${file}" >&2
    echo "  expected ${expected}" >&2
    echo "  actual   ${actual}" >&2
    rm -f "${file}"
    return 1
  fi
}

EXE="$(find "${OUT_DIR}" -type f -name 'piper' 2>/dev/null | head -n 1 || true)"
if [[ -z "${EXE}" ]]; then
  ARCHIVE="$(mktemp /tmp/piper-XXXXXX.tar.gz)"
  echo "Downloading ${URL} ..."
  curl -L --fail --proto '=https' --tlsv1.2 --retry 3 -o "${ARCHIVE}" "${URL}"
  report_and_verify_sha256 "${ARCHIVE}" "${EXPECTED_PIPER_SHA256:-}"
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
  curl -L --fail --proto '=https' --tlsv1.2 --retry 3 -o "${ONNX}" "${VOICE_BASE}/en_US-lessac-low.onnx"
  report_and_verify_sha256 "${ONNX}" "${EXPECTED_PIPER_VOICE_SHA256:-}"
fi
if [[ ! -f "${ONNX_JSON}" ]]; then
  curl -L --fail --proto '=https' --tlsv1.2 --retry 3 -o "${ONNX_JSON}" "${VOICE_BASE}/en_US-lessac-low.onnx.json"
fi

echo "PIPER_CLI_PATH=${EXE}"
echo "PIPER_MODEL_PATH=${ONNX}"
