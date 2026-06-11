# How opensessions Works

opensessions is a local coordination layer between your multiplexer, your agent tools, and a terminal sidebar UI.

It is easiest to think about it as four pieces:

1. a Rust tmux provider that knows how to inspect and control tmux
2. Rust agent watchers and HTTP APIs that translate agent data into `AgentEvent`s
3. a Rust WebSocket/HTTP server that merges state and broadcasts it
4. a Rust ratatui sidebar client that renders the UI and sends user commands back

## Startup Flow

When the tmux plugin or sidebar starts, the helper scripts ensure an `opensessions-server` process is running for the current tmux socket.

If no healthy server is listening, `integrations/tmux-plugin/scripts/server-common.sh` launches the Rust `opensessions-server` binary. The server then:

1. loads config from `~/.config/opensessions/config.json`
2. registers the built-in tmux provider
3. resolves the primary mux provider
4. starts the tmux state observer and, when enabled deliberately, built-in scanner loops for Amp, Claude Code, Codex, OpenCode, Pi, and Droid
5. starts the WebSocket and HTTP control server

## State Assembly

The server owns authoritative runtime state and serves typed query read models to connected sidebar clients.

That state is assembled from several sources:

- session lists from the active mux provider
- custom session order stored on disk
- Git branch and dirty information from each session directory
- pane counts and window counts from providers
- detected listening ports from descendant processes in tmux sessions
- tracked agent instances and unseen state from `AgentTracker`

The result is one live view of the current tmux universe. Internally, the aggregate state is split into typed query results such as `Sessions`, `Agents`, `Focus`, `SidebarLayout`, and `Settings` so clients can refetch only the read models invalidated by a change.

## Authority, Observations, And Projections

Server/query state is the authority. Tmux pane geometry, terminal frames, and visible screen text are observations of the outside world.

That boundary prevents feedback loops: a sidebar may observe that tmux temporarily resized a pane, but the persisted sidebar width changes only through a typed `SetSidebarWidth` command and the server's `SidebarLayout` query result. Likewise, terminal text can help diagnose or map a pane to an agent, but the TUI renders typed agent/session state instead of scraping panes directly.

A projection is a read-only view derived from deeper state. For example, the raw state may contain all sessions, worktree groups, collapsed groups, focused pane, agent instances, and unseen markers; the sidebar projection is the exact row order and glyphs shown to the user. Projection rules should stay pure and derived from typed state so the UI behaves like a React app: server data is fetched/subscribed, local UI state is separate, and rendering is a deterministic function of both.

## Agent Tracking Model

Watchers and external integrations do not know about the TUI. They emit `AgentEvent`s into the server through `POST /api/agent-event`; the built-in JSONL scanner code is present but currently disabled while tmux-pane observation is hardened.

The `AgentTracker` is where those raw events become UI-friendly state:

- it keeps instances separate with `threadId` when available
- it derives the most important session-level state from all instances
- it tracks unseen status per instance for terminal states
- it prunes stale or no-longer-relevant state over time

This separation is why the built-in watchers can be simple and agent-specific while the unseen logic stays consistent across agents.

## Why The Mux Interface Is Capability-Based

The provider model is split into required core operations and optional capabilities instead of one large interface.

That matters because different multiplexers do not expose the same control surface:

- session listing and switching are common needs
- session creation, window awareness, and sidebar management vary by provider
- tmux has hook support and more direct client targeting
- other providers may need to lean more on CLI actions or polling

The capability model lets the server ask for only what a feature needs. For example, sidebar spawning requires both window awareness and sidebar management, so the server narrows providers with `isFullSidebarCapable()`.

## tmux Design

The tmux provider is the more feature-complete reference implementation.

Notable design choices:

- tmux global hooks notify the server about focus changes, session creation, window changes, and resize events
- hidden sidebars are closed; legacy or reserved stash panes in `_os_stash` are filtered out of normal sidebar management
- the TUI refocuses the main pane after capability detection to avoid escape-sequence leakage into the main pane
- typed tmux command helpers live in the Rust tmux provider and tmux scripting modules
- the tmux integration scripts live under `integrations/tmux-plugin`, while the sidebar launcher itself lives with the TUI app in `apps/tui/scripts/start.sh`

## Experimental Providers

The mux contract is intentionally extensible, and the repository still contains older experimental provider code beyond tmux.

That code is not part of the current support promise. In particular, the zellij path is not stable enough to recommend today, and we are looking for maintainers who want to help bring it back to a supported state.

## Why The Server Owns Session Switching

The TUI does not switch sessions directly. It always sends a command to the server.

That centralization matters for three reasons:

1. the server knows which provider owns each session
2. the server can use authoritative client TTY information gathered from hooks or identify messages
3. provider-specific switching logic belongs in one place rather than being duplicated in every client

## Files The Runtime Writes

The runtime keeps a small set of operational files:

- `/tmp/opensessions.pid` for server bootstrap health checks
- `/tmp/opensessions-debug.log` for best-effort debug logging
- `~/.config/opensessions/session-order.json` for user-controlled session ordering
- `~/.config/opensessions/config.json` for user configuration

## Current Constraints

Some pieces are intentionally still narrow in scope:

- the server and TUI are local-only; the default host is `127.0.0.1`, and ports are derived per tmux socket unless explicitly overridden
- parsed config field `keybinding` is not yet wired through the runtime
- inline theme objects exist in the core API surface, but the running server currently uses theme names
- tmux is the only supported mux today

## Why The Codebase Is Split This Way

The repository now follows a Rust-first monorepo boundary model:

- `apps/server-rs` contains the Rust control-plane server
- `apps/tui-rs` contains the Rust ratatui sidebar client
- `packages/runtime-rs` contains reusable runtime logic that both apps depend on
- `apps/tui-rs` contains sidebar state, input, layout, rendering, and terminal client logic
- `integrations/tmux-plugin` contains host-specific tmux glue instead of runtime library code

That keeps entrypoints, reusable libraries, mux adapters, and host integrations separate enough that new contributors can tell what owns what at a glance.
