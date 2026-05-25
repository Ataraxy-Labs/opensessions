use opensessions_sidebar::client::encode_client_command;
use opensessions_sidebar::generated::protocol::{
    ClientCommand, ProtocolHello, ServerMessage, SessionFilterMode,
};
use opensessions_sidebar::runtime_config::{hash_server_key, resolve_server_port};

#[test]
fn parses_protocol_hello_as_first_class_server_message() {
    let msg: ServerMessage =
        serde_json::from_str(r#"{"type":"hello","protocol":1,"serverVersion":"0.2.0-alpha.5"}"#)
            .unwrap();
    assert_eq!(
        msg,
        ServerMessage::Hello(ProtocolHello {
            protocol: 1,
            server_version: "0.2.0-alpha.5".into()
        })
    );
}

#[test]
fn serializes_client_commands_with_kebab_tags_and_camel_fields() {
    let cmd = ClientCommand::IdentifyPane {
        pane_id: "%42".into(),
        session_name: "opensessions".into(),
        window_id: None,
    };

    assert_eq!(
        serde_json::to_string(&cmd).unwrap(),
        r#"{"type":"identify-pane","paneId":"%42","sessionName":"opensessions"}"#,
    );
}

#[test]
fn encodes_client_commands_as_compact_wire_json() {
    assert_eq!(
        encode_client_command(&ClientCommand::SwitchIndex { index: 2 }).unwrap(),
        r#"{"type":"switch-index","index":2}"#,
    );
}

#[test]
fn parses_state_with_nested_agent_and_filter() {
    let json = r#"{
        "type":"state",
        "sessions":[{
            "name":"opensessions","createdAt":1,"dir":"/repo","branch":"main",
            "dirty":false,"isWorktree":false,"unseen":false,"panes":2,
            "ports":[7391],"localLinks":[],"windows":1,"uptime":"1m",
            "agentState":{"agent":"amp","session":"opensessions","status":"tool-running","ts":2,"threadName":"Port TUI"},
            "agents":[],"eventTimestamps":[],"metadata":null
        }],
        "focusedSession":"opensessions","currentSession":"opensessions",
        "theme":"catppuccin-mocha","sessionFilter":"running","sidebarWidth":35,
        "initializing":false,"ts":3
    }"#;

    let msg: ServerMessage = serde_json::from_str(json).unwrap();
    let ServerMessage::State(state) = msg else {
        panic!("expected state")
    };
    assert_eq!(state.session_filter, Some(SessionFilterMode::Running));
    assert_eq!(
        state.sessions[0]
            .agent_state
            .as_ref()
            .unwrap()
            .status
            .to_string(),
        "tool-running"
    );
}

#[test]
fn server_key_hash_matches_typescript_runtime() {
    assert_eq!(hash_server_key("/private/tmp/tmux-501/default"), 19_916);
    assert_eq!(resolve_server_port(None, None), 7_391);
    assert_eq!(resolve_server_port(Some(19_916), None), 36_916);
    assert_eq!(resolve_server_port(Some(19_916), Some("8123")), 8_123);
}
