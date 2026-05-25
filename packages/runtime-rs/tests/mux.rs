use std::sync::Arc;

use opensessions_runtime::mux::{MuxProvider, MuxRegistry, MuxSessionInfo, SidebarPosition};

#[test]
fn mux_contract_exposes_required_fields_and_capability_checks() {
    let provider = FakeMux::minimal("tmux");

    let session = MuxSessionInfo {
        name: "my-session".to_string(),
        created_at: 1_700_000_000,
        dir: "/repo".to_string(),
        windows: 2,
    };
    assert_eq!(session.name, "my-session");
    assert_eq!(session.created_at, 1_700_000_000);

    assert_eq!(provider.specification_version(), "v1");
    assert_eq!(provider.name(), "tmux");
    assert!(!provider.is_window_capable());
    assert!(!provider.is_sidebar_capable());
    assert!(!provider.is_batch_capable());
    assert!(!provider.is_full_sidebar_capable());

    let full = FakeMux::full("zellij");
    assert!(full.is_window_capable());
    assert!(full.is_sidebar_capable());
    assert!(full.is_batch_capable());
    assert!(full.is_full_sidebar_capable());
    assert_eq!(
        full.spawn_sidebar("s", "@1", 30, SidebarPosition::Right, "/scripts"),
        Some("%1".to_string())
    );
}

#[test]
fn mux_registry_registers_overwrites_lists_and_resolves() {
    let mut registry = MuxRegistry::new();
    registry.register(Arc::new(FakeMux::minimal("tmux")));
    registry.register(Arc::new(FakeMux::minimal("zellij")));
    registry.register(Arc::new(FakeMux::minimal("tmux")));

    assert_eq!(registry.list(), vec!["tmux", "zellij"]);
    assert_eq!(registry.get("tmux").unwrap().name(), "tmux");
    assert!(registry.get("missing").is_none());

    assert_eq!(
        registry.resolve(Some("zellij"), |_| None).unwrap().name(),
        "zellij"
    );
    assert!(registry.resolve(Some("missing"), |_| None).is_none());
    assert_eq!(
        registry
            .resolve(None, |key| (key == "TMUX")
                .then(|| "socket,1,0".to_string()))
            .unwrap()
            .name(),
        "tmux"
    );
    assert_eq!(
        registry
            .resolve(None, |key| (key == "ZELLIJ_SESSION_NAME")
                .then(|| "z".to_string()))
            .unwrap()
            .name(),
        "zellij"
    );
    assert!(registry.resolve(None, |_| None).is_none());
}

#[derive(Debug)]
struct FakeMux {
    name: String,
    full: bool,
}

impl FakeMux {
    fn minimal(name: &str) -> Self {
        Self {
            name: name.to_string(),
            full: false,
        }
    }

    fn full(name: &str) -> Self {
        Self {
            name: name.to_string(),
            full: true,
        }
    }
}

impl MuxProvider for FakeMux {
    fn name(&self) -> &str {
        &self.name
    }

    fn list_sessions(&self) -> Vec<MuxSessionInfo> {
        Vec::new()
    }

    fn switch_session(&self, _name: &str, _client_tty: Option<&str>) {}

    fn get_current_session(&self) -> Option<String> {
        None
    }

    fn get_session_dir(&self, _name: &str) -> String {
        String::new()
    }

    fn get_pane_count(&self, _name: &str) -> u32 {
        1
    }

    fn get_client_tty(&self) -> String {
        String::new()
    }

    fn create_session(&self, _name: Option<&str>, _dir: Option<&str>) {}

    fn kill_session(&self, _name: &str) {}

    fn setup_hooks(&self, _server_host: &str, _server_port: u16) {}

    fn cleanup_hooks(&self) {}

    fn is_window_capable(&self) -> bool {
        self.full
    }

    fn is_sidebar_capable(&self) -> bool {
        self.full
    }

    fn is_batch_capable(&self) -> bool {
        self.full
    }

    fn spawn_sidebar(
        &self,
        _session_name: &str,
        _window_id: &str,
        _width: u16,
        _position: SidebarPosition,
        _scripts_dir: &str,
    ) -> Option<String> {
        self.full.then(|| "%1".to_string())
    }
}
