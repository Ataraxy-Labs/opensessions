# Scope quitAll to this server instance - 2026-05-06

## Summary

Fixed a cross-instance blast-radius bug in the opensessions server: when one
opensessions server shut down (WS quit, HTTP `/quit`, idle timeout, SIGINT,
SIGTERM) it would kill **every** sidebar pane on the entire tmux server and
unset every global tmux hook — wiping out unrelated, still-live opensessions
servers running on different sockets/ports.

Branch: `wporter/scope-quitall` off canonical `main` (53c23bd).
Worktree: `/Users/wporter/repos/github/opensessions-quitall-scope`.

---

## Root cause

The runtime intentionally allows multiple opensessions servers to coexist on
one machine: `resolveServerKey` / `resolveServerPort` / `resolvePidFile` in
[`packages/runtime/src/shared.ts`][shared] hash the `$TMUX` socket path to
derive a per-tmux-socket port (`17000 + hash`) and PID file
(`/tmp/opensessions.<key>.pid`). When `$TMUX` is unset, port defaults to 7391
and PID file to `/tmp/opensessions.pid`.

But `quitAll()` in [`packages/runtime/src/server/index.ts`][server] enumerated
sidebar panes via `provider.listSidebarPanes()`, which on the tmux provider
calls `tmux list-panes -a` — a tmux-server-wide query filtered only by
`pane_title == "opensessions-sidebar"`. There was no per-server-instance
ownership tracking, so quitAll killed any pane matching that title regardless
of which opensessions server spawned it.

Then `cleanup()` (called from quitAll, SIGINT, SIGTERM) ran
`for (const p of allProviders) p.cleanupHooks()`, which unset the **global**
tmux hooks (`client-session-changed`, `after-new-window`, `after-select-window`,
`client-resized`, `pane-exited`, `session-created`, `session-closed`) that any
other live opensessions server still depended on.

Empirical observation that triggered this fix: a stale orphan opensessions
server on port 7391 idle-timed-out after 30 s and its quitAll killed 16 sidebar
panes belonging to a sibling per-socket server on port 17000+19916, and tore
down the tmux hooks the sibling needed to keep working.

---

## Fix

Two scoping changes, plus a stale-PID-aware "am I the last live server?" check.

### 1. Track sidebar panes this server instance owns

Added `serverOwnedSidebarPanes: Set<string>` to the per-server closure in
`startServer()`. Updated in three places:

- `ensureSidebarInWindow` — when `provider.spawnSidebar()` returns a non-null
  paneId, add it to the set. This is the primary "we just created this pane"
  signal.
- `identify-pane` WS handler — when a TUI client identifies its paneId, add it
  to the set. This handles post-restart recovery (server crashed/restarted but
  TUI panes survived; they reconnect and we claim them) and is also forward-
  compatible with the `clientPaneIds` map being added on
  `wporter/harden-tui-input`.
- `/pane-exited` HTTP handler — after the existing orphaned-pane cleanup, prune
  pane ids from the set whose tmux pane no longer exists, so the set doesn't
  grow stale over the server's lifetime.

`toggleSidebar` (OFF path) and `quitAll` now intersect `listSidebarPanes()`
with `serverOwnedSidebarPanes` before calling `hideSidebar` /
`killSidebarPane`, so they only touch panes owned by this server. Foreign
panes are logged as `skippedForeignCount` and left alone.

### 2. Gate global tmux hook teardown on "last live instance"

New module [`packages/runtime/src/server/server-instance-scope.ts`][scope]
exports:

- `isProcessAlive(pid)` — uses `process.kill(pid, 0)`. Treats `EPERM` as alive
  (process exists but we can't signal it).
- `findOtherLiveOpensessionsPids({ ownPidFile, ownPid, pidDir?, isAlive?,
  readDir?, readFile? })` — scans `/tmp` (or `dirname(ownPidFile)`) for files
  matching `opensessions[.<key>].pid`, excludes our own PID file and our own
  PID, parses each, and returns PIDs that pass the liveness check.
- `isLastLiveOpensessionsInstance(opts)` — convenience wrapper.

`cleanup()` now wraps `for (const p of allProviders) p.cleanupHooks()` in a
check: only run it if no other live opensessions server is detected. Otherwise
log `"skipping provider.cleanupHooks — sibling opensessions server is still
live"` and leave the global hooks alone.

The scan is robust against stale PID files (the file may exist but the process
may be dead — `kill -0` confirms liveness on Darwin/Linux), garbage file
contents, missing directories, and self-references.

### Things deliberately NOT changed

- `cleanupSidebar()` — kills the legacy `_os_stash` tmux session. Stash session
  is shared but currently empty (hideSidebar kills panes outright now), so
  this is safe defensive cleanup.
- `killStaleSidebarPanes()` / `killOrphanedSidebarPanes()` on startup — these
  are stateless heuristic cleaners that target panes which should die
  regardless of ownership (plain shells with sidebar title from
  tmux-resurrect; sidebars that are the only pane in their window). Safe to
  run server-wide.
- `process.exit(0)` in quitAll — out of scope; only the blast radius before
  the exit was wrong.

---

## Tests

Added [`packages/runtime/test/server-instance-scope.test.ts`][test] covering
the new helpers: 12 tests for `isProcessAlive`, `findOtherLiveOpensessionsPids`,
and `isLastLiveOpensessionsInstance`. All pass.

Full runtime suite: 374 pass / 5 fail. The 5 failures are the pre-existing
`even-horizontal shell helpers` failures (being fixed in parallel on
`wporter/fix-even-horizontal-tests`) and unrelated to this change.

The pane-ownership tracking inside `startServer` is exercised through the
existing integration paths; isolating it for a unit test would require a
substantial extraction. Verified via the code paths only.

---

## Coordination with parallel work

- `wporter/harden-tui-input` adds a `clientPaneIds: WeakMap<ws, paneId>` for
  per-WS pane tracking. My `serverOwnedSidebarPanes` is a separate
  server-wide set with a longer lifetime (persists across WS reconnects and
  restarts via identify-pane recovery). They are complementary and rebase
  cleanly: the only shared touch point is the `identify-pane` handler, which
  both branches modify additively.
- `wporter/fix-even-horizontal-tests` and `wporter/sidebar-active-pane-fix`
  touch unrelated files.

---

## Files changed

- `packages/runtime/src/server/index.ts` — owned-set tracking + scoped quitAll
  + scoped toggle-OFF + last-instance-gated cleanupHooks + identify-pane
  claim + /pane-exited prune
- `packages/runtime/src/server/server-instance-scope.ts` — new module
- `packages/runtime/test/server-instance-scope.test.ts` — new tests

[shared]: ../packages/runtime/src/shared.ts
[server]: ../packages/runtime/src/server/index.ts
[scope]: ../packages/runtime/src/server/server-instance-scope.ts
[test]: ../packages/runtime/test/server-instance-scope.test.ts
