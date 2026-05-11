#!/usr/bin/env sh

server_key() {
  if [ -n "$OPENSESSIONS_SERVER_KEY" ]; then
    printf '%s\n' "$OPENSESSIONS_SERVER_KEY"
    return
  fi

  if [ -z "$TMUX" ]; then
    return
  fi

  socket_path="${TMUX%%,*}"
  [ -n "$socket_path" ] || return

  awk -v input="$socket_path" 'BEGIN {
    hash = 0
    for (i = 1; i <= length(input); i++) {
      hash = (hash + ord(substr(input, i, 1)) * i) % 20000
    }
    printf "%d\n", hash
  }
  function ord(ch,    chars) {
    chars = " !\"#$%&\047()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~"
    return index(chars, ch) + 31
  }'
}

SERVER_KEY="$(server_key)"
if [ -n "$OPENSESSIONS_PORT" ]; then
  PORT="$OPENSESSIONS_PORT"
elif [ -n "$SERVER_KEY" ]; then
  PORT=$((17000 + SERVER_KEY))
else
  PORT="7391"
fi
HOST="${OPENSESSIONS_HOST:-127.0.0.1}"
if [ -n "$OPENSESSIONS_PID_FILE" ]; then
  PID_FILE="$OPENSESSIONS_PID_FILE"
elif [ -n "$SERVER_KEY" ]; then
  PID_FILE="/tmp/opensessions.${SERVER_KEY}.pid"
else
  PID_FILE="/tmp/opensessions.pid"
fi
if [ -n "$OPENSESSIONS_TOKEN_FILE" ]; then
  TOKEN_FILE="$OPENSESSIONS_TOKEN_FILE"
elif [ -n "$SERVER_KEY" ]; then
  TOKEN_FILE="/tmp/opensessions.${SERVER_KEY}.token"
else
  TOKEN_FILE="/tmp/opensessions.token"
fi

# Reads the per-server-instance auth token written by the server at
# startup (mode 0600). Empty if the file is missing — callers should
# treat that as "the server isn't running" and let the request fail
# naturally rather than retrying without auth.
read_token() {
  [ -f "$TOKEN_FILE" ] && cat "$TOKEN_FILE" 2>/dev/null || printf ''
}

# Issues an authenticated POST to the server. Pass the path (without
# the http://host:port prefix) and any extra curl args (e.g. -d "$CTX").
# Silently no-ops if the token isn't readable yet.
auth_post() {
  path="$1"
  shift
  token="$(read_token)"
  [ -n "$token" ] || return 0
  curl -s -o /dev/null -m 0.2 --connect-timeout 0.1 \
    -X POST -H "x-opensessions-token: $token" \
    "http://${HOST}:${PORT}${path}" "$@"
}

PLUGIN_DIR="$(tmux show-environment -g OPENSESSIONS_DIR 2>/dev/null | cut -d= -f2)"
PLUGIN_DIR="${PLUGIN_DIR:-$(cd "$SCRIPT_DIR/../../.." && pwd)}"
BUN_PATH="${BUN_PATH:-$(command -v bun 2>/dev/null || echo "$HOME/.bun/bin/bun")}"
SERVER_ENTRY="$PLUGIN_DIR/apps/server/src/main.ts"
SERVER_LOG="/tmp/opensessions-server.log"

show_startup_error() {
  message="$1"
  tmux display-message "$message" >/dev/null 2>&1 || true
  printf '%s\n' "$message" >&2
}

server_alive() {
  curl -s -o /dev/null -m 0.2 "http://${HOST}:${PORT}/" 2>/dev/null
}

ensure_server() {
  if server_alive; then
    return 0
  fi

  if [ ! -x "$BUN_PATH" ]; then
    show_startup_error "opensessions: bun not found. Install bun and retry."
    return 1
  fi

  "$BUN_PATH" run "$SERVER_ENTRY" >"$SERVER_LOG" 2>&1 &

  attempt=0
  while [ "$attempt" -lt 30 ]; do
    sleep 0.1
    if server_alive; then
      return 0
    fi
    attempt=$((attempt + 1))
  done

  if grep -Eq "Cannot find module '@opensessions/|Cannot find package '@opensessions/|Cannot find module 'xstate'|Cannot find package 'xstate'" "$SERVER_LOG" 2>/dev/null; then
    show_startup_error "opensessions: server dependencies are missing. Run: cd $PLUGIN_DIR && $BUN_PATH install --frozen-lockfile"
    return 1
  fi

  show_startup_error "opensessions: server failed to start. See $SERVER_LOG"

  return 1
}
