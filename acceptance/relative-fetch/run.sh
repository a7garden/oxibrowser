#!/usr/bin/env bash
# §5.3b harness.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
MOCK_PORT="${MOCK_PORT:-18098}"
CDP_PORT="${CDP_PORT:-19339}"
BIN="$REPO/target/debug/oxibrowser"
echo "=== relative-fetch build (debug) ==="
if [ "${OXI_SKIP_BUILD:-0}" != "1" ]; then
  (cd "$REPO" && cargo build --features browser --bin oxibrowser)
else
  echo "(skipped: OXI_SKIP_BUILD=1)"
fi
[ -x "$BIN" ] || { echo "ERROR: binary missing at $BIN"; exit 1; }
MOCK_PORT=$MOCK_PORT bun "$REPO/acceptance/relative-fetch/mock.ts" > "$REPO/acceptance/relative-fetch/mock.log" 2>&1 & MOCK_PID=$!
"$BIN" serve --port "$CDP_PORT" --allow-private-ips > "$REPO/acceptance/relative-fetch/serve.log" 2>&1 & SERVE_PID=$!
cleanup() { kill "$MOCK_PID" "$SERVE_PID" 2>/dev/null || true; wait 2>/dev/null || true; }
trap cleanup EXIT
echo "=== wait for servers ==="
for i in $(seq 1 200); do curl -fsS "http://127.0.0.1:$MOCK_PORT/" >/dev/null 2>&1 && break; sleep 0.1; done
for i in $(seq 1 200); do curl -fsS "http://127.0.0.1:$CDP_PORT/json/version" >/dev/null 2>&1 && break; sleep 0.2; done
sleep 0.4
echo "=== relative-fetch probe ==="
cd "$REPO"
bun acceptance/relative-fetch/run.ts "$CDP_PORT" "$MOCK_PORT"
