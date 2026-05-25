# 07 — Components

The TS code has 5 components inside `apps/tui/src/index.tsx`:

1. `App` (the root, lines 221–1099)
2. `ThemePicker` (lines 1107–1239)
3. `DetailPanel` (lines 1281–1431)
4. `AgentListItem` (lines 1432–1552)
5. `SessionCard` (lines 1553–1742)

Plus utility:

- `buildSparkline` (line 1243)

Each ports to a render function on `App` (or a small struct). No "components"
abstraction in immediate-mode rendering.

## `App` → root render

See `06-rendering-and-layout.md::render`. The skeleton:

```rust
fn render(&mut self, frame: &mut Frame) {
    self.click_zones.clear();
    self.link_zones.clear();
    /* layout split */
    /* render header / list / sep / detail / footer */
    /* render modal if any */
    /* render flash if any */
}
```

## `ThemePicker` → `render_theme_picker`

State shape:

```rust
pub struct ThemePickerState {
    pub query: String,
    pub cursor_pos: usize,
    pub selected: usize,
    pub scroll_offset: usize,
}

const MAX_VISIBLE: usize = 12;
```

Behavior to preserve:
- `↑` / `↓`: move selection (wraps); preview the highlighted theme.
- `Enter`: apply selection (sends `set-theme`).
- `Esc`: close (revert to `theme_before_preview`).
- Typing into the search box filters by `query.to_lowercase()`.
- Footer: `↑↓ browse  ⏎ select  esc close`.

```rust
fn render_theme_picker(&self, frame: &mut Frame, area: Rect, st: &ThemePickerState) {
    let popup = centered_rect(area, 30, 18);
    frame.render_widget(Clear, popup);

    let outer = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(self.theme.palette.blue))
        .style(Style::default().bg(self.theme.palette.mantle))
        .padding(Padding::uniform(1));
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);

    let layout = Layout::vertical([
        Constraint::Length(1),  // title
        Constraint::Length(1),  // separator rule
        Constraint::Length(3),  // search input box (border + 1 row)
        Constraint::Fill(1),    // theme list
        Constraint::Length(1),  // separator
        Constraint::Length(1),  // footer hint
    ]);
    let [title, sep1, input, list, sep2, hint] = layout.areas(inner);

    let title_style = Style::default()
        .fg(self.theme.palette.blue)
        .add_modifier(Modifier::BOLD);
    frame.render_widget(Line::from("Select Theme").style(title_style), title);

    frame.render_widget(
        Line::from("─".repeat(inner.width as usize))
            .style(Style::default().fg(self.theme.palette.surface2)),
        sep1,
    );
    frame.render_widget(
        Line::from("─".repeat(inner.width as usize))
            .style(Style::default().fg(self.theme.palette.surface2)),
        sep2,
    );

    // Search input (rendered with cursor at cursor_pos)
    let input_block = Block::bordered()
        .border_style(Style::default().fg(self.theme.palette.surface1));
    let input_inner = input_block.inner(input);
    frame.render_widget(input_block, input);
    let display = if st.query.is_empty() {
        Span::styled("Search themes…",
            Style::default().fg(self.theme.palette.overlay0))
    } else {
        Span::styled(&st.query,
            Style::default().fg(self.theme.palette.text))
    };
    frame.render_widget(Paragraph::new(Line::from(display)), input_inner);
    if !st.query.is_empty() {
        frame.set_cursor_position(Position {
            x: input_inner.x + st.cursor_pos as u16,
            y: input_inner.y,
        });
    }

    // Theme list
    let names = self.filter_themes(&st.query);
    let visible: Vec<Line> = names.iter().enumerate()
        .skip(st.scroll_offset)
        .take(MAX_VISIBLE)
        .map(|(idx, name)| {
            let is_sel = idx == st.selected;
            let prefix = if is_sel { "▸ " } else { "  " };
            let style = Style::default().fg(if is_sel {
                self.theme.palette.text
            } else {
                self.theme.palette.subtext0
            });
            let bg = if is_sel {
                Style::default().bg(self.theme.palette.surface0)
            } else {
                Style::default()
            };
            Line::from(vec![
                Span::raw(prefix),
                Span::styled(name.clone(), style),
            ]).style(bg)
        })
        .collect();
    frame.render_widget(Paragraph::new(visible), list);

    // Footer hint
    let hint_style = Style::default().fg(self.theme.palette.overlay0);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let footer_line = Line::from(vec![
        Span::styled("↑↓", dim), Span::raw(" browse  "),
        Span::styled("⏎", dim),  Span::raw(" select  "),
        Span::styled("esc", dim), Span::raw(" close"),
    ]).style(hint_style);
    frame.render_widget(footer_line, hint);
}
```

## `DetailPanel` → `render_detail_panel`

Renders the agent list for the focused session, with a path sub-header.
- Top row: `…<truncated dir>` in dim.
- Per agent: a 2-line `AgentListItem`.
- Drag-resize handle is the **separator above** (already rendered by
  `render_separator`).

```rust
fn render_detail_panel(&mut self, frame: &mut Frame, area: Rect) {
    let Some(session) = self.detail_panel_session() else { return; };

    let layout = Layout::vertical([
        Constraint::Length(1),                 // sub-header path
        Constraint::Fill(1),                   // agent list (scrollable)
    ]);
    let [hdr, list_area] = layout.areas(area);

    // Path sub-header
    let path = truncate_left(&session.dir, area.width as usize);
    frame.render_widget(
        Line::from(format!("…{}", path))
            .style(Style::default().fg(self.theme.palette.overlay0)),
        hdr,
    );

    // Agent list
    if session.agents.is_empty() {
        frame.render_widget(
            Line::from("(no agents)")
                .style(Style::default().fg(self.theme.palette.overlay0)),
            list_area,
        );
        return;
    }

    let mut y = list_area.y;
    for (idx, agent) in session.agents.iter().enumerate() {
        if y + 2 > list_area.y + list_area.height { break; }
        self.render_agent_list_item(frame, Rect { y, height: 2, ..list_area },
                                    agent, idx == self.focused_agent_idx
                                        && self.panel_focus == PanelFocus::Agents);
        y += 2;
    }
}
```

## `AgentListItem` → `render_agent_list_item`

2-line item:
- Line 1: `<status_icon> <agent_name>                    <status_text> <kill_icon>`
- Line 2: thread title (truncated), in `text` color when focused.

```rust
fn render_agent_list_item(&mut self, frame: &mut Frame, area: Rect,
                           agent: &AgentEvent, is_focused: bool) {
    let palette = &self.theme.palette;
    let status_color = self.theme.status[&agent.status];
    let icon = self.theme.icons[&agent.status];

    // Optional spinner override for running agents
    let glyph = if agent.status == AgentStatus::Running {
        SPINNER[self.spin_idx % SPINNER.len()]
    } else { icon };

    // Line 1
    let row_bg = if is_focused {
        Style::default().bg(palette.surface1)
    } else {
        Style::default()
    };
    let l1 = Line::from(vec![
        Span::styled(format!(" {} ", glyph),
            Style::default().fg(status_color)),
        Span::styled(agent.agent.clone(),
            Style::default().fg(if is_focused { palette.text } else { palette.subtext0 })),
        Span::raw(" "),
        // Right-aligned status text + kill icon (computed via padding to area.width)
    ]).style(row_bg);

    let line1_area = Rect { y: area.y, height: 1, ..area };
    frame.render_widget(l1, line1_area);

    // Line 2: thread title
    let title = agent.thread_name.clone().unwrap_or_default();
    let line2 = Line::from(format!("  {}", truncate_right(&title, area.width as usize - 2)))
        .style(Style::default().fg(if is_focused { palette.text } else { palette.overlay0 }));
    frame.render_widget(line2, Rect { y: area.y + 1, height: 1, ..area });

    // Record hit zones
    self.click_zones.push(ClickZone::AgentRow {
        rect: area,
        session: agent.session.clone(),
        agent: agent.agent.clone(),
        thread_id: agent.thread_id.clone(),
        thread_name: agent.thread_name.clone(),
    });
}
```

## `SessionCard` → `render_session_card`

3-line card:
- Line 1: `<focus_bar?> <index> <session_name>          <agent_status_icon>`
- Line 2: `    <dirname>` (truncated, color depends on focus)
- Line 3: `    <branch> <port_hint?>`
- Optional rows 4+: local-link rows wrapped per `wrapLocalLinks`

The implementation is mechanical port from lines 1553–1742. Same Catppuccin
palette, same icons, same hover/focus semantics. The trickiest bits:

1. **Local link wrapping** — port `wrapLocalLinks` (line 125) verbatim.
   Returns `Vec<Vec<LocalLink>>`, render one row per inner Vec.
2. **Mouse hit zones** — push to `App.click_zones`:
   - `ClickZone::SessionRow { rect, name }` for the whole card (focus on click)
   - `ClickZone::OpenDir { rect, dir }` on the dirname (when focused)
   - `ClickZone::OpenUrl { rect, url }` on each link
3. **Focus bar `▌`** — render in `green` (#a6e3a1) when this is the *attached*
   session, `lavender` when merely focused, blank otherwise. Matches the PNG
   reference.

## `buildSparkline`

Direct port:

```rust
const SPARK_BLOCKS: &str = " ▁▂▃▄▅▆▇█";  // 9 levels

pub fn build_sparkline(timestamps: &[u64], width: usize, window_ms: u64) -> String {
    if timestamps.is_empty() || width == 0 { return String::new(); }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
    let start = now - window_ms;
    let bucket_size = window_ms / width as u64;
    let mut buckets = vec![0u32; width];
    for &ts in timestamps {
        if ts < start { continue; }
        let idx = ((ts - start) / bucket_size) as usize;
        let idx = idx.min(width - 1);
        buckets[idx] += 1;
    }
    let max = buckets.iter().copied().max().unwrap_or(1).max(1);
    let blocks: Vec<char> = SPARK_BLOCKS.chars().collect();
    buckets.iter().map(|&c| {
        let level = ((c as f64 / max as f64) * (blocks.len() - 1) as f64).round() as usize;
        blocks[level.min(blocks.len() - 1)]
    }).collect()
}
```

## `wrapLocalLinks`

Port the algorithm from line 125 (greedy first-fit by visible width). Use
`unicode_width::UnicodeWidthStr::width` for measurement.

## Confirm-kill modal (`render_confirm_kill`)

Tiny — just a centered popup with text "Kill session `<name>`? [y]es / [any] cancel".
Same `Clear` + `Block::bordered()` pattern.
