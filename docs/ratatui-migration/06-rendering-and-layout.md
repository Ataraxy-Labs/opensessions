# 06 — Rendering and Layout

OpenTUI uses a flexbox-style declarative layout. Ratatui uses programmatic
`Layout` + `Constraint` (cassowary-solved). Each maps directly.

## OpenTUI primitives in use → ratatui equivalent

| OpenTUI | ratatui |
|---|---|
| `<box flexDirection="column">` | `Layout::vertical([…])` |
| `<box flexDirection="row">` | `Layout::horizontal([…])` |
| `flexGrow={1}` | `Constraint::Fill(1)` |
| `flexShrink={0}` | `Constraint::Length(n)` (fixed) |
| `paddingLeft/Top/Right/Bottom` | `Block::default().padding(Padding::new(l, r, t, b))` or manually offset `Rect` |
| `width={N}` / `height={N}` | `Constraint::Length(N)` |
| `backgroundColor={...}` | `Style::default().bg(Color::Rgb(...))` applied to a `Block` over the area |
| `border borderStyle="rounded"` | `Block::bordered().border_type(BorderType::Rounded)` |
| `borderColor={...}` | `Block::bordered().border_style(Style::default().fg(...))` |
| `position="absolute" top={0} left={0} right={0} bottom={0}` | Render over `frame.area()`, use `Clear` first |
| `<text style={{ fg, attributes }}>` | `Span::styled(text, Style::default().fg(...).add_modifier(...))` |
| `<span>` | `Span` inside a `Line` |
| `<scrollbox>` | Custom: `List` with `ListState.offset` + `Scrollbar` widget |
| `truncate wrapMode="none"` | `Line::from(...).style(...)` + manually clip; or `Paragraph::wrap(Wrap{...})` if multiline |
| `<input>` (theme picker) | Hand-rolled: track text + cursor in state; render with `Paragraph` |
| `<For each={...}>{(item) => ...}</For>` | `for item in iter { /* render */ }` inside the draw closure |
| `<Show when={cond}>` | `if cond { /* render */ }` |

## Top-level structure (sidebar 35×56)

```
┌────────────────────────────────┐  ← frame.area()
│ (1) Header bar (height=1)      │  Constraint::Length(2 — header + initLabel?)
├────────────────────────────────┤
│ (2) Session list (Fill)        │  Constraint::Fill(1)
│   - For each filtered session:│
│     - row1: index + name      │
│     - row2: dirname           │
│     - row3: branch + ports    │
│     - links rows (optional)   │
│     - blank line spacer       │
├────────────────────────────────┤
│ (3) Separator ─                │  Constraint::Length(1)
├────────────────────────────────┤
│ (4) Detail panel (height=H)    │  Constraint::Length(H)  ← persisted
│   - sub-header path            │
│   - For each agent: row+row    │
├────────────────────────────────┤
│ (5) Footer (height=2)          │  Constraint::Length(2)
└────────────────────────────────┘
```

Implemented as:

```rust
fn render(&mut self, frame: &mut Frame) {
    let area = frame.area();

    // Background fill (P().crust)
    Block::default()
        .style(Style::default().bg(self.theme.palette.crust))
        .render(area, frame.buffer_mut());

    let layout = Layout::vertical([
        Constraint::Length(2),                              // header
        Constraint::Fill(1),                                // session list (scrollable)
        Constraint::Length(1),                              // separator
        Constraint::Length(self.detail_panel_height),       // detail panel
        Constraint::Length(2),                              // footer
    ]);
    let [hdr, list, sep, detail, ftr] = layout.areas(area);

    self.render_header(frame, hdr);
    self.render_session_list(frame, list);
    self.render_separator(frame, sep);
    self.render_detail_panel(frame, detail);
    self.render_footer(frame, ftr);

    // Modals — render over everything
    match &self.modal {
        Modal::ThemePicker(state) => self.render_theme_picker(frame, area, state),
        Modal::ConfirmKill(name)  => self.render_confirm_kill(frame, area, name),
        Modal::None => {}
    }

    // Flash message overlay
    if let Some(flash) = &self.flash {
        self.render_flash(frame, area, flash);
    }
}
```

## Session list — scrollable

OpenTUI's `<scrollbox>` does it free. In ratatui:

```rust
fn render_session_list(&mut self, frame: &mut Frame, area: Rect) {
    let items: Vec<ListItem> = self.filtered_sessions()
        .enumerate()
        .flat_map(|(i, s)| self.session_card_lines(i, s))  // returns Vec<Line> per session
        .map(ListItem::new)
        .collect();

    let mut list_state = ListState::default()
        .with_offset(self.list_scroll_offset)
        .with_selected(self.focused_index_in_filtered());

    let list = List::new(items)
        .highlight_style(Style::default().bg(self.theme.palette.surface1))
        .highlight_spacing(HighlightSpacing::Always);

    StatefulWidget::render(list, area, frame.buffer_mut(), &mut list_state);

    // After render, save scroll offset back so next frame resumes here
    self.list_scroll_offset = list_state.offset();

    // Record click zones for hit-testing
    self.record_session_click_zones(area, &list_state);
}
```

> **Note:** The session "card" is 3–5 lines tall depending on `localLinks`.
> Use a custom `Widget` impl rather than `List` if hit-testing per-link gets
> unwieldy. Either approach is valid.

## Detail panel resize handle

The TS implementation makes the **first row of the detail panel** the
drag-resize handle. Render the separator line as the hit area:

```rust
fn render_separator(&mut self, frame: &mut Frame, area: Rect) {
    let style = if self.detail_resize.is_some() {
        Style::default().fg(self.theme.palette.blue)
    } else if self.is_resize_hover {
        Style::default().fg(self.theme.palette.lavender)
    } else {
        Style::default().fg(self.theme.palette.overlay0)
    };
    let line = Line::from("─".repeat(area.width as usize)).style(style);
    frame.render_widget(line, area);

    // Record hit-test rect
    self.click_zones.push(ClickZone::DetailResize(area));
}
```

## Modals (popup over base layer)

Use the `Clear` widget to wipe the underlying cells, then render the popup:

```rust
fn centered_rect(area: Rect, w: u16, h: u16) -> Rect {
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect { x, y, width: w.min(area.width), height: h.min(area.height) }
}

fn render_theme_picker(&self, frame: &mut Frame, area: Rect, st: &ThemePickerState) {
    let popup = centered_rect(area, 30, 18);
    frame.render_widget(Clear, popup);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(self.theme.palette.blue))
        .style(Style::default().bg(self.theme.palette.mantle))
        .padding(Padding::uniform(1));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    // Title, search input, list, footer hints inside `inner`
    // ... (see 07-components.md::ThemePicker)
}
```

## Truecolor

All themes use `#rrggbb`. Ratatui `Color::Rgb(r, g, b)` writes the
`\033[38;2;r;g;bm` SGR sequence directly — matches what OpenTUI emits today
(verified against `reference-snapshots/*.ansi`).

```rust
fn rgb(hex: &str) -> Color {
    // hex = "#cdd6f4"
    let h = hex.trim_start_matches('#');
    let r = u8::from_str_radix(&h[0..2], 16).unwrap();
    let g = u8::from_str_radix(&h[2..4], 16).unwrap();
    let b = u8::from_str_radix(&h[4..6], 16).unwrap();
    Color::Rgb(r, g, b)
}
```

## Attributes

`OpenTUI`'s `TextAttributes.BOLD` / `DIM` → `Modifier::BOLD` / `Modifier::DIM`.

```rust
Style::default()
    .fg(self.theme.palette.text)
    .add_modifier(Modifier::BOLD)
```

## Spinner

Same 10-frame Braille glyphs. Ratatui has no built-in spinner widget; we just
pull from a const slice indexed by `app.spin_idx`:

```rust
const SPINNER: [&str; 10] = ["⠋","⠙","⠹","⠸","⠼","⠴","⠦","⠧","⠇","⠏"];
let glyph = SPINNER[app.spin_idx % SPINNER.len()];
```

## Sparkline

Two options:

1. **Use ratatui's `Sparkline` widget** — `data(&[u64])`, `max(...)`,
   `direction(...)`, supports the same `▁▂▃▄▅▆▇█` set. ✅ preferred.
2. **Inline the same `buildSparkline` algorithm** from TS as a string-returning
   helper. Used if we need to embed it inside a `Line`.

The TS implementation (line 1243) buckets timestamps over a 30-min window into
`width` columns. Port verbatim.

## Truncation marker

OpenTUI uses `…` for truncated paths. Use `unicode-width` to measure correctly
and prepend `…` when needed:

```rust
fn truncate_left(s: &str, max_cols: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    if s.width() <= max_cols { return s.to_string(); }
    let mut chars: Vec<char> = s.chars().collect();
    while chars.iter().collect::<String>().width() > max_cols.saturating_sub(1) {
        chars.remove(0);
    }
    format!("…{}", chars.iter().collect::<String>())
}
```

## OSC 8 hyperlinks (optional)

Currently OpenTUI uses **clickable text** that we hit-test ourselves to spawn
`open <url>`. We do NOT use OSC 8 hyperlinks today, but ratatui's `hyperlink`
example shows we could add it as a UX improvement. Skip for parity.
