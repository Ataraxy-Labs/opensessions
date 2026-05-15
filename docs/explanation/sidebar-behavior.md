# Sidebar Behavior Invariants

This document captures the sidebar behaviors that were learned the hard way while making tmux sidebar resizing feel native, stable, and predictable.

If you change sidebar spawning, width sync, tmux hook handling, focus/session switching, or the `sidebar-coordinator` state machine, read this first.

## The Product Contract

The sidebar should behave like a real sidebar, not like an ordinary tmux pane.

That means:

- the sidebar width stays fixed until the user explicitly drags it
- the width propagates globally across every sidebar pane in every managed session/window
- full terminal resizes do not redefine the saved width
- session switching does not cause the sidebar to jump, breathe, or re-proportion itself
- background windows should already be correct before the user lands in them
- the UI should clearly show `warming up…` while sidebars are still spawning and `adjusting…` while width normalization is still in flight

## Width Authority

Only true user intent should persist a new width.

The accepted rule set is:

- only the foreground sidebar in the active session can author a new width
- background sidebars never get to redefine global width
- a user drag may continue for a short tail window even if focus moves immediately after the drag starts
- programmatic tmux resizes, session switches, and terminal resizes must be treated as echoes unless we have evidence of real user drag intent
- when a switch happens too quickly for the TUI to emit `report-width`, the server must opportunistically adopt the source window's actual sidebar pane width before switching

In practice, width authority is split into these cases:

- `user-drag`: a real user-driven sidebar resize
- `client-resize-sync`: the server correcting widths after a whole terminal/client resize
- `programmatic-adjust`: the server normalizing widths during ensure/switch/fan-out paths
- `none`: no resize authority is active

## Global Propagation Rules

When a width change is accepted:

- the persisted width changes once
- the server fans that width out to every other sidebar pane
- the source pane/window should not be fought by the fan-out pass
- rapid switching must not cut propagation short

This was a real bug: a drag could be accepted in one pane, then a fast switch would happen before later reports arrived, and the destination session would snap everything back to the old stored width. The current rule is to capture the source sidebar pane width during switch handoff so explicit user resizing is not lost.

## Terminal Resize Rules

External terminal resizing is not the same thing as sidebar resizing.

Expected behavior:

- moving between monitor sizes or resizing Ghostty/iTerm should not change the saved sidebar width
- the foreground window should be corrected quickly so the sidebar does not visually breathe
- background windows can catch up with a staggered sync pass after a short settle delay
- transient half-window widths reported during client resizes must never become the persisted width

The server therefore needs both:

- a suppression window to ignore server-induced resize echoes
- a client-resize guard window so transient widths during full terminal resize do not get mistaken for user drag

## Session Switching Rules

Session switching should feel boring.

That means:

- the destination session/window should already have a sidebar at the current global width
- switching must not trigger visible layout jumps
- switching must not reset the width to an older value
- if the user resized immediately before switching, the just-resized width must survive the switch

One specific regression we already paid for: forcing `resize-window` during the session-switch path caused visible layout jumps. The fix was to stop doing that in the switch path and instead use targeted width enforcement plus background pre-layout where appropriate.

## Warmup And Adjusting Semantics

There are two user-visible initializing states and they mean different things.

`warming up…` means:

- sidebars are being spawned/restored across windows
- the system is still converging on presence, not width

`adjusting…` means:

- width normalization is still in flight across windows/sessions
- this includes whole-client resize sync, accepted drag propagation, and server-driven cross-window enforcement

Important nuance:

- if warmup and a global adjustment overlap, the UI should prefer `adjusting…`
- warmup must not get stranded forever because a resize sync canceled the only completion timer

## tmux-Specific Invariants

tmux has several behaviors that look reasonable until they break the sidebar.

These are non-negotiable:

- ignore control-mode clients with empty `client_tty` when inferring current session or foreground client
- keep tmux windows in `window-size latest`; do not leave them in manual mode after `resize-window`
- do not use `after-resize-pane` as width authority for this feature; it was tried before and led to sync difficulty and resize loops
- do not refocus the main pane immediately after sidebar spawn/restore; let the TUI refocus after capability detection settles so escape sequences do not leak into the main pane
- invalidate cached sidebar pane listings before logic that depends on just-spawned or just-hidden panes
- topology refresh hooks such as `session-created` should refresh state only; they must not fan out sidebar spawning into every new background session/window created by unrelated tmux automation

## Regressions We Already Paid For

These are the big historical failure modes worth remembering.

### 1. Jamming tmux's resizer

What happened:

- the server tried to do too much synchronous resize work during tmux resize storms
- or it got stuck in echo/enforcement loops where programmatic resizes caused more programmatic resizes

What fixed it:

- deferred client-resize sync after a short settle window
- fast staggered fan-out for background windows
- ignore-only suppression rather than recursive re-enforcement loops
- explicit re-entrancy guards around enforcement passes

### 2. Treating external width changes as user intent

What happened:

- full terminal resizes and other layout churn produced `report-width` values that looked like drags
- the saved width changed even though the user never dragged the divider

What fixed it:

- only the foreground active sidebar can author width
- client-resize guard windows reject transient reports during full terminal resize
- the state machine models causality so programmatic adjustments and user drags are not conflated

### 3. Switching quickly stopped propagation

What happened:

- the initial width report could be accepted
- later reports in the same drag were suppressed or never arrived before a switch
- the destination session then re-enforced the older persisted width

What fixed it:

- longer drag settle windows so drag authority survives realistic report timing
- drag-tail acceptance for the originating pane even after focus changes
- source-window width adoption during switch handoff so the latest real pane width is not lost if the TUI report races with the switch

### 4. Background windows were stale

What happened:

- only the active session/window got corrected promptly
- switching into a background window revealed a stale sidebar width flash

What fixed it:

- pre-layout plus staggered background correction
- global fan-out that includes sibling windows, not just other sessions

### 5. Manual window-size mode poisoned layouts

What happened:

- `resize-window` left tmux windows in `window-size manual`
- later terminal behavior looked padded or broken

What fixed it:

- always restore `window-size latest` after forced window resizes

## Rejected Approaches

These are not theoretical. They were tried and caused problems.

- using `after-resize-pane` as the main width-authority mechanism
- forcing `resize-window` directly in the normal session-switch path
- suppressing width reports so broadly that legitimate drag events got blocked
- setting drag suppression in a way that made the server fight the user's live drag
- treating every TUI as authoritative instead of only the current foreground one

## Performance Constraints

The sidebar should not make tmux feel heavy.

Keep these constraints in mind:

- stagger expensive cross-window work instead of doing it all inline in hooks
- avoid repeated full `list-panes -a` scans inside the same resize cycle
- batch where possible, cache briefly, and invalidate on real topology changes
- prioritize the active window first, then let the rest catch up quickly in the background

## Change Checklist

Before shipping any sidebar behavior change, verify all of these.

- dragging the active sidebar changes width smoothly
- switching sessions immediately after a drag preserves the new width
- resizing the whole terminal does not redefine the persisted width
- background windows land at the current width without visible proportional flash
- `warming up…` clears once spawn/restore is complete
- `adjusting…` appears reliably while global width correction is still happening
- control-mode clients cannot steal foreground/current-session authority
- tmux windows remain in `window-size latest`
- no resize or enforcement loop appears in `/tmp/opensessions-debug.log`

## Files To Read Before Changing This Area

- `packages/runtime/src/server/sidebar-coordinator.ts`
- `packages/runtime/src/server/index.ts`
- `packages/mux/providers/tmux/src/provider.ts`
- `packages/mux/tmux-sdk/src/index.ts`
- `apps/tui/src/index.tsx`
- `packages/runtime/test/sidebar-coordinator.test.ts`

If a future change violates this doc but seems necessary, update the doc in the same change and explain the new invariant explicitly.
