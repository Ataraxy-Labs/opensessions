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
# Pick up the Rust opt-in early so we can pin a separate port range for it.
# This ensures starting the Rust stack never collides with an already-running
# TS bun server (and vice versa) on the same tmux socket. TS keeps the
# original 17000+SERVER_KEY base; Rust uses 22000+SERVER_KEY.
if [ -z "$OPENSESSIONS_RUST" ]; then
  OPENSESSIONS_RUST="$(tmux show-environment -g OPENSESSIONS_RUST 2>/dev/null | cut -d= -f2)"
fi
if [ "$OPENSESSIONS_RUST" = "1" ]; then
  PORT_BASE=22000
else
  PORT_BASE=17000
fi
if [ -n "$OPENSESSIONS_PORT" ]; then
  PORT="$OPENSESSIONS_PORT"
elif [ -n "$SERVER_KEY" ]; then
  PORT=$((PORT_BASE + SERVER_KEY))
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

PLUGIN_DIR="$(tmux show-environment -g OPENSESSIONS_DIR 2>/dev/null | cut -d= -f2)"
PLUGIN_DIR="${PLUGIN_DIR:-$(cd "$SCRIPT_DIR/../../.." && pwd)}"
BUN_PATH="${BUN_PATH:-$(command -v bun 2>/dev/null || echo "$HOME/.bun/bin/bun")}"
SERVER_ENTRY="$PLUGIN_DIR/apps/server/src/main.ts"
SERVER_WIDTH="${OPENSESSIONS_WIDTH:-$(tmux show-environment -g OPENSESSIONS_WIDTH 2>/dev/null | cut -d= -f2)}"
SERVER_LOG="/tmp/opensessions-server.log"

# Opt-in to the Rust server. Default is the TS bun server. Users can flip it
# globally from their tmux config:
#   set-environment -g OPENSESSIONS_RUST 1
# Falls back to TS automatically if the Rust binary has not been built yet.
# (OPENSESSIONS_RUST resolution already happened above so the port logic can
# pin a separate range for the Rust stack — do NOT clobber it here.)
RUST_SERVER_BIN=""
if [ "$OPENSESSIONS_RUST" = "1" ]; then
  if [ -x "$PLUGIN_DIR/target/release/opensessions-server" ]; then
    RUST_SERVER_BIN="$PLUGIN_DIR/target/release/opensessions-server"
  elif [ -x "$PLUGIN_DIR/target/debug/opensessions-server" ]; then
    RUST_SERVER_BIN="$PLUGIN_DIR/target/debug/opensessions-server"
  fi
fi

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

  if [ -n "$RUST_SERVER_BIN" ]; then
    "$RUST_SERVER_BIN" >"$SERVER_LOG" 2>&1 &
  else
    if [ ! -x "$BUN_PATH" ]; then
      show_startup_error "opensessions: bun not found. Install bun and retry."
      return 1
    fi
    "$BUN_PATH" run "$SERVER_ENTRY" >"$SERVER_LOG" 2>&1 &
  fi

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
