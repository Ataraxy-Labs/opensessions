#!/usr/bin/env bash
# opensessions hook for Codex CLI
# Usage: opensessions-hook.sh
# Codex pipes JSON to stdin with hook_event_name, session_id, etc.
# This script maps Codex events → opensessions AgentStatus and POSTs to the server.

set -euo pipefail

SERVER_URL="${OPENSESSIONS_URL:-http://127.0.0.1:7391/event}"
EVENTS_FILE="${OPENSESSIONS_EVENTS_FILE:-/tmp/opensessions-events.jsonl}"

# Read stdin (Codex sends JSON payload)
INPUT=""
if [ ! -t 0 ]; then
  INPUT=$(cat)
fi

# Extract hook_event_name from JSON
EVENT=""
if [ -n "$INPUT" ] && command -v jq &>/dev/null; then
  EVENT=$(echo "$INPUT" | jq -r '.hook_event_name // empty' 2>/dev/null || true)
fi

if [ -z "$EVENT" ]; then
  exit 0
fi

# Get session name from tmux (or zellij, or fallback)
get_session() {
  if [ -n "${TMUX:-}" ]; then
    tmux display-message -p '#S' 2>/dev/null || echo "unknown"
  elif [ -n "${ZELLIJ_SESSION_NAME:-}" ]; then
    echo "$ZELLIJ_SESSION_NAME"
  else
    echo "unknown"
  fi
}

SESSION=$(get_session)

# Map Codex hook event → opensessions status
map_status() {
  case "$EVENT" in
    SessionStart)       echo "idle" ;;
    UserPromptSubmit)   echo "running" ;;
    PreToolUse)         echo "running" ;;
    PostToolUse)        echo "running" ;;
    Stop)               echo "idle" ;;
    *)                  echo "" ;;
  esac
}

STATUS=$(map_status)
if [ -z "$STATUS" ]; then
  exit 0
fi

TIMESTAMP=$(($(date +%s) * 1000))
PAYLOAD=$(printf '{"agent":"codex","session":"%s","status":"%s","ts":%s}' "$SESSION" "$STATUS" "$TIMESTAMP")

# Try HTTP first, fall back to JSONL file
if command -v curl &>/dev/null; then
  curl -s -o /dev/null -X POST "$SERVER_URL" \
    -H 'Content-Type: application/json' \
    -d "$PAYLOAD" 2>/dev/null || \
    echo "$PAYLOAD" >> "$EVENTS_FILE" 2>/dev/null || true
else
  echo "$PAYLOAD" >> "$EVENTS_FILE" 2>/dev/null || true
fi

exit 0
