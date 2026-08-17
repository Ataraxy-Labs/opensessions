# Sidebar Behavior Invariants

This document captures the sidebar behaviors that were learned the hard way while making tmux sidebar resizing feel native, stable, and predictable.

If you change sidebar spawning, width sync, tmux hook handling, focus/session switching, or the `sidebar-coordinator` state machine, read this first.

For current automated coverage, see [`sidebar-behavior-test-matrix.md`](./sidebar-behavior-test-matrix.md).

## The Product Contract

The sidebar should behave like a real sidebar, not like an ordinary tmux pane.

That means:

- the sidebar width is server-owned, persisted, and fixed for the tmux server until the live TUI width slider changes it
- every sidebar pane in every managed session/window is continuously repaired back to Fixed Sidebar Width
- full terminal resizes do not redefine the saved width
- session switching does not cause the sidebar to jump, breathe, or re-proportion itself
- background windows should already be correct before the user lands in them
- the UI should clearly show `warming up…` while sidebars are still spawning and `closing…` while the server is draining clients before exit

## Product Behaviors

These are user-visible behaviors. Preserve them even if the implementation details change.

### The sidebar is global within one tmux server

Each tmux server/socket gets its own opensessions server and its own sidebar state. A sidebar in one tmux server must not talk to another tmux server's opensessions server.

The practical product behavior is:

- every sidebar pane inside the same tmux server shows the same session list, filters, lifecycle state, collapsed groups, and Fixed Sidebar Width
- each attached tmux client owns its own confirmed active row and temporary keyboard focus
- manual tmux-driven width changes are rejected and repaired back to the server-owned width
- a different tmux server may have a different opensessions width/state/server without conflict
- stale hooks or sidebars talking to another port are considered broken configuration, not valid mixed-server behavior

### Sidebar width is owned by the server, not pane resizing

The sidebar width should feel like a fixed application sidebar, not an ordinary tmux pane. It starts from opensessions configuration for that tmux server, can be changed intentionally from the debounced live TUI width slider, is persisted back to configuration, and observed pane width is never promoted to source-of-truth.

Important details from the user's point of view:

- dragging the divider may move the pane momentarily because tmux has no native per-pane width lock, but opensessions snaps it back
- a normal tmux pane, background sidebar, manual sidebar drag, pane exit, or whole-terminal resize must not redefine the sidebar width
- background sidebars should already be at Fixed Sidebar Width before the user lands in them
- pressing `q` quits opensessions only when the key is delivered to a connected sidebar client; pressing `q` in a normal tmux pane is just a normal shell/app keypress

### Server shutdown must not leave stale sidebar clients

When an opensessions server exits, every connected sidebar client in that tmux-server namespace should exit too. A dead server with old sidebar panes still rendering stale state is broken.

Expected shutdown behavior:

- quit can be requested by a connected sidebar keypress, websocket command, `/quit`, or process shutdown
- the server marks the sidebar lifecycle as `closing…`
- the server broadcasts `quit` to websocket sidebar clients
- the server waits briefly for clients to receive the quit frame, then removes hooks and pid file
- restarting the same tmux server should create a fresh server/client generation, not reuse stale sidebars from a previous generation

### Session switching keeps the sidebar in control

Clicking a tmux session in the sidebar should switch to that session and leave focus in that session's sidebar pane. This keeps the global sidebar interaction continuous while navigating.

Agent-row activation is different: it may switch sessions first, then intentionally focus the target agent pane.

### Pane exits must not let tmux steal sidebar space

tmux's native layout repair can give freed pane width to the left neighbor. The sidebar must not permanently keep space just because tmux temporarily assigned it there.

The canonical case is:

```text
before:  sidebar(40) │ pane1(29) │ pane2(29)
action:  pane1 exits or is killed
after:   sidebar(40) │ pane2(59)
```

The broken behavior is:

```text
before:  sidebar(40) │ pane1(29) │ pane2(29)
action:  pane1 exits or is killed
wrong:   sidebar(70) │ pane2(29)
```

This applies to both explicit `kill-pane` and normal shell/process exit from inside `pane1`.

## Width Authority

Only the server-owned Fixed Sidebar Width can author sidebar width. It starts from configuration and may be changed by an explicit width command from the live TUI slider. Slider keypresses update the local slider immediately, then the TUI debounces/coalesces them into `set-sidebar-width` commands so the server remains the owner without receiving a resize storm. The server persists accepted width changes back to configuration for restart.

There is no resize transaction state machine anymore. Tmux hooks observe layout changes and repair sidebar panes back to the configured width. Those observations are evidence of drift only. They do not mutate Fixed Sidebar Width.

The accepted rule set is:

- persisted `sidebarWidth` seeds Fixed Sidebar Width for the tmux server
- deprecated `OPENSESSIONS_WIDTH` is ignored by runtime width ownership; persisted `sidebarWidth` is the restart source of truth
- debounced `set-sidebar-width` from the live TUI width slider is the only runtime command that mutates Fixed Sidebar Width, and the accepted value is saved back to persisted config
- every sidebar pane whose title is `opensessions-sidebar` must be repaired to that width
- `repair-width` from a TUI client carries no width; it only asks the server to re-apply Fixed Sidebar Width after tmux resized the pane
- `after-resize-pane` performs direct repair of only the pane that triggered the hook; it fires during our own repairs, so it must not launch a global scan
- `after-resize-window`, `after-kill-pane`, `pane-exited`, and `client-resized` request server-owned global repair; requests settle for 50 ms and coalesce into one pass
- `pane-exited` also notifies the server for orphan-sidebar cleanup
- hook repair must be idempotent: only panes whose current width differs from Fixed Sidebar Width are resized
- global repair is single-flight: a request that arrives during a pass causes one follow-up pass rather than concurrent work, and every pass reads the latest configured width
- never install an unconditional `after-resize-pane -> resize-pane` loop; that can recurse and destabilize tmux

## Global Width Repair Rules

When width drift is observed:

- Fixed Sidebar Width stays unchanged unless the TUI width slider sends a new debounced live value
- the drifting pane is snapped back when possible
- all other sidebar panes are checked and repaired to Fixed Sidebar Width
- rapid switching must not be delayed by width repair
- another window reporting its old width must be corrected back to the configured target, not promoted to the new target

This intentionally removes the older "drag owns width, then fan out" model. It was too easy for background panes, terminal resizes, and tmux layout repair to look like user intent. A fixed-width sidebar is simpler: there is no competing owner, so stale observations cannot steal authority.

## Terminal Resize Rules

External terminal resizing is not the same thing as sidebar resizing.

Expected behavior:

- moving between monitor sizes or resizing Ghostty/iTerm should not change the saved sidebar width
- the foreground window should be corrected quickly so the sidebar does not visually breathe
- background windows can catch up with a staggered sync pass after a short settle delay
- transient half-window widths reported during client resizes must never become the persisted width

The server therefore treats client resize as a repair trigger only. Transient widths during full terminal resize are drift signals and must never become Fixed Sidebar Width.

## Session Switching Rules

Session switching should feel boring.

That means:

- the destination session/window should already have a sidebar at the current global width
- switching must not trigger visible layout jumps
- switching must not reset the width to an older value
- transient sidebar widths produced while tmux settles after a session/window switch must not redefine the global width
- switching immediately after any manual/sidebar/tmux resize still converges to Fixed Sidebar Width; no observed pane width is adopted as the new width
- switching from a sidebar session row should leave focus on the destination sidebar pane, not the destination main pane

The sidebar session list has one durable local active row: this tmux client's confirmed active session. The keyboard-focused row may temporarily diverge while the user browses with `j`/`k`/arrow keys, but that temporary selection is local-only and must not be server-synced. `Enter` switches to the temporary selection and keeps that row visible as the pending switch target until `YourSession`/pane identity confirms the new context; it must not snap back to the old active row for an intermediate frame. `Tab`/`Shift-Tab` are the only keys that immediately switch to the next/previous visible session without first moving temporary focus. Mouse clicks on sessions also make the clicked concrete session the pending focus target. In all cases, the durable active row stays on the confirmed active session until confirmation.

Worktree group headers are normal temporary focus targets. `j`/`k`/arrow navigation can land on them, and `Enter` toggles collapse/expand. Once the user chooses a concrete child session inside an expanded worktree group, pending focus moves from the group header to that child session row; the group header must not remain focused in the destination session.

Temporary focus must not look like active focus. The confirmed active row owns the strong green active marker; a temporary keyboard selection uses a weaker cursor marker. Otherwise a normal `j`/`k` browse shows two active-looking rows and feels like focus randomly split.

Temporary focus is allowed to diverge only until a concrete activation/switch-settled event. When tmux activates session B, every sidebar pane whose own confirmed local session is B must reset its keyboard focus back to B. This is a session-scoped reset: if session B has sidebars in multiple tmux windows, all of B's sidebars reset; sidebars belonging to other sessions keep their own local temporary focus. The reset is triggered by the settled switch/activation event, not by generic websocket state snapshots or server-owned global focus.

When `Enter` or `Tab` switches from session A to session B, the old A sidebar remains alive in the background. If A keeps `pending=B` forever, returning to A later replays stale focus and feels random. A server `Focus` broadcast is only a fast intent echo and can arrive before tmux visibly switches the attached client, so it must not clear pending state. A settled `State` snapshot for B is allowed to clean up this one local case: if a sidebar has `pending=B` but its own local session is still A, it clears pending and rehomes focus to A. This does not make server focus authoritative; it only cleans up the source pane after its switch request moved the attached tmux client elsewhere.

This keeps per-window state simple: every attached tmux client can show a different active session row if that client is in a different tmux session, while shared server state still provides the common session list, width, filters, collapsed groups, and lifecycle labels. Server focus broadcasts are compatibility hints only; they must not move a client's local active/focused session row.

One specific regression we already paid for: forcing `resize-window` during the session-switch path caused visible layout jumps. The fix was to stop doing that in the switch path and instead use targeted width enforcement plus background pre-layout where appropriate.

Another regression: switching into a session can briefly resize the destination sidebar through impossible widths such as `1 → 20 → 58 → 20` while tmux restores layout. Those reports are layout-settle echoes, not user drags, even when they come from the active session/window/sidebar pane. The fixed-width rule makes this boring: every such report repairs back to Fixed Sidebar Width.

## Warmup And Closing Semantics

There are two user-visible lifecycle labels and they mean different things.

`warming up…` means:

- sidebars are being spawned/restored across windows
- the system is still converging on presence, not width

`closing…` means:

- the server has accepted shutdown and is draining clients
- sidebars should receive `quit` and exit rather than staying around as stale clients

Important nuance:

- width repair should not produce a user-visible lifecycle state; it should be fast and boring
- warmup must not get stranded forever because unrelated hook events arrive while sidebars are spawning

## tmux-Specific Invariants

tmux has several behaviors that look reasonable until they break the sidebar.

These are non-negotiable:

- ignore control-mode clients with empty `client_tty` when inferring current session or foreground client
- infer current session/window/pane from the active tmux command context where possible; do not let an unrelated attached client make width or focus decisions for the current sidebar
- keep tmux windows in `window-size latest`; do not leave them in manual mode after `resize-window`
- install both `pane-exited` and `after-kill-pane`; normal shell/process exit is not covered by `after-kill-pane` alone
- treat `pane-exited` and `after-kill-pane` as topology-change signals only; they must never adopt tmux's redistributed sidebar width as user intent
- use `after-resize-pane` only as an idempotent fixed-width repair trigger for the pane that caused the hook when it is titled `opensessions-sidebar`; it must not scan or resize unrelated panes
- do not refocus the main pane immediately after sidebar spawn/restore; let the TUI refocus after capability detection settles so escape sequences do not leak into the main pane
- invalidate cached sidebar pane listings before logic that depends on just-spawned or just-hidden panes

## Per-tmux-server Technical Contract

The tmux socket is the namespace boundary.

Derived configuration must be stable for a given tmux socket and distinct across tmux sockets:

- `server_key` is derived from the tmux socket path unless explicitly overridden in tmux-scoped environment
- server port, pid file, and log file derive from that key
- tmux hooks for that tmux server point only to that tmux server's derived server port
- sidebar clients derive the same endpoint as the server launcher
- explicit overrides such as `OPENSESSIONS_PORT`, `OPENSESSIONS_HOST`, and `OPENSESSIONS_PID_FILE` should be tmux-scoped, not ambient shell leftovers

Symptoms of broken per-server wiring:

- multiple `opensessions-server` processes for the same tmux socket
- hooks in one tmux server point to another tmux server's port
- sidebars inside the same tmux server have mixed widths after settle
- pressing `q` in a connected sidebar does not stop the expected derived server
- sidebars keep rendering after their server pid file has been removed

The recovery path is: clear stale tmux-scoped overrides, refresh hooks from the current plugin, restart the derived server for that tmux socket, and respawn visible sidebar panes.

## Regressions We Already Paid For

These are the big historical failure modes worth remembering.

### 1. Jamming tmux's resizer

What happened:

- the server tried to do too much synchronous resize work during tmux resize storms
- or it got stuck in echo/enforcement loops where programmatic resizes caused more programmatic resizes

What fixed it:

- idempotent tmux hook repair: only sidebar panes whose current width differs from Fixed Sidebar Width are resized
- targeted `after-resize-pane` repair: each hook invocation checks only its triggering pane, preventing global repair scans from recursively fanning out
- coalesced global repair: topology and client-resize bursts are handled by one server worker instead of concurrent hook pipelines
- no width-authoring path from observed pane width
- no unconditional resize hook that can recursively trigger itself

### 2. Treating external width changes as user intent

What happened:

- full terminal resizes and other layout churn produced observed pane widths that looked like drags in the old model
- the saved width changed even though the user never dragged the divider

What fixed it:

- no sidebar pane can author width
- tmux hooks and `repair-width` are drift signals only
- Fixed Sidebar Width is the only source of truth

### 3. Switching quickly exposed stale widths

What happened:

- a destination session could briefly expose a stale or tmux-restored sidebar width
- old width transaction code could treat the switch-time width as meaningful
- session switching could be delayed by resize fan-out work

What fixed it:

- switch paths no longer carry resize handoff state
- destination width is repaired to Fixed Sidebar Width by hooks/ensure/backstop polling
- width repair is not allowed to become a higher-priority operation than tmux switching

### 4. Background windows were stale

What happened:

- only the active session/window got corrected promptly
- switching into a background window revealed a stale sidebar width flash

What fixed it:

- tmux-local hook repair across panes titled `opensessions-sidebar`
- server backstop repair when hooks or client reports reveal drift

### 5. Manual window-size mode poisoned layouts

What happened:

- `resize-window` left tmux windows in `window-size manual`
- later terminal behavior looked padded or broken

What fixed it:

- always restore `window-size latest` after forced window resizes

### 6. Pane exit gave width to the sidebar

What happened:

- with `sidebar | pane1 | pane2`, tmux gave `pane1`'s freed width to the left sidebar when `pane1` exited
- if the sidebar accepted that observed width, it permanently grew and pane2 did not absorb the space

What fixed it:

- `pane-exited` covers normal shell/process exit and `after-kill-pane` covers explicit kill-pane
- pane-exit handlers re-enforce the coordinator-owned width instead of adopting observed tmux width
- the remaining non-sidebar pane absorbs the freed space

### 7. Active pane was the wrong model for width authority

What happened:

- a border drag can resize the sidebar while tmux focus remains in the main pane
- trying to infer user intent from tmux focus or pane identity made width ownership fragile

What fixed it:

- delete user-authored width ownership entirely
- observed pane width is always drift, never intent

## Rejected Approaches

These are not theoretical. They were tried and caused problems.

- using `after-resize-pane` as the main width-authority mechanism
- forcing `resize-window` directly in the normal session-switch path
- suppressing width reports so broadly that legitimate drag events got blocked
- setting drag suppression in a way that made the server fight the user's live drag
- requiring tmux's active pane to be the sidebar pane before accepting a sidebar width report
- treating every TUI as authoritative instead of only the current foreground one
- treating any observed TUI pane width as authoritative instead of requiring the explicit width slider command

## Performance Constraints

The sidebar should not make tmux feel heavy.

Keep these constraints in mind:

- stagger expensive cross-window work instead of doing it all inline in hooks
- avoid repeated full `list-panes -a` scans inside the same resize cycle
- batch where possible, cache briefly, and invalidate on real topology changes
- prioritize the active window first, then let the rest catch up quickly in the background
- polling may correct a detected sidebar-width drift after a short settle window, but it must not redefine Fixed Sidebar Width

## Change Checklist

Before shipping any sidebar behavior change, verify all of these.

- manually resizing the active sidebar snaps back to Fixed Sidebar Width
- manually resizing the sidebar divider while focus remains in the main pane snaps back to Fixed Sidebar Width
- switching sessions immediately after manual/sidebar/tmux resize preserves Fixed Sidebar Width
- clicking a session row leaves focus in the destination sidebar pane
- resizing the whole terminal does not redefine the persisted width
- in `sidebar | pane1 | pane2`, killing or exiting `pane1` leaves `sidebar` fixed-width and lets `pane2` absorb the freed width
- background windows land at the current width without visible proportional flash
- `warming up…` clears once spawn/restore is complete
- no `adjusting…` lifecycle appears for width repair
- server shutdown broadcasts `quit` to websocket sidebar clients before cleanup
- control-mode clients cannot steal foreground/current-session authority
- hooks and sidebar clients point to the derived server for the current tmux socket
- tmux windows remain in `window-size latest`
- no resize or enforcement loop appears in `/tmp/opensessions-debug.log`

## Files To Read Before Changing This Area

- `apps/server-rs/src/lib.rs`
- `apps/tui-rs/src/main.rs`
- `apps/tui-rs/tests/tmux_e2e.rs`
- `packages/runtime-rs/src/sidebar_coordinator.rs`
- `packages/sidebar-core-rs/src/app.rs`
- `packages/runtime-rs/src/mux.rs`
- `packages/runtime-rs/src/tmux_provider.rs`
- `apps/tui/scripts/start.sh`
- `integrations/tmux-plugin/scripts/server-common.sh`

If a future change violates this doc but seems necessary, update the doc in the same change and explain the new invariant explicitly.
