use opensessions_sidebar::app::{App, PanelFocus};
use opensessions_sidebar::generated::protocol::ClientCommand;
use opensessions_sidebar::input::{UiMouse, apply_ui_mouse};
use opensessions_sidebar::renderer::{HitTarget, compute_hit_map};

const W: u16 = 35;
const H: u16 = 56;

#[test]
fn scroll_down_in_sessions_panel_moves_focus_and_queues_focus_session() {
    let mut app = App::reference_fixture("pane-attached-session-list");
    app.focused_session = Some("opensessions".into());

    apply_ui_mouse(&mut app, UiMouse::ScrollDown { x: 5, y: 5 });

    assert_eq!(
        app.focused_session.as_deref(),
        Some("plane-pdf-word-formatting")
    );
    assert_eq!(
        app.drain_commands(),
        vec![ClientCommand::FocusSession {
            name: "plane-pdf-word-formatting".into(),
        }]
    );
}

#[test]
fn scroll_up_in_agents_panel_moves_agent_focus() {
    let mut app = App::reference_fixture("pane-opensessions-self");
    app.focus_agents_panel();
    app.focused_agent_idx = 1;

    apply_ui_mouse(&mut app, UiMouse::ScrollUp { x: 5, y: 45 });

    assert_eq!(app.focused_agent_idx, 0);
    assert_eq!(app.panel_focus, PanelFocus::Agents);
}

#[test]
fn compute_hit_map_marks_session_rows_with_session_targets() {
    let app = App::reference_fixture("pane-attached-session-list");
    let hits = compute_hit_map(&app, W, H);

    assert_eq!(hits.len(), H as usize);

    // Find a row that maps to "opensessions".
    let opensessions_row = hits
        .iter()
        .position(|hit| matches!(hit, Some(HitTarget::Session(name)) if name == "opensessions"));
    assert!(
        opensessions_row.is_some(),
        "hit map must mark at least one row as the opensessions session card",
    );

    // Find a row mapped to plane-pdf-word-formatting (the focused session in this fixture).
    let pdf_row = hits.iter().position(
        |hit| matches!(hit, Some(HitTarget::Session(name)) if name == "plane-pdf-word-formatting"),
    );
    assert!(
        pdf_row.is_some(),
        "hit map must include the focused session card rows",
    );
}

#[test]
fn click_on_session_row_switches_to_that_session() {
    let mut app = App::reference_fixture("pane-attached-session-list");
    app.current_session = Some("opensessions".into());
    app.focused_session = Some("opensessions".into());

    let hits = compute_hit_map(&app, W, H);
    let target_row = hits
        .iter()
        .position(|hit| matches!(hit, Some(HitTarget::Session(name)) if name == "learning"))
        .expect("learning session must have a clickable row");

    apply_ui_mouse(
        &mut app,
        UiMouse::Click {
            x: 5,
            y: target_row as u16,
            width: W,
            height: H,
        },
    );

    assert_eq!(app.focused_session.as_deref(), Some("learning"));
    assert_eq!(app.current_session.as_deref(), Some("learning"));
    assert_eq!(
        app.drain_commands(),
        vec![ClientCommand::SwitchSession {
            name: "learning".into(),
            client_tty: None,
        }]
    );
}

#[test]
fn click_on_agent_row_focuses_agents_panel_and_switches_pane() {
    let mut app = App::reference_fixture("pane-opensessions-self");
    app.focused_session = Some("opensessions".into());

    let w: u16 = 35;
    let h: u16 = 55;
    let hits = compute_hit_map(&app, w, h);
    let agent_row = hits
        .iter()
        .position(|hit| matches!(hit, Some(HitTarget::Agent(1))))
        .expect("second agent row must be clickable in the detail panel");

    apply_ui_mouse(
        &mut app,
        UiMouse::Click {
            x: 5,
            y: agent_row as u16,
            width: w,
            height: h,
        },
    );

    assert_eq!(app.panel_focus, PanelFocus::Agents);
    assert_eq!(app.focused_agent_idx, 1);

    let commands = app.drain_commands();
    assert!(
        commands.iter().any(|cmd| matches!(
            cmd,
            ClientCommand::SwitchSession { name, .. } if name == "opensessions"
        )),
        "click on agent must switch to its session; got: {commands:?}"
    );
    assert!(
        commands
            .iter()
            .any(|cmd| matches!(cmd, ClientCommand::FocusAgentPane { session, .. } if session == "opensessions")),
        "click on agent must queue focus-agent-pane for that agent; got: {commands:?}"
    );
}

#[test]
fn click_outside_any_target_is_a_noop() {
    let mut app = App::reference_fixture("pane-attached-session-list");
    let drained_before = app.drain_commands().len();

    apply_ui_mouse(
        &mut app,
        UiMouse::Click {
            x: 0,
            y: 0,
            width: W,
            height: H,
        },
    );

    let drained_after = app.drain_commands().len();
    assert_eq!(drained_before, 0);
    assert_eq!(drained_after, 0);
}

#[test]
fn live_tui_enables_mouse_capture_and_handles_mouse_events() {
    let main_rs = include_str!("../src/main.rs");
    assert!(
        main_rs.contains("EnableMouseCapture"),
        "main.rs must enable mouse capture so the user can click sessions and scroll"
    );
    assert!(
        main_rs.contains("DisableMouseCapture"),
        "TerminalGuard must disable mouse capture on drop to restore the parent terminal"
    );
    assert!(
        main_rs.contains("Event::Mouse"),
        "main.rs must dispatch crossterm Event::Mouse to apply_ui_mouse"
    );
    assert!(
        main_rs.contains("apply_ui_mouse"),
        "main.rs must route mouse events through the shared apply_ui_mouse helper"
    );
}
