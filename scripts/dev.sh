#!/usr/bin/env bash
# Run Flask API (127.0.0.1:5001) and Vite client (localhost:5173) together. Ctrl+C stops both.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

[ -f "$ROOT/server/.env" ] || { echo "server/.env 없음: server/.env.example을 복사해서 채우세요"; exit 1; }
[ -f "$ROOT/client/.env" ] || { echo "client/.env 없음: client/.env.example을 복사해서 채우세요"; exit 1; }
[ -x "$ROOT/server/.venv/bin/python" ] || { echo "server/.venv 없음: cd server && python3 -m venv .venv && .venv/bin/pip install -r requirements-dev.txt"; exit 1; }
[ -d "$ROOT/client/node_modules" ] || (cd "$ROOT/client" && npm install)

trap 'kill 0' EXIT INT TERM
(cd "$ROOT/server" && exec .venv/bin/python wsgi.py 2>&1 | sed -u 's/^/[api] /') &
(cd "$ROOT/client" && exec npm run dev 2>&1 | sed -u 's/^/[web] /') &
wait
