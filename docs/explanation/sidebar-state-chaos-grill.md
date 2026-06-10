# Sidebar State Chaos Grill

This is the working document for turning the current sidebar behavior from whack-a-mole into a small set of explicit ownership rules.

Read this alongside [`sidebar-behavior.md`](./sidebar-behavior.md). That file captures product invariants we already learned. This file captures the unresolved model questions and the design pressure behind them.

## Why things feel chaotic right now

The core problem is not one bad resize handler or one bad focus event. The core problem is that several different actors can currently appear to own the same facts:

- sidebar lifecycle: hidden, warming, ready, closing
- sidebar width
- whether the sidebar is globally open or closed
- which session is selected in the sidebar list
- which tmux session is active for a specific terminal client
- which client/window/session is allowed to report user intent

When those facts are not explicitly owned, every fix becomes local compensation:

- a resize report tries to correct a stale width
- a switch path tries to preserve a just-resized width
- a focus hook tries to repair session state
- a client applies optimistic state to avoid flicker
- another server snapshot arrives and overwrites that optimistic state

That is the whack-a-mole pattern. The durable fix is to make ownership impossible to confuse.

## Current symptoms to explain

### 1. `warming up…` and shutdown are not clean enough

Current Rust behavior:

- `q` in the sidebar becomes a `quit` client command.
- the server broadcasts a `closing…` state, then a `quit` message.
- websocket clients normally exit after receiving `quit`.
- the accept loop sleeps briefly, then removes hooks and pid file.
- this is operationally close to the desired behavior, but it is not yet encoded as a terminal lifecycle in the model.

Current gap:

- shutdown is best-effort and process-level, not type-state enforced
- the coordinator can still express illegal lifecycle mutations while shutdown is draining, such as a late sidebar connection acknowledging itself and moving lifecycle back toward ready before the process exits
- `warming up…`, `ready`, and `closing…` are all mutable labels on one coordinator, rather than states in a server generation that can only move forward

Desired behavior:

- pressing `q` in a connected sidebar requests shutdown of the opensessions server for this tmux-server namespace
- the server enters `closing…`
- every attached sidebar client receives `quit`
- websocket clients exit
- hooks and pid file are cleaned up
- restarting creates a fresh generation

The confusing part is whether a client owns quitting itself or whether the server owns quitting the whole sidebar generation.

Recommended ownership rule:

> A connected sidebar client may request shutdown, but only the server owns shutdown. Once accepted, shutdown is terminal for the entire server generation.

Rust model shape:

```rust
enum SidebarGeneration {
    Running(RunningGeneration),
    Closing(ClosingGeneration),
    Closed,
}

struct RunningGeneration {
    clients: ClientRegistry,
    sidebar: SidebarState,
}

struct ClosingGeneration {
    generation_id: GenerationId,
    deadline: Instant,
}
```

Important rule: no code path should be able to go from `Closing` back to `Running`. In Rust terms, accepting shutdown consumes `RunningGeneration` and returns `ClosingGeneration`.

#### Problem 1 solution shape

This should be solved before resize/session/agent chaos, because shutdown defines the lifecycle boundary for every connected sidebar.

Product rule:

> `q` quits the entire opensessions server generation for this tmux socket. It does not merely close the local sidebar pane.

State-machine rule:

```rust
enum ServerPhase {
    Running(RunningGeneration),
    Closing(ClosingGeneration),
    Closed,
}
```

Allowed transitions:

```diagram
╭─────────╮     q / quit request      ╭─────────╮   drain complete   ╭────────╮
│ Running │──────────────────────────▶│ Closing │──────────────────▶│ Closed │
╰─────────╯                           ╰─────────╯                   ╰────────╯
     ▲                                      │
     ╰──────────────────────────────────────╯
          forbidden: no reopen/ready during same generation
```

Implementation implications:

- shutdown acceptance must be idempotent: the first request wins, later requests observe `Closing`
- no command except drain/quit delivery/cleanup should mutate sidebar lifecycle once phase is `Closing`
- `acknowledge_sidebar_connected`, `begin_warmup`, `mark_ready`, `ensure_sidebar`, and toggle paths should no-op or return a closing response after shutdown starts
- clients should render `closing…` only until they receive `quit`; they should not invent local readiness after that
- stale clients from an old generation should fail to reconnect because pid/socket/hooks are removed and the next launch has a fresh generation

Smallest implementation slice later:

1. add a server-level phase guard around `ReadOnlyMuxStateSource`
2. make `begin_shutdown()` idempotent and return whether this was the first accepted shutdown
3. block lifecycle-changing coordinator calls when phase is `Closing`
4. add tests proving late identify/ensure/toggle cannot move `closing…` back to ready/warming
5. keep existing quit broadcast/drain/cleanup mechanics

Isolated proof-of-shape:

- `packages/runtime-rs/src/lifecycle_operation.rs` models a server-owned Lifecycle Operation reducer.
- `packages/runtime-rs/tests/lifecycle_operation.rs` simulates connected clients, quit, late sidebar identify, warmup completion, and drain completion.
- The test proves the key invariant: once `RequestQuit` moves the Server Generation to `Closing`, later lifecycle messages cannot move it back to `Warming` or `Ready`.
- The test also covers 100 connected clients: `Quit` sends `quit` to every other client, does not send `quit` back to the requesting client, and rejects a re-entrant `Quit` submission through the Lifecycle Channel while effects are being delivered.
- Width adjustment was deliberately removed from the isolated reducer. Fixed Sidebar Width is server-owned and changed only by explicit debounced live width commands from the TUI slider, so the reducer models lifecycle/presence/switch/quit only.

Live red contract:

- `quit_lifecycle_announces_shutdown_once` in `apps/server-rs/tests/protocol_shell.rs` pins the next server fix: shutdown should be a single terminal lifecycle transition, not an announcement in the requester path plus another announcement when the accept loop drains.

#### Warmup means sidebar presence reconciliation

`warming up…` is the user-facing label. The operation is Sidebar Presence Reconciliation.

Definition:

> Sidebar Presence Reconciliation is active when opensessions is making every eligible tmux window in the Server Generation have exactly one connected opensessions sidebar.

Eligible target:

> every unique non-stash tmux `window_id` that should show opensessions.

The `window_id` dedupe matters because tmux linked/grouped sessions can report the same physical window under multiple sessions. Warmup must target the physical window once, not once per session row.

Example:

```text
tmux reports:
  alpha  @1
  beta   @2
  gamma  @2   # linked/shared physical window

warmup targets:
  @1 once
  @2 once
```

Warmup ends when every target has connected/identified, vanished, or reached a timeout policy. A timer alone is not the semantic guarantee.

Timeout policy:

> A target that does not connect becomes a Presence Failure with a reason such as spawn failed, connect timeout, or window vanished. Presence Failures let warmup finish with diagnostics; they are not silent success.

Spawn pressure rule:

> Sidebar Presence Reconciliation should spawn sidebars one at a time in a staggered order so the server and tmux are not overloaded. The first targets are the origin session's windows, then the remaining unique windows.

The origin session is the tmux session where the initiating sidebar/client lives. This keeps the user's current area responsive first, then catches up the rest of the Server Generation.

### 2. Width adjustment should not be a lifecycle state

Earlier we thought `adjusting…` should model width convergence: after one sidebar changed width, every other sidebar would acknowledge the target, and stale panes would be rejected until convergence finished.

Final decision:

> Delete the width-adjustment concept. Sidebar width is Fixed Sidebar Width: server-owned per Server Generation, seeded by persisted configuration, changed only by explicit debounced live width commands, saved back to configuration, and never authored by observed tmux pane width. Width drift is repaired, not modeled as a user-visible lifecycle.

Why this is simpler:

- there is no competing owner, so stale panes cannot steal width authority
- manual divider drags, full terminal resizes, pane exits, and tmux layout churn all collapse to the same case: repair back to Fixed Sidebar Width
- `adjusting…` disappears from product state; only `warming up…` and `closing…` remain user-visible lifecycle labels

Recommended ownership rule:

> The server owns width. The live TUI width slider updates local UI immediately and sends debounced explicit width commands; the server persists accepted values. Clients and tmux hooks can report drift, but they cannot mutate Fixed Sidebar Width through observations.

Live contract:

- `tmux_sidebar_width_is_fixed_and_rejects_manual_sidebar_resize` proves manual/sidebar resize does not persist.
- `tmux_sidebar_width_survives_flat_three_pane_layout_churn` proves `sidebar | pane1 | pane2`, then killing/exiting `pane1`, leaves sidebar width fixed.
- `tmux_sidebar_client_resize_never_persists_a_smaller_sidebar_width` proves whole-terminal/client resize does not redefine width.

Rust model shape:

```rust
struct FixedSidebarWidth(u16);

enum WidthObservation {
    Matches,
    Drift { pane_id: PaneId, observed: u16 },
}
```

The key is not a richer transaction enum. The key is that runtime observations do not own Fixed Sidebar Width. If an old pane reports width 24 while Fixed Sidebar Width is 36, that report cannot become a new target; it can only trigger repair back to 36. If the user wants 38, the live TUI slider sends a debounced explicit `set-sidebar-width` command and the server persists it.

### 3. Server-derived vs client-derived state is blurred

Some sidebar state should be global within one tmux server:

- sidebar open/closed
- sidebar width
- session order/filter/theme/collapsed groups
- lifecycle labels like `warming up…` and `closing…`
- agent/session data

Some state is local to one terminal client or one sidebar pane:

- the tmux session active in that Ghostty window/client
- hover state
- transient click flash
- local scroll offset while the user is manually scrolling
- maybe keyboard focus inside the sidebar, depending on desired UX

The hard case:

- Ghostty window A is attached to tmux server S and currently views tmux session `alpha`
- Ghostty window B is attached to the same tmux server S and currently views tmux session `beta`
- both sidebars connect to the same opensessions server because the tmux socket namespace is the same

There is no single global `currentSession` in that world. There is only:

- current session for client A
- current session for client B
- maybe a most-recent/foreground client, but that is not globally authoritative for all clients

Recommended ownership rule:

> The server owns shared sidebar facts. Each connected client owns its own view context. The server may store and broadcast per-client view contexts, but it must not collapse them into one fake global `currentSession`.

Rust model shape:

```rust
struct ServerModel {
    shared: SharedSidebarState,
    clients: HashMap<ClientId, ClientViewState>,
}

struct SharedSidebarState {
    visible: bool,
    width: WidthAuthority,
    session_list: SessionListState,
    lifecycle: SidebarLifecycle,
}

struct ClientViewState {
    client_tty: ClientTty,
    pane_id: PaneId,
    window_id: WindowId,
    current_session: SessionName,
    sidebar_focus: SidebarFocus,
}
```

Protocol implication:

- global broadcasts can still carry shared state
- client-specific frames should include the receiving client's `currentSession` and possibly `sidebarFocus`
- if we broadcast a global state with a fake `currentSession`, clients will keep fighting reality

### 4. Session switching flicker likely comes from fake global state

Observed symptom:

- click or press Enter on a session row
- the sidebar briefly flickers or snaps through an old session/focus state

Likely causal chain:

1. client optimistically marks the target session as current/focused
2. client sends `switch-session`
3. server switches tmux for the relevant client
4. server also broadcasts state derived from one provider/global current session
5. a different client, old hook, or old provider snapshot still says the previous session is current
6. receiving clients apply that global state and temporarily undo the optimistic switch

Recommended ownership rule:

> Session switching is a command against one client context, not a global mutation of `currentSession` for all clients.

Server should respond to the requesting client with its new local current session. Other clients may receive shared session-list updates, but they should not be told their current session changed unless their own tmux client actually changed.

Live red contract:

- `switch_session_updates_requesting_client_without_rewriting_other_client_current_session` in `apps/server-rs/tests/protocol_shell.rs` pins the per-client rule: a switch request from one connected sidebar must not send a fake universal `currentSession` update to a different connected sidebar.

### 5. Agent rows duplicate when pane presence competes with agent-native state

Observed symptom:

- the agent sidebar shows repeated AMP rows for what appears to be the same real task
- some rows are generic `amp` rows, while others have thread names/status from AMP data

Likely causal chain:

1. tmux pane scanning sees a pane title or command that looks like AMP
2. pane scanning emits a synthetic agent row without stable AMP `threadId`
3. AMP thread JSON or plugin event emits canonical thread state with stable thread identity
4. the tracker cannot always prove the pane row and thread row are the same real thing
5. the sidebar renders both

Decision: accepted.

> Agent rows should come from Agent Thread State only. Agent Pane Presence must not create canonical sidebar rows.

Implications:

- AMP rows are sourced from AMP thread JSON or explicit AMP/plugin events, not tmux pane title scans
- pane title/command scanning can remain a separate future diagnostic/focus aid, but it should not populate the agent list
- this intentionally drops broad “agent-looking pane” discovery to avoid duplicate or misleading rows
- if an integration cannot provide stable thread identity, it should not appear as a canonical agent row until it can

Live red contract:

- `amp_agent_events_with_same_thread_name_canonicalize_to_one_row_when_thread_id_arrives` in `apps/server-rs/tests/protocol_shell.rs` pins the AMP canonicalisation rule: a provisional AMP event without `threadId` must merge into the later event with the same thread name once the stable `threadId` arrives, instead of rendering two rows.

## Proposed core model

The model should be split by ownership boundary, not by which file currently emits the data.

```diagram
╭──────────────────────────────╮
│ one tmux server / socket      │
│ one opensessions server       │
╰──────────────┬───────────────╯
               │ owns shared facts
               ▼
╭──────────────────────────────╮
│ SharedSidebarState            │
│ - visible/open                │
│ - width transaction           │
│ - lifecycle label             │
│ - sessions/agents/theme/filter│
╰──────────────┬───────────────╯
               │ plus per-client overlays
     ┌─────────┴─────────┐
     ▼                   ▼
╭──────────────╮   ╭──────────────╮
│ Client A     │   │ Client B     │
│ tty=/dev/1   │   │ tty=/dev/2   │
│ current=alpha│   │ current=beta │
│ focus=alpha  │   │ focus=beta   │
╰──────────────╯   ╰──────────────╯
```

That gives us three explicit layers:

1. **Namespace layer:** one opensessions server per tmux socket.
2. **Shared layer:** facts all sidebars in that namespace agree on.
3. **Client layer:** facts that differ per attached terminal client/window.

## Rust compiler leverage

The target is not “rewrite because Rust is better.” The target is to encode illegal state transitions so they are hard to express.

Use Rust to force:

- `Closing` cannot become `Ready`
- there can be at most one active `WidthAdjustment`
- stale width reports cannot author a new width while an adjustment is owned
- a `SwitchSession` command must carry a `ClientId` or `ClientTty`
- a global server snapshot cannot accidentally pretend to contain a universal current session
- per-client rendering cannot happen without a client context

Possible protocol split:

```rust
enum ServerToClient {
    Hello(Hello),
    SharedState(SharedStateSnapshot),
    ClientState(ClientStateSnapshot),
    Quit,
}
```

This is stricter than a single `ServerState { currentSession }` object. It makes the ownership visible at the wire boundary.

## Design decisions to grill

### Decision 1: What does `q` mean?

Decision: accepted.

Recommended answer:

> `q` means quit the opensessions server generation for this tmux socket, not merely close the local sidebar pane.

Consequences:

- all sidebars connected to this server receive `closing…` then `quit`
- server cleanup is centralized
- stale panes are considered bugs
- if we later need “close only my sidebar,” that should be a different command/key, not overloaded onto `q`

### Decision 2: Is sidebar open/closed global or per-client?

Recommended answer:

> Global within one tmux server.

Reason:

- width is already global
- hooks/sidebar spawning are tmux-server scoped
- a pseudo-global sidebar that is open in some sessions and closed in others creates exactly the same ownership ambiguity as width

### Decision 3: Is active/current session global or per-client?

Decision: accepted.

Recommended answer:

> Per-client. Never global.

Reason:

- two Ghostty windows can attach to the same tmux server and view different sessions at the same time
- tmux `get_current_session()` without a client context is not enough information
- global `currentSession` is the likely source of flicker and cross-client lies

Confirmed active-session rule:

> The green/current row is the confirmed current session for this sidebar's own tmux client. Clicking, pressing Enter, or pressing Tab sends a switch request, but it does not make the target active locally. The target becomes active only after local pane identity or a `your-session` acknowledgement confirms that this sidebar is attached to the destination session.

The sidebar can still keep a local Pending Switch Request so repeated navigation commands such as Tab can continue from the requested target. That pending target is command/navigation state only; it must not be rendered as the active session.

Live red/green contracts:

- `session_switch_request_does_not_make_target_the_confirmed_active_session` proves a click queues `switch-session` while the green/current row stays on the confirmed local tmux session.
- `tab_switch_queues_next_visible_session_without_changing_confirmed_current_session` proves Tab can request the next session without changing active state before tmux confirmation.
- `local_pane_identity_overrides_stale_server_current_session_on_startup` proves a newly attached sidebar uses its own pane identity before stale shared `currentSession`/`focusedSession` values, preventing the destination sidebar from initially painting the old session.
- `shared_state_current_session_is_not_a_confirmed_client_active_session` proves a legacy shared snapshot cannot seed the green/current row before local identity exists.
- `click_on_session_row_queues_switch_without_changing_confirmed_active_session` covers the real mouse path through hit-map/input handling, not only direct app-state calls.

### Decision 4: Is selected/focused row global or per-client?

Decision: accepted.

Recommended answer:

> Per-client by default; only session order/filter/theme are global.

Reason:

- if Client A is browsing around the list, Client B should not have its cursor stolen
- Enter/click acts from the local client context
- agent notifications and unseen counts can remain global/shared without sharing cursor position

Open caveat:

- if the product explicitly wants all sidebars to mirror cursor selection, that should be named as a collaborative mode, not implicit default behavior

Protocol direction:

> Split Shared Sidebar State from Client View State. Shared Sidebar State is broadcast to every client and contains sessions, agents, lifecycle, width, theme, filter, and collapsed groups. Client View State is sent only to one client and contains that client's current session and selected/sidebar focus row.

Switching session from a sidebar is therefore targeted: the requesting client receives a new Client View State immediately, while other clients keep their own current session and selected row. This prevents flicker caused by a fake global `currentSession` broadcast.

### Decision 5: What ends width repair?

Decision: superseded by Fixed Sidebar Width.

Recommended answer:

> Width repair ends immediately for a given event when every observed sidebar pane already matches the configured width or has been issued an idempotent resize back to that width. There is no user-visible `adjusting…` state.

Not enough:

- a fixed 600ms settle timer
- first width report after fanout
- active window alone reaching the target

Needed data:

- Fixed Sidebar Width
- sidebar pane identity/title
- observed pane width
- hook/backstop evidence if repair fails

### Decision 6: Can a stale pane start a new width target?

Decision: accepted.

Recommended answer:

> No. No pane can start a new width target. Stale panes, foreground panes, and background panes all produce drift observations only.

Refinement:

> Competing reports are repaired to Fixed Sidebar Width. They must not queue a future target width and must not overwrite Fixed Sidebar Width.

This is the “use Rust ownership” rule: runtime observations do not own `FixedSidebarWidth`, so they cannot mutate the target width.

## First implementation direction

Do not start by patching flicker locally. Start by separating shared and client-owned state in the model and protocol.

Smallest useful slice:

1. introduce explicit `ClientId` / `ClientViewState` in the Rust server
2. make websocket identify establish a client view
3. derive `currentSession` per client, not globally
4. keep shared invalidation broadcasts separate from typed client query responses
5. make switch-session update only the requesting client's view immediately
6. keep old protocol compatibility only as a temporary adapter

Then width adjustment transactions become much easier because target/owner are typed.

## Grill queue

We should resolve these in order:

1. Should `q` kill the whole server generation for that tmux socket?
2. Should sidebar open/closed be global inside a tmux server?
3. Should selected row/sidebar cursor be per-client or globally mirrored?
4. Should `currentSession` disappear from global `ServerState` and move into `ClientState`?
5. During resize adjustment, what is the exact target set: visible sidebars only, all known sidebars, or all sessions/windows that should have sidebars?
6. What should happen if one target never acknowledges the width?
7. Is “active session” a tmux-client concept, a sidebar-pane concept, or both?
8. Should session switch leave focus in the destination sidebar pane always, or only for session-row activation?
9. Should agent-row activation switch session and then intentionally focus the agent pane, bypassing sidebar focus continuity?
10. What is the recovery story for stale clients from an old server generation?
