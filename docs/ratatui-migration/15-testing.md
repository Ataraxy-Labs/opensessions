# 15 — Testing

The migration's correctness rests on **byte-for-byte snapshot tests** against
the recorded reference snapshots. Plus unit tests for pure logic.

## Test layers

| Layer | Tool | Scope |
|---|---|---|
| Unit | `#[test]` | Pure functions: `build_sparkline`, `wrap_local_links`, `truncate_left`, focus-sync, server-key hash |
| Snapshot | `ratatui::backend::TestBackend` + custom diff | Render output vs reference `.ansi` |
| Integration | tokio test + mock WS server | Full event loop with scripted server messages |
| Visual | reference PNGs | Sanity-check by humans during code review |

## Snapshot test pattern

```rust
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[test]
fn matches_attached_session_list_snapshot() {
    let app = App::from_recorded_state("fixtures/state-attached-session-list.json");
    let backend = TestBackend::new(35, 56);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();

    let actual = terminal.backend().buffer();
    let expected = parse_ansi(include_str!("../reference-snapshots/pane-attached-session-list.ansi"));

    assert_buffers_eq(actual, &expected);
}
```

Where:

- `from_recorded_state` parses a captured `ServerState` JSON dump (one-shot
  recording from a real server).
- `parse_ansi` is a small helper using the `vt100` crate (or hand-rolled
  CSI parser) to convert ANSI bytes into a `(width, height) → Cell` grid.
- `assert_buffers_eq` compares cell-by-cell with a clear diff message.

### Recording fixtures

```sh
# One-time: dump server state to a JSON file
curl http://127.0.0.1:7391/debug-state > apps/tui-rs/fixtures/state-attached.json
```

(Add a `/debug-state` endpoint to the TS server — Phase 0 work item.)

## ANSI buffer comparison

The `vt100` crate (or `terminfo`) parses ANSI sequences into a virtual screen
state. Compare against ratatui's `Buffer`:

```rust
fn assert_buffers_eq(actual: &Buffer, expected: &VtScreen) {
    for y in 0..actual.area.height {
        for x in 0..actual.area.width {
            let a = &actual[(x, y)];
            let e = expected.cell(x, y);
            assert_eq!(
                (a.symbol(), a.style().fg, a.style().bg, a.style().add_modifier),
                (e.contents(), e.fgcolor(), e.bgcolor(), e.modifiers()),
                "mismatch at ({x},{y})"
            );
        }
    }
}
```

If `vt100` is too heavy a dev-dep (~200 KB), hand-roll a minimal CSI parser
in `tests/common/ansi.rs` (~100 LOC).

## Unit test examples

```rust
#[test]
fn server_key_hash_matches_ts() {
    // Verified against TS implementation
    assert_eq!(hash_server_key("/private/tmp/tmux-501/default"), 12345);  // record from TS
}

#[test]
fn build_sparkline_matches_ts() {
    let now = 1_700_000_000_000;
    let timestamps: Vec<u64> = (0..30).map(|i| now - i * 60_000).collect();
    let s = build_sparkline_at(&timestamps, 10, 30 * 60 * 1000, now);
    assert_eq!(s, "▁▁▁▁▁▁▁▁▁▁");  // record expected from TS
}

#[test]
fn truncate_left_handles_unicode() {
    assert_eq!(truncate_left("hello world", 5), "…orld");
    assert_eq!(truncate_left("你好世界", 4), "…世界");
}
```

## Integration test pattern

```rust
#[tokio::test(flavor = "current_thread")]
async fn handles_state_then_focus_messages() {
    let (server, server_url) = mock_ws_server().await;
    let mut app = App::new(server_url);

    server.send(ServerMessage::State(ServerState {
        sessions: vec![mock_session("alpha"), mock_session("beta")],
        focused_session: Some("alpha".into()),
        // ...
    })).await;

    app.tick_until_state_received().await;
    assert_eq!(app.sessions.len(), 2);
    assert_eq!(app.focused_session.as_deref(), Some("alpha"));

    // Simulate Tab key
    app.handle_key(key('\t'));
    let cmd = server.recv_command().await;
    assert!(matches!(cmd, ClientCommand::FocusSession { name } if name == "beta"));
}
```

The mock server uses `tokio-tungstenite` (acceptable as a **dev-dependency**;
keeps the production binary on `fastwebsockets`).

## Performance test

```rust
#[test]
#[ignore]   // run with `cargo test --release -- --ignored perf`
fn rss_under_20mb_after_render() {
    let app = App::from_recorded_state("fixtures/state-with-100-sessions.json");
    let backend = TestBackend::new(80, 200);
    let mut terminal = Terminal::new(backend).unwrap();

    for _ in 0..1000 { terminal.draw(|f| app.render(f)).unwrap(); }

    let rss = current_rss_kb();
    assert!(rss < 20_000, "RSS = {rss} KB, expected < 20 MB");
}

fn current_rss_kb() -> u64 {
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/self/status").unwrap();
        for line in s.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                return rest.trim().split_whitespace().next().unwrap().parse().unwrap();
            }
        }
        0
    }
    #[cfg(target_os = "macos")]
    {
        // Use libc::task_info or shell out to ps
        unimplemented!()
    }
}
```

## CI matrix

```yaml
# .github/workflows/tui-rs.yml
jobs:
  test:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest]
        toolchain: [stable, nightly]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --workspace
      - run: cargo test --release --workspace -- --ignored perf
      - run: cargo bloat --release --crates  # informational
      - run: cargo audit
      - run: cargo tree --duplicates  # zero acceptable
```

## Dogfooding criteria

Before flipping `OPENSESSIONS_TUI=rs` to default:

- 27 panes running for 24 h: zero crashes, zero memory leaks (RSS stable ±5%).
- All snapshot tests green for at least 5 days of branch builds.
- One full week of personal use without falling back to TS client.
- Two external testers (community PRs) confirm parity on different terminals
  (Ghostty, Warp, iTerm2, Alacritty, Kitty).
