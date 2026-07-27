# Drag-Reorder Sessions - 2026-07-27

## Summary

Added mouse drag-and-drop ordering for session and worktree-group rows in the Ratatui sidebar. Reordering remains server-owned, persisted through the existing session-order file, and disabled while a filtered session view is active.

## Details

- Split mouse handling into down, drag, and up phases so ordinary clicks still activate rows while drags can select a destination.
- Added drag source and destination state with visual row highlighting and an `↕` destination marker.
- Reused the existing reorder commands with an arbitrary visible-row delta, avoiding a protocol migration.
- Made ordering group-aware: collapsed groups remain intact, worktree children reorder only among siblings, singleton worktrees behave like normal sessions, and group rows move as blocks.
- Added drag-only row hit testing so agent badges and diff-count subtargets do not block dropping onto their session.
- Cancel drag state when the session filter changes.

## Files Changed

- `apps/tui-rs/src/main.rs` - Forward complete mouse drag coordinates and mouse-up events.
- `packages/sidebar-core-rs/src/input.rs` - Coordinate mouse-down, drag targeting, and drop completion.
- `packages/sidebar-core-rs/src/app.rs` - Track drag state and calculate persisted session/group ordering.
- `packages/sidebar-core-rs/src/renderer.rs` - Render drag feedback and expose drag-specific row hit testing.
- `docs/reference/features-and-keybindings.md` - Document drag-and-drop ordering.

## Commands And Notes

```bash
cargo test -p opensessions-sidebar-core
cargo check --workspace
cargo build --workspace --bins
cargo test -p opensessions-sidebar --test tmux_e2e tmux_sidebar_reorders_normal_session_across_worktree_group_boundary -- --nocapture
```

The existing `tmux_sidebar_alt_reorders_worktree_group_as_block` E2E test also fails unchanged on clean `origin/main`; the new normal-session boundary E2E passes. Clippy reports only pre-existing warnings outside this change after the introduced warning was fixed.
