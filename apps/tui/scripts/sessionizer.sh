#!/usr/bin/env bash
# opensessions sessionizer — fuzzy directory picker for new tmux sessions
# Requires: fzf, find
# Supports colon-separated paths in SESSIONIZER_DIR (e.g. "$HOME/Code:$HOME/.config")
# Search depth is configurable via SESSIONIZER_MAXDEPTH (default: 3)

# Check env first, then tmux global environment, then default
if [ -z "$SESSIONIZER_DIR" ] && command -v tmux &>/dev/null; then
  SESSIONIZER_DIR=$(tmux show-environment -g SESSIONIZER_DIR 2>/dev/null | sed 's/^SESSIONIZER_DIR=//')
fi
SEARCH_DIRS="${SESSIONIZER_DIR:-$HOME/Documents}"

if [ -z "$SESSIONIZER_MAXDEPTH" ] && command -v tmux &>/dev/null; then
  SESSIONIZER_MAXDEPTH=$(tmux show-environment -g SESSIONIZER_MAXDEPTH 2>/dev/null | sed 's/^SESSIONIZER_MAXDEPTH=//')
fi
MAXDEPTH="${SESSIONIZER_MAXDEPTH:-3}"
[[ "$MAXDEPTH" =~ ^[1-9][0-9]*$ ]] || MAXDEPTH=3

if ! command -v fzf &>/dev/null; then
  echo "fzf is required for the sessionizer. Install it: https://github.com/junegunn/fzf"
  exit 1
fi

# Split colon-separated paths and validate each one
IFS=: read -ra dirs <<<"$SEARCH_DIRS"
valid_dirs=()
for dir in "${dirs[@]}"; do
  [ -d "$dir" ] && valid_dirs+=("$dir")
done

if [ ${#valid_dirs[@]} -eq 0 ]; then
  echo "No valid directories found in: $SEARCH_DIRS"
  exit 1
fi

fzf_output=$(find "${valid_dirs[@]}" -mindepth 1 -maxdepth "$MAXDEPTH" -type d -not -path '*/.*' 2>/dev/null | fzf \
  --reverse \
  --print-query \
  --header="Pick a directory (or type a path and press Enter)" \
  --preview=':' \
  --preview-window=hidden \
  --bind='ctrl-c:abort')

# --print-query outputs: line 1 = query, line 2 = selected match (if any)
query=$(echo "$fzf_output" | sed -n '1p')
match=$(echo "$fzf_output" | sed -n '2p')

# Prefer the selected match; fall back to typed query if it's a valid directory
if [ -n "$match" ]; then
  selected="$match"
elif [ -d "$query" ]; then
  selected="$query"
else
  exit 0
fi

# Derive session name from directory basename, replacing dots with underscores
session_name=$(basename "$selected" | tr '.' '_')

notify_opensessions() {
  local target_session="$1"
  local token=""
  local client_tty=""
  local window_id=""
  local server_host="${OPENSESSIONS_HOST:-127.0.0.1}"
  local server_port="${OPENSESSIONS_PORT:-7391}"
  local token_file="${OPENSESSIONS_TOKEN_FILE:-}"

  if [ -z "$token_file" ] || [ ! -r "$token_file" ]; then
    return 0
  fi
  token=$(cat "$token_file" 2>/dev/null || true)
  if [ -z "$token" ]; then
    return 0
  fi

  client_tty=$(tmux display-message -p '#{client_tty}' 2>/dev/null || true)
  window_id=$(tmux display-message -p -t "=$target_session:" '#{window_id}' 2>/dev/null || true)
  if [ -z "$window_id" ]; then
    return 0
  fi

  curl -s -o /dev/null -m 0.5 --connect-timeout 0.1 \
    -X POST \
    -H "x-opensessions-token: $token" \
    "http://$server_host:$server_port/refresh" >/dev/null 2>&1 || true

  curl -s -o /dev/null -m 0.5 --connect-timeout 0.1 \
    -X POST \
    -H "x-opensessions-token: $token" \
    -d "$client_tty|$target_session|$window_id" \
    "http://$server_host:$server_port/ensure-sidebar" >/dev/null 2>&1 || true
}

# If session already exists, just switch to it
if tmux has-session -t "=$session_name" 2>/dev/null; then
  tmux switch-client -t "$session_name"
  notify_opensessions "$session_name"
  exit 0
fi

tmux new-session -d -s "$session_name" -c "$selected"
tmux switch-client -t "$session_name"
notify_opensessions "$session_name"
