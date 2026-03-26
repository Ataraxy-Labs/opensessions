#!/usr/bin/env bash
# Install opensessions hooks into Codex CLI
# Usage: bash setup.sh
#
# This creates/updates ~/.codex/hooks.json and enables the hooks feature
# in ~/.codex/config.toml. Idempotent — safe to run multiple times.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
HOOK_SCRIPT="$SCRIPT_DIR/opensessions-hook.sh"
CODEX_DIR="$HOME/.codex"
HOOKS_FILE="$CODEX_DIR/hooks.json"
CONFIG_FILE="$CODEX_DIR/config.toml"

if [ ! -f "$HOOK_SCRIPT" ]; then
  echo "Error: opensessions-hook.sh not found at $HOOK_SCRIPT"
  exit 1
fi

# Make hook script executable
chmod +x "$HOOK_SCRIPT"

# Ensure ~/.codex exists
mkdir -p "$CODEX_DIR"

# Generate hooks.json
HOOKS_JSON=$(cat <<EOF
{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "startup|resume",
        "hooks": [
          {
            "type": "command",
            "command": "$HOOK_SCRIPT",
            "timeout": 5
          }
        ]
      }
    ],
    "UserPromptSubmit": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "$HOOK_SCRIPT",
            "timeout": 5
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "$HOOK_SCRIPT",
            "timeout": 5
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "$HOOK_SCRIPT",
            "timeout": 5
          }
        ]
      }
    ]
  }
}
EOF
)

# Write hooks.json
if command -v jq &>/dev/null; then
  if [ -f "$HOOKS_FILE" ]; then
    # Merge with existing hooks.json
    EXISTING=$(cat "$HOOKS_FILE")
    MERGED=$(echo "$EXISTING" | jq --argjson new "$HOOKS_JSON" '.hooks = ($new.hooks + (.hooks // {}))' 2>/dev/null || echo "$HOOKS_JSON")
    echo "$MERGED" | jq '.' > "$HOOKS_FILE"
  else
    echo "$HOOKS_JSON" | jq '.' > "$HOOKS_FILE"
  fi
else
  echo "$HOOKS_JSON" > "$HOOKS_FILE"
fi

echo "✅ opensessions hooks installed in $HOOKS_FILE"

# Enable codex_hooks feature in config.toml
if [ -f "$CONFIG_FILE" ]; then
  if grep -q 'codex_hooks' "$CONFIG_FILE" 2>/dev/null; then
    # Update existing value
    sed -i.bak 's/codex_hooks.*/codex_hooks = true/' "$CONFIG_FILE" && rm -f "$CONFIG_FILE.bak"
    echo "✅ codex_hooks feature updated in $CONFIG_FILE"
  elif grep -q '\[features\]' "$CONFIG_FILE" 2>/dev/null; then
    # [features] section exists, add the key
    sed -i.bak '/\[features\]/a\
codex_hooks = true' "$CONFIG_FILE" && rm -f "$CONFIG_FILE.bak"
    echo "✅ codex_hooks feature added to $CONFIG_FILE"
  else
    # No [features] section, append it
    printf '\n[features]\ncodex_hooks = true\n' >> "$CONFIG_FILE"
    echo "✅ codex_hooks feature added to $CONFIG_FILE"
  fi
else
  printf '[features]\ncodex_hooks = true\n' > "$CONFIG_FILE"
  echo "✅ Created $CONFIG_FILE with codex_hooks enabled"
fi

echo ""
echo "Events configured:"
echo "  SessionStart     → idle"
echo "  UserPromptSubmit → running"
echo "  PostToolUse      → running"
echo "  Stop             → idle"
echo ""
echo "Run Codex with hooks enabled:"
echo "  codex -c features.codex_hooks=true"
echo "  (or the feature is already set in your config.toml)"
