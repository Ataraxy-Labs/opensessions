# 17 — Feasibility Matrix

Final per-feature verdict. Every TS/OpenTUI feature in `apps/tui` mapped to
its Rust/ratatui equivalent. **No blockers found.**

Legend:
- ✅ Direct mapping, trivial port.
- 🟡 Possible but requires ~50–200 LOC of mapping/glue code.
- 🔴 Blocker (none found).
- 📦 Bonus: improvements over current TS implementation.

## Core architecture

| Feature | Verdict | Notes |
|---|---|---|
| Server-mediated state daemon | ✅ | Already exists; protocol unchanged |
| WS client | ✅ | `tokio-websockets` is strict, Tokio-native, small, and avoids dependency duplication; `fastwebsockets` remains viable |
| JSON parsing | ✅ | `serde_json` |
| Single-threaded event loop | ✅ | `tokio` current-thread runtime + `EventStream` |
| Auto-reconnect on WS close | ✅ | Manual reconnect loop; ~30 LOC |
| `ensureServer()` bootstrap | ✅ | `std::process::Command::spawn` + port-poll |

## UI primitives

| Feature | Verdict | Notes |
|---|---|---|
| Truecolor (`#rrggbb` → SGR) | ✅ | `Color::Rgb` |
| Bold / Dim modifiers | ✅ | `Modifier::BOLD`, `Modifier::DIM` |
| Background color fills | ✅ | `Style::bg` |
| Flex column/row layout | ✅ | `Layout::vertical/horizontal` + `Constraint` |
| Padding | ✅ | `Block::padding(Padding::new(...))` or manual rect inset |
| Borders (rounded) | ✅ | `Block::bordered().border_type(BorderType::Rounded)` |
| Scrollable list | ✅ | `List` + `ListState` |
| Custom multi-line list items | ✅ | Custom `Widget` impl OR pre-flatten into `Vec<Line>` |
| Scrollbar widget | ✅ | `Scrollbar` widget (built-in) |
| Sparkline | ✅ | Built-in `Sparkline` widget OR port `buildSparkline` verbatim |
| Spinner (Braille) | ✅ | Const slice + index |
| Truncation marker `…` | ✅ | Manual via `unicode-width` |
| Modals (popup over base) | ✅ | `Clear` widget + render on top |
| Centered overlay rect | ✅ | Trivial helper |
| Text input field | 🟡 | Hand-roll (~100 LOC) OR pull `tui-input` (~30 KB) |
| OSC 8 hyperlinks | ✅ (not needed) | We use mouse hit-tests, not OSC 8 |

## Input

| Feature | Verdict | Notes |
|---|---|---|
| Keyboard event stream | ✅ | `crossterm::event::EventStream` (async) |
| Modifier keys (Alt+↑/↓) | ✅ | `KeyModifiers::ALT` |
| Character keys | ✅ | `KeyCode::Char(c)` |
| Special keys (Tab, Enter, Esc) | ✅ | `KeyCode::Tab`, `Enter`, `Esc` |
| Mouse Down / Up / Drag / Move | ✅ | `MouseEventKind::Down/Up/Drag/Moved` |
| Mouse hit-testing | 🟡 | Manual `ClickZone` array; record on render, lookup on click |
| Drag-resize for detail panel | ✅ | Track `DetailResizeState` while button held |
| Scroll wheel | ✅ | `MouseEventKind::ScrollUp/Down` |
| Focus events | ✅ (optional) | `EnableFocusChange` + `Event::FocusGained/Lost` |
| Bracketed paste | ✅ | `EnableBracketedPaste` |

## State management

| Feature | Verdict | Notes |
|---|---|---|
| Reactive store (sessions) | ✅ | Plain `Vec<SessionData>` in `App` |
| Memoized derived state | ✅ | Compute on read in render (cheap) |
| Modal state machine | ✅ | `enum Modal` |
| Optimistic local updates | ✅ | Set state, then send command; reconcile via state broadcast |
| Re-identify on session change | ✅ | Periodic check (2 s) or on focus change |
| `_os_stash` sentinel handling | ✅ | Filter from list; skip identify-pane |
| Flash messages with TTL | ✅ | `Option<Flash>` with `Instant` expiry |
| Spinner only when needed | ✅ | Gate the interval on `has_running()` |
| Detail panel height per-session | ✅ | `HashMap<String, u16>` |

## System integration

| Feature | Verdict | Notes |
|---|---|---|
| `tmux display-message` calls | ✅ | `std::process::Command` (or move to server) |
| `tmux select-pane` (refocus) | ✅ | Same; preferably server-side |
| `zellij action move-focus` | ✅ | Same |
| `Bun.spawn(["open", url])` | ✅ | `std::process::Command::new("open").spawn()` |
| HTTP `POST /quit` fallback | ✅ | Hand-rolled HTTP/1.1 over `tokio::net::TcpStream` |
| `~/.config/opensessions/config.json` | ✅ | `std::fs` + `fs2::FileExt` lock OR server-mediated |
| `/tmp/opensessions-tui-resize.log` | ✅ | `std::fs::OpenOptions::new().append(true)` |
| `/tmp/opensessions-tui-agent-click.log` | ✅ | Same |
| Env vars (TMUX, ZELLIJ_*, OPENSESSIONS_*) | ✅ | `std::env::var` |
| Server-key hash | ✅ | Port verbatim; golden test against TS |
| PID file | ✅ | `std::fs` + `libc::kill(pid, 0)` for liveness |

## Themes

| Feature | Verdict | Notes |
|---|---|---|
| Catppuccin palette | ✅ | Hardcoded fallback + dynamic from server |
| Status colors per AgentStatus | ✅ | `HashMap<AgentStatus, Color>` |
| Status icons per AgentStatus | ✅ | `HashMap<AgentStatus, &'static str>` |
| Theme picker with search | ✅ | Hand-rolled input field |
| Preview vs apply | ✅ | `theme_before_preview: Option<Theme>` |
| Custom partial themes | 🟡 | Defer; server resolves to full theme before broadcast |

## Protocol

| Variant | Verdict | Notes |
|---|---|---|
| All 6 `ServerMessage` variants | ✅ | `serde(tag = "type", rename_all = "kebab-case")` |
| All 18 `ClientCommand` variants | ✅ | Same pattern |
| Schema version handshake (Phase 0) | ✅ | Add `hello` message; trivial |
| `SessionData` + nested types | ✅ | All `Deserialize` derives |
| `AgentEvent` + `AgentStatus` enum | ✅ | Serde with `kebab-case` for `tool-running` |
| `MetadataTone` enum | ✅ | `lowercase` rename |
| `LocalLink` + kind | ✅ | Same |

## Distribution

| Feature | Verdict | Notes |
|---|---|---|
| Single static binary | ✅ | `cargo build --release` per target |
| Multi-arch GH release | ✅ | GH Actions matrix; standard pattern |
| npm postinstall download | ✅ | Standard pattern (esbuild, swc, rg) |
| Homebrew tap | ✅ (later) | Phase 8+ |
| Cross-platform open | ✅ | `#[cfg(target_os = ...)]` for `open`/`xdg-open`/`explorer` |

## Testing

| Feature | Verdict | Notes |
|---|---|---|
| Snapshot tests vs `.ansi` | ✅ | `TestBackend` + `vt100`/hand-rolled CSI parser |
| Unit tests for pure fns | ✅ | `#[test]` |
| Integration tests w/ mock server | ✅ | `tokio-tungstenite` as dev-dependency |
| RSS measurement | ✅ | `/proc/self/status` (Linux) or `task_info` (macOS) |
| CI matrix | ✅ | GH Actions on linux + macos |

## Bonus improvements over TS

| 📦 Improvement | Effort |
|---|---|
| **No 27-way config write race** (server-mediated) | Phase 0 + 11 |
| **No 27 tmux subprocess spawns** (server-mediated mux queries) | Phase 6 |
| **Schema-version handshake** prevents silent breakage | Phase 0 |
| **Lockfile around `ensureServer`** prevents 27-way startup race | Phase 6 |
| **Static binary**, no Bun/Node runtime cost on the client | always |
| **No GC pauses** during rapid resize | always |
| **Faster cold start** (~10 ms vs ~150–250 ms) makes lazy-mount option viable in future | always |
| **Snapshot tests** in CI catch any regression byte-for-byte | always |

## Out-of-scope (not migrated)

- Replacing the TS server.
- Replacing the TS mux providers.
- Removing OpenTUI from `node_modules` until Phase 8.
- Cross-protocol-version coexistence (binary is single-version).
- Cross-version simultaneous TS-and-Rust client running (not supported nor needed).

## Risks & mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| Subtle pixel mismatch on niche terminal | Medium | Snapshot test catches; capture more terminal-specific references during dogfooding |
| `unicode-width` disagreement on icons | Medium | Hardcode width for known icons (see `12-themes.md`) |
| Mouse hit-test drift after layout refactor | Medium | Add hit-zone unit tests with fixed coordinates |
| Distribution: postinstall download fails behind firewall | Low | Document `OPENSESSIONS_BIN_PATH` env override; ship a fallback build instruction |
| Server schema drift between TS and generated Rust | Low | `ts-rs` codegen + CI step |

## Verdict

**🟢 GO.** Every feature has a clear path. Nothing requires rewriting OpenTUI,
forking ratatui, or hand-rolling the renderer. The migration is purely
mechanical work plus careful test coverage.

Estimated total effort:
- Phases 1–5 (skeleton through theme picker): ~1500–2500 LOC of Rust.
- Phase 6 (mux integration + ensureServer): ~300 LOC.
- Phase 7 (multi-theme + flag-gated default): trivial after Phase 6.
- Phase 8 (delete TS client): cleanup only.

The hard work was done by the existing data-daemon architecture; the Rust
port just swaps out the client renderer.
