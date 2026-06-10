# query-core crate rules

- This crate owns framework-agnostic query-cache semantics inspired by TanStack Query core.
- Keep it dependency-free except `std`; no opensessions, Ratatui, Tokio, WebSocket, tmux, serde, or UI concepts.
- Provide typed query keys, status/fetch state, stale invalidation, observer counts, and active-refetch decisions.
- Domain and UI adapter crates define their own key enums and implement `QueryKeyMatch`.
- Keep return ordering deterministic; if a method returns keys, prefer `Ord` over hash-map iteration order.
- Do not add retries, mutation caches, persistence, GC, async execution, or transport behavior until an adapter has a concrete need.
- Treat public semantics as stable and small. Add capabilities incrementally rather than widening the core prematurely.
- Reference: `~/.cache/checkouts/github.com/TanStack/query/packages/query-core/src`.
