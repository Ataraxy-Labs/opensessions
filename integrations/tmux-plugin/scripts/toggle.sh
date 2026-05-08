#!/usr/bin/env sh
# Toggle the opensessions sidebar via the server.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/server-common.sh"

ensure_server || exit 0

CTX=$(tmux display-message -p '#{client_tty}|#{session_name}|#{window_id}' 2>/dev/null)
curl -s -o /dev/null -m 1.5 --connect-timeout 0.3 -X POST "http://${HOST}:${PORT}/toggle" -d "$CTX" >/dev/null 2>&1 || true
tmux switch-client -T root >/dev/null 2>&1
exit 0
