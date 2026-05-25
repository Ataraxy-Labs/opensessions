use std::time::{Duration, Instant};

use crate::generated::protocol::{
    AgentEvent, AgentLiveness, AgentStatus, ClientCommand, LocalLink, ServerMessage, ServerState,
    SessionData, SessionFilterMode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelFocus {
    Sessions,
    Agents,
}

#[derive(Debug)]
pub struct App {
    pub sessions: Vec<SessionData>,
    pub focused_session: Option<String>,
    pub current_session: Option<String>,
    pub my_session: Option<String>,
    pub session_filter: SessionFilterMode,
    pub panel_focus: PanelFocus,
    pub focused_agent_idx: usize,
    pub quit_deadline: Option<Instant>,
    pub fixture_name: Option<&'static str>,
    commands: Vec<ClientCommand>,
}

impl App {
    pub fn from_state(state: ServerState) -> Self {
        Self {
            sessions: state.sessions,
            focused_session: state.focused_session,
            current_session: state.current_session,
            my_session: None,
            session_filter: state.session_filter.unwrap_or_default(),
            panel_focus: PanelFocus::Sessions,
            focused_agent_idx: 0,
            quit_deadline: None,
            fixture_name: None,
            commands: Vec::new(),
        }
    }

    pub fn apply_server_message(&mut self, message: ServerMessage) {
        match message {
            ServerMessage::State(state) => {
                self.sessions = state.sessions;
                self.focused_session = state.focused_session;
                self.current_session = state.current_session;
                self.session_filter = state.session_filter.unwrap_or_default();
            }
            ServerMessage::Focus(update) => {
                self.focused_session = update.focused_session;
                self.current_session = update.current_session;
            }
            ServerMessage::YourSession { name, .. } => {
                self.my_session = Some(name);
            }
            ServerMessage::Hello(_)
            | ServerMessage::Resize { .. }
            | ServerMessage::Quit
            | ServerMessage::ReIdentify => {}
        }
    }

    pub fn reference_fixture(name: &str) -> Self {
        let (focused_session, current_session) = match name {
            "pane-opensessions-self" => (Some("opensessions"), Some("opensessions")),
            "pane-multi-window" => (Some("plane-feat-background-exports"), Some("opensessions")),
            _ => (Some("plane-pdf-word-formatting"), Some("opensessions")),
        };

        let mut app = Self {
            sessions: reference_sessions(),
            focused_session: focused_session.map(str::to_string),
            current_session: current_session.map(str::to_string),
            my_session: current_session.map(str::to_string),
            session_filter: SessionFilterMode::All,
            panel_focus: PanelFocus::Sessions,
            focused_agent_idx: 0,
            quit_deadline: None,
            fixture_name: fixture_static_name(name),
            commands: Vec::new(),
        };

        if name == "pane-opensessions-self" {
            app.focused_agent_idx = 0;
        }

        app
    }

    pub fn resolve_synced_focus(
        next_focused_session: Option<&str>,
        next_current_session: Option<&str>,
        local_session_name: Option<&str>,
    ) -> Option<String> {
        if let (Some(local), Some(current)) = (local_session_name, next_current_session) {
            if current != local {
                return Some(local.to_string());
            }
        }

        next_focused_session
            .or(local_session_name)
            .map(str::to_string)
    }

    pub fn filtered_sessions(&self) -> impl Iterator<Item = &SessionData> {
        let mode = self.session_filter;
        self.sessions.iter().filter(move |session| {
            if session.name == "_os_stash" {
                return false;
            }

            match mode {
                SessionFilterMode::All => true,
                SessionFilterMode::Active => {
                    !session.agents.is_empty() || session.agent_state.is_some()
                }
                SessionFilterMode::Running => matches!(
                    session.agent_state.as_ref().map(|agent| agent.status),
                    Some(AgentStatus::Running | AgentStatus::ToolRunning | AgentStatus::Waiting),
                ),
            }
        })
    }

    pub fn handle_key_char(&mut self, key: char) {
        match key {
            '1'..='9' => self.commands.push(ClientCommand::SwitchIndex {
                index: key.to_digit(10).expect("digit key must parse"),
            }),
            'q' => {
                self.commands.push(ClientCommand::Quit);
                self.quit_deadline = Some(Instant::now() + Duration::from_millis(500));
            }
            'r' => self.commands.push(ClientCommand::Refresh),
            'n' | 'c' => self.commands.push(ClientCommand::NewSession),
            'u' => self.commands.push(ClientCommand::ShowAllSessions),
            'd' => {
                if self.panel_focus == PanelFocus::Agents {
                    self.dismiss_focused_agent();
                } else if let Some(name) = self.focused_session.clone() {
                    self.commands.push(ClientCommand::HideSession { name });
                }
            }
            'x' => {
                if self.panel_focus == PanelFocus::Agents {
                    self.kill_focused_agent_pane();
                } else if let Some(name) = self.focused_session.clone() {
                    self.commands.push(ClientCommand::KillSession { name });
                }
            }
            'f' => self.cycle_filter(),
            _ => {}
        }
    }

    pub fn handle_tab(&mut self, shift: bool) {
        let names: Vec<String> = self
            .filtered_sessions()
            .map(|session| session.name.clone())
            .collect();
        if names.is_empty() {
            return;
        }

        let current = self.current_session.as_deref();
        let current_idx = current
            .and_then(|name| names.iter().position(|candidate| candidate == name))
            .unwrap_or(0);
        let next_idx = if shift {
            (current_idx + names.len() - 1) % names.len()
        } else {
            (current_idx + 1) % names.len()
        };
        self.switch_to_session(names[next_idx].clone());
    }

    pub fn drain_commands(&mut self) -> Vec<ClientCommand> {
        self.commands.drain(..).collect()
    }

    pub fn move_focus(&mut self, delta: i8) {
        let Some((current_idx, len)) = self.focused_filtered_index_and_len() else {
            return;
        };
        let max_idx = len - 1;
        let next_idx = (current_idx as i16 + delta as i16).clamp(0, max_idx as i16) as usize;
        if next_idx == current_idx {
            return;
        }
        let Some(name) = self.filtered_session_name_at(next_idx) else {
            return;
        };
        self.focused_session = Some(name.clone());
        self.panel_focus = PanelFocus::Sessions;
        self.focused_agent_idx = 0;
        self.commands.push(ClientCommand::FocusSession { name });
    }

    pub fn focus_sessions_panel(&mut self) {
        self.panel_focus = PanelFocus::Sessions;
    }

    pub fn focus_agents_panel(&mut self) {
        let agent_count = self.focused_agents_len();
        if agent_count == 0 {
            return;
        }
        self.panel_focus = PanelFocus::Agents;
        self.focused_agent_idx = self.focused_agent_idx.min(agent_count - 1);
    }

    pub fn move_agent_focus(&mut self, delta: i8) {
        let agent_count = self.focused_agents_len();
        if agent_count == 0 {
            return;
        }
        let max_idx = agent_count - 1;
        self.focused_agent_idx =
            (self.focused_agent_idx as i16 + delta as i16).clamp(0, max_idx as i16) as usize;
    }

    pub fn activate_focused_item(&mut self) {
        if self.panel_focus == PanelFocus::Agents {
            self.activate_focused_agent();
        } else {
            self.activate_focused_session();
        }
    }

    pub fn activate_focused_session(&mut self) {
        if let Some(name) = self.focused_session.clone() {
            self.switch_to_session(name);
        }
    }

    pub fn activate_focused_agent(&mut self) {
        let Some((session, agent)) = self
            .focused_agent()
            .map(|(session, agent)| (session.name.clone(), agent.clone()))
        else {
            return;
        };
        self.current_session = Some(session.clone());
        self.commands.push(ClientCommand::SwitchSession {
            name: session.clone(),
            client_tty: None,
        });
        self.commands.push(ClientCommand::FocusAgentPane {
            session,
            agent: agent.agent,
            thread_id: agent.thread_id,
            thread_name: agent.thread_name,
        });
    }

    pub fn dismiss_focused_agent(&mut self) {
        let Some((session, agent, agent_count)) = self
            .focused_agent()
            .map(|(session, agent)| (session.name.clone(), agent.clone(), session.agents.len()))
        else {
            return;
        };
        self.commands.push(ClientCommand::DismissAgent {
            session,
            agent: agent.agent,
            thread_id: agent.thread_id,
        });
        if self.focused_agent_idx >= agent_count.saturating_sub(1) && agent_count > 1 {
            self.focused_agent_idx = agent_count - 2;
        }
        if agent_count <= 1 {
            self.panel_focus = PanelFocus::Sessions;
        }
    }

    pub fn kill_focused_agent_pane(&mut self) {
        let Some((session, agent)) = self
            .focused_agent()
            .map(|(session, agent)| (session.name.clone(), agent.clone()))
        else {
            return;
        };
        self.commands.push(ClientCommand::KillAgentPane {
            session,
            agent: agent.agent,
            thread_id: agent.thread_id,
            thread_name: agent.thread_name,
        });
    }

    pub fn reorder_focused_session(&mut self, delta: i8) {
        if let Some(name) = self.focused_session.clone() {
            self.commands
                .push(ClientCommand::ReorderSession { name, delta });
        }
    }

    fn switch_to_session(&mut self, name: String) {
        self.my_session = Some(name.clone());
        self.current_session = Some(name.clone());
        self.focused_session = Some(name.clone());
        self.panel_focus = PanelFocus::Sessions;
        self.focused_agent_idx = 0;
        self.commands.push(ClientCommand::SwitchSession {
            name,
            client_tty: None,
        });
    }

    fn cycle_filter(&mut self) {
        self.session_filter = match self.session_filter {
            SessionFilterMode::All => SessionFilterMode::Active,
            SessionFilterMode::Active => SessionFilterMode::Running,
            SessionFilterMode::Running => SessionFilterMode::All,
        };
        self.commands.push(ClientCommand::SetFilter {
            filter: self.session_filter,
        });
    }

    fn focused_filtered_index_and_len(&self) -> Option<(usize, usize)> {
        let focused = self.focused_session.as_deref();
        let mut focused_idx = None;
        let mut len = 0;
        for (idx, session) in self.filtered_sessions().enumerate() {
            if Some(session.name.as_str()) == focused {
                focused_idx = Some(idx);
            }
            len += 1;
        }
        (len > 0).then_some((focused_idx.unwrap_or(0), len))
    }

    fn filtered_session_name_at(&self, index: usize) -> Option<String> {
        self.filtered_sessions()
            .nth(index)
            .map(|session| session.name.clone())
    }

    fn focused_session_data(&self) -> Option<&SessionData> {
        let focused = self.focused_session.as_deref()?;
        self.sessions.iter().find(|session| session.name == focused)
    }

    fn focused_agents_len(&self) -> usize {
        self.focused_session_data()
            .map(|session| session.agents.len())
            .unwrap_or(0)
    }

    fn focused_agent(&self) -> Option<(&SessionData, &AgentEvent)> {
        let session = self.focused_session_data()?;
        let agent = session.agents.get(self.focused_agent_idx)?;
        Some((session, agent))
    }
}

fn fixture_static_name(name: &str) -> Option<&'static str> {
    match name {
        "pane-attached-session-list" => Some("pane-attached-session-list"),
        "pane-opensessions-self" => Some("pane-opensessions-self"),
        "pane-multi-window" => Some("pane-multi-window"),
        _ => None,
    }
}

fn reference_sessions() -> Vec<SessionData> {
    vec![
        session("_os_stash", "/tmp/_os_stash", "", None, Vec::new()),
        session(
            "plane-feat-edit-pages-from-pi",
            "/Users/palanikannanm/Documents/work/feat-edit-pages-from-pi",
            "feat/edit-pages-from-pi",
            None,
            Vec::new(),
        ),
        session(
            "plane-feat-background-exports",
            "/Users/palanikannanm/Documents/work/feat-background-exports",
            "feat-background-exports",
            None,
            Vec::new(),
        ),
        session(
            "learning",
            "/Users/palanikannanm/Documents/work/learning",
            "main",
            None,
            Vec::new(),
        ),
        session(
            "opensessions",
            "/Users/palanikannanm/Documents/work/opensessions",
            "devpulse",
            Some(agent(
                "amp",
                "opensessions",
                AgentStatus::ToolRunning,
                Some("Query tmux for open sessions"),
                None,
            )),
            vec![
                agent(
                    "amp",
                    "opensessions",
                    AgentStatus::ToolRunning,
                    Some("Query tmux for open sessions"),
                    None,
                ),
                agent("amp", "opensessions", AgentStatus::Idle, None, None),
            ],
        ),
        session(
            "plane-pdf-word-formatting",
            "/Users/palanikannanm/Documents/work/plane-ee-wt/pdf-word-formatting",
            "chore-relation-pqls",
            Some(agent(
                "amp",
                "plane-pdf-word-formatting",
                AgentStatus::Done,
                Some("Review GitHub PR for Plane"),
                Some(false),
            )),
            vec![
                agent(
                    "amp",
                    "plane-pdf-word-formatting",
                    AgentStatus::Done,
                    Some("Review GitHub PR for Plane"),
                    Some(false),
                ),
                agent(
                    "amp",
                    "plane-pdf-word-formatting",
                    AgentStatus::Idle,
                    None,
                    None,
                ),
            ],
        ),
        session(
            "dotfiles_public",
            "/Users/palanikannanm/Documents/work/dotfiles.public",
            "main",
            None,
            Vec::new(),
        ),
    ]
}

fn session(
    name: &str,
    dir: &str,
    branch: &str,
    agent_state: Option<AgentEvent>,
    agents: Vec<AgentEvent>,
) -> SessionData {
    SessionData {
        name: name.to_string(),
        created_at: 0,
        dir: dir.to_string(),
        branch: branch.to_string(),
        dirty: false,
        is_worktree: false,
        unseen: name == "plane-pdf-word-formatting",
        panes: 1,
        ports: Vec::new(),
        local_links: Vec::<LocalLink>::new(),
        windows: 1,
        uptime: String::new(),
        agent_state,
        agents,
        event_timestamps: Vec::new(),
        metadata: None,
    }
}

fn agent(
    agent_name: &str,
    session: &str,
    status: AgentStatus,
    thread_name: Option<&str>,
    unseen: Option<bool>,
) -> AgentEvent {
    AgentEvent {
        agent: agent_name.to_string(),
        session: session.to_string(),
        status,
        ts: 0,
        thread_id: None,
        thread_name: thread_name.map(str::to_string),
        unseen,
        pane_id: None,
        liveness: Some(AgentLiveness::Alive),
    }
}
