# Sidebar Active Input Gate - 2026-05-20

## Summary

Fixed an overcorrection from the send-keys guard where real keyboard shortcuts could stop working after selecting the sidebar. The TUI now trusts tmux's `pane_active` state before falling back to terminal focus-event tracking.

---

## Details

The previous guard marked a sidebar pane unfocused immediately after it programmatically refocused the main pane. That blocked stale `tmux send-keys` input, but tmux does not always deliver a focus-in sequence when a user later re-selects the sidebar. As a result, pressing `n` in the selected sidebar could be ignored.

The fix adds a direct `#{pane_active}` query for the TUI's own tmux pane. If tmux says the sidebar pane is active, shortcuts are accepted even if focus-event state is stale. If tmux says the pane is inactive, shortcuts are ignored. The old focus-event signal remains as a fallback when tmux active-state cannot be queried.

Regression coverage also explicitly verifies that the normal key handler still routes `n`/`c` to `createNewSession()` after the active-pane gate, so the guard cannot silently break new-session creation again.

---

## Files Changed

- `apps/tui/src/index.tsx` - Added `getLocalPaneActive()` and changed the keyboard gate to prefer tmux pane-active state.
- `packages/runtime/test/close-sidebar.test.ts` - Added regression coverage for the pane-active keyboard gate and the reachable `n`/`c` new-session shortcut path.

---

## Verification

```bash
bun test packages/runtime/test/close-sidebar.test.ts
cd apps/tui && bun run build
```
