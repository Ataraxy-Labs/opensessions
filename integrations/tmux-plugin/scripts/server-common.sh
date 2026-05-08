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

# Defer plugin-dir / bun lookup until the cold-start path actually needs it.
# tmux show-environment forks tmux (~10-20ms); command -v bun is cheap but
# the conditional below ensures hot paths (alive cache hit) skip both.
SERVER_LOG="/tmp/opensessions-server.log"

resolve_cold_start_paths() {
  PLUGIN_DIR="$(tmux show-environment -g OPENSESSIONS_DIR 2>/dev/null | cut -d= -f2)"
  PLUGIN_DIR="${PLUGIN_DIR:-$(cd "$SCRIPT_DIR/../../.." && pwd)}"
  BUN_PATH="${BUN_PATH:-$(command -v bun 2>/dev/null || echo "$HOME/.bun/bin/bun")}"
  SERVER_ENTRY="$PLUGIN_DIR/apps/server/src/main.ts"
}

show_startup_error() {
  message="$1"
  tmux display-message "$message" >/dev/null 2>&1 || true
  printf '%s\n' "$message" >&2
}

server_alive() {
  curl -s -o /dev/null -m 0.2 "http://${HOST}:${PORT}/" 2>/dev/null
}

# Cache the alive-check result. If we confirmed the server alive within
# ALIVE_CACHE_TTL_S seconds, trust it and skip the ~80ms curl. Hot keypress
# paths (switch-index, focus, toggle) all call ensure_server first; without
# this they each pay the curl tax even though the server is fine.
ALIVE_CACHE_FILE="${PID_FILE%.pid}.alive"
ALIVE_CACHE_TTL_S=5

alive_cache_fresh() {
  [ -f "$ALIVE_CACHE_FILE" ] || return 1
  # Cross-platform mtime delta. macOS stat -f %m, Linux stat -c %Y.
  now=$(date +%s)
  mtime=$(stat -f %m "$ALIVE_CACHE_FILE" 2>/dev/null || stat -c %Y "$ALIVE_CACHE_FILE" 2>/dev/null)
  [ -n "$mtime" ] || return 1
  delta=$((now - mtime))
  [ "$delta" -lt "$ALIVE_CACHE_TTL_S" ]
}

ensure_server() {
  if alive_cache_fresh; then
    return 0
  fi

  if server_alive; then
    : > "$ALIVE_CACHE_FILE" 2>/dev/null
    return 0
  fi

  # Cold start: only now do we need plugin dir + bun resolution
  resolve_cold_start_paths

  if [ ! -x "$BUN_PATH" ]; then
    show_startup_error "opensessions: bun not found. Install bun and retry."
    return 1
  fi

  "$BUN_PATH" run "$SERVER_ENTRY" >"$SERVER_LOG" 2>&1 &

  attempt=0
  while [ "$attempt" -lt 30 ]; do
    sleep 0.1
    if server_alive; then
      : > "$ALIVE_CACHE_FILE" 2>/dev/null
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
