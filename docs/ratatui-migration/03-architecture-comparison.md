# 03 — Architecture Comparison

## Current architecture (TS)

```
┌─────────────────────────────────────────────────────┐
│  apps/server (TS, Bun)                              │
│  • PluginLoader                                     │
│  • MuxProvider (tmux/zellij) — owns tmux subprocess │
│  • AgentWatchers (Amp/ClaudeCode/Codex/OpenCode/Pi) │
│  • SessionTracker, MetadataStore, FocusCoordinator  │
│  • Bun.serve() WS endpoint :SERVER_PORT             │
│  • HTTP /quit fallback                              │
└─────────────────────┬───────────────────────────────┘
                      │ JSON over WS
   ┌──────────────────┼──────────────────┬───────...
┌──▼──────┐      ┌────▼──────┐      ┌────▼──────┐
│ apps/tui│      │ apps/tui  │      │ apps/tui  │   27 pane processes
│ Bun     │      │ Bun       │      │ Bun       │
│ + OpenTUI       │ + OpenTUI │      │ + OpenTUI │
│ + Solid │      │ + Solid   │      │ + Solid   │
│ ~80 MB  │      │ ~80 MB    │      │ ~80 MB    │
└─────────┘      └───────────┘      └───────────┘
```

**Per-pane TUI process responsibilities:**
- Detect mux context (tmux/zellij/none) from env vars at startup.
- Connect to WS, subscribe to `ServerMessage`s.
- Render with OpenTUI flexbox + Solid reactive primitives.
- Read/write `~/.config/opensessions/config.json` directly (`loadConfig`/`saveConfig`).
- Spawn `tmux` subprocesses for `getClientTty`, `getLocalSessionName`,
  `getLocalWindowId`, `refocusMainPane`.
- Spawn `open <url|dir>` on mouse clicks.
- Maintain optimistic local state for `switch-session`.
- 120 ms `setInterval` for spinner animation while any agent is `running`.

## Target architecture (Rust)

```
┌─────────────────────────────────────────────────────┐
│  apps/server  (TS, Bun)  — UNCHANGED                │
└─────────────────────┬───────────────────────────────┘
                      │ JSON over WS  (frozen)
   ┌──────────────────┼──────────────────┬───────...
┌──▼─────────┐  ┌─────▼───────┐    ┌─────▼───────┐    27 pane processes
│ apps/tui-rs│  │ apps/tui-rs │    │ apps/tui-rs │
│ Rust       │  │ Rust        │    │ Rust        │
│ + ratatui  │  │ + ratatui   │    │ + ratatui   │
│ + crossterm│  │ + crossterm │    │ + crossterm │
│ + tokio    │  │ + tokio     │    │ + tokio     │
│ + fastws   │  │ + fastws    │    │ + fastws    │
│ ~10 MB     │  │ ~10 MB      │    │ ~10 MB      │
└────────────┘  └─────────────┘    └─────────────┘
```

**Server stays identical.** No protocol changes (Phase 0 just adds a version
handshake — purely additive). All current TS clients (zellij, dev mode, etc.)
keep working.

## Process model differences

| Concern | TS (Bun + OpenTUI + Solid) | Rust (ratatui + crossterm + tokio) |
|---|---|---|
| **Render trigger** | Reactive: Solid signals re-evaluate dependent effects | Immediate-mode: `tokio::time::interval(16ms).tick()` redraws everything |
| **State store** | `createStore<SessionData[]>` + `createSignal`s | `App { state: Arc<Mutex<AppState>> }` shared between WS reader task and main task |
| **Layout** | Declarative JSX with flexbox: `<box flexDirection="column" flexGrow={1}>` | Programmatic: `Layout::vertical([Constraint::Length(1), Constraint::Fill(1)])` |
| **Event loop** | OpenTUI's renderer + Solid's scheduler interleaved | `tokio::select!` between WS messages, `EventStream` (kbd/mouse), and frame interval |
| **Mouse hit-testing** | Per-element `onMouseDown` handler bound to JSX | Manual: track `Rect`s by component, match on `(MouseEvent.col, .row)` |
| **Modal** | `<Show when={modal() === "..."}>` overlay rendered | Render base layer, then `Clear` + popup widget on top |
| **Drag-resize** | `onMouseMove` while button held; debounced FS write | Same: track drag state in `App`, throttle persistence call |

## Concurrency model

### TS

- Single-threaded JS event loop.
- All WS callbacks, keyboard events, timers run on the same thread.
- Solid's reactivity propagates synchronously within a tick.
- No lock contention possible.

### Rust

- Single-threaded `tokio::runtime::Builder::new_current_thread()`.
- One **WS reader task**: `loop { let frame = ws.read_frame().await; tx.send(parsed).await; }`.
- One **input task**: `crossterm::event::EventStream` → `tx.send(input_event).await`.
- **Main task**: `tokio::select!` over `rx.recv()` + `interval.tick()` → updates `App` → calls `terminal.draw(...)`.
- All shared state lives directly in `App`; no `Mutex` needed because
  current-thread runtime guarantees no parallel access.

```rust
// Sketch
pub async fn run(mut app: App, mut term: DefaultTerminal) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<Event>(64);
    spawn_ws_reader(tx.clone(), app.config.clone());
    spawn_input_reader(tx.clone());

    let mut frame_interval = interval(Duration::from_millis(16));

    while !app.should_quit {
        tokio::select! {
            Some(ev) = rx.recv() => app.handle(ev),
            _ = frame_interval.tick() => term.draw(|f| app.render(f))?,
        }
    }
    Ok(())
}
```

## What we explicitly preserve byte-for-byte

| Aspect | Strategy |
|---|---|
| Visible output | Snapshot tests vs `reference-snapshots/*.ansi` |
| Keybindings | 1:1 mapping table (see `08-input-keyboard.md`) |
| Mouse behavior | Hit-test rects matching the same regions (see `09-input-mouse.md`) |
| Persisted config schema | Same JSON file at same path, server-mediated writes (see `11-config-and-persistence.md`) |
| WS protocol | Frozen — Phase 0 just adds version handshake |
| Server behavior | Untouched |

## What we intentionally change

| Aspect | Old | New | Why |
|---|---|---|---|
| Theme palette source | TS const + bundled themes | Server pushes via WS state | Already most-of-the-way there; complete the data-daemon pattern |
| `loadConfig`/`saveConfig` | Per-client direct FS access | New WS commands `get-config`/`set-config-key` | Eliminates 27-way write contention; consistent state |
| `getClientTty` etc. | Per-client `tmux` subprocess spawn | New WS commands | One mux subprocess (in server), not 27 |
| Mouse-click `open` | Stays per-client (correct) | Same | Correct as-is |
| Spinner timer | Per-client `setInterval` | Per-client `tokio::time::interval` | Same model, just Rust |
