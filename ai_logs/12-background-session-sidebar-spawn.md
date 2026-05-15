# Background Session Sidebar Spawn Fix - 2026-05-15

## Summary

Fixed a tmux hook regression where unrelated background tmux automation could cause opensessions to spawn sidebar panes in newly created detached sessions/windows.

---

## Details

### Root Cause

The tmux `session-created` hook posts to the server's `/refresh` endpoint. That endpoint rebroadcasted state and also queued `ensureSidebar` across every active window in every session. When another tool ran `tmux new-session -d ...`, the new detached session appeared in that all-window sweep and received an opensessions sidebar pane even though the user had not navigated there.

### Fix

Changed `/refresh` to only broadcast updated state. Explicit foreground/user-intent paths still own sidebar restoration:

- `/ensure-sidebar` handles focused window/session changes.
- The sessionizer still posts `/refresh` and then targeted `/ensure-sidebar` after switching.
- Toggling the sidebar on still intentionally warms/restores sidebars across managed windows.

Documented the invariant that topology refresh hooks must not fan out sidebar spawning into background-created sessions/windows.

---

## Files Changed

- `packages/runtime/src/server/index.ts` - Removed all-window sidebar ensure from `/refresh`.
- `packages/runtime/test/close-sidebar.test.ts` - Added a regression test that `/refresh` only broadcasts state and does not queue sidebar spawns.
- `docs/explanation/sidebar-behavior.md` - Documented the tmux hook invariant for topology refreshes.

---

## Verification

```bash
bun test packages/runtime/test/close-sidebar.test.ts
bun test packages/runtime/test
```
