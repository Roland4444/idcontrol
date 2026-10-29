#!/usr/bin/env bash
# ID-Control v0.9.26: one-time local model setup for Ubuntu + systemd.
# First install requires Internet to fetch the Ollama runtime and weights.
# Subsequent checks run wholly on loopback without external model calls.
set -Eeuo pipefail
MODEL="${BLOCK1_LOCAL_MODEL:-qwen3:4b-instruct}"
if (( EUID != 0 )); then
  echo 'Запустите: sudo bash scripts/setup-local-semantic.sh' >&2
  exit 2
fi
if [[ ! "$MODEL" =~ ^[a-zA-Z0-9_.:/-]+$ ]]; then
  echo 'Недопустимое имя BLOCK1_LOCAL_MODEL' >&2
  exit 2
fi
if ! command -v systemctl >/dev/null || ! command -v curl >/dev/null; then
  echo 'Требуется Ubuntu с systemd и curl (sudo apt install -y curl)' >&2
  exit 2
fi
mem_kib="$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)"
if [[ "$MODEL" == 'qwen3:4b-instruct' && "${mem_kib:-0}" -lt 6291456 ]]; then
  echo 'Модель qwen3:4b-instruct рассчитана примерно на 8 ГБ RAM или больше с учётом приложения и OCR.' >&2
  echo 'Недостаточно оперативной памяти. Установите 8+ ГБ RAM или задайте BLOCK1_LOCAL_MODEL=qwen3:1.7b.' >&2
  exit 3
fi
avail_kib="$(df -Pk /var/lib 2>/dev/null | awk 'NR==2 {print $4}')"
if [[ "${avail_kib:-0}" -lt 5242880 ]]; then
  echo 'На файловой системе /var/lib требуется минимум ~5 ГБ свободного места.' >&2
  exit 3
fi
if ! command -v ollama >/dev/null; then
  echo 'Устанавливаю локальный движок Ollama по официальному install.sh...'
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT
  curl -fL --retry 3 --connect-timeout 15 --max-time 180 -o "$tmpdir/ollama-install.sh" https://ollama.com/install.sh
  sh "$tmpdir/ollama-install.sh"
fi
if ! systemctl cat ollama.service >/dev/null 2>&1; then
  echo 'Служба ollama.service отсутствует. Проверьте установку Ollama.' >&2
  exit 4
fi
# Localhost-only: does not expose an unauthenticated inference API on LAN.
install -d -m 0755 /etc/systemd/system/ollama.service.d
cat > /etc/systemd/system/ollama.service.d/10-idcontrol-local.conf <<'EOF'
[Service]
Environment="OLLAMA_HOST=127.0.0.1:11434"
Environment="OLLAMA_NUM_PARALLEL=1"
Environment="OLLAMA_MAX_LOADED_MODELS=1"
Environment="OLLAMA_CONTEXT_LENGTH=8192"
Environment="OLLAMA_KEEP_ALIVE=5m"
EOF
systemctl daemon-reload
systemctl enable --now ollama.service
systemctl restart ollama.service
for _ in $(seq 1 40); do
  if curl -fsS --connect-timeout 2 --max-time 3 http://127.0.0.1:11434/api/tags >/dev/null; then break; fi
  sleep 1
done
if ! curl -fsS --max-time 3 http://127.0.0.1:11434/api/tags >/dev/null; then
  echo 'Ollama не запустился. См. sudo journalctl -u ollama -n 100 --no-pager' >&2
  exit 4
fi
# Ollama uses only the localhost service; download once and reuse cached weights.
echo "Загружаю локальную модель: $MODEL"
OLLAMA_HOST=127.0.0.1:11434 ollama pull "$MODEL"
OLLAMA_HOST=127.0.0.1:11434 ollama show "$MODEL" >/dev/null
# Verify host is not listening on an external interface: loopback override above.
echo
printf 'Готово: Ollama работает только на 127.0.0.1:11434, модель %s установлена.\n' "$MODEL"
echo 'Следующее действие: cd ~/rust/idcontrol && ./target/release/id-control-axum --check-semantic-model'
echo 'После этого перезапустите сервис id-control и откройте КОСМО → Земляные работы → Семантическая проверка.'
