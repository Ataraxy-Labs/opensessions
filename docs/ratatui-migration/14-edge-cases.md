# 14 — Edge Cases & Quirks

A grab-bag of subtle behaviors that the Rust port must replicate. Each is
something the TS implementation handles correctly today; missing any of them
will cause user-visible regressions.

## 1. Optimistic local updates

`switchToSession()` mutates local state **before** server confirms (TS line
308). Rapid Tab presses must feel instant; the server's `state` broadcast
reconciles eventually. Mirror in Rust — see `05-state-management.md`.

**Pitfall**: don't blindly overwrite `App.focused_session` on incoming
`state` messages if the user just optimistically moved focus and the server
hasn't caught up yet. TS uses `resolveSyncedFocus(...)` (in `focus-sync.ts`)
to reconcile. **Port verbatim**:

```rust
// focus-sync.rs — port `resolveSyncedFocus`
pub fn resolve_synced_focus(
    server_focused: Option<&str>,
    server_current: Option<&str>,
    local_focused: Option<&str>,
    local_my_session: Option<&str>,
    last_optimistic_at: Instant,
) -> Option<String> {
    // Match the TS algorithm in apps/tui/src/focus-sync.ts
    todo!()
}
```

The TS file is 11 lines — trivial to port. Includes a unit test
(`focus-sync.test.ts`) — port that too as a Rust `#[test]`.

## 2. Re-identify on session move

If a tmux session is renamed or the pane moves to a different session, the
server tracks the wrong session for our pane. The TS code re-identifies
proactively whenever `focus` changes (lines 339–346):

```ts
function maybeReIdentify() {
  const sessionName = getLocalSessionName();
  const windowId = getLocalWindowId();
  if (!sessionName || sessionName === "_os_stash") return;
  if (sessionName !== lastIdentifiedSessionName || windowId !== lastIdentifiedWindowId) {
    reIdentify();
  }
}
```

The `_os_stash` sentinel is a special tmux session used during session moves.
**Honor it.** Don't identify-pane while in `_os_stash`.

## 3. Stash restore (refocusMainPane window-id resolution)

The TS `refocusMainPane()` (lines 154–182) explicitly handles the case where
the pane has been moved to a different window than the original. It calls
`tmux display-message -t <pane> -p '#{window_id}'` to find the **current**
window. Important during stash/unstash.

When we move this to server-side, the server already has the pane → window
mapping; one round-trip.

## 4. Flash messages

`flash("filter: agents")` displays a transient overlay for 1.2 s after
actions like `cycleSessionFilter`. Implementation:

```rust
pub struct Flash { pub text: String, pub expires_at: Instant }

fn flash(&mut self, text: &str) {
    self.flash = Some(Flash {
        text: text.into(),
        expires_at: Instant::now() + Duration::from_millis(1200),
    });
}

// On every render, drop expired:
if let Some(f) = &self.flash {
    if Instant::now() >= f.expires_at { self.flash = None; }
}
```

Render position: bottom-right of the sidebar, dim background, single-line.

## 5. Spinner only when needed

`createEffect` in TS only starts the 120ms interval when `hasRunning() ||
initializing()`. Mirror in Rust — gate the interval entirely:

```rust
let spinner_interval = self.has_running() || self.initializing;
if spinner_interval {
    if last_spin_tick.elapsed() >= Duration::from_millis(120) {
        self.spin_idx = (self.spin_idx + 1) % SPINNER.len();
        last_spin_tick = Instant::now();
    }
}
```

Saves CPU when nothing's running.

## 6. Detail panel height stash on session switch

When focus moves to a different session, load that session's persisted
height (TS line 694). Use the `detail_panel_heights` HashMap.

```rust
// In handle of focus change:
fn on_focus_change(&mut self) {
    if let Some(name) = self.detail_panel_session_name() {
        if let Some(&h) = self.detail_panel_heights.get(name) {
            self.detail_panel_height = h.max(MIN_DETAIL_PANEL_HEIGHT);
        } else {
            self.detail_panel_height = DEFAULT_DETAIL_PANEL_HEIGHT;
        }
    }
}
```

## 7. Resize debug log format

Match `/tmp/opensessions-tui-resize.log` exactly (already documented in
`09-input-mouse.md`). The TS format:

```
[<ISO8601>] [pid:<pid>] <message> {<key>:<json-encoded-value>,...}
```

For multi-host debugging, the Rust binary writes to the same path unless
overridden by `OPENSESSIONS_RESIZE_LOG` env var. Same format.

## 8. Agent click log

Lines 377–378: agent clicks log to `/tmp/opensessions-tui-agent-click.log`.
Same format. Same path.

## 9. Window id gating for refocus

`process.env.REFOCUS_WINDOW` is a fast-path env var the launcher script can
set to skip the tmux query. Honor it in Rust.

## 10. SIGWINCH / resize handling

Crossterm's `EventStream` emits `Event::Resize(w, h)` on terminal resize.
Update `App.terminal_width` and `App.terminal_height`; ratatui re-derives
the layout next frame.

## 11. Bracketed paste

We don't accept paste input, but enabling it prevents paste-as-keystroke
behavior in Crossterm. Already enabled in `08-input-keyboard.md`.

## 12. Focus events

Some terminals send focus-in/focus-out events. We use these to:
- Pause the spinner when blurred (optional polish, defer).
- Cancel modals when blurred (matches OpenTUI default? verify).

## 13. CJK / wide chars in session names

Session names can contain wide chars. Use `unicode-width` for **content**
measurement, hardcoded width=1 for **icons** (see themes.md note).

```rust
use unicode_width::UnicodeWidthStr;
let visible_cols = "你好".width();  // 4
```

## 14. Theme picker preview during open

When the picker opens, the *currently active* theme's name is highlighted
(not "catppuccin-mocha"). On reopen, restore the last selection.

## 15. `_os_stash` session name

Filter from session list rendering. The server may include it in `sessions[]`
during a move; don't render it.

```rust
fn filtered_sessions(&self) -> impl Iterator<Item = &SessionData> {
    self.sessions.iter()
        .filter(|s| s.name != "_os_stash")
        .filter(|s| /* user filter */)
}
```

## 16. Transparent background

OpenTUI honors `backgroundColor="transparent"` on the modal overlay
(line 1175). Ratatui doesn't have a "transparent" Color. Render with no `bg`
style at all — the underlying buffer cells stay as they were.

## 17. Empty-state messages

- "No matches" inside theme picker when filter has no hits.
- "(no agents)" when focused session has no agents.
- "Connecting…" / `initLabel` while server is initializing.

Match TS strings verbatim.

## 18. Connection-lost banner

When `connected = false` (after WS disconnect), render a thin banner across
the top in `red`. The TS code doesn't have this explicitly; consider adding
it — useful UX, no protocol change.

## 19. Cursor visibility

OpenTUI hides the cursor by default. Ratatui's `terminal.draw()` does the
same. The theme picker shows a cursor inside its input box; restore by
calling `frame.set_cursor_position(...)` only while the modal is open.

## 20. Truecolor detection

Both OpenTUI and our Rust client assume `$TERM` is truecolor-capable. We
don't fall back to 256-color. Documented in README.md alongside the snapshots.
If a user runs in a non-truecolor terminal, output will look wrong; the same
is true today.
