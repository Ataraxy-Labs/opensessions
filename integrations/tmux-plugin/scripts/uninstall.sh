#!/usr/bin/env sh
# opensessions uninstall — clean up all tmux hooks, keybindings, sidebar panes, and env vars
# Run this BEFORE removing the plugin files.
#
# Usage:
#   sh /path/to/opensessions/integrations/tmux-plugin/scripts/uninstall.sh

set -e

echo "opensessions: uninstalling..."

# --- Remove global hooks ---
for hook in \
  client-session-changed \
  session-created \
  session-closed \
  client-resized \
  after-select-window \
  after-new-window \
  after-resize-pane; do
  tmux set-hook -gu "$hook" 2>/dev/null || true
done
echo "  ✓ removed global hooks"

# --- Kill sidebar panes ---
# Find all panes titled "opensessions-sidebar" and kill them
sidebar_panes=$(tmux list-panes -a -F '#{pane_id} #{pane_title}' 2>/dev/null | grep 'opensessions-sidebar' | awk '{print $1}') || true
if [ -n "$sidebar_panes" ]; then
  for pane in $sidebar_panes; do
    tmux kill-pane -t "$pane" 2>/dev/null || true
  done
  echo "  ✓ killed sidebar panes"
fi

# --- Kill stash session ---
tmux kill-session -t "_os_stash" 2>/dev/null || true
echo "  ✓ removed stash session"

# --- Kill the server ---
# Source server-common.sh so we resolve the per-instance PORT/HOST and
# token the same way the runtime does. Use a subshell to keep `set -e`
# from aborting the rest of the uninstall if the server's already gone.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
(
  set +e
  . "$SCRIPT_DIR/server-common.sh"
  # /quit is the canonical "tear down this server instance" endpoint;
  # /shutdown was a leftover that never existed in the runtime.
  auth_post "/quit" >/dev/null 2>&1 || true
) || true
echo "  ✓ stopped server (if running)"

# --- Remove keybindings ---
# Command table bindings (opensessions key table)
PREFIX_KEY=$(tmux show-option -gqv "@opensessions-prefix-key" 2>/dev/null)
PREFIX_KEY="${PREFIX_KEY:-o}"
tmux unbind-key "$PREFIX_KEY" 2>/dev/null || true

# Unbind all keys in the opensessions command table
tmux unbind-key -T opensessions s 2>/dev/null || true
tmux unbind-key -T opensessions t 2>/dev/null || true
tmux unbind-key -T opensessions e 2>/dev/null || true
for i in 1 2 3 4 5 6 7 8 9; do
  tmux unbind-key -T opensessions "$i" 2>/dev/null || true
done
tmux unbind-key -T opensessions Any 2>/dev/null || true

# Direct prefix bindings
tmux unbind-key C-s 2>/dev/null || true
tmux unbind-key C-t 2>/dev/null || true
for i in 1 2 3 4 5 6 7 8 9; do
  tmux unbind-key "M-$i" 2>/dev/null || true
done

# Global keys (if configured)
FOCUS_GLOBAL_KEY=$(tmux show-option -gqv "@opensessions-focus-global-key" 2>/dev/null)
if [ -n "$FOCUS_GLOBAL_KEY" ]; then
  tmux unbind-key -n "$FOCUS_GLOBAL_KEY" 2>/dev/null || true
fi
INDEX_KEYS=$(tmux show-option -gqv "@opensessions-index-keys" 2>/dev/null)
for key in $INDEX_KEYS; do
  tmux unbind-key -n "$key" 2>/dev/null || true
done
echo "  ✓ removed keybindings"

# --- Remove environment variables ---
tmux set-environment -gu OPENSESSIONS_DIR 2>/dev/null || true
tmux set-environment -gu OPENSESSIONS_WIDTH 2>/dev/null || true
echo "  ✓ removed environment variables"

echo "opensessions: uninstall complete. You can now remove the plugin files."
echo "  If using TPM: remove the line from .tmux.conf and run prefix + alt + u"
