# 09 — Mouse Input

OpenTUI binds `onMouseDown`/`onMouseMove`/`onMouseUp` to JSX nodes so the
runtime knows which element was hit. Ratatui has no DOM — we record hit zones
during render and look them up on click.

## Hit-zone pattern

```rust
pub enum ClickZone {
    SessionRow { rect: Rect, name: String },
    OpenDir    { rect: Rect, dir: String, only_when_focused: String }, // session name
    OpenUrl    { rect: Rect, url: String },
    AgentRow   { rect: Rect, session: String, agent: String,
                 thread_id: Option<String>, thread_name: Option<String> },
    DetailResize { rect: Rect },
    ThemeOption { rect: Rect, name: String },  // inside theme-picker modal
    SessionFilterCycle { rect: Rect },         // header click → cycle filter
}

impl App {
    pub fn click_zones_clear(&mut self) { self.click_zones.clear(); }
    pub fn record_zone(&mut self, z: ClickZone) { self.click_zones.push(z); }

    pub fn hit_test(&self, col: u16, row: u16) -> Option<&ClickZone> {
        self.click_zones.iter().rev().find(|z| z.rect().contains(col, row))
    }
}

trait RectContains { fn contains(&self, col: u16, row: u16) -> bool; }
impl RectContains for Rect {
    fn contains(&self, c: u16, r: u16) -> bool {
        c >= self.x && c < self.x + self.width
            && r >= self.y && r < self.y + self.height
    }
}
```

> **Order matters**: iterate in reverse so later (more-specific) zones win
> over earlier (broader) zones. E.g., a link inside a session card should
> beat the card-level click.

## Crossterm event types

```rust
use crossterm::event::{MouseEvent, MouseEventKind, MouseButton};

pub fn handle_mouse(&mut self, ev: MouseEvent) {
    match ev.kind {
        MouseEventKind::Down(MouseButton::Left)  => self.on_left_down(ev.column, ev.row),
        MouseEventKind::Up(MouseButton::Left)    => self.on_left_up(ev.column, ev.row),
        MouseEventKind::Drag(MouseButton::Left)  => self.on_drag(ev.column, ev.row),
        MouseEventKind::Moved                    => self.on_move(ev.column, ev.row),
        MouseEventKind::ScrollUp                 => self.scroll(-3),
        MouseEventKind::ScrollDown               => self.scroll(3),
        _ => {}
    }
}
```

## Left-click dispatch

Mirrors the TS `onMouseDown` handlers:

```rust
fn on_left_down(&mut self, col: u16, row: u16) {
    let Some(zone) = self.hit_test(col, row).cloned() else { return };
    match zone {
        ClickZone::SessionRow { name, .. } => {
            self.focused_session = Some(name.clone());
            self.send(ClientCommand::FocusSession { name });
        }
        ClickZone::OpenDir { dir, only_when_focused, .. } => {
            // Match TS behavior: only open dir when row is focused
            if self.focused_session.as_deref() == Some(only_when_focused.as_str()) {
                spawn_open(&dir);
            }
        }
        ClickZone::OpenUrl { url, .. } => spawn_open(&url),
        ClickZone::AgentRow { session, agent, thread_id, thread_name, .. } => {
            // First switch to the session, then focus the agent pane
            self.send(ClientCommand::SwitchSession { name: session.clone(), client_tty: None });
            self.send(ClientCommand::FocusAgentPane { session, agent, thread_id, thread_name });
        }
        ClickZone::DetailResize { rect } => self.begin_detail_resize(row, rect),
        ClickZone::ThemeOption { name, .. } => {
            if let Modal::ThemePicker(_) = &self.modal {
                self.send(ClientCommand::SetTheme { theme: name });
                self.modal = Modal::None;
            }
        }
        ClickZone::SessionFilterCycle { .. } => self.cycle_session_filter(),
    }
}

fn spawn_open(target: &str) {
    let target = target.to_string();
    std::thread::spawn(move || {
        let _ = std::process::Command::new("open").arg(&target).status();
    });
    // Or tokio::task::spawn_blocking; std::thread is fine since we don't await.
}
```

## Drag-resize logic

Faithful port of TS lines 456–530.

```rust
pub struct DetailResizeState {
    pub start_y: u16,
    pub start_height: u16,
}

fn begin_detail_resize(&mut self, click_row: u16, _rect: Rect) {
    self.detail_resize = Some(DetailResizeState {
        start_y: click_row,
        start_height: self.detail_panel_height,
    });
    self.log_resize("beginDetailResize", &[("y", click_row.to_string())]);
}

fn on_drag(&mut self, _col: u16, row: u16) {
    let Some(st) = self.detail_resize.as_ref() else { return };
    let dy = (st.start_y as i32) - (row as i32);
    // Drag up = bigger detail panel; drag down = smaller (matches TS)
    let next = (st.start_height as i32 + dy)
        .clamp(MIN_DETAIL_PANEL_HEIGHT as i32, MAX_DETAIL_PANEL_HEIGHT as i32) as u16;
    if next != self.detail_panel_height {
        self.detail_panel_height = next;
        self.log_resize("dragDetailResize", &[("h", next.to_string())]);
    }
}

fn on_left_up(&mut self, _col: u16, _row: u16) {
    if let Some(st) = self.detail_resize.take() {
        // Persist to server (which writes config)
        if let Some(name) = self.detail_panel_session_name() {
            self.send(ClientCommand::SetDetailPanelHeight {
                session: name.to_string(),
                height: self.detail_panel_height,
            });
            self.detail_panel_heights.insert(name.to_string(), self.detail_panel_height);
        }
        self.log_resize("endDetailResize", &[("h", self.detail_panel_height.to_string())]);
        let _ = st;
    }
}
```

> **Note:** `SetDetailPanelHeight` is a *new* `ClientCommand` variant we add
> in Phase 0 to move config writes to server. Until then, write directly to
> `~/.config/opensessions/config.json` from the client.

## Hover state (visual only)

```rust
fn on_move(&mut self, col: u16, row: u16) {
    let was = self.is_resize_hover;
    self.is_resize_hover = self.click_zones.iter().any(|z|
        matches!(z, ClickZone::DetailResize { rect } if rect.contains(col, row))
    );
    if was != self.is_resize_hover {
        // Trigger re-render on next tick (the interval handles it; nothing to do)
    }
}
```

## Scroll wheel

`MouseEventKind::ScrollUp/Down` → adjust list scroll offset. Bounds-check on
render. Optional polish, not required for parity.

## Mouse capture lifecycle

Enable on startup, restore on exit. Already handled by
`crossterm::event::EnableMouseCapture` + `ratatui::restore()`.

## Edge cases & bugs to avoid

1. **Click outside any zone**: do nothing (don't reset focus, matches TS).
2. **Click during modal**: only theme picker rows are clickable; everything
   else is dead while modal is open.
3. **Drag started, mouse leaves terminal**: terminal still emits drag events
   bounded to terminal coords; clamp normally.
4. **Click during resize**: ignore other zones until `Up` fires.

## Logging

The TS code writes resize debug to `/tmp/opensessions-tui-resize.log`. Keep
the same format for diffability:

```rust
fn log_resize(&self, kind: &str, fields: &[(&str, String)]) {
    use std::io::Write;
    let ts = chrono_or_iso8601_now();
    let extra: String = fields.iter()
        .map(|(k,v)| format!("\"{k}\":{v}"))
        .collect::<Vec<_>>().join(",");
    let line = format!("[{ts}] [pid:{}] {kind} {{{extra}}}\n", std::process::id());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true).append(true).open("/tmp/opensessions-tui-resize.log") {
        let _ = f.write_all(line.as_bytes());
    }
}
```

(Avoid pulling `chrono` for this — implement ISO8601 inline in ~20 LOC.)
