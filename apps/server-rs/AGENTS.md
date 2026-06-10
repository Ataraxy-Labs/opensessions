# Server crate rules

- Own authoritative runtime state: tmux facts, git facts, agent facts, metadata, and persisted user preferences.
- Treat WebSocket `ServerState` as the read model clients subscribe to; do not move rendering or UI focus policy here.
- Accept typed `ClientCommand`s, validate them, mutate server-owned state, then broadcast fresh state.
- Treat tmux pane geometry and terminal screen text as observations. They may inform repair, diagnostics, focus routing, and pane-agent mapping, but durable state changes must flow through explicit server-owned commands, trackers, or queries.
- Keep provider-specific details behind runtime mux/provider APIs; server orchestration may wire concrete tmux pieces, domain rules should not.
- Preserve local-first performance: batch tmux reads, reuse caches, avoid per-client recomputation when a shared snapshot will do.
- Do not depend on `apps/tui-rs` or renderer modules. If server and TUI both need logic, move pure code to `packages/runtime-rs`.
- Watcher/API events are the source of agent status. Panes may help route/focus/kill but must not become status truth.
- Reference files: `docs/explanation/architecture.md`, `docs/explanation/sidebar-behavior.md`, `CONTRACTS.md`.
