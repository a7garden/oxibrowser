#!/usr/bin/env bash
# §5.3a harness.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
MOCK_PORT="${MOCK_PORT:-18097}"
CDP_PORT="${CDP_PORT:-19338}"
BIN="$REPO/target/debug/oxibrowser"
echo "=== window-ael build (debug) ==="
if [ "${OXI_SKIP_BUILD:-0}" != "1" ]; then
  (cd "$REPO" && cargo build --features browser --bin oxibrowser)
else
  echo "(skipped: OXI_SKIP_BUILD=1)"
fi
[ -x "$BIN" ] || { echo "ERROR: binary missing at $BIN"; exit 1; }
MOCK_PORT=$MOCK_PORT bun "$REPO/acceptance/window-ael/mock.ts" > "$REPO/acceptance/window-ael/mock.log" 2>&1 & MOCK_PID=$!
"$BIN" serve --port "$CDP_PORT" --allow-private-ips > "$REPO/acceptance/window-ael/serve.log" 2>&1 & SERVE_PID=$!
cleanup() { kill "$MOCK_PID" "$SERVE_PID" 2>/dev/null || true; wait 2>/dev/null || true; }
trap cleanup EXIT
echo "=== wait for servers ==="
for i in $(seq 1 200); do curl -fsS "http://127.0.0.1:$MOCK_PORT/" >/dev/null 2>&1 && break; sleep 0.1; done
for i in $(seq 1 200); do curl -fsS "http://127.0.0.1:$CDP_PORT/json/version" >/dev/null 2>&1 && break; sleep 0.2; done
sleep 0.4
echo "=== window-ael probe ==="
cd "$REPO"
bun acceptance/window-ael/run.ts "$CDP_PORT" "$MOCK_PORT"
