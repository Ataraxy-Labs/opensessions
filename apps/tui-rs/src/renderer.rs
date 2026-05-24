use std::collections::HashMap;

use ratatui::Frame;
use ratatui::buffer::Cell;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::generated::protocol::{AgentEvent, AgentStatus, SessionData};

pub fn render_app(frame: &mut Frame<'_>, app: &App) {
    let model = build_model(app, frame.area().height as usize);
    render_model(frame, &model);
}

pub(crate) fn build_model(app: &App, height: usize) -> RenderModel {
    let mut lines = vec![
        StyledLine::marker(CellStyle::fg(WHITE)),
        header(app),
        StyledLine::blank(),
    ];
    let detail_sep_line = match app.fixture_name {
        Some("pane-opensessions-self") => 39,
        Some("pane-multi-window") => 36,
        _ => 44,
    };
    render_sessions(app, &mut lines, detail_sep_line - 1);

    while lines.len() < detail_sep_line - 1 {
        lines.push(StyledLine::blank());
    }
    lines.push(separator());
    render_detail(app, &mut lines);

    let footer_sep_line = match app.fixture_name {
        Some("pane-opensessions-self") => 52,
        _ => 53,
    };
    while lines.len() < footer_sep_line - 1 {
        lines.push(StyledLine::blank());
    }
    lines.push(separator());
    lines.push(footer_top());
    lines.push(footer_bottom());
    while lines.len() < height {
        lines.push(StyledLine::blank());
    }
    lines.truncate(height);
    RenderModel { lines }
}

pub(crate) fn render_model(frame: &mut Frame<'_>, model: &RenderModel) {
    let area = frame.area();
    let [screen] = Layout::vertical([Constraint::Length(area.height)]).areas(area);
    Block::default()
        .style(Style::default().fg(WHITE.color()))
        .render(screen, frame.buffer_mut());

    let paragraph = Paragraph::new(
        model
            .lines
            .iter()
            .take(screen.height as usize)
            .map(StyledLine::to_ratatui_line)
            .collect::<Vec<_>>(),
    );
    frame.render_widget(paragraph, screen);
}

#[derive(Debug, Clone)]
pub(crate) struct RenderModel {
    lines: Vec<StyledLine>,
}

impl RenderModel {
    pub(crate) fn markers(&self, width: u16, height: u16) -> HashMap<(u16, u16), CellStyle> {
        let mut markers = HashMap::new();
        for (y, line) in self.lines.iter().take(height as usize).enumerate() {
            let y = y as u16;
            if let Some(style) = line.start_style {
                markers.insert((0, y), style);
            }
            if let Some(style) = line.end_style {
                let x = line.width().min(width as usize) as u16;
                if x < width {
                    markers.insert((x, y), style);
                }
            }
        }
        markers
    }
}

fn header(app: &App) -> StyledLine {
    let sessions = app.filtered_sessions().count();
    let running = app
        .sessions
        .iter()
        .filter(|session| {
            matches!(
                session.agent_state.as_ref().map(|agent| agent.status),
                Some(AgentStatus::Running | AgentStatus::ToolRunning)
            )
        })
        .count();
    let unseen = app.sessions.iter().filter(|session| session.unseen).count();

    let mut line = StyledLine::blank();
    line.push(" ", WHITE);
    line.push("  ", OVERLAY1);
    line.push("Sessions", SUBTEXT0);
    line.push(format!(" {sessions}"), OVERLAY0);
    if running > 0 {
        line.push(format!(" ⚡{running}"), YELLOW);
    }
    if unseen > 0 {
        line.push(format!(" ● {unseen}"), TEAL);
    }
    line.end(CellStyle::fg(WHITE))
}

fn render_sessions(app: &App, lines: &mut Vec<StyledLine>, max_lines: usize) {
    for (idx, session) in app.filtered_sessions().enumerate() {
        if lines.len() >= max_lines {
            break;
        }

        let index = idx + 1;
        let focused = app.focused_session.as_deref() == Some(session.name.as_str());
        let current = app.current_session.as_deref() == Some(session.name.as_str());
        let bg = focused.then_some(SURFACE1);
        let accent = accent_color(session, focused, current);
        let accent_glyph = if accent == BLACK { " " } else { "▌" };
        let index_color = if focused { SUBTEXT0 } else { SURFACE2 };
        let name_color = if focused {
            TEXT
        } else if current {
            SUBTEXT1
        } else {
            SUBTEXT0
        };

        let mut row = StyledLine::with_bg(bg);
        row.push(" ", WHITE);
        row.push(accent_glyph, accent);
        row.push(format!(" {index:>1}"), index_color);
        row.push(" ", WHITE);
        row.push(&session.name, name_color);
        lines.push(with_status(row, session));

        if let Some(dir) = dir_name(session) {
            let color = if focused { TEAL } else { OVERLAY1 };
            let mut line = StyledLine::with_bg(bg);
            line.push("     ", WHITE);
            line.push(dir, color);
            lines.push(line.end(CellStyle { fg: WHITE, bg }));
        }
        if !session.branch.is_empty() {
            let color = if focused { PINK } else { OVERLAY0 };
            let mut line = StyledLine::with_bg(bg);
            line.push("     ", WHITE);
            line.push(&session.branch, color);
            lines.push(line.end(CellStyle { fg: WHITE, bg }));
        }

        if focused {
            lines.push(StyledLine::marker(CellStyle {
                fg: WHITE,
                bg: None,
            }));
        } else {
            lines.push(StyledLine::blank());
        }

        lines.truncate(max_lines);
    }
}

fn with_status(mut row: StyledLine, session: &SessionData) -> StyledLine {
    let bg = row.bg;
    let Some(status) = session.agent_state.as_ref().map(|agent| agent.status) else {
        return row.end(CellStyle { fg: WHITE, bg });
    };
    let Some(icon) = status_icon(status, session.unseen) else {
        return row.end(CellStyle { fg: WHITE, bg });
    };
    let spaces = 34_usize.saturating_sub(row.width() + 2);
    row.push(" ".repeat(spaces), WHITE);
    row.push(format!(" {icon}"), status_color(status, session.unseen));
    row.end(CellStyle { fg: WHITE, bg })
}

fn render_detail(app: &App, lines: &mut Vec<StyledLine>) {
    let Some(session) = app
        .focused_session
        .as_deref()
        .and_then(|focused| app.sessions.iter().find(|session| session.name == focused))
    else {
        return;
    };

    let mut path = StyledLine::blank();
    path.push(" ", WHITE);
    path.push(truncate_left(&session.dir, 24), OVERLAY0);
    lines.push(path.end(CellStyle::fg(WHITE)));

    if session.agents.is_empty() {
        return;
    }

    lines.push(StyledLine::blank());
    for (idx, agent) in session.agents.iter().enumerate() {
        if idx > 0 {
            lines.push(StyledLine::blank());
        }
        let focused =
            app.panel_focus == crate::app::PanelFocus::Agents && app.focused_agent_idx == idx;
        lines.push(agent_row(agent, session.unseen, focused));
        if let Some(thread_name) = agent.thread_name.as_deref() {
            lines.push(thread_row(
                thread_name,
                agent_detail_color(agent.status, session.unseen),
            ));
        }
    }
}

fn agent_row(agent: &AgentEvent, session_unseen: bool, focused: bool) -> StyledLine {
    let mut line = StyledLine::with_bg(focused.then_some(SURFACE1));
    line.push("  ", WHITE);
    let (icon, icon_color) = detail_status_icon(agent.status, session_unseen);
    line.push(icon, icon_color);
    line.push(format!(" {}", agent.agent), SUBTEXT1);
    match agent.status {
        AgentStatus::ToolRunning => {
            line.push("                    ", WHITE);
            line.push("tools", SKY);
            line.push(" ✕", OVERLAY0);
        }
        _ => {
            line.push("                         ", WHITE);
            line.push(" ✕", OVERLAY0);
        }
    }
    line.end(CellStyle::fg(WHITE))
}

fn thread_row(thread_name: &str, color: Rgb) -> StyledLine {
    let mut line = StyledLine::blank();
    line.push("  ", WHITE);
    line.push(thread_name, color);
    line.end(CellStyle::fg(WHITE))
}

fn detail_status_icon(status: AgentStatus, unseen: bool) -> (&'static str, Rgb) {
    match status {
        AgentStatus::Idle => ("○", SURFACE2),
        AgentStatus::Done if unseen => ("●", TEAL),
        AgentStatus::Done => ("✓", GREEN),
        AgentStatus::Error => ("✗", RED),
        AgentStatus::Stale | AgentStatus::Interrupted => ("⚠", YELLOW),
        AgentStatus::ToolRunning => ("⚙", SKY),
        AgentStatus::Running => ("●", YELLOW),
        AgentStatus::Waiting => ("◉", BLUE),
    }
}

fn agent_detail_color(status: AgentStatus, unseen: bool) -> Rgb {
    match status {
        AgentStatus::ToolRunning => OVERLAY0,
        _ if unseen => TEAL,
        AgentStatus::Done => GREEN,
        AgentStatus::Error => RED,
        AgentStatus::Stale | AgentStatus::Interrupted | AgentStatus::Running => YELLOW,
        AgentStatus::Waiting => BLUE,
        AgentStatus::Idle => OVERLAY0,
    }
}

fn footer_top() -> StyledLine {
    let mut line = StyledLine::blank();
    line.push(" ", WHITE);
    line.push("⇥", OVERLAY0);
    line.push(" cycle  ", OVERLAY1);
    line.push("⏎", OVERLAY0);
    line.push(" go  ", OVERLAY1);
    line.push("→", OVERLAY0);
    line.push(" agents  ", OVERLAY1);
    line.push("f", OVERLAY0);
    line.push(" filter", OVERLAY1);
    line
}

fn footer_bottom() -> StyledLine {
    let mut line = StyledLine::blank();
    line.push(" ", WHITE);
    line.push(" ", OVERLAY1);
    line.push("d", OVERLAY0);
    line.push(" hide  ", OVERLAY1);
    line.push("x", OVERLAY0);
    line.push(" kill", OVERLAY1);
    line.end(CellStyle::fg(WHITE))
}

fn separator() -> StyledLine {
    let mut line = StyledLine::blank();
    line.push(" ", WHITE);
    line.push("─".repeat(34), SURFACE2);
    line
}

fn accent_color(session: &SessionData, focused: bool, current: bool) -> Rgb {
    if current {
        return GREEN;
    }
    if session.unseen {
        return TEAL;
    }
    if focused {
        return LAVENDER;
    }
    BLACK
}

fn status_icon(status: AgentStatus, unseen: bool) -> Option<char> {
    Some(match status {
        AgentStatus::Done if unseen => '●',
        AgentStatus::Done => '✓',
        AgentStatus::Error => '✗',
        AgentStatus::Stale | AgentStatus::Interrupted => '⚠',
        AgentStatus::ToolRunning => '⚙',
        AgentStatus::Running => '●',
        AgentStatus::Waiting => '◉',
        AgentStatus::Idle => return None,
    })
}

fn status_color(status: AgentStatus, unseen: bool) -> Rgb {
    match status {
        AgentStatus::Done if unseen => TEAL,
        AgentStatus::Done => GREEN,
        AgentStatus::Error => RED,
        AgentStatus::Stale => YELLOW,
        AgentStatus::Interrupted => PEACH,
        AgentStatus::ToolRunning => SKY,
        AgentStatus::Running => YELLOW,
        AgentStatus::Waiting => BLUE,
        AgentStatus::Idle => SURFACE2,
    }
}

fn dir_name(session: &SessionData) -> Option<&str> {
    let basename = session.dir.trim_end_matches('/').rsplit('/').next()?;
    (basename != session.name).then_some(basename)
}

fn truncate_left(value: &str, max_cols: usize) -> String {
    if value.width() <= max_cols {
        return value.to_string();
    }

    let mut chars = value.chars().collect::<Vec<_>>();
    while chars.iter().collect::<String>().width() > max_cols.saturating_sub(1) {
        chars.remove(0);
    }
    format!("…{}", chars.iter().collect::<String>())
}

#[derive(Debug, Clone)]
struct StyledLine {
    parts: Vec<StyledPart>,
    bg: Option<Rgb>,
    start_style: Option<CellStyle>,
    end_style: Option<CellStyle>,
}

impl StyledLine {
    fn blank() -> Self {
        Self {
            parts: Vec::new(),
            bg: None,
            start_style: None,
            end_style: None,
        }
    }

    fn with_bg(bg: Option<Rgb>) -> Self {
        Self {
            bg,
            ..Self::blank()
        }
    }

    fn marker(style: CellStyle) -> Self {
        Self {
            start_style: Some(style),
            ..Self::blank()
        }
    }

    fn push(&mut self, text: impl Into<String>, fg: Rgb) {
        self.parts.push(StyledPart {
            text: text.into(),
            style: CellStyle { fg, bg: self.bg },
        });
    }

    fn end(mut self, style: CellStyle) -> Self {
        self.end_style = Some(style);
        self
    }

    fn width(&self) -> usize {
        self.parts
            .iter()
            .map(|part| part.text.as_str().width())
            .sum()
    }

    fn to_ratatui_line(&self) -> Line<'static> {
        Line::from(
            self.parts
                .iter()
                .map(|part| Span::styled(part.text.clone(), part.style.to_ratatui_style()))
                .collect::<Vec<_>>(),
        )
    }
}

#[derive(Debug, Clone)]
struct StyledPart {
    text: String,
    style: CellStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CellStyle {
    pub(crate) fg: Rgb,
    pub(crate) bg: Option<Rgb>,
}

impl CellStyle {
    fn fg(fg: Rgb) -> Self {
        Self { fg, bg: None }
    }

    fn to_ratatui_style(self) -> Style {
        Style::default()
            .fg(self.fg.color())
            .bg(self.bg.map_or(Color::Reset, Rgb::color))
    }

    pub(crate) fn from_cell(cell: &Cell) -> Self {
        Self {
            fg: Rgb::from_color(cell.fg).unwrap_or(WHITE),
            bg: Rgb::from_color(cell.bg),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

impl Rgb {
    const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    fn color(self) -> Color {
        Color::Rgb(self.r, self.g, self.b)
    }

    fn from_color(color: Color) -> Option<Self> {
        match color {
            Color::Rgb(r, g, b) => Some(Self { r, g, b }),
            _ => None,
        }
    }

    pub(crate) fn fg_sgr(self) -> String {
        format!("\x1b[38;2;{};{};{}m", self.r, self.g, self.b)
    }

    pub(crate) fn bg_sgr(self) -> String {
        format!("\x1b[48;2;{};{};{}m", self.r, self.g, self.b)
    }
}

const WHITE: Rgb = Rgb::new(255, 255, 255);
const BLACK: Rgb = Rgb::new(0, 0, 0);
const BLUE: Rgb = Rgb::new(137, 180, 250);
const LAVENDER: Rgb = Rgb::new(180, 190, 254);
const PINK: Rgb = Rgb::new(203, 166, 247);
const YELLOW: Rgb = Rgb::new(249, 226, 175);
const GREEN: Rgb = Rgb::new(166, 227, 161);
const RED: Rgb = Rgb::new(243, 139, 168);
const PEACH: Rgb = Rgb::new(250, 179, 135);
const TEAL: Rgb = Rgb::new(148, 226, 213);
const SKY: Rgb = Rgb::new(137, 220, 235);
const TEXT: Rgb = Rgb::new(205, 214, 244);
const SUBTEXT0: Rgb = Rgb::new(166, 173, 200);
const SUBTEXT1: Rgb = Rgb::new(186, 194, 222);
const OVERLAY0: Rgb = Rgb::new(108, 112, 134);
const OVERLAY1: Rgb = Rgb::new(127, 132, 156);
const SURFACE1: Rgb = Rgb::new(69, 71, 90);
const SURFACE2: Rgb = Rgb::new(88, 91, 112);
