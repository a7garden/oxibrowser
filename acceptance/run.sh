#!/usr/bin/env bash
# OxiBrowser acceptance harness orchestrator.
# Builds the binary, starts the mock SPA + CDP server, drives the flow.
#
#   bash acceptance/run.sh          # full (builds if missing)
#   OXI_SKIP_BUILD=1 bash acceptance/run.sh   # reuse existing binary
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
MOCK_PORT="${MOCK_PORT:-18080}"
CDP_PORT="${CDP_PORT:-19321}"
BIN="$REPO/target/debug/oxibrowser"

echo "=== build (debug, --features browser) ==="
if [ "${OXI_SKIP_BUILD:-0}" != "1" ]; then
  (cd "$REPO" && cargo build --features browser --bin oxibrowser)
else
  echo "(skipped: OXI_SKIP_BUILD=1)"
fi
[ -x "$BIN" ] || { echo "ERROR: binary missing at $BIN"; exit 1; }

MOCK_PORT=$MOCK_PORT bun "$REPO/acceptance/server.ts" > "$REPO/acceptance/server.log" 2>&1 & MOCK_PID=$!
"$BIN" serve --port "$CDP_PORT" --allow-private-ips > "$REPO/acceptance/serve.log" 2>&1 & SERVE_PID=$!
cleanup() { kill "$MOCK_PID" "$SERVE_PID" 2>/dev/null || true; wait 2>/dev/null || true; }
trap cleanup EXIT

echo "=== wait for servers ==="
# Mock readiness (200 × 0.1s = 20s ceiling).
for i in $(seq 1 200); do
  curl -fsS "http://127.0.0.1:$MOCK_PORT/" >/dev/null 2>&1 && break; sleep 0.1
done
# CDP readiness (200 × 0.2s = 40s ceiling — binary cold start can be slow).
for i in $(seq 1 200); do
  curl -fsS "http://127.0.0.1:$CDP_PORT/json/version" >/dev/null 2>&1 && break; sleep 0.2
done
sleep 0.4

echo "=== serve.log (head) ==="; head -20 "$REPO/acceptance/serve.log" || true
echo "=== HARNESS ==="
bun "$REPO/acceptance/harness.ts" "$CDP_PORT" "$MOCK_PORT"
