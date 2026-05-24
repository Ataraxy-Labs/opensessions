# 08 — Keyboard Input

The TS keyboard FSM (`useKeyboard` callback in `App`, lines 713–1099) is a
flat `switch` with state-based early returns. Port to a single `match` on
`crossterm::event::KeyEvent`.

## Crossterm event source

```rust
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers, KeyEventKind};
use tokio_stream::StreamExt;

let mut events = EventStream::new();

// In the main loop (tokio::select!):
Some(Ok(event)) = events.next() => match event {
    Event::Key(k) if k.kind == KeyEventKind::Press => app.handle_key(k),
    Event::Mouse(m) => app.handle_mouse(m),
    Event::Resize(w, h) => app.handle_resize(w, h),
    _ => {}
}
```

> ⚠ Filter `kind == Press` — Windows + some terminals emit `Release`/`Repeat`
> events that the TS code didn't see and shouldn't act on.

## Modal-aware dispatch (matches TS structure)

```rust
impl App {
    pub fn handle_key(&mut self, key: KeyEvent) {
        match self.modal {
            Modal::ThemePicker(_) => self.handle_theme_picker_key(key),
            Modal::ConfirmKill(_) => self.handle_confirm_kill_key(key),
            Modal::None => self.handle_normal_key(key),
        }
    }
}
```

## Confirm-kill modal

```rust
fn handle_confirm_kill_key(&mut self, key: KeyEvent) {
    let target = match &self.modal { Modal::ConfirmKill(t) => t.clone(), _ => return };
    match key.code {
        KeyCode::Char('y') => {
            self.send(ClientCommand::KillSession { name: target });
            self.modal = Modal::None;
        }
        _ => self.modal = Modal::None, // any other key cancels
    }
}
```

## Theme picker

```rust
fn handle_theme_picker_key(&mut self, key: KeyEvent) {
    let Modal::ThemePicker(st) = &mut self.modal else { return };
    match key.code {
        KeyCode::Up        => self.theme_picker_move(-1),
        KeyCode::Down      => self.theme_picker_move(1),
        KeyCode::Enter     => self.theme_picker_confirm(),
        KeyCode::Esc       => self.theme_picker_close(),
        KeyCode::Backspace => {
            if st.cursor_pos > 0 { st.query.remove(st.cursor_pos - 1); st.cursor_pos -= 1; }
            self.theme_picker_filter_changed();
        }
        KeyCode::Left      => st.cursor_pos = st.cursor_pos.saturating_sub(1),
        KeyCode::Right     => st.cursor_pos = (st.cursor_pos + 1).min(st.query.len()),
        KeyCode::Char(c)   => {
            st.query.insert(st.cursor_pos, c);
            st.cursor_pos += 1;
            self.theme_picker_filter_changed();
        }
        _ => {}
    }
}
```

## Normal mode (full table)

Verified against TS lines 713–1099:

| Key | Action | TS reference |
|---|---|---|
| `q` | quit (WS quit + HTTP fallback + 500ms timeout) | line 747 |
| `tab` | `togglePanelFocus()` | line ~770 |
| `enter` | activate (switch to focused session OR activate focused agent) | line ~780 |
| `→` (Right) | enter agents panel if any agents | line ~795 |
| `←` (Left) | back to sessions panel | line ~800 |
| `↑` (Up) | move local focus -1 OR move agent focus -1 (depending on panel) | line ~810 |
| `↓` (Down) | move local focus +1 OR move agent focus +1 | line ~820 |
| `j` | alias for `↓` | line ~825 |
| `k` | alias for `↑` | line ~830 |
| `f` | `cycleSessionFilter()` (all → agents → running) | line 259 |
| `d` | `hide-session` for focused | line ~840 |
| `x` | open confirm-kill modal for focused (or kill agent pane if in agents panel) | line ~850 |
| `r` | `refresh` (re-broadcast) | line ~860 |
| `t` | open theme picker | line ~870 |
| `n` | `new-session` | line ~875 |
| `D` (shift+d) | `dismiss-agent` (when in agents panel) | line ~880 |
| `Alt+↑` | `reorder-session -1` | line 737 |
| `Alt+↓` | `reorder-session +1` | line 740 |
| `escape` | dismiss modal (handled above) / no-op in normal | line 1145 |
| Number keys 1–9 | `switch-index N-1` | line ~890 |

## Implementation pattern

```rust
fn handle_normal_key(&mut self, key: KeyEvent) {
    // Alt+↑/↓ first (modifier-bearing)
    if key.modifiers.contains(KeyModifiers::ALT) {
        match key.code {
            KeyCode::Up   => return self.reorder_focused(-1),
            KeyCode::Down => return self.reorder_focused(1),
            _ => {}
        }
    }

    match key.code {
        KeyCode::Char('q')      => self.start_quit(),
        KeyCode::Tab            => self.toggle_panel_focus(),
        KeyCode::Enter          => self.activate_focus(),
        KeyCode::Right          => self.enter_agents_panel(),
        KeyCode::Left           => self.exit_agents_panel(),
        KeyCode::Up | KeyCode::Char('k')   => self.move_focus(-1),
        KeyCode::Down | KeyCode::Char('j') => self.move_focus(1),
        KeyCode::Char('f')      => self.cycle_session_filter(),
        KeyCode::Char('d')      => self.hide_focused_session(),
        KeyCode::Char('x')      => self.kill_or_open_modal(),
        KeyCode::Char('r')      => self.send(ClientCommand::Refresh),
        KeyCode::Char('t')      => self.open_theme_picker(),
        KeyCode::Char('n')      => self.send(ClientCommand::NewSession),
        KeyCode::Char('D')      => self.dismiss_focused_agent(),
        KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
            let idx = (c as u8 - b'1') as u32;
            self.send(ClientCommand::SwitchIndex { index: idx });
        }
        _ => {}
    }
}

fn move_focus(&mut self, delta: i8) {
    match self.panel_focus {
        PanelFocus::Sessions => self.move_local_focus(delta),
        PanelFocus::Agents   => self.move_agent_focus(delta),
    }
}
```

## Quit sequence

Faithful port of TS lines 747–759:

```rust
fn start_quit(&mut self) {
    // 1. Primary: WS message
    self.send(ClientCommand::Quit);

    // 2. Fallback: spawn fire-and-forget HTTP POST /quit
    let host = self.server_host.clone();
    let port = self.server_port;
    tokio::spawn(async move {
        // hand-rolled HTTP/1.1 to avoid pulling in reqwest:
        if let Ok(mut s) = TcpStream::connect((host.as_str(), port)).await {
            let _ = s.write_all(format!(
                "POST /quit HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Length: 0\r\n\r\n"
            ).as_bytes()).await;
        }
    });

    // 3. Last-resort timeout: 500 ms
    self.quit_deadline = Some(Instant::now() + Duration::from_millis(500));
}
```

The main loop checks `quit_deadline` on every interval tick; if exceeded,
`should_quit = true`.

## Crossterm capability negotiation

Enable mouse + bracketed paste + focus events on startup:

```rust
use crossterm::execute;
use crossterm::event::{EnableMouseCapture, EnableBracketedPaste, EnableFocusChange};
use std::io::stdout;

execute!(
    stdout(),
    EnableMouseCapture,
    EnableBracketedPaste,
    EnableFocusChange,
)?;
```

Disable on exit (also done by `ratatui::restore()`).
