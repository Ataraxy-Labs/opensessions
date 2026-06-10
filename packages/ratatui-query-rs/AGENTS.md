# ratatui-query crate rules

- This crate is the terminal/Ratatui adapter over `query-core`, like `@tanstack/react-query` sits over `@tanstack/query-core`.
- Keep it dependency-light. It may depend on `query-core`; do not depend on opensessions crates, Tokio, WebSocket clients, or serde unless there is a concrete reusable need.
- Expose TUI-friendly query clients and result types by delegating to core; do not duplicate cache semantics here.
- Do not add retries, mutation caches, persistence, garbage collection, or async executors until a real caller needs them.
- Prefer value types, borrowed result views, and deterministic return order for tests and predictable UIs.
- Domain crates define their own key enums and implement `QueryKeyMatch`; this crate should not know about sessions, agents, panes, or tmux.
- Treat the public API as stable once used by apps; extend with small additive methods rather than changing semantics.
- Reference implementation: `~/.cache/checkouts/github.com/TanStack/query/packages/react-query/src`.
