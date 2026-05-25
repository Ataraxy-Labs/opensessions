#!/usr/bin/env bash
# Start the opensessions TUI sidebar (Rust/Ratatui).

if [ -n "${TMUX:-}" ]; then
    OPENSESSIONS_DIR="$(tmux show-environment -g OPENSESSIONS_DIR 2>/dev/null | cut -d= -f2)"
fi
OPENSESSIONS_DIR="${OPENSESSIONS_DIR:-$(cd "$(dirname "$0")/../../.." && pwd)}"

RUST_BIN=""
if [ -x "$OPENSESSIONS_DIR/target/release/opensessions-sidebar" ]; then
    RUST_BIN="$OPENSESSIONS_DIR/target/release/opensessions-sidebar"
elif [ -x "$OPENSESSIONS_DIR/target/debug/opensessions-sidebar" ]; then
    RUST_BIN="$OPENSESSIONS_DIR/target/debug/opensessions-sidebar"
fi

if [ -z "$RUST_BIN" ]; then
    echo "opensessions: sidebar binary not found. Run: cd $OPENSESSIONS_DIR && cargo build --release -p opensessions-sidebar" >&2
    exit 1
fi

export REFOCUS_WINDOW
export OPENSESSIONS_DIR

printf '\n--- launch %s pid=%s ---\n' "$(date +%s)" "$$" >>/tmp/opensessions-err.log
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"
exec "$RUST_BIN" 2>>/tmp/opensessions-err.log
