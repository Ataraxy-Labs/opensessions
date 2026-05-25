use std::collections::HashMap;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::renderer::{CellStyle, Rgb, build_model, render_model};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedRows {
    pub width: u16,
    pub height: u16,
    pub rows: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameDiff {
    Full(RenderedRows),
    Patch {
        width: u16,
        height: u16,
        changed_rows: Vec<(u16, Vec<u8>)>,
        clear_from_row: Option<u16>,
    },
}

pub fn render_rows(app: &mut App, width: u16, height: u16) -> RenderedRows {
    let model = build_model(app, width as usize, height as usize);
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend terminal should initialize");
    terminal
        .draw(|frame| render_model(frame, &model))
        .expect("test backend draw should succeed");

    let buffer = terminal.backend().buffer().clone();
    let markers = model.markers(width, height);
    let rows = (0..height)
        .map(|y| row_to_ansi(&buffer, &markers, width, y))
        .collect();

    RenderedRows {
        width,
        height,
        rows,
    }
}

pub fn diff_rows(before: &RenderedRows, after: &RenderedRows) -> FrameDiff {
    if before.width != after.width || before.height != after.height {
        return FrameDiff::Full(after.clone());
    }

    let changed_rows = before
        .rows
        .iter()
        .zip(after.rows.iter())
        .enumerate()
        .filter_map(|(idx, (before, after))| {
            (before != after).then_some((idx as u16, after.clone()))
        })
        .collect();

    FrameDiff::Patch {
        width: after.width,
        height: after.height,
        changed_rows,
        clear_from_row: None,
    }
}

pub fn apply_patch_rows(before: &RenderedRows, diff: &FrameDiff) -> RenderedRows {
    match diff {
        FrameDiff::Full(rows) => rows.clone(),
        FrameDiff::Patch {
            width,
            height,
            changed_rows,
            clear_from_row,
        } => {
            let mut next = RenderedRows {
                width: *width,
                height: *height,
                rows: before.rows.clone(),
            };
            next.rows.resize(*height as usize, Vec::new());
            for (row, bytes) in changed_rows {
                if let Some(slot) = next.rows.get_mut(*row as usize) {
                    *slot = bytes.clone();
                }
            }
            if let Some(row) = clear_from_row {
                for slot in next.rows.iter_mut().skip(*row as usize) {
                    slot.clear();
                }
            }
            next
        }
    }
}

fn row_to_ansi(
    buffer: &Buffer,
    markers: &HashMap<(u16, u16), CellStyle>,
    width: u16,
    y: u16,
) -> Vec<u8> {
    let mut ansi = "\x1b[0m".to_string();
    let mut terminal_style = TerminalStyle::default();
    let last_content_x = (0..width).rev().find(|x| buffer[(*x, y)].symbol() != " ");
    let last_marker_x = markers
        .keys()
        .filter_map(|(x, marker_y)| (*marker_y == y).then_some(*x))
        .max();

    if last_content_x.is_none() {
        let mut row_markers = markers
            .iter()
            .filter_map(|(&(x, marker_y), &style)| (marker_y == y).then_some((x, style)))
            .collect::<Vec<_>>();
        row_markers.sort_by_key(|(x, _)| *x);
        for (_, marker) in row_markers {
            terminal_style.write_change(marker, &mut ansi);
        }
        return ansi.into_bytes();
    }

    let Some(last_x) = last_content_x.max(last_marker_x) else {
        return Vec::new();
    };
    let mut skip_cells = 0;
    for x in 0..=last_x {
        if let Some(marker) = markers.get(&(x, y)) {
            terminal_style.write_change(*marker, &mut ansi);
        }

        if skip_cells > 0 {
            skip_cells -= 1;
            continue;
        }

        let cell = &buffer[(x, y)];
        if cell.symbol() == " " && x > last_content_x.unwrap_or(0) {
            continue;
        }

        let style = CellStyle::from_cell(cell);
        terminal_style.write_change(style, &mut ansi);
        ansi.push_str(cell.symbol());
        skip_cells = cell.symbol().width().saturating_sub(1);
    }
    ansi.into_bytes()
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
