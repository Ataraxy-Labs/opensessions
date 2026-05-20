# Send-Keys Popup Guard - 2026-05-20

## Summary

Investigated a bug where an AI agent's `tmux send-keys` command could open the opensessions new-session popup. Hardened the TUI focus guard so sidebar panes mark themselves unfocused immediately after they programmatically refocus the main pane.

---

## Details

### Root Cause

`tmux send-keys -t session:window ...` sends input to the active pane in that target window. If the active pane is an opensessions sidebar, the injected command text is parsed as TUI key input. Printable characters in shell commands can include shortcuts such as `n` or `c`, which open the new-session popup.

The TUI already tracked terminal focus events to reject input when the sidebar is not focused, but it defaulted to focused until a focus-out escape sequence was observed. If that sequence was delayed or missed after the TUI selected the main pane, the sidebar could remain logically focused and accept injected keys.

### Fix

`refocusMainPane()` now returns whether it successfully moved focus away from the sidebar. Startup refocus immediately calls `setPaneHasTerminalFocus(false)` on success, so the TUI does not wait for focus-out bytes before ignoring shortcut input.

---

## Files Changed

- `apps/tui/src/index.tsx` - Make `refocusMainPane()` return success and immediately clear sidebar focus state after successful startup refocus.
- `packages/runtime/test/close-sidebar.test.ts` - Add regression coverage for immediate unfocused state after startup refocus.

---

## Verification

```bash
bun test packages/runtime/test/close-sidebar.test.ts
cd apps/tui && bun run build
```
