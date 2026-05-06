# Port sessionizer typed-path fallback + hidden-dir filter from TPM install - 2026-05-06

## Summary

Ports two small UX improvements to `apps/tui/scripts/sessionizer.sh` from the
diverged TPM install (`~/.tmux/plugins/opensessions`, commit `3b9c451` on
`origin/Ataraxy-Labs`). After this change, typing a directory path in the fzf
prompt is honored as a fallback when no list match is selected, and hidden
directories (anything starting with `.`) are excluded from the candidate list.

This is the first ported piece from a larger TPM-divergence salvage effort —
see "Investigation findings" below for what was *not* ported and why.

---

## Sessionizer changes (this PR)

Two narrow changes to the `fzf` invocation:

1. **`-not -path '*/.*'`** added to `find` so candidate list skips dotted
   directories (`.git`, `.cache`, `.config`, etc.). Cuts noise on most
   `$HOME`-rooted searches.
2. **`--print-query`** added to `fzf`, plus shell logic to read fzf's two-line
   output (line 1 = typed query, line 2 = selected match). When the user types
   an absolute path that exists, but doesn't pick from the list, the typed
   query is honored as the directory.

Header text updated from "Pick a directory for new session" to "Pick a
directory (or type a path and press Enter)" so the new affordance is
discoverable.

Preserves canonical's `MAXDEPTH` env/tmux config and colon-separated
`SESSIONIZER_DIR` parsing untouched.

---

## Investigation findings (everything else from TPM `3b9c451`)

The TPM install at `~/.tmux/plugins/opensessions` was on a diverged lineage
(`origin/Ataraxy-Labs/main` at `0.1.0-alpha.26` + 2 local commits) before we
repointed it at `wessport/opensessions`. The diverged commits contained 7
distinct improvements — only one (this sessionizer fix) was easily portable.

The other six are documented here for follow-up decisions. Both backup branches
exist locally inside `~/.tmux/plugins/opensessions`:
`wp-pre-rebase-backup` (HEAD before reset) and `wp-pre-rebase-uncommitted`
(WIP that was sitting in the worktree, mostly duplicating already-merged
stale-sidebar-pane-cleanup work).

### 1. Auth token system — not ported, needs design decision

Server generates 32-byte random token at startup, writes to
`/tmp/opensessions.token` with mode `0o600`, validates `?token=` query param
or `x-opensessions-token` header on every HTTP request except `GET /`
(liveness). Tmux hooks (provider.ts) read the token from the file and include
it in every curl.

**Why not ported now:** canonical main has external integration callers that
the TPM commit predates:
  - `integrations/amp/opensessions.ts` (POSTs to `/api/agent-event`)
  - `integrations/pi-extension/opensessions-runtime.ts` (POSTs to
    `/api/runtime/pi/upsert` + `/api/runtime/pi/delete`)

Adding mandatory auth to all endpoints would break those integrations until
they're updated to also read `/tmp/opensessions.token`. Open question: do the
agent / pi runtime endpoints stay open (defeats the security purpose for those
attack surfaces) or do they require auth (every external integration must be
updated)?

Recommended scoping for a follow-up PR:
  - Phase 1: introduce the token + validate on tmux hook endpoints (`/focus`,
    `/toggle`, `/quit`, `/switch-index`, `/ensure-sidebar`, `/pane-exited`,
    `/client-resized`, `/suppress-width-reports`, `/refresh`) and on the WS
    upgrade. Leave `/api/*` and `/notify` etc. unauthenticated for now.
  - Phase 2: update `integrations/amp/opensessions.ts` and
    `integrations/pi-extension/opensessions-runtime.ts` to read the token.
  - Phase 3: extend auth to `/api/*` and the rest.

### 2. Pane-exited last-pane behavior — needs decision, NOT ported

TPM: when the sidebar is the only pane left in a window after pane-exited,
spawn a new shell pane (`tmux split-window -h -b`) so the session doesn't
become a dead sidebar-only state.

Canonical: `killOrphanedSidebarPanes()` in the tmux provider *kills* the
lonely sidebar instead, deliberately collapsing the empty window.

These are opposite strategies. Owner call needed before porting either way.

### 3. Session identity clobbering fix (`syncClientSessionsForTty` →
`syncTtyMapping`) — bug may or may not still exist on canonical, NOT ported

TPM commit replaces `syncClientSessionsForTty` with a narrower
`syncTtyMapping` that updates the TTY→session map but *doesn't* broadcast
`your-session` to all TUI instances. The bug it fixed: every sidebar's
`mySession` was being overwritten to the just-switched-to session, so all
sidebars showed the same identity.

Canonical still has `syncClientSessionsForTty` (server/index.ts:610). Need
empirical reproduction on canonical to confirm the bug is still present
before porting.

### 4. Agent watcher `refresh()` on focus — NOT ported

TPM adds an optional `refresh()` method on `AgentWatcher` that
`handleFocus()` calls so watchers can re-scan on session switch. Tangled with
a near-total rewrite of `packages/runtime/src/agents/watchers/amp.ts` (TPM's
amp.ts has WS subscriptions to `production.ampworkers.com`, tool-boundary
state machine, etc. — none of which is in canonical). Can't extract the
`refresh()` plumbing cleanly without dragging the rewrite in.

### 5. Sidebar toggle/resize simplification — NOT ported (out of scope)

Removes staggered spawn tiers, `initializing` state machine, debounced width
enforcement. Canonical has since formalized the opposite direction (added
`sidebar-coordinator` xstate machine, made resize pipeline more elaborate
not less). Conflicts heavily with current architecture; not a port-back
candidate.

### 6. Misc cleanups (split git info, remove unused state, report-width
drift filtering) — NOT ported individually, low value in isolation

---

## Files changed in this PR

- `apps/tui/scripts/sessionizer.sh` — `--print-query` typed-path fallback +
  hidden-dir filter.

## Verification

Manual: run `apps/tui/scripts/sessionizer.sh` with no `SESSIONIZER_DIR` set.

- Type a partial dir name → fzf narrows the list as before.
- Press Enter on a list match → that dir is used (unchanged).
- Type `/Users/wporter/Downloads` (absolute path that exists, no list match)
  and press Enter → the typed path is used.
- Type a nonexistent path and press Enter → exit 0 (no session created).
- Hidden dirs (`.config`, `.git`, etc.) no longer appear in the candidate
  list.

Tests: no new tests; existing test suite still 374 pass / 5 pre-existing
even-horizontal failures (unaffected by a shell-script change).
