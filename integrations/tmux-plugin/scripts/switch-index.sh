#!/usr/bin/env sh
# Switch to the Nth visible opensessions session (1-indexed).
#
# Args:
#   $1 = index (required)
#   $2 = pre-expanded ctx string "client_tty|session|window_id" (optional)
# When $2 is supplied (from tmux format-string expansion at bind-key time),
# we skip the ~16ms `tmux display-message` fork.

INDEX="${1:?Usage: switch-index.sh <index> [ctx]}"
CTX="${2:-}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/server-common.sh"

# Fast path: read the ordering file the server writes on every state change
# and call `tmux switch-client -t <name>` directly. This avoids a round-trip
# through the server's HTTP handler and its own synchronous tmux fork —
# user-perceived latency drops to one tmux subprocess fork (~30-50ms) plus
# shell overhead. The server still gets notified asynchronously via tmux
# hooks (client-session-changed → POST /focus).
ORDERING_FILE="${PID_FILE%.pid}.ordering"
if [ -f "$ORDERING_FILE" ]; then
  TARGET=$(awk -v idx="$INDEX" 'NR == idx { print; exit }' "$ORDERING_FILE")
  if [ -n "$TARGET" ]; then
    tmux switch-client -t "$TARGET" >/dev/null 2>&1
    # Fire-and-forget POST so the server can update side effects (sidebar
    # focus, agent unseen flags, custom ordering) async. Timeout generous;
    # exit code swallowed so tmux's status line never shows curl errors.
    if [ -z "$CTX" ]; then
      CTX="|$TARGET|"
    fi
    (curl -s -o /dev/null -m 1.5 --connect-timeout 0.3 -X POST "http://${HOST}:${PORT}/switch-index?index=${INDEX}" -d "$CTX" >/dev/null 2>&1 || true) &
    exit 0
  fi
fi

# Cold path: ordering file missing or empty. Server hasn't broadcast yet
# (cold boot). Fall back to the original server-mediated switch.
ensure_server || exit 0
if [ -z "$CTX" ]; then
  CTX=$(tmux display-message -p '#{client_tty}|#{session_name}|#{window_id}' 2>/dev/null)
fi
curl -s -o /dev/null -m 1.5 --connect-timeout 0.3 -X POST "http://${HOST}:${PORT}/switch-index?index=${INDEX}" -d "$CTX" >/dev/null 2>&1 || true
exit 0
