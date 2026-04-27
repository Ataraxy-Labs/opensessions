#!/usr/bin/env sh

PORT="${OPENSESSIONS_PORT:-7391}"
HOST="${OPENSESSIONS_HOST:-127.0.0.1}"

PLUGIN_DIR="$(tmux show-environment -g OPENSESSIONS_DIR 2>/dev/null | cut -d= -f2)"
PLUGIN_DIR="${PLUGIN_DIR:-$(cd "$SCRIPT_DIR/../../.." && pwd)}"
BUN_PATH="${BUN_PATH:-$(command -v bun 2>/dev/null || echo "$HOME/.bun/bin/bun")}"
SERVER_ENTRY="$PLUGIN_DIR/apps/server/src/main.ts"

TOKEN_FILE="/tmp/opensessions.token"

read_token() {
  [ -f "$TOKEN_FILE" ] && cat "$TOKEN_FILE" 2>/dev/null || echo ""
}

server_alive() {
  curl -s -o /dev/null -m 0.2 "http://${HOST}:${PORT}/" 2>/dev/null
}

ensure_server() {
  if server_alive; then
    return 0
  fi

  "$BUN_PATH" run "$SERVER_ENTRY" >/dev/null 2>&1 &

  attempt=0
  while [ "$attempt" -lt 30 ]; do
    sleep 0.1
    if server_alive; then
      return 0
    fi
    attempt=$((attempt + 1))
  done

  return 1
}
