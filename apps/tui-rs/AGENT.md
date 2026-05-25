# Ratatui TUI Agent Discipline

Snapshot tests against `docs/ratatui-migration/reference-snapshots/*.ansi` are the ultimate gate for render fidelity. Do not mark read-only rendering complete unless the Ratatui output matches the documented reference snapshots.

Follow the architectural decisions in `docs/ratatui-migration/00-index.md` through `17-feasibility-matrix.md`. Do not substitute crates, async runtime features, distribution strategy, layout behavior, protocol shape, or performance targets unless the migration docs are updated by a human reviewer first.

When practical, add tests for new behavior — but pragmatic feature parity work may land without a prior failing test.
