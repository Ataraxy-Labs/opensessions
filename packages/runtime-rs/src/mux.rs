use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuxSessionInfo {
    pub name: String,
    pub created_at: u64,
    pub dir: String,
    pub windows: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveWindow {
    pub id: String,
    pub session_name: String,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarPane {
    pub pane_id: String,
    pub session_name: String,
    pub window_id: String,
    pub width: Option<u16>,
    pub window_width: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPane {
    pub agent: String,
    pub pane_id: String,
    pub thread_id: Option<String>,
    pub thread_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarPosition {
    Left,
    Right,
}

pub trait MuxProvider: Send + Sync {
    fn specification_version(&self) -> &'static str {
        "v1"
    }

    fn name(&self) -> &str;
    fn list_sessions(&self) -> Vec<MuxSessionInfo>;
    fn switch_session(&self, name: &str, client_tty: Option<&str>);
    fn get_current_session(&self) -> Option<String>;
    fn get_session_dir(&self, name: &str) -> String;
    fn get_session_pane_pids(&self, _name: &str) -> Vec<u32> {
        Vec::new()
    }
    fn get_pane_count(&self, name: &str) -> u32;
    fn get_client_tty(&self) -> String;
    fn create_session(&self, name: Option<&str>, dir: Option<&str>);
    fn kill_session(&self, name: &str);
    fn setup_hooks(&self, server_host: &str, server_port: u16);
    fn cleanup_hooks(&self);

    fn is_window_capable(&self) -> bool {
        false
    }

    fn is_sidebar_capable(&self) -> bool {
        false
    }

    fn is_batch_capable(&self) -> bool {
        false
    }

    fn is_full_sidebar_capable(&self) -> bool {
        self.is_window_capable() && self.is_sidebar_capable()
    }

    fn list_active_windows(&self) -> Vec<ActiveWindow> {
        Vec::new()
    }

    fn get_current_window_id(&self) -> Option<String> {
        None
    }

    fn list_sidebar_panes(&self, _session_name: Option<&str>) -> Vec<SidebarPane> {
        Vec::new()
    }

    fn list_agent_panes(&self, _session_name: &str) -> Vec<AgentPane> {
        Vec::new()
    }

    fn spawn_sidebar(
        &self,
        _session_name: &str,
        _window_id: &str,
        _width: u16,
        _position: SidebarPosition,
        _scripts_dir: &str,
    ) -> Option<String> {
        None
    }

    fn hide_sidebar(&self, _pane_id: &str) {}

    fn kill_sidebar_pane(&self, _pane_id: &str) {}
    fn resize_sidebar_pane(&self, _pane_id: &str, _width: u16) {}
    fn kill_orphaned_sidebar_panes(&self) {}
    fn cleanup_sidebar(&self) {}

    fn resolve_agent_pane_id(
        &self,
        _session: &str,
        _agent: &str,
        _thread_id: Option<&str>,
        _thread_name: Option<&str>,
    ) -> Option<String> {
        None
    }

    fn focus_pane(&self, _pane_id: &str) {}
    fn kill_pane(&self, _pane_id: &str) {}

    fn get_all_pane_counts(&self) -> HashMap<String, u32> {
        HashMap::new()
    }
}

#[derive(Default)]
pub struct MuxRegistry {
    providers: HashMap<String, Arc<dyn MuxProvider>>,
    order: Vec<String>,
}

impl MuxRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, provider: Arc<dyn MuxProvider>) {
        let name = provider.name().to_string();
        if !self.providers.contains_key(&name) {
            self.order.push(name.clone());
        }
        self.providers.insert(name, provider);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn MuxProvider>> {
        self.providers.get(name).cloned()
    }

    pub fn list(&self) -> Vec<String> {
        self.order.clone()
    }

    pub fn resolve(
        &self,
        preference: Option<&str>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Option<Arc<dyn MuxProvider>> {
        if let Some(preference) = preference {
            return self.get(preference);
        }

        if env("TMUX").is_some() {
            return self.get("tmux");
        }

        if env("ZELLIJ_SESSION_NAME").is_some() {
            return self.get("zellij");
        }

        None
    }
}
