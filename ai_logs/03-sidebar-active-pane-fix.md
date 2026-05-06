# Fix sidebar TUI pane sometimes becoming the active pane - 2026-05-05

## Summary

Fixed a race in `refocusMainPane()` (apps/tui/src/index.tsx) where the
sidebar TUI pane could end up as the active pane in its window, causing
`tmux send-keys -t SESSION:WINDOW` (without an explicit `.PANE`) to land
in the sidebar instead of the user's intended target.

## Root cause

`refocusMainPane()` identified the "main" (non-sidebar) pane by listing
panes with `#{pane_id} #{pane_title}` and excluding lines containing
`opensessions-sidebar`. This racing pattern fails on fresh sidebar
spawns when the sidebar is on the **left**:

1. Server spawns the new sidebar pane via `tmux split-window -bh`. The
   new pane is on the left and gets a lower `pane_index`.
2. Server calls `tmux.setPaneTitle(newPane.id, "opensessions-sidebar")`.
3. Concurrently, the freshly-launched TUI runs `refocusMainPane()`,
   listing panes for the window.
4. If the TUI's `list-panes` lands before the server's `setPaneTitle`
   reaches tmux, the new sidebar pane has no title yet, so it doesn't
   match `"opensessions-sidebar"` — and being first in pane_index order,
   `find((l) => !l.includes("opensessions-sidebar"))` returns *the
   sidebar itself*. The TUI then `select-pane`s the sidebar as "main",
   leaving the sidebar pane active.

## Fix

Identify "main" as any pane in the window whose `pane_id` is NOT the
TUI's own `muxCtx.paneId`. The TUI always knows its own pane id (it's
exported as the env var the launcher captures), so this is race-free.

Also added structured `logResizeDebug()` traces around each branch of
`refocusMainPane()` so future regressions surface in the resize debug
log: `no-window-id`, `tmux` (with windowId/selfPaneId/paneIds/main),
`select-failed`, `error`.

## Files changed

- `apps/tui/src/index.tsx` — replace title-based main-pane detection
  with self-pane-id exclusion in `refocusMainPane()`; add debug
  logging around each branch.

## Verification

Manual repro before fix: spawn sidebar on left in a fresh window;
sidebar starts as the active pane in ~1 of 3 attempts.
After fix: sidebar never becomes the active pane on spawn.
