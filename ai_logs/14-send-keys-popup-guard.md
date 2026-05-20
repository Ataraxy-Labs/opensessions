# Send-Keys Popup Guard - 2026-05-20

## Summary

Investigated a bug where an AI agent's `tmux send-keys` command could open the opensessions new-session popup. Hardened the TUI focus guard and added printable-burst suppression so injected shell command text does not execute single-letter sidebar shortcuts.

---

## Details

### Root Cause

`tmux send-keys -t session:window ...` sends input to the active pane in that target window. If the active pane is an opensessions sidebar, the injected command text is parsed as TUI key input. Printable characters in shell commands can include shortcuts such as `n` or `c`, which open the new-session popup.

The TUI already tracked terminal focus events to reject input when the sidebar is not focused, but it defaulted to focused until a focus-out escape sequence was observed. If that sequence was delayed or missed after the TUI selected the main pane, the sidebar could remain logically focused and accept injected keys.

There is also an unavoidable tmux edge: if a sidebar pane is the target window's active pane, injected text is indistinguishable from keyboard input at the TUI layer. The command `cd ...` starts with `c`, which was an alias for "create new session", so the popup could open before later characters arrived.

### Fix

`refocusMainPane()` now returns whether it successfully moved focus away from the sidebar. Startup refocus immediately calls `setPaneHasTerminalFocus(false)` on success, so the TUI does not wait for focus-out bytes before ignoring shortcut input.

High-impact printable shortcuts (`c`, `n`, `d`, `x`, `q`, `r`, `t`, `u`, `f`, and digits) are now delayed very briefly. If another printable character arrives during that delay, opensessions treats the input as a burst of injected text and suppresses shortcut handling until the burst goes quiet. A normal single user keypress still executes after the tiny delay.

---

## Files Changed

- `apps/tui/src/index.tsx` - Make `refocusMainPane()` return success, immediately clear sidebar focus state after successful startup refocus, and suppress rapid printable bursts before they execute single-letter shortcuts.
- `packages/runtime/test/close-sidebar.test.ts` - Add regression coverage for immediate unfocused state after startup refocus and printable-burst shortcut suppression.

---

## Verification

```bash
bun test packages/runtime/test/close-sidebar.test.ts
cd apps/tui && bun run build
```
