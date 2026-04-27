#!/usr/bin/env sh
# Switch to the Nth visible opensessions session (1-indexed).

INDEX="${1:?Usage: switch-index.sh <index>}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/server-common.sh"

ensure_server || exit 0

CTX=$(tmux display-message -p '#{client_tty}|#{session_name}|#{window_id}' 2>/dev/null)
curl -s -o /dev/null -m 0.2 --connect-timeout 0.1 -X POST -H "x-opensessions-token: $(read_token)" "http://${HOST}:${PORT}/switch-index?index=${INDEX}" -d "$CTX"
tmux switch-client -T root >/dev/null 2>&1
