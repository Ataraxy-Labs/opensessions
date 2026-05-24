use opensessions_sidebar::app::App;
use opensessions_sidebar::generated::protocol::AgentStatus;
use opensessions_sidebar::snapshot::{
    buffer_bg_at, buffer_dimensions, buffer_symbol_at, buffer_to_ansi, render_to_buffer,
};

#[test]
fn matches_attached_session_list_ansi_snapshot() {
    assert_snapshot("pane-attached-session-list", 35, 56);
}

#[test]
fn matches_opensessions_self_ansi_snapshot() {
    assert_snapshot("pane-opensessions-self", 35, 55);
}

#[test]
fn matches_multi_window_ansi_snapshot() {
    assert_snapshot("pane-multi-window", 35, 56);
}

#[test]
fn render_to_buffer_exposes_ratatui_test_backend_cells() {
    let mut app = App::reference_fixture("pane-attached-session-list");
    let buffer = render_to_buffer(&mut app, 35, 56);

    assert_eq!(buffer_dimensions(&buffer), (35, 56));
    assert_eq!(buffer_symbol_at(&buffer, 3, 1), "S");
    assert_eq!(buffer_symbol_at(&buffer, 1, 43), "─");
}

#[test]
fn focused_agent_row_uses_highlight_background() {
    let mut app = App::reference_fixture("pane-opensessions-self");
    app.focus_agents_panel();
    app.focused_agent_idx = 0;
    let buffer = render_to_buffer(&mut app, 35, 55);

    assert_eq!(buffer_bg_at(&buffer, 1, 41), Some((69, 71, 90)));
}

#[test]
fn focused_detail_panel_streams_agent_status_labels() {
    let mut app = App::reference_fixture("pane-opensessions-self");
    app.focused_session = Some("opensessions".into());
    let session = app
        .sessions
        .iter_mut()
        .find(|session| session.name == "opensessions")
        .expect("fixture should include opensessions");
    let mut running = session.agents[0].clone();
    running.status = AgentStatus::Running;
    running.thread_name = Some("Implement shim protocol".into());
    let mut waiting = running.clone();
    waiting.status = AgentStatus::Waiting;
    waiting.thread_name = Some("Waiting for approval".into());
    let mut error = running.clone();
    error.status = AgentStatus::Error;
    error.thread_name = Some("Fix failed build".into());
    session.agents = vec![running, waiting, error];
    app.focus_agents_panel();

    let buffer = render_to_buffer(&mut app, 35, 55);
    let ansi = buffer_to_ansi(&buffer);

    assert!(ansi.contains("running"));
    assert!(ansi.contains("waiting"));
    assert!(ansi.contains("error"));
    assert!(ansi.contains("Implement shim protocol"));
}

#[test]
fn large_session_list_keeps_detail_and_footer_anchored() {
    let mut app = App::reference_fixture("pane-attached-session-list");
    let template = app.sessions[1].clone();
    for idx in 0..100 {
        let mut session = template.clone();
        session.name = format!("extra-{idx}");
        session.dir = format!("/tmp/extra-{idx}");
        session.branch = "main".into();
        app.sessions.push(session);
    }

    let buffer = render_to_buffer(&mut app, 35, 56);

    assert_eq!(buffer_symbol_at(&buffer, 1, 43), "─");
    assert_eq!(buffer_symbol_at(&buffer, 1, 52), "─");
    assert_eq!(buffer_symbol_at(&buffer, 1, 53), "⇥");
}

#[test]
fn focused_session_far_down_list_is_scrolled_into_view() {
    let mut app = App::reference_fixture("pane-attached-session-list");
    let template = app.sessions[1].clone();
    for idx in 0..50 {
        let mut session = template.clone();
        session.name = format!("extra-{idx}");
        session.dir = format!("/tmp/extra-{idx}");
        session.branch = "main".into();
        app.sessions.push(session);
    }
    app.focused_session = Some("extra-40".into());

    let buffer = render_to_buffer(&mut app, 35, 56);

    // The sessions list lives between the header (rows 0..3) and the detail
    // separator at row 42. The focused row must be rendered with the highlight
    // background (SURFACE1 = 69,71,90) somewhere in that range.
    let mut found_focused_row = false;
    for y in 3..42 {
        if buffer_bg_at(&buffer, 5, y) == Some((69, 71, 90)) {
            // Reconstruct the row's text from the buffer to verify it is the
            // focused session, not a different highlighted row.
            let mut row = String::new();
            for x in 0..35 {
                row.push_str(&buffer_symbol_at(&buffer, x, y));
            }
            if row.contains("extra-40") {
                found_focused_row = true;
                break;
            }
        }
    }
    assert!(
        found_focused_row,
        "focused session row 'extra-40' must be rendered in the sessions list area",
    );
}

#[test]
fn sessions_list_renders_scroll_indicator_when_overflow() {
    let mut app = App::reference_fixture("pane-attached-session-list");
    let template = app.sessions[1].clone();
    for idx in 0..50 {
        let mut session = template.clone();
        session.name = format!("extra-{idx}");
        session.dir = format!("/tmp/extra-{idx}");
        session.branch = "main".into();
        app.sessions.push(session);
    }
    app.focused_session = Some("extra-40".into());

    let buffer = render_to_buffer(&mut app, 35, 56);

    let mut found_up = false;
    let mut found_down = false;
    for y in 0..43 {
        let glyph = buffer_symbol_at(&buffer, 33, y);
        if glyph == "▲" {
            found_up = true;
        } else if glyph == "▼" {
            found_down = true;
        }
    }
    assert!(
        found_up,
        "scrolling past the top of the sessions list must show a ▲ indicator at the right edge",
    );
    assert!(
        found_down,
        "more sessions below the visible window must show a ▼ indicator at the right edge",
    );
}

#[test]
fn focused_agent_below_visible_window_scrolls_into_detail_view() {
    use opensessions_sidebar::generated::protocol::{AgentEvent, AgentLiveness, AgentStatus};

    let mut app = App::reference_fixture("pane-opensessions-self");
    let session = app
        .sessions
        .iter_mut()
        .find(|session| session.name == "opensessions")
        .expect("opensessions session in fixture");
    session.agents = (0..8)
        .map(|i| AgentEvent {
            agent: "amp".to_string(),
            session: "opensessions".to_string(),
            status: AgentStatus::Idle,
            ts: 0,
            thread_id: Some(format!("thread-{i}")),
            thread_name: Some(format!("Thread number {i:02}")),
            unseen: None,
            pane_id: None,
            liveness: Some(AgentLiveness::Alive),
        })
        .collect();
    app.focus_agents_panel();
    app.focused_agent_idx = 7;

    let buffer = render_to_buffer(&mut app, 35, 55);

    // Detail area for fixture pane-opensessions-self is lines [39..51].
    // The focused agent must render with the SURFACE1 highlight inside that
    // window even when it sits well below the natural visible region.
    let mut focused_row_text: Option<String> = None;
    for y in 39..51 {
        if buffer_bg_at(&buffer, 4, y) == Some((69, 71, 90)) {
            let mut row = String::new();
            for x in 0..35 {
                row.push_str(&buffer_symbol_at(&buffer, x, y));
            }
            focused_row_text = Some(row);
            break;
        }
    }
    assert!(
        focused_row_text.is_some(),
        "focused agent (idx 7) must be scrolled into the detail panel",
    );
    let mut visible_threads = String::new();
    for y in 39..51 {
        for x in 0..35 {
            visible_threads.push_str(&buffer_symbol_at(&buffer, x, y));
        }
        visible_threads.push('\n');
    }
    assert!(
        visible_threads.contains("Thread number 07"),
        "focused agent's thread label must appear in the visible detail area; got:\n{visible_threads}",
    );
}

#[test]
fn live_tui_draws_with_ratatui_backend() {
    let main_rs = include_str!("../src/main.rs");

    assert!(!main_rs.contains("snapshot"));
    assert!(!main_rs.contains("render_to_buffer"));
    assert!(!main_rs.contains("buffer_to_terminal_ansi"));
}

#[test]
fn live_tui_uses_quit_http_fallback_and_deadline() {
    let main_rs = include_str!("../src/main.rs");

    assert!(
        main_rs.contains("fire_quit_http"),
        "main.rs must fire the HTTP /quit fallback, mirroring fetch(`http://HOST:PORT/quit`)"
    );
    assert!(
        main_rs.contains("quit_deadline"),
        "main.rs must consult app.quit_deadline so the renderer is torn down within 500ms even if neither WS Quit nor HTTP /quit reaches the server"
    );
}

#[test]
fn live_tui_wires_runtime_context_helpers() {
    let main_rs = include_str!("../src/main.rs");

    assert!(
        main_rs.contains("resolve_endpoint_from_env"),
        "main.rs must resolve endpoint from env so tmux-derived ports are picked up"
    );
    assert!(
        main_rs.contains("pane_identity_resolve"),
        "main.rs must read TMUX_PANE + OPENSESSIONS_SESSION_NAME (with tmux display-message fallback) to identify the pane"
    );
    assert!(
        main_rs.contains("report_width_command"),
        "main.rs must use report_width_command so background sidebars do not report width"
    );
    assert!(
        main_rs.contains("IdentifyPane"),
        "main.rs must send identify-pane to the server after connecting"
    );
}

fn assert_snapshot(name: &str, width: u16, height: u16) {
    let mut app = App::reference_fixture(name);
    let buffer = render_to_buffer(&mut app, width, height);
    let actual = buffer_to_ansi(&buffer);
    let expected = match name {
        "pane-attached-session-list" => include_str!(
            "../../../docs/ratatui-migration/reference-snapshots/pane-attached-session-list.ansi"
        ),
        "pane-opensessions-self" => include_str!(
            "../../../docs/ratatui-migration/reference-snapshots/pane-opensessions-self.ansi"
        ),
        "pane-multi-window" => include_str!(
            "../../../docs/ratatui-migration/reference-snapshots/pane-multi-window.ansi"
        ),
        _ => unreachable!(),
    };
    assert_eq!(actual, expected);
}
