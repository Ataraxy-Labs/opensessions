# OpenSessions → Ratatui Migration

This directory contains the **complete specification** for porting `apps/tui`
(Bun + OpenTUI + Solid) to Rust + Ratatui, while keeping `apps/server` and the
WS protocol untouched.

## Read in order

| # | File | Topic |
|---|---|---|
| 00 | [`00-index.md`](./00-index.md) | This file |
| 01 | [`01-overview.md`](./01-overview.md) | Goals, success criteria, phased plan |
| 02 | [`02-lightweight-stack.md`](./02-lightweight-stack.md) | **Crate selection** — fastest, lowest-RAM Rust libs for every layer |
| 03 | [`03-architecture-comparison.md`](./03-architecture-comparison.md) | Current TS architecture vs target Rust architecture |
| 04 | [`04-protocol-and-types.md`](./04-protocol-and-types.md) | WS protocol → serde structs (1:1 type mapping) |
| 05 | [`05-state-management.md`](./05-state-management.md) | Solid signals/stores → Rust `App` state |
| 06 | [`06-rendering-and-layout.md`](./06-rendering-and-layout.md) | OpenTUI flex layout → ratatui `Layout`/`Constraint` |
| 07 | [`07-components.md`](./07-components.md) | Per-component port plan (App, ThemePicker, DetailPanel, AgentListItem, SessionCard) |
| 08 | [`08-input-keyboard.md`](./08-input-keyboard.md) | `useKeyboard` FSM → `crossterm::KeyEvent` matcher |
| 09 | [`09-input-mouse.md`](./09-input-mouse.md) | Mouse handlers + drag-resize → `crossterm::MouseEvent` |
| 10 | [`10-mux-and-system.md`](./10-mux-and-system.md) | tmux SDK calls + `Bun.spawn(["open", ...])` |
| 11 | [`11-config-and-persistence.md`](./11-config-and-persistence.md) | `loadConfig`/`saveConfig` |
| 12 | [`12-themes.md`](./12-themes.md) | Catppuccin palette, status icons, theme picker logic |
| 13 | [`13-server-bootstrap.md`](./13-server-bootstrap.md) | `ensureServer()` launcher |
| 14 | [`14-edge-cases.md`](./14-edge-cases.md) | Flash messages, optimistic updates, re-identify, debug logs, unicode width |
| 15 | [`15-testing.md`](./15-testing.md) | `TestBackend` snapshot tests + reference `.ansi` diffs |
| 16 | [`16-distribution.md`](./16-distribution.md) | Cargo build, multi-arch artifacts, npm postinstall |
| 17 | [`17-feasibility-matrix.md`](./17-feasibility-matrix.md) | Per-feature feasibility verdict |

## Reference snapshots (visual ground truth)

See [`README.md`](./README.md) and [`reference-snapshots/`](./reference-snapshots/)
for the pixel-for-pixel reference the Rust client must reproduce.

## TL;DR

- **Architecture stays identical**: TS server (unchanged) ↔ WS protocol (frozen, see `03`) ↔ Rust client (`apps/tui-rs`).
- **Every TS feature has a ratatui equivalent.** No blockers found. (See `16-feasibility-matrix.md`.)
- **The tricky bits** (and where to look):
  - Mouse hit-testing with no per-element handlers → manual rect tracking (see `08`)
  - Solid reactivity → immediate-mode redraw with `App` struct (see `04`)
  - OpenTUI flex layout → `Layout::vertical([…Constraint…])` (see `05`)
  - Per-pane re-identify on session moves → background poll task (see `13`)
- **Memory expectation:** ~73 MB → ~10–15 MB per process. 27 panes: ~2.0 GB → ~270–400 MB.
