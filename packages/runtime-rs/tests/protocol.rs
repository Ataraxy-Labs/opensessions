use opensessions_runtime::protocol::{
    ClientCommand, LocalLink, LocalLinkKind, ProtocolHello, ServerMessage, ServerState,
    SessionData, SessionFilterMode,
};

#[test]
fn parses_protocol_hello_wire_message() {
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
fn serializes_client_commands_with_typescript_tags_and_fields() {
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
fn deserializes_client_commands_from_typescript_wire_format() {
    let cmd: ClientCommand = serde_json::from_str(
        r#"{"type":"switch-session","name":"api","clientTty":"/dev/ttys001"}"#,
    )
    .unwrap();
    assert_eq!(
        cmd,
        ClientCommand::SwitchSession {
            name: "api".to_string(),
            client_tty: Some("/dev/ttys001".to_string()),
        }
    );

    let cmd: ClientCommand =
        serde_json::from_str(r#"{"type":"set-filter","filter":"running"}"#).unwrap();
    assert_eq!(
        cmd,
        ClientCommand::SetFilter {
            filter: SessionFilterMode::Running,
        }
    );
}

#[test]
fn parses_state_with_nested_agent_metadata_and_filter() {
    let json = r##"{
        "type":"state",
        "sessions":[{
            "name":"opensessions","createdAt":1,"dir":"/repo","branch":"main",
            "dirty":false,"isWorktree":false,"unseen":false,"panes":2,
            "ports":[7391],"localLinks":[{"kind":"direct","port":7391,"url":"http://127.0.0.1:7391","label":":7391"}],"windows":1,"uptime":"1m",
            "agentState":{"agent":"amp","session":"opensessions","status":"tool-running","ts":2,"threadName":"Port TUI","liveness":"alive"},
            "agents":[],"eventTimestamps":[],"metadata":{"status":{"text":"working","tone":"info","ts":4},"progress":null,"logs":[]}
        }],
        "focusedSession":"opensessions","currentSession":"opensessions",
        "theme":"catppuccin-mocha","sessionFilter":"running","sidebarWidth":35,
        "initializing":false,"ts":3
    }"##;

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
    assert_eq!(state.sessions[0].local_links[0].label, ":7391");
    assert_eq!(
        state.sessions[0]
            .metadata
            .as_ref()
            .unwrap()
            .status
            .as_ref()
            .unwrap()
            .text,
        "working"
    );
}

#[test]
fn serializes_server_state_with_typescript_fields_and_omits_undefined_optionals() {
    let state = ServerState {
        sessions: vec![SessionData {
            name: "opensessions".to_string(),
            created_at: 1,
            dir: "/repo".to_string(),
            branch: "main".to_string(),
            dirty: false,
            is_worktree: false,
            unseen: false,
            panes: 2,
            ports: vec![7391],
            local_links: vec![LocalLink {
                kind: LocalLinkKind::Direct,
                port: 7391,
                url: "http://127.0.0.1:7391".to_string(),
                label: ":7391".to_string(),
            }],
            windows: 1,
            uptime: "1m".to_string(),
            agent_state: None,
            agents: Vec::new(),
            event_timestamps: Vec::new(),
            metadata: None,
        }],
        focused_session: Some("opensessions".to_string()),
        current_session: None,
        theme: None,
        session_filter: None,
        sidebar_width: 35,
        initializing: false,
        init_label: None,
        ts: 3,
    };

    assert_eq!(
        serde_json::to_string(&ServerMessage::State(state)).unwrap(),
        r#"{"type":"state","sessions":[{"name":"opensessions","createdAt":1,"dir":"/repo","branch":"main","dirty":false,"isWorktree":false,"unseen":false,"panes":2,"ports":[7391],"localLinks":[{"kind":"direct","port":7391,"url":"http://127.0.0.1:7391","label":":7391"}],"windows":1,"uptime":"1m","agentState":null,"agents":[],"eventTimestamps":[]}],"focusedSession":"opensessions","currentSession":null,"sidebarWidth":35,"initializing":false,"ts":3}"#,
    );
}
