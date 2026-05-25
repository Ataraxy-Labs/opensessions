use anyhow::{Context, Result, bail};
use clap::Parser;
use crossterm::cursor::Show;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use futures_util::{SinkExt, StreamExt};
use opensessions_sidebar::app::{App, LaunchTarget};
use opensessions_sidebar::cli::{Args, resolve_endpoint_from_env};
use opensessions_sidebar::client::{
    connect_ws, decode_server_message, encode_client_command, fire_quit_http, validate_hello,
};
use opensessions_sidebar::generated::protocol::{ClientCommand, ServerMessage};
use opensessions_sidebar::input::{UiKey, UiMouse, apply_ui_key, apply_ui_mouse};
use opensessions_sidebar::renderer::render_app;
use opensessions_sidebar::runtime_context::{
    PaneIdentity as RuntimePaneIdentity, pane_identity_resolve, refocus_plan, report_width_command,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io;
use tokio_websockets::Message;

const DEFAULT_SERVER_HOST: &str = "127.0.0.1";
const DEFAULT_SERVER_PORT: u16 = 7_391;

/// Append a single debug line to the path in `OPENSESSIONS_DEBUG_LOG`
/// (defaults to `/tmp/opensessions-debug.log`). Mirrors the helper in
/// `apps/server-rs/src/lib.rs` so we can correlate client and server events
/// in the live tmux A/B harness.
fn debug_log(line: impl AsRef<str>) {
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};
    let path = std::env::var("OPENSESSIONS_DEBUG_LOG")
        .unwrap_or_else(|_| "/tmp/opensessions-debug-rs.log".to_string());
    if path.is_empty() {
        return;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "[{now}] [sidebar] {}", line.as_ref());
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();
    let env = resolve_endpoint_from_env(|key| std::env::var(key).ok());
    let server_host = if args.server_host == DEFAULT_SERVER_HOST {
        env.server_host
    } else {
        args.server_host
    };
    let server_port = if args.server_port == DEFAULT_SERVER_PORT {
        env.server_port
    } else {
        args.server_port
    };

    let identity = pane_identity_resolve(
        |key| std::env::var(key).ok(),
        |format, target| tmux_display_message(format, target),
    );

    debug_log(format!(
        "starting: connecting to ws://{server_host}:{server_port}/ identity={identity:?}"
    ));
    let mut ws = connect_ws(&server_host, server_port)
        .await
        .with_context(|| format!("connect ws://{server_host}:{server_port}/"))?;
    debug_log("ws: connected");

    let first = ws.next().await.context("read protocol hello")??;
    if !first.is_text() {
        bail!("expected text hello frame");
    }
    let hello = decode_server_message(first.as_payload())?;
    validate_hello(&hello).map_err(anyhow::Error::msg)?;

    if let Some(RuntimePaneIdentity {
        pane_id,
        session_name,
        window_id,
    }) = identity.clone()
    {
        let command = ClientCommand::IdentifyPane {
            pane_id,
            session_name,
            window_id,
        };
        ws.send(Message::text(encode_client_command(&command)?))
            .await?;
    }

    let mut terminal = TerminalGuard::enter()?;
    let mut events = EventStream::new();
    let mut app: Option<App> = None;
    let mut last_reported_width: Option<u32> = None;
    let mut startup_refocused = false;
    // Render-tick interval: advance the spinner clock and redraw at ~120ms so
    // the "warming up…" / "adjusting…" / agent-running spinners animate even
    // when no server state arrives. Mirrors the React render loop in the TS
    // sidebar driven by Date.now() inside Yoga's frame timer.
    let render_epoch = std::time::Instant::now();
    let mut render_tick = tokio::time::interval(tokio::time::Duration::from_millis(120));
    render_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        // Hard-exit timer: once the user presses 'q', App::handle_key_char sets
        // `quit_deadline` to now+500ms. If neither the WS Quit response nor the
        // HTTP /quit fallback tears us down before then, we exit anyway so the
        // user is never stuck in a dead TUI. Mirrors
        // `setTimeout(() => renderer.destroy(), 500)` in apps/tui/src/index.tsx.
        let quit_deadline = app.as_ref().and_then(|app| app.quit_deadline);
        // Click-flash expiry: when a click arms a 150ms flash highlight, force
        // a re-render at the deadline so the highlight clears even without
        // any other event. Mirrors `setTimeout` in TS `triggerFlash`.
        let flash_deadline = app.as_ref().and_then(|app| app.flash_deadline);

        tokio::select! {
            biased;

            _ = async {
                match quit_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                return Ok(());
            }

            _ = render_tick.tick() => {
                if let Some(app) = &mut app {
                    let now_ms = render_epoch.elapsed().as_millis() as u64;
                    if app.spinner_now != now_ms {
                        app.spinner_now = now_ms;
                        // Only redraw if there's something animating. Otherwise
                        // a 120ms idle wakeup costs about a single buffer diff
                        // which still fights the terminal for cursor focus.
                        let needs_redraw = app.initializing
                            || app.sessions.iter().any(|session| {
                                session.agents.iter().any(|agent| {
                                    matches!(
                                        agent.status,
                                        opensessions_sidebar::generated::protocol::AgentStatus::Running
                                            | opensessions_sidebar::generated::protocol::AgentStatus::ToolRunning
                                    )
                                })
                                    || session
                                        .agent_state
                                        .as_ref()
                                        .map(|state| {
                                            matches!(
                                                state.status,
                                                opensessions_sidebar::generated::protocol::AgentStatus::Running
                                            )
                                        })
                                        .unwrap_or(false)
                            });
                        if needs_redraw {
                            terminal.draw(app)?;
                        }
                    }
                }
                continue;
            }

            _ = async {
                match flash_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                if let Some(app) = &mut app {
                    app.flash_target = None;
                    app.flash_deadline = None;
                    terminal.draw(app)?;
                }
                continue;
            }

            event = events.next() => {
                let Some(event) = event else {
                    return Ok(());
                };
                let event = event?;
                if let Event::Key(key) = event {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(key.code, KeyCode::Char('c'))
                    {
                        return Ok(());
                    }
                    let Some(app) = &mut app else {
                        continue;
                    };

                    handle_key(app, key);
                    for command in app.drain_commands() {
                        let is_quit = matches!(command, ClientCommand::Quit);
                        ws.send(Message::text(encode_client_command(&command)?)).await?;
                        if is_quit {
                            // HTTP fallback on a separate TCP connection.
                            // Whichever path reaches the server first triggers
                            // quitAll → close WS → renderer teardown. We
                            // fire-and-forget on the current_thread runtime.
                            let host = server_host.clone();
                            let port = server_port;
                            tokio::spawn(async move {
                                fire_quit_http(&host, port).await;
                            });
                        }
                    }
                    for launch in app.drain_launches() {
                        let dir = app
                            .focused_session
                            .as_deref()
                            .and_then(|name| app.sessions.iter().find(|s| s.name == name))
                            .map(|s| s.dir.as_str())
                            .unwrap_or(".");
                        launch_lazydiff(launch, dir);
                    }
                    terminal.draw(app)?;
                } else if let Event::Resize(width, _) = event {
                    if let Some(app) = &mut app {
                        app.set_terminal_width(width);
                        let width = u32::from(width);
                        if last_reported_width != Some(width) {
                            let local_session = identity
                                .as_ref()
                                .map(|identity| identity.session_name.as_str())
                                .or(app.my_session.as_deref());
                            if let Some(command) = report_width_command(
                                width,
                                local_session,
                                app.current_session.as_deref(),
                            ) {
                                last_reported_width = Some(width);
                                ws.send(Message::text(encode_client_command(&command)?))
                                    .await?;
                            }
                        }
                        terminal.draw(app)?;
                    }
                } else if let Event::Mouse(mouse) = event {
                    if let Some(app) = &mut app
                        && let Some(ui_mouse) = ui_mouse_from_crossterm(mouse)
                    {
                        apply_ui_mouse(app, ui_mouse);
                        for command in app.drain_commands() {
                            ws.send(Message::text(encode_client_command(&command)?)).await?;
                        }
                        terminal.draw(app)?;
                    }
                }
            }

            message = ws.next() => {
                let Some(message) = message else {
                    return Ok(());
                };
                let message = message?;
                if message.is_close() {
                    return Ok(());
                }
                if message.is_text() {
                    let decoded = decode_server_message(message.as_payload())?;
                    if matches!(decoded, ServerMessage::Quit) {
                        return Ok(());
                    }
                    match (&mut app, decoded) {
                        (slot @ None, ServerMessage::State(state)) => {
                            debug_log(format!(
                                "ws: initial state received init={} init_label={:?} sessions={}",
                                state.initializing,
                                state.init_label,
                                state.sessions.len(),
                            ));
                            let mut new_app = App::from_state(state);
                            if let Some(identity) = identity.clone() {
                                new_app.set_pane_identity(
                                    identity.pane_id,
                                    identity.session_name,
                                    identity.window_id,
                                );
                            }
                            *slot = Some(new_app);
                        }
                        (Some(app), ServerMessage::State(state)) => {
                            debug_log(format!(
                                "ws: state update init={} init_label={:?} sessions={}",
                                state.initializing,
                                state.init_label,
                                state.sessions.len(),
                            ));
                            app.apply_server_message(ServerMessage::State(state));
                        }
                        (Some(app), message) => {
                            debug_log(format!("ws: received {message:?}"));
                            app.apply_server_message(message);
                        }
                        (None, _) => {}
                    }
                    if let Some(app) = &mut app {
                        for command in app.drain_commands() {
                            ws.send(Message::text(encode_client_command(&command)?)).await?;
                        }
                        terminal.draw(app)?;
                        if !startup_refocused {
                            startup_refocused = true;
                            if let Some(identity) = identity.as_ref() {
                                do_startup_refocus(&identity.pane_id);
                            }
                        }
                    }
                }
            }
        }
    }
}

fn handle_key(app: &mut App, key: KeyEvent) {
    if let Some(key) = ui_key_from_crossterm(key) {
        apply_ui_key(app, key);
    }
}

fn ui_mouse_from_crossterm(mouse: MouseEvent) -> Option<UiMouse> {
    match mouse.kind {
        MouseEventKind::ScrollUp => Some(UiMouse::ScrollUp {
            x: mouse.column,
            y: mouse.row,
        }),
        MouseEventKind::ScrollDown => Some(UiMouse::ScrollDown {
            x: mouse.column,
            y: mouse.row,
        }),
        MouseEventKind::Down(MouseButton::Left) => {
            // The hit map is computed against the current terminal size; query
            // it here so callers don't need to thread dimensions through the
            // event loop. Mirrors per-component `onMouseDown` in
            // `apps/tui/src/index.tsx`.
            let (width, height) = terminal::size().unwrap_or((0, 0));
            Some(UiMouse::Click {
                x: mouse.column,
                y: mouse.row,
                width,
                height,
            })
        }
        MouseEventKind::Drag(MouseButton::Left) => Some(UiMouse::Drag { y: mouse.row }),
        MouseEventKind::Up(MouseButton::Left) => Some(UiMouse::DragEnd),
        _ => None,
    }
}

fn ui_key_from_crossterm(key: KeyEvent) -> Option<UiKey> {
    if key.modifiers.contains(KeyModifiers::ALT) {
        return match key.code {
            KeyCode::Up => Some(UiKey::AltUp),
            KeyCode::Down => Some(UiKey::AltDown),
            _ => None,
        };
    } else if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('j') => Some(UiKey::CtrlJ),
            KeyCode::Char('k') => Some(UiKey::CtrlK),
            _ => None,
        };
    }

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => Some(UiKey::Down),
        KeyCode::Char('k') | KeyCode::Up => Some(UiKey::Up),
        KeyCode::Left => Some(UiKey::Left),
        KeyCode::Right => Some(UiKey::Right),
        KeyCode::Char(ch) => Some(UiKey::Char(ch)),
        KeyCode::Tab => Some(UiKey::Tab { shift: false }),
        KeyCode::BackTab => Some(UiKey::Tab { shift: true }),
        KeyCode::Enter => Some(UiKey::Enter),
        KeyCode::Esc => Some(UiKey::Esc),
        KeyCode::Backspace => Some(UiKey::Backspace),
        _ => None,
    }
}

/// Run `tmux display-message -p -t <target> <format>` and return the trimmed
/// stdout if the command succeeds with non-empty output. Mirrors the OpenTUI
/// client `getLocalSessionName` / `getLocalWindowId` fallback in
/// `apps/tui/src/index.tsx`.
fn tmux_display_message(format: &str, target: &str) -> Option<String> {
    let output = std::process::Command::new("tmux")
        .args(["display-message", "-p", "-t", target, format])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Invoke `tmux <args>` synchronously and return trimmed stdout when it
/// succeeds with non-empty output.
fn tmux_run(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("tmux").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let trimmed = value.trim_end_matches('\n').to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Refocus the main pane after the sidebar finishes drawing its first frame.
/// Mirrors `apps/tui/src/index.tsx::refocusMainPane` (called from
/// `doStartupRefocus`).
fn do_startup_refocus(pane_id: &str) {
    let refocus_window = std::env::var("REFOCUS_WINDOW").ok();
    let plan = refocus_plan(pane_id, refocus_window.as_deref(), |args| tmux_run(args));
    if let Some(plan) = plan {
        let _ = std::process::Command::new("tmux")
            .args(["select-pane", "-t", &plan.select_pane])
            .output();
    }
}

fn launch_lazydiff(target: LaunchTarget, dir: &str) {
    match target {
        LaunchTarget::LazydiffTmux => {
            let _ = std::process::Command::new("tmux")
                .args(["new-window", "-c", dir, "lazydiff"])
                .output();
        }
        LaunchTarget::LazydiffTerminal => {
            #[cfg(target_os = "macos")]
            {
                // Open a new Terminal.app window and run lazydiff in the session dir.
                let script = format!(
                    "tell application \"Terminal\" to do script \"cd {} && lazydiff\"",
                    dir.replace('\\', "\\\\").replace('"', "\\\"")
                );
                let _ = std::process::Command::new("osascript")
                    .args(["-e", &script])
                    .output();
            }
            #[cfg(not(target_os = "macos"))]
            {
                // Fallback: try common terminal emulators.
                let spawned = std::process::Command::new("x-terminal-emulator")
                    .args(["-e", "sh", "-c", &format!("cd {} && lazydiff", dir)])
                    .spawn();
                if spawned.is_err() {
                    let _ = std::process::Command::new("xterm")
                        .args(["-e", "sh", "-c", &format!("cd {} && lazydiff", dir)])
                        .spawn();
                }
            }
        }
    }
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.hide_cursor()?;
        Ok(Self { terminal })
    }

    fn draw(&mut self, app: &App) -> Result<()> {
        self.terminal.draw(|frame| render_app(frame, app))?;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.terminal.show_cursor();
        let _ = execute!(
            self.terminal.backend_mut(),
            Show,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = terminal::disable_raw_mode();
    }
}
