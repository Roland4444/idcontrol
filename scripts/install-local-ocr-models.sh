#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p models/ocr
BASE=https://huggingface.co/monkt/paddleocr-onnx/resolve/main
get() {
  local filename="$1" url="$2" min="$3" hash="$4" target="models/ocr/$1"
  if [[ -f "$target" ]] && [[ $(wc -c < "$target") -ge "$min" ]]; then
    if [[ -z "$hash" ]] || echo "$hash  $target" | sha256sum --status -c -; then echo "OK: $filename"; return; fi
  fi
  echo "Download: $filename"
  curl --fail --location --retry 3 --connect-timeout 15 --output "$target.partial" "$url"
  [[ $(wc -c < "$target.partial") -ge "$min" ]] || { echo "Too small: $filename" >&2; exit 1; }
  if [[ -n "$hash" ]]; then echo "$hash  $target.partial" | sha256sum --status -c - || { echo "SHA mismatch: $filename" >&2; exit 1; }; fi
  mv -f "$target.partial" "$target"
}
get det.onnx "$BASE/detection/v5/det.onnx" 80000000 61824840edf6e74581898930b8091b1b2318f4b2705a2e8a40ad3de7ac480133
get rec.onnx "$BASE/languages/eslav/rec.onnx" 7000000 dc6bf0e855247decce214ba6dae5bc135fa0ad725a5918a7fcfb59fad6c9cdee
get dict.txt "$BASE/languages/eslav/dict.txt" 1000 ''
echo 'Done. cargo run -- --check-local-models'
