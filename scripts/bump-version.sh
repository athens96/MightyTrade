#!/usr/bin/env bash
# 사용법:
#   ./scripts/bump-version.sh patch      # 0.1.0 -> 0.1.1
#   ./scripts/bump-version.sh minor      # 0.1.0 -> 0.2.0
#   ./scripts/bump-version.sh major      # 0.1.0 -> 1.0.0
#   ./scripts/bump-version.sh 0.2.0      # 지정 버전으로 설정
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

CUR="$(tr -d '[:space:]' < VERSION)"
IFS='.' read -r MAJ MIN PAT <<< "$CUR"

TARGET="${1:-}"
case "$TARGET" in
  major) NEXT="$((MAJ + 1)).0.0" ;;
  minor) NEXT="${MAJ}.$((MIN + 1)).0" ;;
  patch) NEXT="${MAJ}.${MIN}.$((PAT + 1))" ;;
  [0-9]*.[0-9]*.[0-9]*) NEXT="$TARGET" ;;
  *)
    echo "사용법: $0 <major|minor|patch|X.Y.Z> (현재: $CUR)" >&2
    exit 2
    ;;
esac

printf '%s\n' "$NEXT" > VERSION
python3 - "$NEXT" <<'PY'
import json, pathlib, re, sys
ver = sys.argv[1]
p = pathlib.Path("client/package.json")
data = json.loads(p.read_text(encoding="utf-8"))
data["version"] = ver
p.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")

cargo = pathlib.Path("server-rs/Cargo.toml")
if cargo.exists():
    txt = cargo.read_text(encoding="utf-8")
    txt = re.sub(r'(?m)^version = "[^"]+"', f'version = "{ver}"', txt, count=1)
    cargo.write_text(txt, encoding="utf-8")
PY

echo "버전 갱신 완료: $CUR -> $NEXT (VERSION, client/package.json, server-rs/Cargo.toml)"
