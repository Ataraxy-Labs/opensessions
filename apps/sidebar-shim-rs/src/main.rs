use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Command;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{
    EnableMouseCapture, Event, EventStream, KeyCode as CrosstermKeyCode, KeyEventKind,
    KeyModifiers as CrosstermKeyModifiers, MouseButton as CrosstermMouseButton,
    MouseEventKind as CrosstermMouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use futures_util::StreamExt;
use opensessions_sidebar_protocol::{
    KeyCode, KeyMessage, KeyModifiers, MouseButton, MouseEventKind, MouseMessage, ServerToShim,
    ShimHello, ShimToServer, decode_server_message, encode_shim_message,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    let socket_path = socket_path();
    let context = PaneContext::detect();
    let (width, height) = terminal::size().unwrap_or((35, 56));
    let mut stream = UnixStream::connect(socket_path).await?;
    stream
        .write_all(&encode_shim_message(&ShimToServer::Hello(ShimHello {
            protocol: 1,
            pane_id: context.pane_id,
            session_name: context.session_name,
            window_id: context.window_id,
            client_tty: context.client_tty,
            width,
            height,
        })))
        .await?;

    let mut terminal = TerminalGuard::enter()?;
    let mut events = EventStream::new();
    loop {
        tokio::select! {
            event = events.next() => {
                let Some(event) = event else { return Ok(()); };
                match event? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if key.modifiers.contains(CrosstermKeyModifiers::CONTROL)
                            && matches!(key.code, CrosstermKeyCode::Char('c'))
                        {
                            return Ok(());
                        }
                        if let Some(message) = key_message(key.code, key.modifiers) {
                            stream.write_all(&encode_shim_message(&ShimToServer::Key(message))).await?;
                        }
                    }
                    Event::Mouse(mouse) => {
                        if let Some(message) = mouse_message(mouse) {
                            stream.write_all(&encode_shim_message(&ShimToServer::Mouse(message))).await?;
                        }
                    }
                    Event::Resize(width, height) => {
                        stream.write_all(&encode_shim_message(&ShimToServer::Resize { width, height })).await?;
                    }
                    _ => {}
                }
            }
            frame = read_frame(&mut stream) => {
                let frame = frame?;
                let message = decode_server_message(&frame).map_err(io::Error::other)?;
                match message {
                    ServerToShim::Hello { .. } => {}
                    ServerToShim::Quit => return Ok(()),
                    ServerToShim::FullFrame { rows, .. } => terminal.write_full_frame(&rows)?,
                    ServerToShim::PatchFrame { changed_rows, clear_from_row, .. } => {
                        terminal.write_patch_frame(&changed_rows, clear_from_row)?;
                    }
                }
            }
        }
    }
}

fn socket_path() -> PathBuf {
    if let Some(path) = arg_value("--socket-path") {
        return PathBuf::from(path);
    }
    if let Ok(path) = std::env::var("OPENSESSIONS_SHIM_SOCKET") {
        return PathBuf::from(path);
    }
    if let Ok(pid_file) = std::env::var("OPENSESSIONS_PID_FILE") {
        return PathBuf::from(pid_file).with_extension("sock");
    }
    if let Ok(tmux) = std::env::var("TMUX") {
        if let Some(socket) = tmux.split(',').next().filter(|value| !value.is_empty()) {
            return PathBuf::from(format!(
                "/tmp/opensessions.{}.sock",
                hash_server_key(socket)
            ));
        }
    }
    PathBuf::from("/tmp/opensessions.sock")
}

fn hash_server_key(input: &str) -> u16 {
    let mut hash = 0_u32;
    for (i, byte) in input.bytes().enumerate() {
        hash = (hash + u32::from(byte) * (i as u32 + 1)) % 20_000;
    }
    hash as u16
}

fn arg_value(name: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == name {
            return args.next();
        }
    }
    None
}

#[derive(Debug)]
struct PaneContext {
    pane_id: String,
    session_name: String,
    window_id: Option<String>,
    client_tty: Option<String>,
}

impl PaneContext {
    fn detect() -> Self {
        let tmux = tmux_context();
        Self {
            pane_id: std::env::var("TMUX_PANE")
                .ok()
                .or_else(|| tmux.as_ref().map(|ctx| ctx.pane_id.clone()))
                .unwrap_or_else(|| "unknown".to_string()),
            session_name: std::env::var("OPENSESSIONS_SESSION_NAME")
                .ok()
                .or_else(|| tmux.as_ref().map(|ctx| ctx.session_name.clone()))
                .unwrap_or_else(|| "unknown".to_string()),
            window_id: std::env::var("OPENSESSIONS_WINDOW_ID")
                .ok()
                .or_else(|| tmux.as_ref().and_then(|ctx| ctx.window_id.clone())),
            client_tty: std::env::var("OPENSESSIONS_CLIENT_TTY")
                .ok()
                .or_else(|| tmux.and_then(|ctx| ctx.client_tty)),
        }
    }
}

fn tmux_context() -> Option<PaneContext> {
    if std::env::var("TMUX").ok()?.is_empty() {
        return None;
    }
    let output = Command::new("tmux")
        .args([
            "display-message",
            "-p",
            "#{pane_id}|#{session_name}|#{window_id}|#{client_tty}",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = text.trim().split('|');
    let pane_id = parts.next()?.to_string();
    let session_name = parts.next()?.to_string();
    let window_id = parts
        .next()
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let client_tty = parts
        .next()
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    Some(PaneContext {
        pane_id,
        session_name,
        window_id,
        client_tty,
    })
}

fn key_message(code: CrosstermKeyCode, modifiers: CrosstermKeyModifiers) -> Option<KeyMessage> {
    let code = match code {
        CrosstermKeyCode::Char(ch) => KeyCode::Char(ch),
        CrosstermKeyCode::Up => KeyCode::Up,
        CrosstermKeyCode::Down => KeyCode::Down,
        CrosstermKeyCode::Tab | CrosstermKeyCode::BackTab => KeyCode::Tab,
        CrosstermKeyCode::Enter => KeyCode::Enter,
        CrosstermKeyCode::Esc => KeyCode::Esc,
        _ => return None,
    };
    let mut shim_modifiers = modifiers_from_crossterm(modifiers);
    if matches!(code, KeyCode::Tab) && modifiers.contains(CrosstermKeyModifiers::SHIFT) {
        shim_modifiers = shim_modifiers | KeyModifiers::SHIFT;
    }
    Some(KeyMessage {
        code,
        modifiers: shim_modifiers,
    })
}

fn mouse_message(mouse: crossterm::event::MouseEvent) -> Option<MouseMessage> {
    let (kind, button) = match mouse.kind {
        CrosstermMouseEventKind::Down(button) => (MouseEventKind::Down, mouse_button(button)),
        CrosstermMouseEventKind::Up(button) => (MouseEventKind::Up, mouse_button(button)),
        CrosstermMouseEventKind::Drag(button) => (MouseEventKind::Drag, mouse_button(button)),
        CrosstermMouseEventKind::Moved => (MouseEventKind::Move, MouseButton::None),
        CrosstermMouseEventKind::ScrollUp => (MouseEventKind::ScrollUp, MouseButton::None),
        CrosstermMouseEventKind::ScrollDown => (MouseEventKind::ScrollDown, MouseButton::None),
        _ => return None,
    };
    Some(MouseMessage {
        kind,
        button,
        column: mouse.column,
        row: mouse.row,
        modifiers: modifiers_from_crossterm(mouse.modifiers),
    })
}

fn mouse_button(button: CrosstermMouseButton) -> MouseButton {
    match button {
        CrosstermMouseButton::Left => MouseButton::Left,
        CrosstermMouseButton::Middle => MouseButton::Middle,
        CrosstermMouseButton::Right => MouseButton::Right,
    }
}

fn modifiers_from_crossterm(modifiers: CrosstermKeyModifiers) -> KeyModifiers {
    let mut shim_modifiers = KeyModifiers::empty();
    if modifiers.contains(CrosstermKeyModifiers::SHIFT) {
        shim_modifiers = shim_modifiers | KeyModifiers::SHIFT;
    }
    if modifiers.contains(CrosstermKeyModifiers::ALT) {
        shim_modifiers = shim_modifiers | KeyModifiers::ALT;
    }
    if modifiers.contains(CrosstermKeyModifiers::CONTROL) {
        shim_modifiers = shim_modifiers | KeyModifiers::CONTROL;
    }
    shim_modifiers
}

async fn read_frame(stream: &mut UnixStream) -> io::Result<Vec<u8>> {
    let mut len = [0_u8; 4];
    stream.read_exact(&mut len).await?;
    let len = u32::from_le_bytes(len) as usize;
    let mut frame = Vec::with_capacity(4 + len);
    frame.extend_from_slice(&(len as u32).to_le_bytes());
    frame.resize(4 + len, 0);
    stream.read_exact(&mut frame[4..]).await?;
    Ok(frame)
}

struct TerminalGuard {
    stdout: io::Stdout,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, Hide, EnableMouseCapture)?;
        Ok(Self { stdout })
    }

    fn write_full_frame(&mut self, rows: &[Vec<u8>]) -> io::Result<()> {
        for (idx, row) in rows.iter().enumerate() {
            execute!(self.stdout, MoveTo(0, idx as u16))?;
            self.stdout.write_all(row)?;
            execute!(self.stdout, Clear(ClearType::UntilNewLine))?;
        }
        execute!(self.stdout, MoveTo(0, rows.len() as u16))?;
        execute!(self.stdout, Clear(ClearType::FromCursorDown))?;
        self.stdout.flush()
    }

    fn write_patch_frame(
        &mut self,
        changed_rows: &[(u16, Vec<u8>)],
        clear_from_row: Option<u16>,
    ) -> io::Result<()> {
        for (row, bytes) in changed_rows {
            execute!(self.stdout, MoveTo(0, *row))?;
            self.stdout.write_all(bytes)?;
            execute!(self.stdout, Clear(ClearType::UntilNewLine))?;
        }
        if let Some(row) = clear_from_row {
            execute!(
                self.stdout,
                MoveTo(0, row),
                Clear(ClearType::FromCursorDown)
            )?;
        }
        self.stdout.flush()
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(
            self.stdout,
            crossterm::event::DisableMouseCapture,
            Show,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}
