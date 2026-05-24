use std::collections::HashMap;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::renderer::{CellStyle, Rgb, build_model, render_model};

pub struct RenderedBuffer {
    buffer: Buffer,
    markers: HashMap<(u16, u16), CellStyle>,
}

pub fn render_to_buffer(app: &mut App, width: u16, height: u16) -> RenderedBuffer {
    let model = build_model(app, height as usize);
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend terminal should initialize");
    terminal
        .draw(|frame| render_model(frame, &model))
        .expect("test backend draw should succeed");

    RenderedBuffer {
        buffer: terminal.backend().buffer().clone(),
        markers: model.markers(width, height),
    }
}

pub fn buffer_dimensions(buffer: &RenderedBuffer) -> (u16, u16) {
    (buffer.buffer.area.width, buffer.buffer.area.height)
}

pub fn buffer_symbol_at(buffer: &RenderedBuffer, x: u16, y: u16) -> String {
    buffer
        .buffer
        .cell((x, y))
        .map(|cell| cell.symbol().to_string())
        .unwrap_or_default()
}

pub fn buffer_bg_at(buffer: &RenderedBuffer, x: u16, y: u16) -> Option<(u8, u8, u8)> {
    match buffer.buffer.cell((x, y))?.bg {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    }
}

pub fn buffer_to_ansi(buffer: &RenderedBuffer) -> String {
    let mut ansi = String::new();
    let mut terminal_style = TerminalStyle::default();
    let Rect { width, height, .. } = buffer.buffer.area;

    for y in 0..height {
        let last_content_x = (0..width)
            .rev()
            .find(|x| buffer.buffer[(*x, y)].symbol() != " ");
        let last_marker_x = buffer
            .markers
            .keys()
            .filter_map(|(x, marker_y)| (*marker_y == y).then_some(*x))
            .max();
        if last_content_x.is_none() {
            let mut markers = buffer
                .markers
                .iter()
                .filter_map(|(&(x, marker_y), &style)| (marker_y == y).then_some((x, style)))
                .collect::<Vec<_>>();
            markers.sort_by_key(|(x, _)| *x);
            for (_, marker) in markers {
                terminal_style.write_change(marker, &mut ansi);
            }
        } else if let Some(last_x) = last_content_x.max(last_marker_x) {
            let mut skip_cells = 0;
            for x in 0..=last_x {
                if let Some(marker) = buffer.markers.get(&(x, y)) {
                    terminal_style.write_change(*marker, &mut ansi);
                }

                if skip_cells > 0 {
                    skip_cells -= 1;
                    continue;
                }

                let cell = &buffer.buffer[(x, y)];
                if cell.symbol() == " " && x > last_content_x.unwrap_or(0) {
                    continue;
                }

                let style = CellStyle::from_cell(cell);
                terminal_style.write_change(style, &mut ansi);
                ansi.push_str(cell.symbol());
                skip_cells = cell.symbol().width().saturating_sub(1);
            }
        }
        ansi.push('\n');
    }

    ansi
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct TerminalStyle {
    fg: Option<Rgb>,
    bg: Option<Rgb>,
}

impl TerminalStyle {
    fn write_change(&mut self, next: CellStyle, ansi: &mut String) {
        if self.bg != next.bg {
            match next.bg {
                Some(color) => ansi.push_str(&color.bg_sgr()),
                None => ansi.push_str("\x1b[49m"),
            }
            self.bg = next.bg;
        }
        if self.fg != Some(next.fg) {
            ansi.push_str(&next.fg.fg_sgr());
            self.fg = Some(next.fg);
        }
    }
}
