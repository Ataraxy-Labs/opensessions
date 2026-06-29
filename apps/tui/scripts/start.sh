#!/usr/bin/env bash
# Start the opensessions TUI.
# Works in both tmux and zellij — detects the mux from environment.

if [ -n "${TMUX:-}" ]; then
    OPENSESSIONS_DIR="$(tmux show-environment -g OPENSESSIONS_DIR 2>/dev/null | cut -d= -f2)"
fi
OPENSESSIONS_DIR="${OPENSESSIONS_DIR:-$(cd "$(dirname "$0")/../../.." && pwd)}"
TUI_DIR="$OPENSESSIONS_DIR/apps/tui"

BUN_PATH="${BUN_PATH:-$(command -v bun 2>/dev/null || echo "$HOME/.bun/bin/bun")}"

if [ -n "${TMUX:-}" ] && [ -n "${TMUX_PANE:-}" ]; then
    # Keep sidebar panes from accumulating stale OpenTUI/agent output in tmux
    # history. Scroll/copy-mode should not be able to reveal a previous sidebar
    # process after a server/plugin restart. This is pane-scoped and does not
    # touch the user's main Amp/shell pane history.
    tmux send-keys -R -t "$TMUX_PANE" >/dev/null 2>&1 || true
    tmux clear-history -t "$TMUX_PANE" >/dev/null 2>&1 || true
    printf '\033[2J\033[3J\033[H'
fi

cd "$TUI_DIR"
export REFOCUS_WINDOW
export OPENSESSIONS_DIR
exec "$BUN_PATH" run src/index.tsx 2>/tmp/opensessions-err.log
