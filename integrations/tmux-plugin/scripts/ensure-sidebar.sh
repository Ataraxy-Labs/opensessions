#!/usr/bin/env sh
# Ensure the current window has a sidebar pane.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/server-common.sh"

ensure_server || exit 0

CTX=$(tmux display-message -p '#{client_tty}|#{session_name}|#{window_id}' 2>/dev/null)
curl -s -o /dev/null -m 0.2 --connect-timeout 0.1 -X POST -H "x-opensessions-token: $(read_token)" "http://${HOST}:${PORT}/ensure-sidebar" -d "$CTX"
