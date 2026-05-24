#!/usr/bin/env bash
# Start the opensessions TUI.
# Works in both tmux and zellij — detects the mux from environment.
#
# By default this launches the TS sidebar (`bun run src/index.tsx`).
# Set OPENSESSIONS_RUST=1 to launch the experimental Rust sidebar binary
# (`target/release/opensessions-sidebar`, falling back to debug). The default
# stays on TS so live user tmux is not affected by ratatui-migration work.

if [ -n "${TMUX:-}" ]; then
    OPENSESSIONS_DIR="$(tmux show-environment -g OPENSESSIONS_DIR 2>/dev/null | cut -d= -f2)"
    # Pick up tmux-configured opt-in for the Rust sidebar:
    #   tmux set-environment -g OPENSESSIONS_RUST 1
    # so users can flip it from .tmux.conf without re-exporting the shell env.
    if [ -z "${OPENSESSIONS_RUST:-}" ]; then
        OPENSESSIONS_RUST="$(tmux show-environment -g OPENSESSIONS_RUST 2>/dev/null | cut -d= -f2)"
    fi
fi
OPENSESSIONS_DIR="${OPENSESSIONS_DIR:-$(cd "$(dirname "$0")/../../.." && pwd)}"
TUI_DIR="$OPENSESSIONS_DIR/apps/tui"
RUST_RELEASE_DIR="$OPENSESSIONS_DIR/target/release"
RUST_DEBUG_DIR="$OPENSESSIONS_DIR/target/debug"

BUN_PATH="${BUN_PATH:-$(command -v bun 2>/dev/null || echo "$HOME/.bun/bin/bun")}"

export REFOCUS_WINDOW
export OPENSESSIONS_DIR

if [ "${OPENSESSIONS_RUST:-0}" = "1" ]; then
    RUST_BIN=""
    if [ -x "$RUST_RELEASE_DIR/opensessions-sidebar" ]; then
        RUST_BIN="$RUST_RELEASE_DIR/opensessions-sidebar"
    elif [ -x "$RUST_DEBUG_DIR/opensessions-sidebar" ]; then
        RUST_BIN="$RUST_DEBUG_DIR/opensessions-sidebar"
    fi
    if [ -n "$RUST_BIN" ]; then
        # Append (not overwrite) so a panic from a previous launch survives the
        # next attempt. Truncate manually with `: > /tmp/opensessions-err.log`
        # if the log gets noisy.
        :
        # Mirror the Rust port offset from
        # integrations/tmux-plugin/scripts/server-common.sh so the sidebar
        # connects to the Rust server (22000+SERVER_KEY) instead of the TS
        # server (17000+SERVER_KEY) when both stacks coexist on the same
        # tmux socket. If OPENSESSIONS_PORT is already set we honor it.
        if [ -z "${OPENSESSIONS_PORT:-}" ] && [ -n "${TMUX:-}" ]; then
            socket_path="${TMUX%%,*}"
            if [ -n "$socket_path" ]; then
                server_key=$(awk -v input="$socket_path" 'BEGIN {
                    hash = 0
                    for (i = 1; i <= length(input); i++) {
                      hash = (hash + ord(substr(input, i, 1)) * i) % 20000
                    }
                    printf "%d\n", hash
                  }
                  function ord(ch,    chars) {
                    chars = " !\"#$%&\047()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~"
                    return index(chars, ch) + 31
                  }')
                if [ -n "$server_key" ]; then
                    export OPENSESSIONS_PORT=$((22000 + server_key))
                fi
            fi
        fi
        printf '\n--- launch %s pid=%s ---\n' "$(date +%s)" "$$" >>/tmp/opensessions-err.log
        export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"
        exec "$RUST_BIN" 2>>/tmp/opensessions-err.log
    fi
    # Fall through to TS if no Rust binary is built.
fi

cd "$TUI_DIR"
exec "$BUN_PATH" run src/index.tsx 2>/tmp/opensessions-err.log
