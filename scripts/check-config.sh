#!/usr/bin/env bash
set -euo pipefail
# Run after exporting .env. Never prints credentials.
for program in psql tesseract pdftotext pdfinfo pdftoppm; do
  if command -v "$program" >/dev/null 2>&1; then echo "OK $program"; else echo "MISSING $program"; fi
done
[[ -n "${DATABASE_URL:-}" ]] || { echo 'DATABASE_URL missing'; exit 1; }
[[ -n "${SYNOLOGY_URL:-}" ]] || { echo 'SYNOLOGY_URL missing'; exit 1; }
[[ -n "${SYNOLOGY_ROOT:-}" ]] || { echo 'SYNOLOGY_ROOT missing'; exit 1; }
psql "$DATABASE_URL" -Atqc 'SELECT version();' >/dev/null
echo 'PostgreSQL connection OK'
