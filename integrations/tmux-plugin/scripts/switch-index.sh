#!/usr/bin/env sh
# Switch to the Nth visible opensessions session (1-indexed).

INDEX="${1:?Usage: switch-index.sh <index>}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/server-common.sh"

ensure_server || exit 0

CTX=$(tmux display-message -p '#{client_tty}|#{session_name}|#{window_id}' 2>/dev/null)
auth_post "/switch-index?index=${INDEX}" -d "$CTX"
tmux switch-client -T root >/dev/null 2>&1
