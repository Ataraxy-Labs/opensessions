# Sidebar TUI crate rules

- This crate is the client UI: fetch/subscribe to server state over WebSocket, render it, and send typed commands back.
- Keep server state separate from local UI state. Server state is sessions, agents, theme, filters, widths, and timestamps; local UI state is focus, scroll, modals, hover, flashes, and pending optimistic intent.
- Server/query state is authority. Tmux pane geometry and terminal frames are observations; never promote observed terminal width/text/layout into durable app state unless it came through a typed server command/query.
- Do not inspect tmux, git, watcher files, or agent logs directly from the TUI. Ask the server with `ClientCommand` or render the latest `ServerState`.
- Projection means read-only display rows derived from deeper state. Shared projection rules belong in `opensessions-runtime`; rendering geometry stays here.
- Snapshot tests against `docs/ratatui-migration/reference-snapshots/*.ansi` are the render-fidelity gate.
- Preserve terminal performance: avoid cloning session lists during render, redraw only when input/state/animation requires it, and keep layout calculations linear.
- Use `ratatui-query` for query freshness, staleness, and invalidation semantics; do not grow a parallel cache inside `App`.
- Do not add a reusable UI crate unless a second real UI consumes it; Ratatui-specific app/render code should stay here.
- Reference files: `docs/ratatui-migration/00-index.md`, `docs/explanation/architecture.md`, `docs/explanation/sidebar-behavior.md`.
