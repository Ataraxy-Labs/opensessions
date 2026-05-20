# Sidebar Focus Restore - 2026-05-18

## Summary

Fixed two tmux sidebar focus regressions: switching sessions from the sidebar could leave tmux focus on the sidebar pane, and `prefix o s` did not revive a killed/missing sidebar when global sidebar state was still visible.

---

## Details

### Session switch focus

Tmux preserves the target session/window's active pane across `switch-client`. If the sidebar pane had become active in the target session, selecting that session from opensessions left the sidebar as tmux's active pane, making normal tmux commands like `prefix x` operate on the sidebar.

`TmuxProvider.switchSession()` now selects a non-sidebar pane in the target session's active window immediately after switching. It is conservative: if the active pane is already not the sidebar, it leaves focus alone; if only a sidebar pane exists, it does nothing.

### Sidebar revive shortcut

`integrations/tmux-plugin/scripts/focus.sh` used `/toggle` when the current window did not have a sidebar pane. That works only when the global sidebar is hidden. If the sidebar pane was manually killed while global state remained visible, `/toggle` hid the sidebar state instead of recreating the missing pane.

The fallback now posts `/ensure-sidebar`, waits for the pane to appear, and then focuses it.

---

## Files Changed

- `packages/mux/providers/tmux/src/provider.ts` - Selects a non-sidebar pane after tmux session switches.
- `integrations/tmux-plugin/scripts/focus.sh` - Uses `/ensure-sidebar` instead of `/toggle` to restore a missing sidebar.
- `packages/runtime/test/close-sidebar.test.ts` - Adds regression checks for session-switch focus and focus-shortcut restore behavior.

---

## Commands Reference

```bash
bun test packages/runtime/test/close-sidebar.test.ts
cd packages/runtime && bun test
```

---

## Follow-up: Reveal vs Restore

After the first fix, `prefix o s` restored a missing sidebar only when global sidebar state was already visible. It regressed the normal hidden-sidebar reveal path because `/ensure-sidebar` intentionally no-ops while hidden for hook safety.

Updated the focus shortcut to call `/ensure-sidebar?reveal=1`. The server treats that explicit reveal flag as user intent: if the sidebar is hidden it toggles the sidebar on; if it is already visible, it restores/ensures the current window sidebar without hiding global state.
