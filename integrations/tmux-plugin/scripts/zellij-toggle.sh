#!/usr/bin/env bash
# Toggle opensessions sidebar in Zellij via the server.
# Ensures the server is running first, then calls POST /toggle.
#
# Designed to be called from a zellij keybinding. Add to ~/.config/zellij/config.kdl:
#
#   bind "s" {
#     Run "bash" "${OPENSESSIONS_DIR}/integrations/tmux-plugin/scripts/zellij-toggle.sh" {
#       close_on_exit true
#     };
#     SwitchToMode "Normal";
#   }

set -euo pipefail

PORT="${OPENSESSIONS_PORT:-7391}"
HOST="${OPENSESSIONS_HOST:-127.0.0.1}"
# Zellij never sets $TMUX, so SERVER_KEY is empty and the server uses
# the unkeyed defaults. Honor explicit overrides for parity with
# server-common.sh.
TOKEN_FILE="${OPENSESSIONS_TOKEN_FILE:-/tmp/opensessions.token}"

OPENSESSIONS_DIR="${OPENSESSIONS_DIR:-$(cd "$(dirname "$0")/../../.." && pwd)}"
BUN_PATH="${BUN_PATH:-$(command -v bun 2>/dev/null || echo "$HOME/.bun/bin/bun")}"
SERVER_ENTRY="$OPENSESSIONS_DIR/apps/server/src/main.ts"

# --- Ensure server is running ---
server_alive() {
    curl -s -o /dev/null -m 0.2 "http://${HOST}:${PORT}/" 2>/dev/null
}

if ! server_alive; then
    "$BUN_PATH" run "$SERVER_ENTRY" &>/dev/null &
    disown
    for i in $(seq 1 30); do
        sleep 0.1
        server_alive && break
    done
fi

# --- Build context: |session|tabId ---
SESSION_NAME="${ZELLIJ_SESSION_NAME:-}"
# Get active tab ID from JSON (works from inside zellij)
TAB_ID="0"
TAB_JSON=$(zellij action list-tabs --json 2>/dev/null || echo "")
if [ -n "$TAB_JSON" ]; then
    TAB_ID=$(echo "$TAB_JSON" | python3 -c "
import json,sys
try:
    tabs=json.load(sys.stdin)
    active=[t for t in tabs if t.get('active')]
    print(active[0]['tab_id'] if active else tabs[0]['tab_id'] if tabs else '0')
except: print('0')" 2>/dev/null || echo "0")
fi

CTX="|${SESSION_NAME}|${TAB_ID}"
TOKEN=""
if [ -f "$TOKEN_FILE" ]; then
    TOKEN="$(cat "$TOKEN_FILE" 2>/dev/null || true)"
fi
if [ -n "$TOKEN" ]; then
    curl -s -o /dev/null -m 0.2 --connect-timeout 0.1 \
        -X POST -H "x-opensessions-token: $TOKEN" \
        "http://${HOST}:${PORT}/toggle" -d "$CTX"
fi
