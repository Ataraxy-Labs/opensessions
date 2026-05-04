# Stale Sidebar Pane Cleanup After tmux-continuum Restore - 2026-05-04

## Summary

Fixed a regression where the opensessions sidebar would silently fail to spawn
in any session after a system restart. tmux-continuum restored the saved tmux
layout — including the sidebar panes' titles — but launched plain shells
instead of the bun TUI. The sidebar-detection logic matched on title alone, so
every window appeared to already have a sidebar and `ensure-sidebar` was
skipped. Switching to `locreq` (or any restored session) showed a `tui` zsh
prompt where the sidebar should have been.

---

## Root Cause

`TmuxProvider.listSidebarPanes` filtered panes purely by `pane_title ===
"opensessions-sidebar"`. After a reboot, tmux-continuum recreated panes from
its persisted state with the saved title preserved, but spawned them as the
default shell rather than re-running the original `start.sh exec bun ...`
command. Effects:

1. `listSidebarPanes` returned 8 zombie zsh panes plus 1 real bun pane.
2. `ensureSidebarInWindow` saw `hasInWindow = true` for every window and
   skipped spawning.
3. Each session window showed a stale shell where the sidebar should have been.

The previous fix that added `ensureCmd` to the `client-session-changed` hook
worked correctly — `/ensure-sidebar` was being called, but the title-only
detection prevented the spawn.

---

## Fix

### Detect stale sidebar panes by command

Real sidebar panes always run the `bun` process (start.sh `exec`s bun). A
sidebar-titled pane whose `pane_current_command` is a plain shell (`zsh`,
`bash`, `fish`, `sh`, `ksh`, `dash`) is a tmux-resurrect/continuum zombie.

- `TmuxProvider.listSidebarPanes` now filters out panes whose command is in
  the shell set, so the rest of the server treats them as nonexistent.
- New `TmuxProvider.killStaleSidebarPanes()` actively kills any
  sidebar-titled pane whose command is a plain shell.
- `ZellijProvider.killStaleSidebarPanes()` is a no-op (Zellij has no
  equivalent restore behavior).

### Run cleanup at server bootstrap

`packages/runtime/src/server/index.ts` now calls
`killStaleSidebarPanes()` once during bootstrap, after `setupHooks` and
**before** `reconcileSidebarPresence`, so the visibility check sees an
accurate count and missing sidebars get spawned automatically.

### Contract & tests

- Added `killStaleSidebarPanes()` to the `SidebarCapable` interface and the
  `isSidebarCapable` type guard.
- Updated `mux-contract.test.ts` mocks to include the new method.
- All 320 runtime tests pass.

### Live cleanup

For the user's currently running server (which still ran old code), the stale
panes were killed manually with `tmux kill-pane`, then `/ensure-sidebar` was
pinged for each session (with delays to avoid the 150ms debouncer collapsing
calls). Result: every session now has a fresh `bun`-running sidebar pane.

---

## Files Changed

- `packages/mux/contract/src/types.ts` — added `killStaleSidebarPanes()` to
  `SidebarCapable` interface and `isSidebarCapable` type guard.
- `packages/mux/providers/tmux/src/provider.ts` — added `SHELL_COMMANDS` set,
  filtered `listSidebarPanes` by command, added `killStaleSidebarPanes()`
  method.
- `packages/mux/providers/zellij/src/provider.ts` — added no-op
  `killStaleSidebarPanes()`.
- `packages/runtime/src/server/index.ts` — bootstrap now invokes
  `killStaleSidebarPanes()` for each sidebar-capable provider before
  reconciling sidebar presence.
- `packages/runtime/test/mux-contract.test.ts` — updated full-provider mocks
  to include the new method.

---

## Commands Reference

```bash
# Identify stale sidebar panes (sidebar-titled, plain shell command)
tmux list-panes -a -F '#{pane_id}|#{pane_title}|#{pane_current_command}|#{session_name}' \
  | grep opensessions-sidebar | grep -v _os_stash

# Manually trigger sidebar spawn (with token auth) for a single session
TOKEN=$(cat /tmp/opensessions.token)
WID=$(tmux list-windows -t SESSION -F '#{window_id}' | head -1)
curl -s -X POST -H "x-opensessions-token: $TOKEN" \
  "http://127.0.0.1:7391/ensure-sidebar" \
  -d "/dev/ttys001|SESSION|${WID}"

# Run runtime tests
cd packages/runtime && bun test
```
