# Rust Server/Runtime Migration Plan

The TypeScript server/runtime remains the behavioral oracle while the Rust implementation is built in small red/green slices. Any intentional behavior change must be documented before implementation; otherwise Rust should match the existing TypeScript wire format, config shape, mux semantics, and agent status mapping one to one.

## Goals

- Replace `packages/runtime` and `apps/server` with Rust equivalents without breaking the current WebSocket protocol.
- Keep the Ratatui sidebar and TypeScript sidebar compatible during migration.
- Preserve npm distribution: users get prebuilt binaries and do not need Cargo.
- Improve provider efficiency where the external behavior is identical, especially file watching.

## Provider Efficiency Direction

- Use one shared watcher service for filesystem-backed providers instead of provider-owned ad hoc watchers.
- Coalesce duplicate watch roots and avoid watching a child path when a parent recursive watch already covers it.
- Debounce filesystem bursts per provider/root before parsing transcripts.
- Keep polling only where the source requires it: Amp cloud discovery/API, OpenCode SQLite freshness, and fallback scans for missed recursive file events.
- Keep parser/status logic pure and fixture-driven so each provider can be ported one behavior at a time.

## Slices

1. **Foundation**: Rust protocol/config/server-resolution parity plus a watcher-plan model for all built-in providers.
2. **Pure Runtime State**: Port tracker, metadata store, session ordering, project-dir mapping, portless links, sidebar width sync, and coordinator state with TypeScript parity fixtures.
3. **Provider Parsers**: Port Amp, Claude Code, Codex, OpenCode, and Pi status parsers behind pure JSON/SQLite-row fixtures.
4. **Efficient Watch Service**: Implement native file watching with provider/root coalescing, debounce, and fallback scans; wire JSONL providers into it.
5. **Mux Contracts**: Define synchronous Rust mux traits and command-runner fakes, then port tmux and zellij behavior one method at a time.
6. **Server Shell**: Add Rust WebSocket/HTTP server that sends `hello`, computes read-only state, and accepts no-risk commands.
7. **Command Parity**: Port client command handlers one by one: focus, switch, theme/filter persistence, ordering, hide/show, metadata, pane identify, agent pane focus/kill, width reports, quit.
8. **Bootstrap/Distribution**: Ship `opensessions-server` beside `opensessions-sidebar`, opt in via launcher/env flag, then dogfood before making Rust default.

## Gates

- Every Rust behavior starts as a failing `cargo test`.
- Wire examples from `packages/runtime/src/shared.ts` must deserialize in Rust.
- Config tests must preserve `~/.config/opensessions/config.json` merge behavior.
- Watcher/provider tests must prove observable status events, not implementation details.
- Release binary size and dependency duplication are checked before defaulting to Rust.

## Current Checkpoint

- Rust workspace tests pass with 137 tests.
- Rust server/runtime have command, metadata, read-only state including tracker-backed session unseen flags, cached live ports/local-links, cached live git info, current-session visibility parity, pure sidebar coordinator width-authority/lifecycle parity, initial sidebar warmup labels, pure live-port attribution, tmux pane PID roots, tmux bootstrap, core tmux sidebar primitive coverage, basic sidebar hook routes, `/api/agent-event`, Pi runtime registry/API validation, and basic agent pane focus/kill routing with tmux Amp title resolution.
- Ratatui sidebar now renders through Ratatui terminals end to end: `Terminal<TestBackend>` for byte-for-byte snapshot coverage against the recorded OpenTUI ANSI references, and `Terminal<CrosstermBackend<Stdout>>` for the live interactive TUI. The dogfood loop still connects to the Rust server, applies focus/state/session messages, and sends basic commands for `q`, number keys, `Tab`/`Shift+Tab`, arrows, `Enter`, `r`, `n`, `d`, `x`, and `f`.

## Remaining Work

- Expand the Ratatui renderer beyond the current read-only parity surface into remaining interactive components and widgets.
- Finish advanced HTTP/sidebar parity around server-wired resize authority, warmup completion/adjusting flows, fan-out timing, and full agent pane focus/kill resolution/highlighting.
- Port the full sidebar lifecycle and width-authority state machine while preserving `docs/explanation/sidebar-behavior.md`.
- Fill server state parity for remaining unseen edge cases and filter edge cases.
- Port live watchers/providers for Amp, Claude Code, Codex, OpenCode, and full Pi pane-presence resolution.
- Complete zellij/cross-mux behavior, startup/distribution integration, and final TypeScript-to-Rust launch replacement.
