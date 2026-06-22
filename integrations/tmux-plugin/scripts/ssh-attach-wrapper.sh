#!/usr/bin/env sh
set -eu

node_id="${1:-}"
shift || true
attach_command="${1:-}"

if [ -z "$node_id" ] || [ -z "$attach_command" ]; then
  printf '%s\n' "usage: ssh-attach-wrapper.sh <node-id> <attach-command>" >&2
  exit 2
fi

state_dir="${XDG_RUNTIME_DIR:-/tmp}/opensessions-ssh-bridges"
mkdir -p "$state_dir"
state_file="$state_dir/$node_id.pid"
return_file="$state_file.return"

cleanup() {
  rm -f "$state_file"
  rm -f "$return_file"
}
trap cleanup EXIT INT TERM HUP

reset_terminal_modes() {
  # The sidebar TUI enables mouse capture/alternate-screen modes. When tmux
  # detaches the client into this wrapper, the TUI may not get a chance to run
  # its normal terminal cleanup. Reset those modes before handing the terminal
  # to ssh, otherwise the remote shell receives raw mouse escape sequences.
  # SSH/tmux can also leave the terminal in an alternate character set after a
  # forced bridge return. Reset charsets and common private modes so the next
  # local tmux attach gets a clean terminal and does a full redraw instead of
  # inheriting remote line-drawing or mouse state.
  printf '\017\033(B\033)B\033[0m\033[?1l\033[?7h\033[?12l\033[?25h\033[?1000l\033[?1002l\033[?1003l\033[?1006l\033[?1015l\033[?1049l\033[2J\033[H'
  stty sane 2>/dev/null || true
  if [ -n "${OPENSESSIONS_ATTACH_COLS:-}" ] && [ -n "${OPENSESSIONS_ATTACH_ROWS:-}" ]; then
    stty cols "$OPENSESSIONS_ATTACH_COLS" rows "$OPENSESSIONS_ATTACH_ROWS" 2>/dev/null || true
  fi
}

drain_terminal_input() {
  # A sidebar click can leave the matching mouse-release SGR bytes queued after
  # tmux detaches the client into this wrapper. If ssh starts before those bytes
  # are discarded, the remote active pane receives and renders them literally.
  [ -t 0 ] || return 0
  saved_stty="$(stty -g 2>/dev/null || true)"
  [ -n "$saved_stty" ] || return 0
  stty -icanon min 0 time 1 2>/dev/null || return 0
  while :; do
    bytes="$(dd bs=1024 count=1 2>/dev/null | wc -c | tr -d ' ')"
    [ "${bytes:-0}" -gt 0 ] 2>/dev/null || break
  done
  stty "$saved_stty" 2>/dev/null || true
}

reset_terminal_modes
drain_terminal_input
sh -lc "$attach_command" &
child_pid="$!"
printf '%s\n' "$child_pid" >"$state_file"
set +e
wait "$child_pid"
status="$?"
set -e

return_command=""
if [ -s "$return_file" ]; then
  return_command="$(cat "$return_file" 2>/dev/null || true)"
fi
cleanup

if [ -n "$return_command" ]; then
  reset_terminal_modes
  drain_terminal_input
  exec sh -lc "$return_command"
fi

exit "$status"
