# Stray `cvm` Session in Sidebar — Investigation - 2026-06-10

## Summary

A tmux session named `cvm` appeared as a top-level entry in the opensessions sidebar. Expected behavior was a *window* nested under the existing `regression-report-toolkit` session (visible in the tmux status bar, not the sidebar). Investigation confirmed **no opensessions bug**: an agent in another thread created a real top-level session, and the sidebar faithfully mirrored `tmux list-sessions`.

---

## What Occurred

1. Amp thread `T-019eb233-ae15-7294-97e7-4f20069e7b7d`, working inside the `regression-report-toolkit` session, needed a persistent SSH gateway to a CVM host and ran:

   ```bash
   tmux new-session -d -s cvm -n ssh 2>/dev/null || tmux new-window -t cvm -n ssh
   ```

   This is the "create namespace if missing" idiom — its first branch creates a **new top-level session** `cvm` instead of a window in the current session.

2. opensessions correctly observed the new session via the tmux provider (`listSessions()` → `tmux list-sessions`), listed it in the sidebar, and spawned a sidebar pane into it (`cvm:ssh` ended up with 2 panes: `%537` `opensessions-sidebar` + `%536` the actual ssh).

3. The sidebar has no session-nesting concept because tmux sessions are flat; *windows* are the nesting unit and are visualized by tmux's status bar (e.g. `opensessions 0:amp`).

## Root Cause

The global `tmux` skill (`~/.agents/skills/tmux/SKILL.md`) instructed agents to spawn windows but:

- never explicitly **forbade** `tmux new-session` when inside tmux,
- didn't explain the consequence (session-list/sidebar pollution, sidebar UI spawned into stray sessions),
- offered no idempotent "create window if missing" pattern, so the agent reached for the session-based idiom.

## Prevention Applied

Hardened `~/.agents/skills/tmux/SKILL.md` with a "Hard Rule: Windows, Never Sessions" section:

- `$TMUX` set → **never** `tmux new-session`; always `tmux new-window` in the current session.
- Idempotent replacement for the bad idiom:
  `tmux list-windows -F '#W' | grep -qxF "NAME" || tmux new-window -n "NAME" -d`
- Explicit carve-out: `new-session` is fine where tmux is *not* the workspace (e.g. daemonizing on a remote host over SSH with `$TMUX` empty — the same thread's remote `regression-report-canary` session on the CVM was legitimate).

No opensessions code change made: the sidebar mirroring real tmux state is correct behavior; masking stray sessions would hide real state.

## Remediation of the Live Stray Session (optional, not executed)

The `cvm` session may still be in use by the other thread (it targets `cvm:ssh`). If/when safe:

```bash
tmux kill-pane -t %537                                      # drop the sidebar pane from cvm:ssh
tmux move-window -s cvm:ssh -t regression-report-toolkit:   # nest window; empty cvm session dies
```

Caveat: any tooling addressing `cvm:ssh` breaks after the move (target becomes `regression-report-toolkit:ssh`).

## Files Changed

- `~/.agents/skills/tmux/SKILL.md` — added hard rule forbidding `new-session` inside tmux + idempotent window pattern (outside this repo).
- `ai_logs/19-stray-cvm-session-investigation.md` — this log.
