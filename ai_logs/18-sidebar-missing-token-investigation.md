# Sidebar Missing Token Investigation - 2026-06-08

## Summary

Investigated a live issue where creating a new `carmen` tmux session from the OpenSessions sidebar left the session without a sidebar pane, and `prefix o s` did not restore it.

---

## Findings

- The source checkout could not be switched to `main` because existing uncommitted work on `fix/sidebar-scroll-isolation` would be overwritten.
- The active runtime was the TPM install at `~/.tmux/plugins/opensessions`, running from `main`.
- The new `carmen` session existed and was attached, but had only the Amp pane; it was missing the usual `bun` pane titled `opensessions-sidebar`.
- tmux hooks were installed for `http://127.0.0.1:36916`, but both `/tmp/opensessions.19916.pid` and `/tmp/opensessions.19916.token` were missing.
- Because the token file was gone while the server was still listening, authenticated hook calls and `focus.sh` no-oped or returned unauthorized, so automatic sidebar restoration could not run.
- The Amp plugin log showed repeated `SKIP no-token`, consistent with the missing token file.

---

## Live Repair

Restarted the orphaned OpenSessions server using the active tmux socket context, which recreated the PID/token files and reinstalled hooks. Then ran the focus/ensure path, which spawned the missing sidebar in `carmen`.

Verification after repair:

- `/tmp/opensessions.19916.pid` exists.
- `/tmp/opensessions.19916.token` exists with mode `0600`.
- `carmen:0` has two panes: `opensessions-sidebar` (`bun`) and the Amp pane.

---

## Useful Commands

```bash
# Inspect active install and running server
tmux show-environment -g OPENSESSIONS_DIR
ps -axo pid,ppid,command | grep opensessions/apps/server

# Check tmux hooks and sidebar panes
tmux show-hooks -g | grep -i opensessions
tmux list-panes -t carmen:0 -F '#{pane_index}|#{pane_id}|#{pane_active}|#{pane_current_command}|#{pane_title}|#{pane_width}|#{pane_current_path}'

# Recreate tmux socket context for scripts from outside tmux env
TMUX_VALUE="$(tmux display-message -p '#{socket_path},#{pid},#{client_pid}')"
TMUX="$TMUX_VALUE" sh ~/.tmux/plugins/opensessions/integrations/tmux-plugin/scripts/focus.sh
```

---

## Follow-up Idea

Consider making the server rewrite its auth token file when the unauthenticated liveness probe is hit and the token file is missing. That would let `ensure_server` self-heal if `/tmp/opensessions.<key>.token` disappears while the server is still alive.
