use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

use opensessions_runtime::git_info::GitInfo;
use opensessions_runtime::metadata_store::SessionMetadataStore;
use opensessions_runtime::mux::{MuxProvider, MuxSessionInfo};
use opensessions_runtime::portless::{
    PortlessState, build_local_links, load_portless_state, parse_portless_routes,
};
use opensessions_runtime::project_dir_session::{
    build_dir_session_map, resolve_session_for_project_dir,
};
use opensessions_runtime::protocol::{LocalLinkKind, MetadataTone};
use opensessions_runtime::server_state::{ReadOnlyStateInput, build_read_only_state};
use opensessions_runtime::session_order::SessionOrder;
use opensessions_runtime::sidebar_width_sync::{MIN_SIDEBAR_WIDTH, clamp_sidebar_width};

#[test]
fn metadata_store_truncates_caps_clears_and_prunes() {
    let mut store = SessionMetadataStore::new();
    assert_eq!(store.get("unknown"), None);

    store.set_status("api", Some(("x".repeat(200), Some(MetadataTone::Info))));
    let meta = store.get("api").unwrap();
    assert_eq!(meta.status.as_ref().unwrap().text.chars().count(), 100);
    assert!(meta.status.as_ref().unwrap().text.ends_with('…'));

    store.set_progress(
        "api",
        Some((Some(3), Some(10), Some(0.3), Some("files".to_string()))),
    );
    store.append_log(
        "api",
        "Build started".to_string(),
        Some(MetadataTone::Info),
        Some("ci".to_string()),
    );
    for i in 0..60 {
        store.append_log("api", format!("log {i}"), None, None);
    }

    let meta = store.get("api").unwrap();
    assert_eq!(meta.progress.as_ref().unwrap().current, Some(3));
    assert_eq!(meta.logs.len(), 50);
    assert_eq!(meta.logs[0].message, "log 10");

    store.clear_logs("api");
    store.set_status("api", None);
    store.set_progress("api", None);
    assert_eq!(store.get("api"), None);

    store.set_status("web", Some(("building".to_string(), None)));
    store.prune_sessions(["api".to_string()]);
    assert_eq!(store.get("web"), None);
}

#[test]
fn session_order_reorders_hides_persists_and_ignores_corrupt_files() {
    let path = temp_path("session-order.json");
    let mut order = SessionOrder::new(Some(path.clone()));
    order.sync(["a".to_string(), "b".to_string(), "c".to_string()]);

    order.reorder("c", -1);
    assert_eq!(
        order.apply(["a".to_string(), "b".to_string(), "c".to_string()]),
        vec!["a", "c", "b"]
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&path).unwrap()).unwrap(),
        serde_json::json!(["a", "c", "b"])
    );

    order.hide("c");
    assert_eq!(
        order.apply(["a".to_string(), "b".to_string(), "c".to_string()]),
        vec!["a", "b"]
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&path).unwrap()).unwrap(),
        serde_json::json!({ "order": ["a", "c", "b"], "hidden": ["c"] })
    );

    let mut loaded = SessionOrder::new(Some(path.clone()));
    loaded.sync(["a".to_string(), "b".to_string(), "c".to_string()]);
    assert_eq!(
        loaded.apply(["a".to_string(), "b".to_string(), "c".to_string()]),
        vec!["a", "b"]
    );
    loaded.show_all();
    assert_eq!(
        loaded.apply(["a".to_string(), "b".to_string(), "c".to_string()]),
        vec!["a", "c", "b"]
    );

    fs::write(&path, "not json{{{").unwrap();
    let mut corrupt = SessionOrder::new(Some(path.clone()));
    corrupt.sync(["x".to_string(), "y".to_string()]);
    assert_eq!(
        corrupt.apply(["x".to_string(), "y".to_string()]),
        vec!["x", "y"]
    );

    let _ = fs::remove_file(path);
}

#[test]
fn project_dir_resolution_matches_exact_related_and_encoded_rules() {
    let sessions = build_dir_session_map([
        ("api".to_string(), "/projects/my.app".to_string()),
        ("web".to_string(), "/projects/web".to_string()),
    ]);
    assert_eq!(
        resolve_session_for_project_dir("/projects/my.app", &sessions),
        Some("api".to_string())
    );
    assert_eq!(
        resolve_session_for_project_dir("__encoded__:-projects-my-app", &sessions),
        Some("api".to_string())
    );

    let ambiguous = build_dir_session_map([
        ("root".to_string(), "/projects".to_string()),
        ("pkg".to_string(), "/projects/myapp/packages/ui".to_string()),
    ]);
    assert_eq!(
        resolve_session_for_project_dir("/projects/myapp", &ambiguous),
        None
    );
}

#[test]
fn portless_helpers_parse_load_and_build_links() {
    let routes = parse_portless_routes(
        r#"[{"hostname":"editor.localhost","port":4549},{"hostname":"bad","port":"nope"},null]"#,
    );
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].hostname, "editor.localhost");

    let dir = temp_dir("portless");
    fs::write(
        dir.join("routes.json"),
        r#"[{"hostname":"editor.localhost","port":4549},{"hostname":"api.localhost","port":4312}]"#,
    )
    .unwrap();
    fs::write(dir.join("proxy.port"), "1355").unwrap();

    let state = load_portless_state([dir.clone()]).unwrap();
    assert_eq!(state.proxy_port, 1355);
    assert_eq!(
        state.routes_by_port.get(&4549).unwrap(),
        &vec!["editor.localhost".to_string()]
    );

    let links = build_local_links([4549, 9000], Some(&state));
    assert_eq!(links[0].kind, LocalLinkKind::Portless);
    assert_eq!(links[0].url, "http://editor.localhost:1355");
    assert_eq!(links[1].kind, LocalLinkKind::Direct);
    assert_eq!(links[1].label, "localhost:9000");

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn sidebar_width_clamps_to_minimum() {
    assert_eq!(clamp_sidebar_width(0), MIN_SIDEBAR_WIDTH);
    assert_eq!(clamp_sidebar_width(10), MIN_SIDEBAR_WIDTH);
    assert_eq!(clamp_sidebar_width(30), 30);
}

#[test]
fn read_only_state_sorts_sessions_and_selects_current_focus_and_uptime() {
    let mux = StateMux {
        current: Some("beta".to_string()),
        sessions: vec![
            mux_session("beta", 200, "/repo/beta", 1),
            mux_session("alpha", 100, "/repo/alpha", 2),
            mux_session("aardvark", 100, "/repo/aardvark", 1),
        ],
        pane_counts: vec![("alpha".to_string(), 3), ("beta".to_string(), 4)],
    };

    let state = build_read_only_state(ReadOnlyStateInput {
        providers: vec![&mux],
        visible_session_names: None,
        metadata_by_session: None,
        git_by_session: None,
        agent_state_by_session: None,
        agents_by_session: None,
        event_timestamps_by_session: None,
        unseen_sessions: None,
        ports_by_session: None,
        portless_state: None,
        focused_session: None,
        theme: Some("mocha".to_string()),
        session_filter: None,
        sidebar_width: 31,
        initializing: false,
        init_label: None,
        now_ms: 1_000_000,
    });

    assert_eq!(
        state
            .sessions
            .iter()
            .map(|session| session.name.as_str())
            .collect::<Vec<_>>(),
        vec!["aardvark", "alpha", "beta"]
    );
    assert_eq!(state.current_session.as_deref(), Some("beta"));
    assert_eq!(state.focused_session.as_deref(), Some("beta"));
    assert_eq!(state.theme.as_deref(), Some("mocha"));
    assert_eq!(state.sidebar_width, 31);
    assert_eq!(state.sessions[0].uptime, "15m");
    assert_eq!(state.sessions[1].panes, 3);
    assert_eq!(state.sessions[2].panes, 4);
    assert!(
        state
            .sessions
            .iter()
            .all(|session| session.branch.is_empty())
    );
    assert!(
        state
            .sessions
            .iter()
            .all(|session| session.ports.is_empty())
    );
    assert_eq!(state.ts, 1_000_000);
}

#[test]
fn read_only_state_keeps_valid_focus_or_falls_back_to_first_session() {
    let mux = StateMux {
        current: Some("missing".to_string()),
        sessions: vec![
            mux_session("beta", 200, "/repo/beta", 1),
            mux_session("alpha", 100, "/repo/alpha", 1),
        ],
        pane_counts: Vec::new(),
    };

    let kept = build_read_only_state(ReadOnlyStateInput {
        providers: vec![&mux],
        visible_session_names: None,
        metadata_by_session: None,
        git_by_session: None,
        agent_state_by_session: None,
        agents_by_session: None,
        event_timestamps_by_session: None,
        unseen_sessions: None,
        ports_by_session: None,
        portless_state: None,
        focused_session: Some("beta".to_string()),
        theme: None,
        session_filter: None,
        sidebar_width: 26,
        initializing: true,
        init_label: Some("Loading".to_string()),
        now_ms: 10_000,
    });
    assert_eq!(kept.focused_session.as_deref(), Some("beta"));
    assert_eq!(kept.current_session.as_deref(), Some("missing"));
    assert!(kept.initializing);
    assert_eq!(kept.init_label.as_deref(), Some("Loading"));

    let fallback = build_read_only_state(ReadOnlyStateInput {
        providers: vec![&mux],
        visible_session_names: None,
        metadata_by_session: None,
        git_by_session: None,
        agent_state_by_session: None,
        agents_by_session: None,
        event_timestamps_by_session: None,
        unseen_sessions: None,
        ports_by_session: None,
        portless_state: None,
        focused_session: Some("gone".to_string()),
        theme: None,
        session_filter: None,
        sidebar_width: 26,
        initializing: false,
        init_label: None,
        now_ms: 10_000,
    });
    assert_eq!(fallback.focused_session.as_deref(), Some("alpha"));
}

#[test]
fn read_only_state_applies_visible_session_order() {
    let mux = StateMux {
        current: Some("beta".to_string()),
        sessions: vec![
            mux_session("alpha", 100, "/repo/alpha", 1),
            mux_session("beta", 200, "/repo/beta", 1),
            mux_session("gamma", 300, "/repo/gamma", 1),
        ],
        pane_counts: Vec::new(),
    };

    let state = build_read_only_state(ReadOnlyStateInput {
        providers: vec![&mux],
        visible_session_names: Some(vec!["gamma".to_string(), "alpha".to_string()]),
        metadata_by_session: None,
        git_by_session: None,
        agent_state_by_session: None,
        agents_by_session: None,
        event_timestamps_by_session: None,
        unseen_sessions: None,
        ports_by_session: None,
        portless_state: None,
        focused_session: Some("beta".to_string()),
        theme: None,
        session_filter: None,
        sidebar_width: 26,
        initializing: false,
        init_label: None,
        now_ms: 10_000,
    });

    assert_eq!(
        state
            .sessions
            .iter()
            .map(|session| session.name.as_str())
            .collect::<Vec<_>>(),
        vec!["gamma", "alpha"]
    );
    assert_eq!(state.focused_session.as_deref(), Some("gamma"));
}

#[test]
fn read_only_state_marks_unseen_sessions() {
    let mux = StateMux {
        current: Some("beta".to_string()),
        sessions: vec![
            mux_session("alpha", 100, "/repo/alpha", 1),
            mux_session("beta", 200, "/repo/beta", 1),
        ],
        pane_counts: Vec::new(),
    };

    let state = build_read_only_state(ReadOnlyStateInput {
        providers: vec![&mux],
        visible_session_names: None,
        metadata_by_session: None,
        git_by_session: None,
        agent_state_by_session: None,
        agents_by_session: None,
        event_timestamps_by_session: None,
        unseen_sessions: Some(vec!["alpha".to_string()]),
        ports_by_session: None,
        portless_state: None,
        focused_session: None,
        theme: None,
        session_filter: None,
        sidebar_width: 26,
        initializing: false,
        init_label: None,
        now_ms: 10_000,
    });

    assert!(
        state
            .sessions
            .iter()
            .find(|session| session.name == "alpha")
            .unwrap()
            .unseen
    );
    assert!(
        !state
            .sessions
            .iter()
            .find(|session| session.name == "beta")
            .unwrap()
            .unseen
    );
}

#[test]
fn read_only_state_includes_ports_and_local_links() {
    let mux = StateMux {
        current: Some("api".to_string()),
        sessions: vec![mux_session("api", 100, "/repo/api", 1)],
        pane_counts: Vec::new(),
    };
    let portless_state = PortlessState {
        proxy_port: 1355,
        secure: false,
        routes_by_port: BTreeMap::from([(4549, vec!["editor.localhost".to_string()])]),
    };

    let state = build_read_only_state(ReadOnlyStateInput {
        providers: vec![&mux],
        visible_session_names: None,
        metadata_by_session: None,
        git_by_session: None,
        agent_state_by_session: None,
        agents_by_session: None,
        event_timestamps_by_session: None,
        unseen_sessions: None,
        ports_by_session: Some(HashMap::from([("api".to_string(), vec![4549, 9000])])),
        portless_state: Some(portless_state),
        focused_session: None,
        theme: None,
        session_filter: None,
        sidebar_width: 26,
        initializing: false,
        init_label: None,
        now_ms: 10_000,
    });

    let session = &state.sessions[0];
    assert_eq!(session.ports, vec![4549, 9000]);
    assert_eq!(session.local_links.len(), 2);
    assert_eq!(session.local_links[0].kind, LocalLinkKind::Portless);
    assert_eq!(session.local_links[0].url, "http://editor.localhost:1355");
    assert_eq!(session.local_links[1].kind, LocalLinkKind::Direct);
    assert_eq!(session.local_links[1].url, "http://localhost:9000");
}

#[test]
fn read_only_state_includes_git_info() {
    let mux = StateMux {
        current: Some("api".to_string()),
        sessions: vec![mux_session("api", 100, "/repo/api", 1)],
        pane_counts: Vec::new(),
    };

    let state = build_read_only_state(ReadOnlyStateInput {
        providers: vec![&mux],
        visible_session_names: None,
        metadata_by_session: None,
        git_by_session: Some(HashMap::from([(
            "api".to_string(),
            GitInfo {
                branch: "main".to_string(),
                dirty: true,
                is_worktree: true,
            },
        )])),
        agent_state_by_session: None,
        agents_by_session: None,
        event_timestamps_by_session: None,
        unseen_sessions: None,
        ports_by_session: None,
        portless_state: None,
        focused_session: None,
        theme: None,
        session_filter: None,
        sidebar_width: 26,
        initializing: false,
        init_label: None,
        now_ms: 10_000,
    });

    let session = &state.sessions[0];
    assert_eq!(session.branch, "main");
    assert!(session.dirty);
    assert!(session.is_worktree);
}

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "opensessions-runtime-rs-{name}-{}",
        std::process::id()
    ))
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = temp_path(name);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn mux_session(name: &str, created_at: u64, dir: &str, windows: u32) -> MuxSessionInfo {
    MuxSessionInfo {
        name: name.to_string(),
        created_at,
        dir: dir.to_string(),
        windows,
    }
}

#[derive(Debug)]
struct StateMux {
    current: Option<String>,
    sessions: Vec<MuxSessionInfo>,
    pane_counts: Vec<(String, u32)>,
}

impl MuxProvider for StateMux {
    fn name(&self) -> &str {
        "state-mux"
    }

    fn list_sessions(&self) -> Vec<MuxSessionInfo> {
        self.sessions.clone()
    }

    fn switch_session(&self, _name: &str, _client_tty: Option<&str>) {}

    fn get_current_session(&self) -> Option<String> {
        self.current.clone()
    }

    fn get_session_dir(&self, _name: &str) -> String {
        String::new()
    }

    fn get_pane_count(&self, name: &str) -> u32 {
        self.pane_counts
            .iter()
            .find(|(session, _)| session == name)
            .map(|(_, count)| *count)
            .unwrap_or(1)
    }

    fn get_client_tty(&self) -> String {
        String::new()
    }

    fn create_session(&self, _name: Option<&str>, _dir: Option<&str>) {}

    fn kill_session(&self, _name: &str) {}

    fn setup_hooks(&self, _server_host: &str, _server_port: u16) {}

    fn cleanup_hooks(&self) {}
}
