# Session Switch Identity Fix & Sessionizer Path Fallback — 2026-04-27

## Summary

Fixed a critical bug where session switching in the sidebar clobbered every TUI instance's session identity, causing the sidebar to lose track of which session each pane belongs to. Also fixed the sessionizer fzf picker to honor typed paths as a fallback.

---

## Bug 1: Session Identity Clobbering on Switch

### Symptoms
- After switching sessions in the sidebar, the TUI would show the wrong "current" session
- Creating a new session and trying to rename it (Ctrl-b $) would rename the old session instead
- `getCurrentSession` would return stale results after switches
- Ghost sessions (e.g., "Music", "go") would appear in the sidebar from previously deleted sessions

### Root Cause
`syncClientSessionsForTty()` was called on every session switch (both from the WS `switch-session` command and the HTTP `/focus` hook). This function iterated ALL connected TUI WebSocket clients matching the same `clientTty` and sent `your-session` to each one — which set **both** `mySession` and `currentSession` on every TUI to the switched-to session.

Since all sidebar TUI panes share the same terminal client TTY (`/dev/ttys004`), every sidebar instance had its `mySession` overwritten on every switch. `mySession` is supposed to be immutable per TUI — it represents which tmux session that sidebar pane lives in.

### Debug Evidence
Added logging to `sendYourSession` and `syncClientSessionsForTty`:
```
[sendYourSession] sent {"prev":"caffeine","sessionName":"apartment","clientTty":"/dev/ttys004"}
[sendYourSession] sent {"prev":"apartment","sessionName":"apartment","clientTty":"/dev/ttys004"}
[sendYourSession] sent {"prev":"coffee","sessionName":"apartment","clientTty":"/dev/ttys004"}
...
[syncClientSessionsForTty] done {"clientTty":"/dev/ttys004","sessionName":"apartment","connectedClients":6,"matched":6}
```
All 6 TUI clients were being told they belong to "apartment" after a single switch.

### Fix
Replaced `syncClientSessionsForTty()` with `syncTtyMapping()` — a function that only updates the server-side `clientTtyBySession` map without sending `your-session` to TUI clients. Session switches now update `currentSession` exclusively through the existing `broadcastFocusOnly()` / `broadcastState()` path, which correctly sends `focus` messages (not `your-session`).

**Key distinction:**
- `your-session` → sets `mySession` (TUI's own session identity) — sent once at startup
- `focus` → sets `currentSession` (which session the user is viewing) — sent on every switch

---

## Bug 2: Sessionizer Picks Wrong Directory

### Symptoms
Typing `/Users/wporter` in the fzf sessionizer popup and pressing Enter would create a session in whatever directory was the top fuzzy match (e.g., `/Users/wporter/go`), not the typed path.

### Root Cause
fzf selects from the **list** on Enter, not the typed query text. Without `--print-query`, there was no way to use a typed path as a literal directory.

### Fix
Added `--print-query` to the fzf invocation. The output now has two lines: line 1 = query, line 2 = selected match. Logic:
1. If a match was selected → use it (normal fzf behavior)
2. Else if the typed query is a valid directory → use it as the path
3. Else → exit (no selection)

Also added `-not -path '*/.*'` to filter hidden directories from the find results.

---

## Other Changes in This Commit (Pre-existing Uncommitted Work)

These were already in the working tree before this session:

- **Auth token**: Server generates a random token on startup, writes to `TOKEN_FILE`, validates on all requests
- **Simplified sidebar toggle**: Removed staggered spawn tiers, initializing state, debounced width enforcement
- **Agent watcher refresh**: `handleFocus()` now calls `watcher.refresh()` if available
- **Pane-exited safety**: Spawns a shell pane if sidebar is the last pane in a window
- **Git info**: Split combined shell command into separate `git rev-parse` and `git status` calls
- **Report-width drift filtering**: Ignore ≤2 column rounding, reject >20 column jumps

---

## Files Changed

- `packages/runtime/src/server/index.ts` — Core fix: `syncClientSessionsForTty` → `syncTtyMapping`, debug logging, auth, simplified resize/toggle
- `apps/tui/scripts/sessionizer.sh` — `--print-query` fallback, hidden dir filtering
- `apps/tui/src/index.tsx` — TUI cleanup (removed initializing state references)
- `integrations/tmux-plugin/scripts/*.sh` — Auth token header in hook curl calls
- `packages/mux/providers/tmux/src/provider.ts` — Provider simplifications
- `packages/runtime/src/agents/watchers/amp.ts` — Watcher refresh support
- `packages/runtime/src/contracts/agent-watcher.ts` — Optional `refresh()` method
- `packages/runtime/src/index.ts` — Export additions
- `packages/runtime/src/shared.ts` — `TOKEN_FILE` constant

---

## Commands Reference

```bash
# Restart the server after changes
pkill -f "bun run.*apps/server/src/main.ts"
cd ~/.tmux/plugins/opensessions && bun run apps/server/src/main.ts

# Check server debug log
tail -f /tmp/opensessions-debug.log

# Test session switch via hook endpoint
TOKEN=$(cat /tmp/opensessions.token)
TTY=$(tmux display-message -p '#{client_tty}')
curl -H "x-opensessions-token: $TOKEN" -X POST http://127.0.0.1:7391/focus -d "${TTY}|<session>|@0"
```
