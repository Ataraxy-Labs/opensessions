# Sidebar Scrollback Isolation - 2026-06-05

## Summary

Investigated a tmux/sidebar visual bug where scrolling could expose stale content and make the opensessions sidebar appear partially hidden after agent/sidebar restarts. Added pane-scoped tmux reset/history cleanup for sidebar panes so stale OpenTUI output is not available to scroll/copy-mode.

---

## Details

### tmux/sidebar lifecycle diagnosis

- Confirmed the local tmux environment already had `mouse` and `alternate-screen` enabled, so the fix avoided changing global user tmux behavior.
- Chose a narrow pane-local mitigation: reset and clear only the opensessions sidebar pane, leaving the user's main Amp/shell pane history untouched.
- Preserved existing sidebar behavior invariants: no immediate server-side refocus, no global mouse option changes, no width-authority changes.

### Implementation

- Added `TmuxClient.resetPane()` using `tmux send-keys -R -t <pane>`.
- Added `TmuxClient.clearPaneHistory()` using `tmux clear-history -t <pane>`.
- Reset/cleared newly spawned sidebar panes after assigning the `opensessions-sidebar` pane title.
- Updated the TUI launcher to reset/clear its own `$TMUX_PANE` before OpenTUI starts and emit a clear-screen/clear-scrollback sequence.

---

## Files Changed

- `apps/tui/scripts/start.sh` - Clears only the current sidebar tmux pane before launching OpenTUI.
- `packages/mux/providers/tmux/src/client.ts` - Adds pane reset/history helpers for the tmux provider-local client.
- `packages/mux/providers/tmux/src/provider.ts` - Applies pane reset/history cleanup after spawning a sidebar.
- `packages/mux/tmux-sdk/src/index.ts` - Adds the same pane reset/history helpers to the shared tmux SDK.
- `packages/mux/tmux-sdk/test/tmux-client.test.ts` - Covers the new pane-scoped tmux commands.
- `packages/runtime/test/close-sidebar.test.ts` - Guards the sidebar spawn and launcher scrollback-isolation wiring.

---

## Verification

```bash
bun test packages/mux/tmux-sdk/test/tmux-client.test.ts
cd packages/runtime && bun test test/close-sidebar.test.ts
cd packages/runtime && bun test
cd packages/mux/tmux-sdk && bun test
git diff --check
bun run build
```

All checks passed.
