use opensessions_sidebar::app::{App, PanelFocus};
use opensessions_sidebar::generated::protocol::ClientCommand;
use opensessions_sidebar::input::{UiKey, apply_ui_key};

#[test]
fn shared_input_mapping_drives_existing_session_navigation() {
    let mut app = App::reference_fixture("pane-attached-session-list");
    app.focused_session = Some("opensessions".into());

    apply_ui_key(&mut app, UiKey::Down);

    assert_eq!(
        app.focused_session.as_deref(),
        Some("plane-pdf-word-formatting")
    );
    assert_eq!(
        app.drain_commands(),
        vec![ClientCommand::FocusSession {
            name: "plane-pdf-word-formatting".into()
        }]
    );
}

#[test]
fn shared_input_mapping_preserves_agent_panel_bindings() {
    let mut app = App::reference_fixture("pane-opensessions-self");

    apply_ui_key(&mut app, UiKey::CtrlJ);
    assert_eq!(app.panel_focus, PanelFocus::Agents);

    apply_ui_key(&mut app, UiKey::Down);
    assert_eq!(app.focused_agent_idx, 1);

    apply_ui_key(&mut app, UiKey::CtrlK);
    assert_eq!(app.panel_focus, PanelFocus::Sessions);
}

#[test]
fn shared_input_mapping_keeps_reorder_and_switch_shortcuts() {
    let mut app = App::reference_fixture("pane-attached-session-list");
    app.focused_session = Some("plane-pdf-word-formatting".into());
    app.current_session = Some("opensessions".into());

    apply_ui_key(&mut app, UiKey::AltUp);
    apply_ui_key(&mut app, UiKey::Char('2'));
    apply_ui_key(&mut app, UiKey::Tab { shift: true });

    assert_eq!(
        app.drain_commands(),
        vec![
            ClientCommand::ReorderSession {
                name: "plane-pdf-word-formatting".into(),
                delta: -1,
            },
            ClientCommand::SwitchIndex { index: 2 },
            ClientCommand::SwitchSession {
                name: "learning".into(),
                client_tty: None,
            },
        ]
    );
}
