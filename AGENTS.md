# opensessions — AI Agent Instructions

You are working on **opensessions**, an agent-agnostic terminal session manager and parallel-agent control plane.

## North Star

**opensessions is becoming the parallel-agent operating system.** It should make many CLI agents, panes, tmux sessions, and git worktrees feel like one coherent control plane instead of a pile of terminals.

The product direction is:

- **Observe everything**: track tmux sessions, windows, panes, focused pane, layouts, worktrees, git state, agent status, approval state, unread/done state, and session activity as first-class runtime state.
- **One worktree === one session by default** for opensessions-created work: a new isolated task should get a worktree-backed tmux session with predictable naming, pane layout, agent launch, and cleanup.
- **Existing work stays valid**: users and agents must also be able to launch agents inside existing worktrees, existing sessions, and existing panes when that is the right workflow.
- **Server as control plane**: the server owns durable state, synchronization, launch jobs, tmux/worktree mappings, and agent-facing APIs. The sidebar is one UI over that control plane, not the control plane itself.
- **CLI/API for agents**: opensessions should expose commands and eventually an API/MCP surface that lets agents create sessions, launch sibling agents, inspect panes, read useful context, send prompts, wait for state changes, and report handoffs safely.
- **Tmux-native, not tmux-hostile**: tmux remains the supported substrate. opensessions should repair and manage only what it owns, preserve user layouts by default, and reserve fully managed layouts for explicit modes.
- **Review and merge are part of the OS**: parallel execution is only useful when the user can compare outputs, detect conflicts, review diffs, merge winning work, and clean up sessions/worktrees without ceremony.

## Project Structure

```
opensessions/
├── apps/
│   ├── server-rs/          # opensessions-server — Rust WebSocket/HTTP server and control plane
│   ├── tui-rs/             # opensessions-sidebar — Rust ratatui sidebar client
│   └── tui/scripts/        # tmux sidebar launcher and sessionizer shell scripts
├── integrations/
│   ├── tmux-plugin/        # tmux-facing scripts and host integration glue
│   ├── amp/                # Amp helper integration
│   └── pi-extension/       # Pi runtime helper integration
├── packages/
│   ├── runtime-rs/         # Shared Rust runtime: config, protocol, tracker, tmux provider, watchers
├── CONTRACTS.md            # Supported agent event and runtime integration contracts
├── opensessions.tmux       # Root TPM entrypoint
├── Cargo.toml              # Rust workspace root
└── package.json            # Release version used by npm/TPM download helpers
```

## Key Architecture Decisions

1. **Rust-first runtime**: the supported server and TUI are Rust crates in `apps/*-rs` and `packages/*-rs`.
2. **Ratatui sidebar**: rendering is immediate-mode Ratatui/Crossterm. Sidebar UI state, input, layout, rendering, and hit testing live in `apps/tui-rs`; shared non-UI runtime logic lives in `packages/runtime-rs`.
3. **Built-in agent watchers**: the Rust server scans Amp, Claude Code, Codex, OpenCode, Pi, and Droid state directly and converts it into `AgentEvent`s.
4. **External agent events via HTTP**: third-party agents should POST to `/api/agent-event` or use the metadata endpoints. TypeScript plugin loading is not a supported runtime path right now.
5. **Tmux is the supported mux**: abstractions remain mux-shaped, but tmux is the only documented supported provider. Older zellij helper code is not part of the support promise.
6. **Release binaries, not local builds**: TPM users get prebuilt `opensessions-sidebar`, `opensessions-server`, and `lazydiff` binaries in `bin/`. `cargo build --release` is for development or unsupported platforms.

## Contracts

### AgentEvent

```typescript
{
  agent: string,
  session: string,
  status: "idle" | "running" | "tool-running" | "done" | "error" | "waiting" | "interrupted" | "stale",
  ts: number,
  threadId?: string,
  threadName?: string,
  lastUserPrompt?: string,
  unseen?: boolean,
  paneId?: string,
  liveness?: "alive" | "exited" | "unknown"
}
```

External tools can send events with:

```bash
curl -X POST http://127.0.0.1:<port>/api/agent-event \
  -H 'content-type: application/json' \
  -d '{"agent":"my-agent","status":"running","tmuxSession":"work","threadId":"task-1"}'
```

The server can resolve the session from `tmuxSession` or `projectDir`.

### MuxProvider

The Rust trait lives in `packages/runtime-rs/src/mux.rs`. Keep methods synchronous because the tmux provider is command-driven and the server uses it as a simple control surface.

## Stack

- **Runtime**: Rust 2024 edition
- **TUI**: Ratatui 0.30 + Crossterm 0.29
- **Async/networking**: Tokio + tokio-websockets
- **Tests**: `cargo test`, with tmux E2E coverage in `apps/tui-rs/tests/tmux_e2e.rs`
- **Release**: GitHub Actions builds `opensessions-sidebar`, `opensessions-server`, and bundled `lazydiff` for release artifacts

## Development Guidelines

- **TDD**: Red-green-refactor, vertical slices, one test at a time. Tests verify behavior through public interfaces.
- **Sync tmux calls**: keep mux provider methods synchronous unless the architecture changes deliberately.
- **Preserve optimizations**: batched tmux calls, git cache with HEAD watchers, lightweight focus-only broadcasts, fixed-width sidebar repair, and per-client focus state.
- **Sidebar resize work**: before changing sidebar spawning, width sync, tmux resize handling, or `sidebar-coordinator`, read `docs/explanation/sidebar-behavior.md` and preserve those invariants unless you update the doc in the same change.
- **Built-in watchers in Rust runtime/server**: Amp, Claude Code, Codex, OpenCode, Pi, and Droid watcher parsing lives in `packages/runtime-rs/src/agent_watchers.rs` and server scanning lives in `apps/server-rs/src/lib.rs`.
- **Do not reintroduce pane-derived agent status**: panes can help focus/kill/routing, but watcher/API events are the source of agent status.

## Common Commands

```bash
cargo test --workspace                         # Run Rust tests
cargo test -p opensessions-sidebar             # Focused sidebar and UI module tests
cargo test -p opensessions-sidebar --test tmux_e2e -- --nocapture
cargo build --release                          # Build local dev binaries
cargo run -p opensessions-server               # Start server directly
cargo run -p opensessions-sidebar              # Start sidebar directly
bun test scripts/postinstall.test.ts           # Postinstall helper tests
```

Use `rtk` prefixes when running shell commands, per the user-level instructions.

## Adding A New Built-In Mux Provider

1. Implement the Rust `MuxProvider` trait in `packages/runtime-rs/src/mux.rs` or a new Rust module/package.
2. Register it in the server bootstrap if it should be built in.
3. Add focused command-runner tests and E2E coverage at the highest useful layer.
4. Document whether it is supported or experimental. Do not document a provider as supported until install/setup and sidebar behavior are stable.

## Adding Agent Support

1. Prefer an external HTTP integration first: POST `/api/agent-event` with stable `agent`, `threadId`, `projectDir` or `tmuxSession`, and `status`.
2. For built-in support, add parser/scanner logic in Rust and tests in `packages/runtime-rs` or `apps/server-rs`.
3. Preserve per-thread unseen semantics and pane focus clearing.
4. See `CONTRACTS.md` for integration examples.
