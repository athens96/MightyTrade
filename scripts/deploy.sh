#!/usr/bin/env bash
# Update the Oracle VM to origin/main: server code + deps, systemd unit, client build.
#
# Settings come from deploy/.env.deploy (git-ignored), e.g.
#   DEPLOY_HOST=ubuntu@1.2.3.4
#   DEPLOY_KEY=~/.ssh/oci-free
#   DEPLOY_URL=https://example.duckdns.org
# First-time server setup is described in README (배포).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[ -f "$ROOT/deploy/.env.deploy" ] && . "$ROOT/deploy/.env.deploy"
: "${DEPLOY_HOST:?DEPLOY_HOST is not set (deploy/.env.deploy)}"
DEPLOY_KEY="${DEPLOY_KEY:-$HOME/.ssh/oci-free}"
DEPLOY_KEY="${DEPLOY_KEY/#\~/$HOME}"
APP_DIR=/opt/mighty-trade
SSH=(ssh -i "$DEPLOY_KEY" -o BatchMode=yes "$DEPLOY_HOST")

cd "$ROOT"
git fetch -q origin main
if [ "$(git rev-parse HEAD)" != "$(git rev-parse origin/main)" ] || [ -n "$(git status --porcelain)" ]; then
  echo "! 로컬이 origin/main과 다릅니다 (미커밋 또는 미푸시). 서버는 origin/main을, 클라이언트는 로컬 빌드를 올립니다." >&2
  read -r -p "계속할까요? [y/N] " ok; [ "$ok" = y ] || exit 1
fi

echo "== client build"
(cd client && npm run build >/dev/null)

echo "== server update"
"${SSH[@]}" "set -e
  cd $APP_DIR && git pull -q --ff-only && git log --oneline -1
  server/.venv/bin/pip install -q -r server/requirements.txt
  if ! cmp -s deploy/mighty-trade-api.service /etc/systemd/system/mighty-trade-api.service; then
    sudo cp deploy/mighty-trade-api.service /etc/systemd/system/ && sudo systemctl daemon-reload
  fi
  sudo systemctl restart mighty-trade-api && sleep 2 && systemctl is-active mighty-trade-api"

echo "== client upload"
rsync -az --delete -e "ssh -i $DEPLOY_KEY -o BatchMode=yes" client/dist/ "$DEPLOY_HOST:$APP_DIR/client/dist/"

if [ -n "${DEPLOY_URL:-}" ]; then
  code=$(curl -s -o /dev/null -w '%{http_code}' "$DEPLOY_URL/api/auth/me")
  echo "== $DEPLOY_URL/api/auth/me -> $code (401 = 정상, 로그인 전)"
fi
