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

OPENSESSIONS_DIR="${OPENSESSIONS_DIR:-$(cd "$(dirname "$0")/../../.." && pwd)}"

RUST_SERVER_BIN=""
if [ -x "$OPENSESSIONS_DIR/target/release/opensessions-server" ]; then
    RUST_SERVER_BIN="$OPENSESSIONS_DIR/target/release/opensessions-server"
elif [ -x "$OPENSESSIONS_DIR/target/debug/opensessions-server" ]; then
    RUST_SERVER_BIN="$OPENSESSIONS_DIR/target/debug/opensessions-server"
fi

# --- Ensure server is running ---
server_alive() {
    curl -s -o /dev/null -m 0.2 "http://${HOST}:${PORT}/" 2>/dev/null
}

if ! server_alive; then
    if [ -z "$RUST_SERVER_BIN" ]; then
        echo "opensessions: server binary not found. Run: cd $OPENSESSIONS_DIR && cargo build --release -p opensessions-server" >&2
        exit 1
    fi
    "$RUST_SERVER_BIN" &>/dev/null &
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
curl -s -o /dev/null -m 0.2 --connect-timeout 0.1 -X POST "http://${HOST}:${PORT}/toggle" -d "$CTX"
