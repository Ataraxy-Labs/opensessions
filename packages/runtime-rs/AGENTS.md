# Runtime crate rules

- This crate is shared Rust policy and contracts, not an app. Both server and TUI may depend on it; it must not depend on either app.
- Put protocol types, config parsing, mux traits, pure projections, trackers, and provider-neutral rules here.
- Keep mux provider calls synchronous unless the architecture changes deliberately; tmux is command-driven and server code expects a simple control surface.
- Prefer pure functions with borrowed inputs for projections and parsers. Avoid cloning hot session/agent collections unless ownership is required.
- Projection code derives read-only UI rows from typed state. Tmux geometry and terminal frames are observation inputs only; keep authority boundaries explicit in protocol types, trackers, and provider-neutral policies.
- Runtime code may know domain language like session, worktree group, pane, agent event, unseen, and projection; it should not know Ratatui widgets or WebSocket handlers.
- Built-in watcher parsing should be fixture-driven and preserve per-thread unseen semantics.
- When a rule is needed by only one app, keep it in that app. Move code here only when it is genuinely shared policy or a stable contract.
- Reference files: `CONTRACTS.md`, `docs/explanation/architecture.md`, `docs/rust-server-runtime-migration.md`.
