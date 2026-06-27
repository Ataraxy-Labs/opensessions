#!/usr/bin/env sh
# Open the agent-status panel as a centered floating popup via tmux display-popup.
# Unlike the docked sidebar, the TUI runs in popup mode (OPENSESSIONS_POPUP) and
# does not shut the shared server down on quit, so it can be toggled freely.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/server-common.sh"

# Cold-start the server if a prior popup/sidebar quit tore it down.
ensure_server || exit 0

POPUP_WIDTH="$(tmux show-option -gqv '@opensessions-popup-width')"
POPUP_HEIGHT="$(tmux show-option -gqv '@opensessions-popup-height')"
POPUP_WIDTH="${POPUP_WIDTH:-80%}"
POPUP_HEIGHT="${POPUP_HEIGHT:-70%}"

exec tmux display-popup \
  -w "$POPUP_WIDTH" \
  -h "$POPUP_HEIGHT" \
  -e "OPENSESSIONS_POPUP=1" \
  -E "\"${PLUGIN_DIR}\"/apps/tui/scripts/start.sh"
