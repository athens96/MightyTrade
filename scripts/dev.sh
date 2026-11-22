#!/usr/bin/env bash
# Run Rust API (127.0.0.1:5001) and Vite client (localhost:5173) together. Ctrl+C stops both.
# Pass USE_PYTHON_API=1 to run the legacy Python Flask API instead.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

[ -f "$ROOT/server/.env" ] || { echo "server/.env 없음: server/.env.example을 복사해서 채우세요"; exit 1; }
[ -f "$ROOT/client/.env" ] || { echo "client/.env 없음: client/.env.example을 복사해서 채우세요"; exit 1; }
[ -d "$ROOT/client/node_modules" ] || (cd "$ROOT/client" && npm install)

trap 'kill 0' EXIT INT TERM
if [ "${USE_PYTHON_API:-0}" = "1" ]; then
  [ -x "$ROOT/server/.venv/bin/python" ] || { echo "server/.venv 없음: cd server && python3 -m venv .venv && .venv/bin/pip install -r requirements-dev.txt"; exit 1; }
  (cd "$ROOT/server" && exec .venv/bin/python wsgi.py 2>&1 | sed -u 's/^/[api-py] /') &
else
  (cd "$ROOT/server-rs" && cargo build --release >/dev/null)
  (cd "$ROOT/server" && exec "$ROOT/server-rs/target/release/mighty-trade-server" 2>&1 | sed -u 's/^/[api-rs] /') &
fi
(cd "$ROOT/client" && exec npm run dev 2>&1 | sed -u 's/^/[web] /') &
wait
