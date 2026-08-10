#!/usr/bin/env bash
# External-stylesheet acceptance harness (§5.2).
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
MOCK_PORT="${MOCK_PORT:-18096}"
CDP_PORT="${CDP_PORT:-19326}"
BIN="$REPO/target/debug/oxibrowser"
echo "=== external-stylesheet build (debug) ==="
if [ "${OXI_SKIP_BUILD:-0}" != "1" ]; then
  (cd "$REPO" && cargo build --features browser --bin oxibrowser)
else
  echo "(skipped: OXI_SKIP_BUILD=1)"
fi
[ -x "$BIN" ] || { echo "ERROR: binary missing at $BIN"; exit 1; }
MOCK_PORT=$MOCK_PORT bun "$REPO/acceptance/external-stylesheet/mock.ts" > "$REPO/acceptance/external-stylesheet/mock.log" 2>&1 & MOCK_PID=$!
"$BIN" serve --port "$CDP_PORT" --allow-private-ips > "$REPO/acceptance/external-stylesheet/serve.log" 2>&1 & SERVE_PID=$!
cleanup() { kill "$MOCK_PID" "$SERVE_PID" 2>/dev/null || true; wait 2>/dev/null || true; }
trap cleanup EXIT
echo "=== wait for servers ==="
for i in $(seq 1 200); do curl -fsS "http://127.0.0.1:$MOCK_PORT/" >/dev/null 2>&1 && break; sleep 0.1; done
for i in $(seq 1 200); do curl -fsS "http://127.0.0.1:$CDP_PORT/json/version" >/dev/null 2>&1 && break; sleep 0.2; done
sleep 0.4
echo "=== external-stylesheet probe ==="
cd "$REPO"
bun acceptance/external-stylesheet/run.ts "$CDP_PORT" "$MOCK_PORT"
