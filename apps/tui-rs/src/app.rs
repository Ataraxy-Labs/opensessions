use std::collections::HashSet;
use std::time::{Duration, Instant};

use opensessions_runtime::sidebar_width_sync::{MAX_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH};

use crate::generated::protocol::{
    AgentEvent, AgentLiveness, AgentPanelScope, ClientCommand, ClientSidebarFocus, ClientUiFocus,
    ClientUiPanel, ServerMessage, ServerQueryData, ServerQueryKey, ServerState, SessionAgentsData,
    SessionData, SessionFilterMode,
};
use crate::renderer::{AgentPaneTarget, HitTarget, agent_focus_target};
use crate::server_query::{QueryResult, ServerStateQuery};
pub use crate::session_display::DisplaySessionEntry;
use crate::session_display::{
    SessionProjectionOptions, display_sessions as projection_display_sessions, filtered_sessions,
    project_sessions, reordered_session_names as projection_reordered_session_names,
    reordered_worktree_group_names as projection_reordered_worktree_group_names,
    worktree_group_key, worktree_group_label, worktree_group_session_names,
};

pub const SESSION_CARD_HEIGHT: usize = 2;
const MIN_DETAIL_PANEL_HEIGHT: usize = 4;
const MAX_DETAIL_PANEL_HEIGHT: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelFocus {
    Sessions,
    Agents,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchTarget {
    /// Open lazydiffs in a tmux popup.
    LazydiffTmux { session_name: Option<String> },
    /// Open lazydiff in a new terminal window.
    LazydiffTerminal { session_name: Option<String> },
}

impl LaunchTarget {
    pub fn session_name(&self) -> Option<&str> {
        match self {
            Self::LazydiffTmux { session_name } | Self::LazydiffTerminal { session_name } => {
                session_name.as_deref()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarFocus {
    Session(String),
    WorktreeGroup(String),
}

impl SidebarFocus {
    pub fn session_name(&self) -> Option<&str> {
        match self {
            Self::Session(name) => Some(name),
            Self::WorktreeGroup(_) => None,
        }
    }

    pub fn worktree_group_key(&self) -> Option<&str> {
        match self {
            Self::Session(_) => None,
            Self::WorktreeGroup(key) => Some(key),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillTarget {
    Session(String),
    WorktreeGroup(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    None,
    ThemePicker {
        query: String,
        selected: usize,
        original_theme: Option<String>,
    },
    WidthSlider {
        draft_width: u16,
    },
    KillConfirm {
        target: KillTarget,
    },
}

#[derive(Debug)]
pub struct App {
    pub server_state_query: ServerStateQuery,
    pub sessions: Vec<SessionData>,
    pub sidebar_focus: Option<SidebarFocus>,
    pub current_session: Option<String>,
    pub my_session: Option<String>,
    pub initializing: bool,
    pub init_label: Option<String>,
    pub sidebar_width: u16,
    pub theme: Option<String>,
    pub ts: u64,
    /// Locally-driven spinner clock in ms. Advances on every render tick
    /// (see `main.rs` event loop) so spinners animate even when no server
    /// state arrives. Starts at 0 so deterministic snapshot tests are
    /// unaffected (`spinner()` falls back to `ts` when this is 0).
    pub spinner_now: u64,
    pub session_filter: SessionFilterMode,
    pub panel_focus: PanelFocus,
    pub agent_panel_scope: AgentPanelScope,
    pub focused_agent_idx: usize,
    pub quit_deadline: Option<Instant>,
    pub flash_target: Option<HitTarget>,
    pub flash_deadline: Option<Instant>,
    pub hover_target: Option<HitTarget>,
    pub modal: Modal,
    pub detail_panel_height: usize,
    pub session_scroll_offset: usize,
    session_scroll_follows_focus: bool,
    pub resize_drag_state: Option<(u16, usize)>,
    pub pending_switch_session: Option<String>,
    group_focus_surrogate_for: Option<String>,
    collapsed_worktree_groups: HashSet<String>,
    last_activated_session: Option<String>,
    terminal_width: Option<u16>,
    pane_identity: Option<PaneIdentity>,
    client_tty: Option<String>,
    pending_sidebar_width_intent: Option<u16>,
    pending_detail_panel_height_intent: Option<usize>,
    pending_agent_panel_scope_intent: Option<AgentPanelScope>,
    initial_secondary_queries_sent: bool,
    commands: Vec<ClientCommand>,
    pending_launches: Vec<LaunchTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneIdentity {
    pub pane_id: String,
    pub session_name: String,
    pub window_id: Option<String>,
}

impl App {
    pub fn connecting() -> Self {
        let mut app = Self {
            server_state_query: ServerStateQuery::connecting(),
            sessions: Vec::new(),
            sidebar_focus: None,
            current_session: None,
            my_session: None,
            initializing: true,
            init_label: Some("loading…".to_string()),
            sidebar_width: 26,
            theme: None,
            ts: 0,
            spinner_now: 0,
            session_filter: SessionFilterMode::All,
            panel_focus: PanelFocus::Sessions,
            agent_panel_scope: AgentPanelScope::Current,
            focused_agent_idx: 0,
            quit_deadline: None,
            flash_target: None,
            flash_deadline: None,
            hover_target: None,
            modal: Modal::None,
            detail_panel_height: MIN_DETAIL_PANEL_HEIGHT,
            session_scroll_offset: 0,
            session_scroll_follows_focus: true,
            resize_drag_state: None,
            pending_switch_session: None,
            group_focus_surrogate_for: None,
            collapsed_worktree_groups: HashSet::new(),
            last_activated_session: None,
            terminal_width: None,
            pane_identity: None,
            client_tty: None,
            pending_sidebar_width_intent: None,
            pending_detail_panel_height_intent: None,
            pending_agent_panel_scope_intent: None,
            initial_secondary_queries_sent: false,
            commands: Vec::new(),
            pending_launches: Vec::new(),
        };
        app.commands
            .extend(ServerStateQuery::initial_fetch_commands());
        app
    }

    pub fn from_state(state: ServerState) -> Self {
        let mut server_state_query = ServerStateQuery::connecting();
        for key in [
            ServerQueryKey::Sessions,
            ServerQueryKey::Agents,
            ServerQueryKey::Focus,
            ServerQueryKey::SidebarLayout,
            ServerQueryKey::Settings,
        ] {
            server_state_query.observe_query_success(key, state.ts);
        }
        let mut app = Self {
            server_state_query,
            sessions: state.sessions,
            sidebar_focus: None,
            current_session: None,
            my_session: None,
            initializing: state.initializing,
            init_label: state.init_label,
            sidebar_width: state.sidebar_width.min(u16::MAX as u32) as u16,
            theme: state.theme,
            ts: state.ts,
            spinner_now: 0,
            session_filter: state.session_filter.unwrap_or_default(),
            panel_focus: PanelFocus::Sessions,
            agent_panel_scope: state.agent_panel_scope,
            focused_agent_idx: 0,
            quit_deadline: None,
            flash_target: None,
            flash_deadline: None,
            hover_target: None,
            modal: Modal::None,
            detail_panel_height: state
                .detail_panel_height
                .max(MIN_DETAIL_PANEL_HEIGHT as u32) as usize,
            session_scroll_offset: 0,
            session_scroll_follows_focus: true,
            resize_drag_state: None,
            pending_switch_session: None,
            group_focus_surrogate_for: None,
            collapsed_worktree_groups: state.collapsed_worktree_groups.into_iter().collect(),
            last_activated_session: None,
            terminal_width: None,
            pane_identity: None,
            client_tty: None,
            pending_sidebar_width_intent: None,
            pending_detail_panel_height_intent: None,
            pending_agent_panel_scope_intent: None,
            initial_secondary_queries_sent: true,
            commands: Vec::new(),
            pending_launches: Vec::new(),
        };
        app.sidebar_focus = app.display_session_entries().first().map(entry_focus);
        app
    }

    pub fn set_terminal_width(&mut self, width: u16) {
        self.terminal_width = Some(width);
    }

    pub fn terminal_width(&self) -> Option<u16> {
        self.terminal_width
    }

    /// Record the running pane's identity and queue an `IdentifyPane` command.
    /// Calling again replaces the stored identity so subsequent `ReIdentify`
    /// requests use the freshest values.
    pub fn identify_pane(
        &mut self,
        pane_id: String,
        session_name: String,
        window_id: Option<String>,
    ) {
        let identity = PaneIdentity {
            pane_id,
            session_name,
            window_id,
        };
        self.confirm_local_session(identity.session_name.clone(), true);
        self.commands.push(ClientCommand::IdentifyPane {
            pane_id: identity.pane_id.clone(),
            session_name: identity.session_name.clone(),
            window_id: identity.window_id.clone(),
        });
        self.pane_identity = Some(identity);
    }

    /// Store the pane identity without queuing an `IdentifyPane` command.
    /// Used after main.rs has already sent the initial identify, so future
    /// `ReIdentify` requests can resend without doubling the first call.
    pub fn set_pane_identity(
        &mut self,
        pane_id: String,
        session_name: String,
        window_id: Option<String>,
    ) {
        self.pane_identity = Some(PaneIdentity {
            pane_id,
            session_name: session_name.clone(),
            window_id,
        });
        self.confirm_local_session(session_name, true);
    }

    fn confirm_local_session(&mut self, session_name: String, update_focus: bool) {
        self.my_session = Some(session_name.clone());
        self.current_session = Some(session_name.clone());
        if self.last_activated_session.is_none() {
            self.last_activated_session = Some(session_name.clone());
        }
        if self.pending_switch_session.as_deref() == Some(session_name.as_str()) {
            self.pending_switch_session = None;
        }
        if update_focus {
            let focus = self
                .visible_focus_for_session(&session_name)
                .unwrap_or_else(|| SidebarFocus::Session(session_name.clone()));
            self.set_focus_for_session_if_changed(&session_name, focus);
        }
    }

    pub fn pane_identity(&self) -> Option<&PaneIdentity> {
        self.pane_identity.as_ref()
    }

    pub fn server_state_result(&self) -> QueryResult<'_> {
        self.server_state_query.result()
    }

    pub fn apply_server_message(&mut self, message: ServerMessage) {
        match message {
            ServerMessage::Invalidate { keys, .. } => {
                let active = self.server_state_query.observe_invalidation(keys);
                self.commands
                    .extend(self.server_state_query.refetch_commands(active));
            }
            ServerMessage::QueryResult { key, data, ts } => {
                self.apply_query_result(key, data, ts);
            }
            ServerMessage::YourSession {
                name,
                client_tty,
                ui_focus,
            } => {
                self.client_tty = client_tty;
                self.confirm_local_session(name, true);
                if let Some(focus) = ui_focus {
                    self.apply_client_ui_focus(focus);
                }
            }
            ServerMessage::UiFocus { client_tty, focus } => {
                if self.client_tty.as_deref() == Some(client_tty.as_str()) {
                    self.apply_client_ui_focus(focus);
                }
            }
            ServerMessage::ActivateSession {
                name,
                source_pane_id,
            } => {
                let previous_activated = self.last_activated_session.replace(name.clone());
                let from_this_pane = self
                    .pane_identity
                    .as_ref()
                    .and_then(|identity| {
                        source_pane_id
                            .as_ref()
                            .map(|source| source == &identity.pane_id)
                    })
                    .unwrap_or(false);
                if !from_this_pane
                    && self.confirmed_local_session_name() == Some(name.as_str())
                    && (previous_activated.as_deref() != Some(name.as_str())
                        || self.activation_focus_needs_rehome(name.as_str()))
                {
                    self.confirm_local_session(name, true);
                }
            }
            ServerMessage::ReIdentify => {
                if let Some(identity) = self.pane_identity.clone() {
                    self.commands.push(ClientCommand::IdentifyPane {
                        pane_id: identity.pane_id,
                        session_name: identity.session_name,
                        window_id: identity.window_id,
                    });
                }
            }
            ServerMessage::Hello(_) | ServerMessage::Quit => {}
        }
    }

    fn apply_query_result(&mut self, key: ServerQueryKey, data: ServerQueryData, ts: u64) {
        match data {
            ServerQueryData::Sessions { sessions } => {
                self.server_state_query.observe_query_success(key, ts);
                self.ts = ts;
                self.apply_sessions_query(sessions);
                if !self.initial_secondary_queries_sent {
                    self.initial_secondary_queries_sent = true;
                    self.commands
                        .extend(ServerStateQuery::secondary_initial_fetch_commands());
                }
            }
            ServerQueryData::Agents { sessions } => {
                self.server_state_query.observe_query_success(key, ts);
                self.ts = ts;
                self.apply_agents_query(sessions);
            }
            ServerQueryData::Focus {
                current_session, ..
            } => {
                self.server_state_query.observe_query_success(key, ts);
                self.ts = ts;
                self.apply_focus_query(current_session);
            }
            ServerQueryData::SidebarLayout {
                sidebar_width,
                detail_panel_height,
                initializing,
                init_label,
                collapsed_worktree_groups,
            } => {
                self.server_state_query.observe_query_success(key, ts);
                self.ts = ts;
                self.apply_sidebar_layout_query(
                    sidebar_width,
                    detail_panel_height,
                    initializing,
                    init_label,
                    collapsed_worktree_groups,
                );
            }
            ServerQueryData::Settings {
                theme,
                session_filter,
                agent_panel_scope,
            } => {
                self.server_state_query.observe_query_success(key, ts);
                self.ts = ts;
                self.apply_settings_query(theme, session_filter, agent_panel_scope);
            }
        }
    }

    fn apply_sessions_query(&mut self, mut sessions: Vec<SessionData>) {
        let previous_focus = self.sidebar_focus.clone();
        for session in &mut sessions {
            if let Some(previous) = self
                .sessions
                .iter()
                .find(|previous| previous.name == session.name)
                && (!previous.agents.is_empty()
                    || previous.agent_state.is_some()
                    || !previous.event_timestamps.is_empty())
            {
                session.unseen = previous.unseen;
                session.agent_state = previous.agent_state.clone();
                session.agents = previous.agents.clone();
                session.event_timestamps = previous.event_timestamps.clone();
            }
        }
        self.sessions = sessions;
        self.clear_missing_pending_switch();
        self.rehome_expanded_group_surrogate();
        let focus_still_exists = previous_focus
            .as_ref()
            .is_some_and(|focus| self.focus_exists(focus));
        if !focus_still_exists {
            self.rehome_missing_focus();
        }
        self.clamp_session_scroll_offset(0);
    }

    fn apply_agents_query(&mut self, sessions: Vec<SessionAgentsData>) {
        for agents in sessions {
            if let Some(session) = self
                .sessions
                .iter_mut()
                .find(|session| session.name == agents.session)
            {
                session.unseen = agents.unseen;
                session.agent_state = agents.agent_state;
                session.agents = agents.agents;
                session.event_timestamps = agents.event_timestamps;
            }
        }
    }

    fn apply_focus_query(&mut self, current_session: Option<String>) {
        self.clear_background_pending_switch(current_session.as_deref());
    }

    fn apply_sidebar_layout_query(
        &mut self,
        sidebar_width: u32,
        detail_panel_height: u32,
        initializing: bool,
        init_label: Option<String>,
        collapsed_worktree_groups: Vec<String>,
    ) {
        self.initializing = initializing;
        self.init_label = init_label;
        self.apply_server_sidebar_width(sidebar_width.min(u16::MAX as u32) as u16);
        self.apply_server_detail_panel_height(detail_panel_height as usize);
        self.collapsed_worktree_groups = collapsed_worktree_groups.into_iter().collect();
        self.clamp_session_scroll_offset(0);
    }

    fn apply_settings_query(
        &mut self,
        theme: Option<String>,
        session_filter: Option<SessionFilterMode>,
        agent_panel_scope: AgentPanelScope,
    ) {
        self.theme = theme;
        self.session_filter = session_filter.unwrap_or_default();
        self.apply_server_agent_panel_scope(agent_panel_scope);
        self.clamp_session_scroll_offset(0);
    }

    pub fn filtered_sessions(&self) -> impl Iterator<Item = &SessionData> {
        filtered_sessions(&self.sessions, self.session_filter)
    }

    pub fn display_session_entries(&self) -> Vec<DisplaySessionEntry<'_>> {
        project_sessions(&self.sessions, self.session_projection_options())
    }

    pub fn display_sessions(&self) -> Vec<&SessionData> {
        projection_display_sessions(&self.sessions, self.session_projection_options())
    }

    pub fn reordered_session_names(&self, name: &str, delta: i8) -> Option<Vec<String>> {
        projection_reordered_session_names(
            &self.sessions,
            self.session_projection_options(),
            name,
            delta,
        )
    }

    pub fn reordered_worktree_group_names(&self, key: &str, delta: i8) -> Option<Vec<String>> {
        projection_reordered_worktree_group_names(
            &self.sessions,
            self.session_projection_options(),
            key,
            delta,
        )
    }

    fn session_projection_options(&self) -> SessionProjectionOptions<'_> {
        SessionProjectionOptions {
            filter: self.session_filter,
            collapsed_groups: &self.collapsed_worktree_groups,
        }
    }

    pub fn focused_session_name(&self) -> Option<&str> {
        self.sidebar_focus.as_ref()?.session_name()
    }

    fn activation_focus_needs_rehome(&self, session_name: &str) -> bool {
        match self.sidebar_focus.as_ref() {
            Some(SidebarFocus::Session(focused)) => focused != session_name,
            Some(SidebarFocus::WorktreeGroup(_)) => matches!(
                self.visible_focus_for_session(session_name),
                Some(SidebarFocus::Session(focused)) if focused == session_name
            ),
            None => true,
        }
    }

    pub fn focused_group_key(&self) -> Option<&str> {
        self.sidebar_focus.as_ref()?.worktree_group_key()
    }

    pub fn set_sidebar_focus(&mut self, focus: SidebarFocus) {
        self.group_focus_surrogate_for = None;
        self.sidebar_focus = Some(focus);
        self.panel_focus = PanelFocus::Sessions;
        self.focused_agent_idx = 0;
        self.session_scroll_follows_focus = true;
    }

    fn client_ui_focus(&self) -> ClientUiFocus {
        ClientUiFocus {
            panel: match self.panel_focus {
                PanelFocus::Sessions => ClientUiPanel::Sessions,
                PanelFocus::Agents => ClientUiPanel::Agents,
            },
            sidebar: self.sidebar_focus.as_ref().map(|focus| match focus {
                SidebarFocus::Session(name) => ClientSidebarFocus::Session { name: name.clone() },
                SidebarFocus::WorktreeGroup(key) => {
                    ClientSidebarFocus::WorktreeGroup { key: key.clone() }
                }
            }),
            agent_index: self.focused_agent_idx,
        }
    }

    fn publish_client_ui_focus(&mut self) {
        if self.client_tty.is_none() {
            return;
        }
        self.commands.push(ClientCommand::SetUiFocus {
            focus: self.client_ui_focus(),
        });
    }

    fn apply_client_ui_focus(&mut self, focus: ClientUiFocus) {
        if let Some(sidebar) = focus.sidebar {
            let sidebar_focus = match sidebar {
                ClientSidebarFocus::Session { name } => SidebarFocus::Session(name),
                ClientSidebarFocus::WorktreeGroup { key } => SidebarFocus::WorktreeGroup(key),
            };
            if self.focus_exists(&sidebar_focus) {
                self.group_focus_surrogate_for = None;
                self.sidebar_focus = Some(sidebar_focus);
                self.session_scroll_follows_focus = true;
            }
        }
        self.focused_agent_idx = focus
            .agent_index
            .min(self.focused_agents_len().saturating_sub(1));
        self.panel_focus = match focus.panel {
            ClientUiPanel::Sessions => PanelFocus::Sessions,
            ClientUiPanel::Agents if self.focused_agents_len() > 0 => PanelFocus::Agents,
            ClientUiPanel::Agents => PanelFocus::Sessions,
        };
    }

    fn set_sidebar_focus_for_session(&mut self, session_name: &str, focus: SidebarFocus) {
        self.group_focus_surrogate_for =
            matches!(focus, SidebarFocus::WorktreeGroup(_)).then(|| session_name.to_string());
        self.sidebar_focus = Some(focus);
        self.panel_focus = PanelFocus::Sessions;
        self.focused_agent_idx = 0;
        self.session_scroll_follows_focus = true;
    }

    fn set_focus_for_session_if_changed(&mut self, session_name: &str, focus: SidebarFocus) {
        if self.sidebar_focus.as_ref() != Some(&focus)
            || self.group_focus_surrogate_for.as_deref() != Some(session_name)
        {
            self.set_sidebar_focus_for_session(session_name, focus);
        }
    }

    pub fn set_focused_session(&mut self, name: impl Into<String>) {
        self.set_sidebar_focus(SidebarFocus::Session(name.into()));
    }

    fn focus_exists(&self, focus: &SidebarFocus) -> bool {
        self.display_session_entries()
            .iter()
            .any(|entry| &entry_focus(entry) == focus)
    }

    fn visible_focus_for_session(&self, name: &str) -> Option<SidebarFocus> {
        let session_focus = SidebarFocus::Session(name.to_string());
        if self.focus_exists(&session_focus) {
            return Some(session_focus);
        }
        let session = self.sessions.iter().find(|session| session.name == name)?;
        let group_focus = SidebarFocus::WorktreeGroup(worktree_group_key(session)?);
        self.focus_exists(&group_focus).then_some(group_focus)
    }

    fn local_session_focus(&self) -> Option<SidebarFocus> {
        let name = self
            .my_session
            .as_deref()
            .or(self.current_session.as_deref())?;
        self.visible_focus_for_session(name)
            .or_else(|| Some(SidebarFocus::Session(name.to_string())))
    }

    pub fn handle_key_char(&mut self, key: char) {
        match key {
            '1'..='9' => {
                let index = key.to_digit(10).expect("digit key must parse") as usize;
                if let Some(session) = self.display_sessions().get(index.saturating_sub(1)) {
                    self.switch_to_session(session.name.clone());
                }
            }
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
                } else if let Some(name) = self.focused_session_name().map(str::to_string) {
                    self.commands.push(ClientCommand::HideSession { name });
                }
            }
            'x' => {
                if self.panel_focus == PanelFocus::Agents {
                    if !self.kill_focused_agent_pane() {
                        self.open_kill_confirm_for_focus();
                    }
                } else {
                    self.open_kill_confirm_for_focus();
                }
            }
            'l' => self.pending_launches.push(LaunchTarget::LazydiffTmux {
                session_name: self.local_session_name().map(str::to_string),
            }),
            'L' => self.pending_launches.push(LaunchTarget::LazydiffTerminal {
                session_name: self.local_session_name().map(str::to_string),
            }),
            't' => self.open_theme_picker(),
            'w' => self.open_width_slider(),
            'f' => self.cycle_filter(),
            'a' => self.toggle_agent_panel_scope(),
            _ => {}
        }
    }

    pub fn handle_tab(&mut self, shift: bool) {
        self.switch_to_relative_session(if shift { -1 } else { 1 });
    }

    pub fn drain_commands(&mut self) -> Vec<ClientCommand> {
        self.commands.drain(..).collect()
    }

    pub fn drain_launches(&mut self) -> Vec<LaunchTarget> {
        self.pending_launches.drain(..).collect()
    }

    pub fn commands_push(&mut self, command: ClientCommand) {
        self.commands.push(command);
    }

    pub fn move_focus(&mut self, delta: i8) {
        let targets = self.session_focus_targets();
        if targets.is_empty() {
            return;
        }
        let current_idx = self
            .sidebar_focus
            .as_ref()
            .and_then(|focus| targets.iter().position(|target| target == focus))
            .or_else(|| {
                self.local_session_focus()
                    .and_then(|focus| targets.iter().position(|target| target == &focus))
            })
            .unwrap_or(0);
        let max_idx = targets.len() - 1;
        let next_idx = (current_idx as i16 + delta as i16).clamp(0, max_idx as i16) as usize;
        if next_idx != current_idx {
            self.set_sidebar_focus(targets[next_idx].clone());
            self.publish_client_ui_focus();
        }
    }

    pub fn session_scroll_offset(&self) -> usize {
        self.session_scroll_offset
    }

    pub fn session_scroll_follows_focus(&self) -> bool {
        self.session_scroll_follows_focus
    }

    pub fn scroll_sessions(&mut self, delta: i8, viewport_rows: usize) {
        let entries = self.display_session_entries();
        let len = entries.len();
        if len == 0 || viewport_rows == 0 {
            self.session_scroll_offset = 0;
            return;
        }

        let max_offset = max_scroll_offset(&entries, viewport_rows);
        let next_offset = if delta < 0 {
            self.session_scroll_offset
                .saturating_sub(delta.unsigned_abs() as usize)
        } else {
            self.session_scroll_offset.saturating_add(delta as usize)
        }
        .min(max_offset);

        self.session_scroll_offset = next_offset;
        self.session_scroll_follows_focus = false;
        self.panel_focus = PanelFocus::Sessions;
    }

    pub fn ensure_focused_session_visible(&mut self, viewport_rows: usize) {
        let Some((focused_idx, len)) = self.focused_filtered_index_and_len() else {
            self.session_scroll_offset = 0;
            return;
        };
        if viewport_rows == 0 {
            return;
        }

        let visible_cards = visible_session_cards(viewport_rows);
        let max_offset = len.saturating_sub(visible_cards);
        if focused_idx < self.session_scroll_offset {
            self.session_scroll_offset = focused_idx;
        } else if focused_idx >= self.session_scroll_offset.saturating_add(visible_cards) {
            self.session_scroll_offset =
                focused_idx.saturating_add(1).saturating_sub(visible_cards);
        }
        self.session_scroll_offset = self.session_scroll_offset.min(max_offset);
    }

    pub fn focus_sessions_panel(&mut self) {
        if self.panel_focus != PanelFocus::Sessions {
            self.panel_focus = PanelFocus::Sessions;
            self.publish_client_ui_focus();
        }
    }

    pub fn focus_agents_panel(&mut self) {
        let agent_count = self.focused_agents_len();
        if agent_count == 0 {
            return;
        }
        let next_idx = self.focused_agent_idx.min(agent_count - 1);
        if self.panel_focus != PanelFocus::Agents || self.focused_agent_idx != next_idx {
            self.panel_focus = PanelFocus::Agents;
            self.focused_agent_idx = next_idx;
            self.publish_client_ui_focus();
        }
    }

    pub fn toggle_agent_panel_scope(&mut self) {
        self.agent_panel_scope = match self.agent_panel_scope {
            AgentPanelScope::Current => AgentPanelScope::All,
            AgentPanelScope::All => AgentPanelScope::Current,
        };
        self.pending_agent_panel_scope_intent = Some(self.agent_panel_scope);
        self.commands.push(ClientCommand::SetAgentPanelScope {
            scope: self.agent_panel_scope,
        });
        self.focused_agent_idx = self
            .focused_agent_idx
            .min(self.focused_agents_len().saturating_sub(1));
        if self.focused_agents_len() == 0 {
            self.panel_focus = PanelFocus::Sessions;
        }
    }

    pub fn move_agent_focus(&mut self, delta: i8) {
        let agent_count = self.focused_agents_len();
        if agent_count == 0 {
            return;
        }
        let max_idx = agent_count - 1;
        let next_idx =
            (self.focused_agent_idx as i16 + delta as i16).clamp(0, max_idx as i16) as usize;
        if self.focused_agent_idx != next_idx {
            self.focused_agent_idx = next_idx;
            self.publish_client_ui_focus();
        }
    }

    pub fn activate_focused_item(&mut self) {
        if self.panel_focus == PanelFocus::Agents {
            self.activate_focused_agent();
            return;
        }
        match self.sidebar_focus.clone() {
            Some(SidebarFocus::Session(_)) => self.activate_focused_session(),
            Some(SidebarFocus::WorktreeGroup(key)) => {
                self.activate_hit_target(HitTarget::Group(key))
            }
            None => {}
        }
    }

    pub fn activate_focused_session(&mut self) {
        if let Some(name) = self.focused_session_name().map(str::to_string) {
            self.request_session_switch(name, true);
        }
    }

    /// Click on a session row in the list.
    pub fn click_session(&mut self, name: String) {
        self.activate_hit_target(HitTarget::Session(name));
    }

    pub fn click_group(&mut self, key: String) {
        self.activate_hit_target(HitTarget::Group(key));
    }

    pub fn click_diff_count(&mut self, name: String) {
        self.activate_hit_target(HitTarget::DiffCount(name));
    }

    /// Click on an agent row in the detail panel. Mirrors the TS
    /// `onFocusPane`/`onFocusAgentPane` flow that switches to the agent's
    /// session and sends `focus-agent-pane`.
    pub fn click_agent(&mut self, idx: usize) {
        self.activate_hit_target(HitTarget::Agent(idx));
    }

    pub fn activate_hit_target(&mut self, target: HitTarget) {
        self.trigger_flash(target.clone());
        match target {
            HitTarget::Session(name) => self.switch_to_session(name),
            HitTarget::Group(key) => {
                self.set_sidebar_focus(SidebarFocus::WorktreeGroup(key.clone()));
                self.publish_client_ui_focus();
                self.toggle_worktree_group(&key);
            }
            HitTarget::DiffCount(name) => {
                self.pending_launches.push(LaunchTarget::LazydiffTmux {
                    session_name: Some(name),
                });
            }
            HitTarget::Agent(idx) => self.activate_agent_target(idx),
            HitTarget::AgentPane(target) => self.activate_agent_pane_target(target),
            HitTarget::AgentScopeToggle => self.toggle_agent_panel_scope(),
        }
    }

    fn activate_agent_target(&mut self, idx: usize) {
        let agent_count = self.focused_agents_len();
        if idx >= agent_count {
            return;
        }
        self.panel_focus = PanelFocus::Agents;
        self.focused_agent_idx = idx;
        self.publish_client_ui_focus();
        self.activate_focused_agent();
    }

    fn activate_agent_pane_target(&mut self, target: AgentPaneTarget) {
        self.queue_focus_agent_pane(target);
    }

    /// Queue a `SetTheme` command for the server. The server replies with a
    /// fresh `State` broadcast carrying the new theme name, which
    /// `apply_server_message` stores on `self.theme`.
    pub fn set_theme_request(&mut self, theme: String) {
        self.commands.push(ClientCommand::SetTheme { theme });
    }

    /// Arm a 150ms click-flash highlight on the given target. Mirrors the TS
    /// `triggerFlash()` helper which sets `flashUntil = Date.now() + 150`.
    pub fn trigger_flash(&mut self, target: HitTarget) {
        self.flash_target = Some(target);
        self.flash_deadline = Some(Instant::now() + Duration::from_millis(150));
    }

    /// Returns the currently active flash target, or `None` if the flash has
    /// expired or was never armed.
    pub fn active_flash_target(&self) -> Option<&HitTarget> {
        let deadline = self.flash_deadline?;
        if Instant::now() >= deadline {
            return None;
        }
        self.flash_target.as_ref()
    }

    pub fn set_hover_target(&mut self, target: Option<HitTarget>) {
        self.hover_target = target;
    }

    pub fn is_modal_open(&self) -> bool {
        !matches!(self.modal, Modal::None)
    }

    pub fn open_theme_picker(&mut self) {
        self.modal = Modal::ThemePicker {
            query: String::new(),
            selected: 0,
            original_theme: self.theme.clone(),
        };
    }

    pub fn close_theme_picker(&mut self) {
        if let Modal::ThemePicker { original_theme, .. } = &self.modal {
            self.theme = original_theme.clone();
        }
        self.modal = Modal::None;
    }

    pub fn confirm_theme_picker(&mut self) {
        if let Some(name) = self.theme.clone() {
            self.commands.push(ClientCommand::SetTheme { theme: name });
        }
        self.modal = Modal::None;
    }

    pub fn open_width_slider(&mut self) {
        let draft_width = self.sidebar_width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
        self.modal = Modal::WidthSlider { draft_width };
    }

    pub fn open_kill_confirm_for_focus(&mut self) {
        let Some(target) = self.focused_kill_target() else {
            return;
        };
        self.modal = Modal::KillConfirm { target };
    }

    pub fn confirm_kill_target(&mut self) {
        let Modal::KillConfirm { target } = self.modal.clone() else {
            return;
        };
        self.modal = Modal::None;
        for name in self.kill_target_session_names(&target) {
            self.commands.push(ClientCommand::KillSession { name });
        }
    }

    pub fn kill_confirm_copy(&self, target: &KillTarget) -> (String, String) {
        match target {
            KillTarget::Session(name) => ("Kill session?".to_string(), name.clone()),
            KillTarget::WorktreeGroup(key) => {
                let count = self.kill_target_session_names(target).len();
                let label = worktree_group_display_label(key);
                (
                    "Kill worktree?".to_string(),
                    format!("{count} sessions in {label}"),
                )
            }
        }
    }

    pub fn adjust_width_slider(&mut self, delta: i16) {
        if let Modal::WidthSlider { draft_width, .. } = &mut self.modal {
            let next = (*draft_width as i16 + delta)
                .clamp(MIN_SIDEBAR_WIDTH as i16, MAX_SIDEBAR_WIDTH as i16)
                as u16;
            if next == *draft_width {
                return;
            }
            *draft_width = next;
            self.sidebar_width = next;
            self.pending_sidebar_width_intent = Some(next);
            self.commands.push(ClientCommand::SetSidebarWidth {
                width: u32::from(next),
            });
        }
    }

    pub fn close_width_slider(&mut self) {
        if let Modal::WidthSlider { draft_width } = self.modal {
            self.pending_sidebar_width_intent = Some(draft_width);
            self.commands.push(ClientCommand::SetSidebarWidth {
                width: u32::from(draft_width),
            });
        }
        self.modal = Modal::None;
    }

    pub fn confirm_width_slider(&mut self) {
        if let Modal::WidthSlider { draft_width } = self.modal {
            self.pending_sidebar_width_intent = Some(draft_width);
            self.commands.push(ClientCommand::SetSidebarWidth {
                width: u32::from(draft_width),
            });
        }
        self.modal = Modal::None;
    }

    pub fn resize_detail_panel(&mut self, delta: i8) {
        let new_height = (self.detail_panel_height as i16 + delta as i16).clamp(
            MIN_DETAIL_PANEL_HEIGHT as i16,
            MAX_DETAIL_PANEL_HEIGHT as i16,
        ) as usize;
        self.set_detail_panel_height(new_height);
    }

    pub fn set_detail_panel_height(&mut self, height: usize) {
        let height = height.clamp(MIN_DETAIL_PANEL_HEIGHT, MAX_DETAIL_PANEL_HEIGHT);
        if height == self.detail_panel_height {
            return;
        }
        self.detail_panel_height = height;
        self.pending_detail_panel_height_intent = Some(height);
        self.commands.push(ClientCommand::SetDetailPanelHeight {
            height: height.min(u32::MAX as usize) as u32,
        });
    }

    fn apply_server_sidebar_width(&mut self, server_width: u16) {
        if let Some(intent) = self.pending_sidebar_width_intent {
            if server_width == intent {
                self.pending_sidebar_width_intent = None;
                self.sidebar_width = server_width;
                if let Modal::WidthSlider { draft_width } = &mut self.modal {
                    *draft_width = server_width;
                }
            }
            return;
        }

        self.sidebar_width = server_width;
        if let Modal::WidthSlider { draft_width } = &mut self.modal {
            *draft_width = server_width;
        }
    }

    fn apply_server_detail_panel_height(&mut self, server_height: usize) {
        let server_height = server_height.clamp(MIN_DETAIL_PANEL_HEIGHT, MAX_DETAIL_PANEL_HEIGHT);
        if let Some(intent) = self.pending_detail_panel_height_intent {
            if server_height == intent {
                self.pending_detail_panel_height_intent = None;
                self.detail_panel_height = server_height;
            }
            return;
        }

        self.detail_panel_height = server_height;
    }

    fn apply_server_agent_panel_scope(&mut self, server_scope: AgentPanelScope) {
        if let Some(intent) = self.pending_agent_panel_scope_intent {
            if server_scope == intent {
                self.pending_agent_panel_scope_intent = None;
                self.agent_panel_scope = server_scope;
            }
            return;
        }

        self.agent_panel_scope = server_scope;
        self.focused_agent_idx = self
            .focused_agent_idx
            .min(self.focused_agents_len().saturating_sub(1));
        if self.focused_agents_len() == 0 {
            self.panel_focus = PanelFocus::Sessions;
        }
    }

    pub fn activate_focused_agent(&mut self) {
        let Some(target) = self
            .focused_agent()
            .map(|(session, agent)| agent_focus_target(session, agent))
        else {
            return;
        };
        self.queue_focus_agent_pane(target);
    }

    fn queue_focus_agent_pane(&mut self, target: AgentPaneTarget) {
        self.commands.push(ClientCommand::SwitchSession {
            name: target.session.clone(),
            client_tty: None,
        });
        self.commands.push(ClientCommand::FocusAgentPane {
            session: target.session,
            agent: target.agent,
            thread_id: target.thread_id,
            thread_name: target.thread_name,
            pane_id: target.pane_id,
        });
    }

    pub fn dismiss_focused_agent(&mut self) {
        let agent_count = self.focused_agents_len();
        let Some((session, agent)) = self
            .focused_agent()
            .map(|(session, agent)| (session.name.clone(), agent.clone()))
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

    pub fn kill_focused_agent_pane(&mut self) -> bool {
        let Some((session, agent)) = self
            .focused_agent()
            .map(|(session, agent)| (session.name.clone(), agent.clone()))
        else {
            return false;
        };
        if agent.pane_id.is_none() && agent.liveness != Some(AgentLiveness::Alive) {
            return false;
        }
        self.commands.push(ClientCommand::KillAgentPane {
            session,
            agent: agent.agent,
            thread_id: agent.thread_id,
            thread_name: agent.thread_name,
            pane_id: agent.pane_id,
        });
        true
    }

    pub fn reorder_focused_session(&mut self, delta: i8) {
        match self.sidebar_focus.clone() {
            Some(SidebarFocus::Session(name)) => {
                self.commands
                    .push(ClientCommand::ReorderSession { name, delta });
            }
            Some(SidebarFocus::WorktreeGroup(key)) => {
                self.commands
                    .push(ClientCommand::ReorderWorktreeGroup { key, delta });
            }
            None => {}
        }
    }

    fn switch_to_session(&mut self, name: String) {
        self.request_session_switch(name, false);
    }

    fn request_session_switch(&mut self, name: String, preserve_focus: bool) {
        self.pending_switch_session = Some(name.clone());
        if preserve_focus {
            if let Some(focus) = self.visible_focus_for_session(&name) {
                self.set_focus_for_session_if_changed(&name, focus);
            }
        } else if let Some(focus) = self.visible_focus_for_session(&name) {
            self.set_focus_for_session_if_changed(&name, focus);
        } else {
            self.rehome_focus_to_local_session();
        }
        self.publish_client_ui_focus();
        self.commands.push(ClientCommand::SwitchSession {
            name,
            client_tty: None,
        });
    }

    fn rehome_focus_to_local_session(&mut self) {
        if let Some(name) = self.confirmed_local_session_name().map(str::to_string)
            && let Some(focus) = self
                .visible_focus_for_session(&name)
                .or_else(|| Some(SidebarFocus::Session(name.clone())))
        {
            self.set_focus_for_session_if_changed(&name, focus);
        } else if self
            .sidebar_focus
            .as_ref()
            .is_none_or(|focus| !self.focus_exists(focus))
        {
            self.sidebar_focus = self.display_session_entries().first().map(entry_focus);
            self.session_scroll_follows_focus = true;
        }
    }

    fn rehome_missing_focus(&mut self) {
        if let Some(pending) = self.pending_switch_session.clone()
            && let Some(focus) = self.visible_focus_for_session(&pending)
        {
            self.set_focus_for_session_if_changed(&pending, focus);
            return;
        }
        self.rehome_focus_to_local_session();
    }

    fn rehome_expanded_group_surrogate(&mut self) {
        let Some(session_name) = self.group_focus_surrogate_for.clone() else {
            return;
        };
        let Some(SidebarFocus::Session(_)) = self.visible_focus_for_session(&session_name) else {
            return;
        };
        self.set_sidebar_focus_for_session(
            &session_name,
            SidebarFocus::Session(session_name.clone()),
        );
    }

    fn clear_background_pending_switch(&mut self, broadcast_session: Option<&str>) {
        let Some(broadcast_session) = broadcast_session else {
            return;
        };
        if self.pending_switch_session.as_deref() != Some(broadcast_session) {
            return;
        }
        if self.confirmed_local_session_name() == Some(broadcast_session) {
            return;
        }
        self.pending_switch_session = None;
        self.rehome_focus_to_local_session();
    }

    fn clear_missing_pending_switch(&mut self) {
        let Some(pending) = self.pending_switch_session.as_deref() else {
            return;
        };
        if self.sessions.iter().any(|session| session.name == pending) {
            return;
        }
        self.pending_switch_session = None;
    }

    fn confirmed_local_session_name(&self) -> Option<&str> {
        self.my_session
            .as_deref()
            .or(self.current_session.as_deref())
    }

    fn local_session_name(&self) -> Option<&str> {
        self.confirmed_local_session_name()
            .or_else(|| self.focused_session_name())
    }

    fn switch_to_relative_session(&mut self, delta: i8) {
        let names: Vec<String> = self
            .display_sessions()
            .into_iter()
            .map(|session| session.name.clone())
            .collect();
        if names.is_empty() {
            return;
        }

        let anchor = self.local_session_name();
        let current_idx = anchor
            .and_then(|name| names.iter().position(|candidate| candidate == name))
            .unwrap_or(0);
        let max_idx = names.len() - 1;
        let next_idx = (current_idx as i16 + delta as i16).clamp(0, max_idx as i16) as usize;
        if next_idx == current_idx {
            self.rehome_focus_to_local_session();
            return;
        }
        self.request_session_switch(names[next_idx].clone(), false);
    }

    fn session_focus_targets(&self) -> Vec<SidebarFocus> {
        self.display_session_entries()
            .into_iter()
            .map(|entry| entry_focus(&entry))
            .collect()
    }

    fn cycle_filter(&mut self) {
        self.session_filter = match self.session_filter {
            SessionFilterMode::All => SessionFilterMode::Active,
            SessionFilterMode::Active => SessionFilterMode::Running,
            SessionFilterMode::Running => SessionFilterMode::All,
        };
        self.clamp_session_scroll_offset(0);
        self.commands.push(ClientCommand::SetFilter {
            filter: self.session_filter,
        });
    }

    fn clamp_session_scroll_offset(&mut self, viewport_rows: usize) {
        let entries = self.display_session_entries();
        self.session_scroll_offset = self
            .session_scroll_offset
            .min(max_scroll_offset(&entries, viewport_rows));
    }

    pub fn is_group_collapsed(&self, key: &str) -> bool {
        self.collapsed_worktree_groups.contains(key)
    }

    fn toggle_worktree_group(&mut self, key: &str) {
        if !self.collapsed_worktree_groups.insert(key.to_string()) {
            self.collapsed_worktree_groups.remove(key);
        }
        self.commands.push(ClientCommand::ToggleWorktreeGroup {
            key: key.to_string(),
        });
    }

    fn focused_filtered_index_and_len(&self) -> Option<(usize, usize)> {
        let focused = self.sidebar_focus.as_ref();
        let mut focused_idx = None;
        let mut len = 0;
        for (idx, entry) in self.display_session_entries().into_iter().enumerate() {
            if Some(&entry_focus(&entry)) == focused {
                focused_idx = Some(idx);
            }
            len += 1;
        }
        (len > 0).then_some((focused_idx.unwrap_or(0), len))
    }

    fn focused_session_data(&self) -> Option<&SessionData> {
        let focused = self.focused_session_name()?;
        self.sessions.iter().find(|session| session.name == focused)
    }

    fn focused_kill_target(&self) -> Option<KillTarget> {
        match self.sidebar_focus.as_ref()? {
            SidebarFocus::Session(name) => Some(KillTarget::Session(name.clone())),
            SidebarFocus::WorktreeGroup(key) => {
                (!self.worktree_group_session_names(key).is_empty())
                    .then(|| KillTarget::WorktreeGroup(key.clone()))
            }
        }
    }

    fn kill_target_session_names(&self, target: &KillTarget) -> Vec<String> {
        match target {
            KillTarget::Session(name) => vec![name.clone()],
            KillTarget::WorktreeGroup(key) => self.worktree_group_session_names(key),
        }
    }

    fn worktree_group_session_names(&self, key: &str) -> Vec<String> {
        worktree_group_session_names(&self.sessions, key)
    }

    fn focused_agents_len(&self) -> usize {
        match self.agent_panel_scope {
            AgentPanelScope::Current => self
                .focused_session_data()
                .map(|session| session.agents.len())
                .unwrap_or(0),
            AgentPanelScope::All => self
                .sessions
                .iter()
                .map(|session| session.agents.len())
                .sum(),
        }
    }

    fn focused_agent(&self) -> Option<(&SessionData, &AgentEvent)> {
        match self.agent_panel_scope {
            AgentPanelScope::Current => {
                let session = self.focused_session_data()?;
                let agent = session.agents.get(self.focused_agent_idx)?;
                Some((session, agent))
            }
            AgentPanelScope::All => self
                .sessions
                .iter()
                .flat_map(|session| session.agents.iter().map(move |agent| (session, agent)))
                .nth(self.focused_agent_idx),
        }
    }
}

fn visible_session_cards(viewport_rows: usize) -> usize {
    viewport_rows.div_ceil(SESSION_CARD_HEIGHT).max(1)
}

fn max_scroll_offset(entries: &[DisplaySessionEntry<'_>], viewport_rows: usize) -> usize {
    if entries.is_empty() || viewport_rows == 0 {
        return 0;
    }
    let total_rows = entries
        .iter()
        .map(DisplaySessionEntry::row_height)
        .sum::<usize>();
    if total_rows <= viewport_rows {
        return 0;
    }
    let mut offset = 0;
    let mut remaining_rows = total_rows;
    while offset < entries.len() && remaining_rows > viewport_rows {
        remaining_rows = remaining_rows.saturating_sub(entries[offset].row_height());
        offset += 1;
    }
    offset.min(entries.len().saturating_sub(1))
}

fn entry_focus(entry: &DisplaySessionEntry<'_>) -> SidebarFocus {
    match entry {
        DisplaySessionEntry::Session { session, .. } => SidebarFocus::Session(session.name.clone()),
        DisplaySessionEntry::Group { key, .. } => SidebarFocus::WorktreeGroup(key.clone()),
    }
}

fn worktree_group_display_label(key: &str) -> String {
    worktree_group_label(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated::protocol::AgentStatus;

    fn empty_state(detail_panel_height: u32) -> ServerState {
        ServerState {
            sessions: Vec::new(),
            focused_session: None,
            current_session: None,
            theme: None,
            session_filter: None,
            agent_panel_scope: AgentPanelScope::Current,
            sidebar_width: 40,
            detail_panel_height,
            initializing: false,
            init_label: None,
            collapsed_worktree_groups: Vec::new(),
            ts: 0,
        }
    }

    fn session(name: &str, dir: &str, is_worktree: bool) -> SessionData {
        SessionData {
            name: name.to_string(),
            created_at: 0,
            dir: dir.to_string(),
            branch: String::new(),
            dirty: false,
            changed_files: 0,
            insertions: 0,
            deletions: 0,
            is_worktree,
            unseen: false,
            panes: 1,
            ports: Vec::new(),
            local_links: Vec::new(),
            windows: 1,
            uptime: String::new(),
            agent_state: None,
            agents: Vec::new(),
            event_timestamps: Vec::new(),
            metadata: None,
        }
    }

    fn agent_without_pane(agent: &str, status: AgentStatus) -> AgentEvent {
        AgentEvent {
            agent: agent.to_string(),
            session: String::new(),
            status,
            ts: 0,
            thread_id: None,
            thread_name: None,
            last_user_prompt: None,
            unseen: None,
            pane_id: None,
            liveness: None,
        }
    }

    fn sidebar_layout_message(state: ServerState) -> ServerMessage {
        ServerMessage::QueryResult {
            key: ServerQueryKey::SidebarLayout,
            data: ServerQueryData::SidebarLayout {
                sidebar_width: state.sidebar_width,
                detail_panel_height: state.detail_panel_height,
                initializing: state.initializing,
                init_label: state.init_label,
                collapsed_worktree_groups: state.collapsed_worktree_groups,
            },
            ts: state.ts,
        }
    }

    fn settings_message(state: ServerState) -> ServerMessage {
        ServerMessage::QueryResult {
            key: ServerQueryKey::Settings,
            data: ServerQueryData::Settings {
                theme: state.theme,
                session_filter: state.session_filter,
                agent_panel_scope: state.agent_panel_scope,
            },
            ts: state.ts,
        }
    }

    #[test]
    fn detail_panel_height_comes_from_server_state() {
        let mut app = App::from_state(empty_state(14));

        assert_eq!(app.detail_panel_height, 14);

        app.apply_server_message(sidebar_layout_message(empty_state(7)));
        assert_eq!(app.detail_panel_height, 7);
    }

    #[test]
    fn query_invalidation_refetches_active_read_models() {
        let mut app = App::from_state(empty_state(14));

        app.apply_server_message(ServerMessage::Invalidate {
            keys: vec![ServerQueryKey::SidebarLayout, ServerQueryKey::Settings],
            ts: 20,
        });

        assert_eq!(
            app.drain_commands(),
            vec![
                ClientCommand::FetchQuery {
                    key: ServerQueryKey::SidebarLayout,
                },
                ClientCommand::FetchQuery {
                    key: ServerQueryKey::Settings,
                },
            ]
        );
    }

    #[test]
    fn exact_query_invalidation_refetches_that_read_model() {
        let mut app = App::from_state(empty_state(14));

        app.apply_server_message(ServerMessage::Invalidate {
            keys: vec![ServerQueryKey::Agents],
            ts: 20,
        });

        assert_eq!(
            app.drain_commands(),
            vec![ClientCommand::FetchQuery {
                key: ServerQueryKey::Agents,
            }]
        );
    }

    #[test]
    fn moving_session_focus_publishes_client_scoped_ui_focus() {
        let mut state = empty_state(10);
        state.sessions = vec![
            session("work", "/repo/work", false),
            session("ops", "/repo/ops", false),
        ];
        let mut app = App::from_state(state);
        app.apply_server_message(ServerMessage::YourSession {
            name: "work".to_string(),
            client_tty: Some("/dev/ttys001".to_string()),
            ui_focus: None,
        });

        app.move_focus(1);

        assert_eq!(
            app.drain_commands(),
            vec![ClientCommand::SetUiFocus {
                focus: ClientUiFocus {
                    panel: ClientUiPanel::Sessions,
                    sidebar: Some(ClientSidebarFocus::Session {
                        name: "ops".to_string(),
                    }),
                    agent_index: 0,
                },
            }]
        );
    }

    #[test]
    fn ui_focus_messages_apply_only_for_this_tmux_client() {
        let mut state = empty_state(10);
        let mut work = session("work", "/repo/work", false);
        work.agents
            .push(agent_without_pane("amp", AgentStatus::Running));
        state.sessions = vec![work, session("ops", "/repo/ops", false)];
        let mut app = App::from_state(state);
        app.apply_server_message(ServerMessage::YourSession {
            name: "work".to_string(),
            client_tty: Some("/dev/ttys001".to_string()),
            ui_focus: None,
        });

        app.apply_server_message(ServerMessage::UiFocus {
            client_tty: "/dev/ttys002".to_string(),
            focus: ClientUiFocus {
                panel: ClientUiPanel::Agents,
                sidebar: Some(ClientSidebarFocus::Session {
                    name: "ops".to_string(),
                }),
                agent_index: 0,
            },
        });

        assert_eq!(app.panel_focus, PanelFocus::Sessions);
        assert_eq!(app.focused_session_name(), Some("work"));

        app.apply_server_message(ServerMessage::UiFocus {
            client_tty: "/dev/ttys001".to_string(),
            focus: ClientUiFocus {
                panel: ClientUiPanel::Agents,
                sidebar: Some(ClientSidebarFocus::Session {
                    name: "work".to_string(),
                }),
                agent_index: 0,
            },
        });

        assert_eq!(app.panel_focus, PanelFocus::Agents);
        assert_eq!(app.focused_session_name(), Some("work"));
    }

    #[test]
    fn query_result_updates_server_state_read_model() {
        let mut app = App::from_state(empty_state(14));

        app.apply_server_message(ServerMessage::QueryResult {
            key: ServerQueryKey::SidebarLayout,
            data: ServerQueryData::SidebarLayout {
                sidebar_width: 40,
                detail_panel_height: 9,
                initializing: false,
                init_label: None,
                collapsed_worktree_groups: Vec::new(),
            },
            ts: 42,
        });

        assert_eq!(app.detail_panel_height, 9);
        assert_eq!(app.ts, 42);
        assert!(app.server_state_result().is_success());
    }

    #[test]
    fn typed_query_results_update_independent_read_models() {
        let mut state = empty_state(10);
        state.sessions = vec![session("work", "/repo", false)];
        let mut app = App::from_state(state);

        app.apply_server_message(ServerMessage::QueryResult {
            key: ServerQueryKey::Settings,
            data: ServerQueryData::Settings {
                theme: Some("dark".to_string()),
                session_filter: Some(SessionFilterMode::Running),
                agent_panel_scope: AgentPanelScope::All,
            },
            ts: 50,
        });
        app.apply_server_message(ServerMessage::QueryResult {
            key: ServerQueryKey::SidebarLayout,
            data: ServerQueryData::SidebarLayout {
                sidebar_width: 52,
                detail_panel_height: 16,
                initializing: true,
                init_label: Some("warming".to_string()),
                collapsed_worktree_groups: vec!["/repo".to_string()],
            },
            ts: 51,
        });
        app.apply_server_message(ServerMessage::QueryResult {
            key: ServerQueryKey::Agents,
            data: ServerQueryData::Agents {
                sessions: vec![SessionAgentsData {
                    session: "work".to_string(),
                    unseen: true,
                    agent_state: Some(agent_without_pane("amp", AgentStatus::Running)),
                    agents: vec![agent_without_pane("amp", AgentStatus::Running)],
                    event_timestamps: vec![123],
                }],
            },
            ts: 52,
        });

        assert_eq!(app.theme.as_deref(), Some("dark"));
        assert_eq!(app.session_filter, SessionFilterMode::Running);
        assert_eq!(app.agent_panel_scope, AgentPanelScope::All);
        assert_eq!(app.sidebar_width, 52);
        assert_eq!(app.detail_panel_height, 16);
        assert!(app.initializing);
        assert_eq!(app.init_label.as_deref(), Some("warming"));
        assert!(app.is_group_collapsed("/repo"));
        assert!(app.sessions[0].unseen);
        assert_eq!(app.sessions[0].agents.len(), 1);
        assert_eq!(app.sessions[0].event_timestamps, vec![123]);
    }

    #[test]
    fn pending_detail_panel_height_ignores_stale_server_echoes_until_ack() {
        let mut app = App::from_state(empty_state(10));

        app.set_detail_panel_height(14);
        app.apply_server_message(sidebar_layout_message(empty_state(11)));

        assert_eq!(app.detail_panel_height, 14);

        app.apply_server_message(sidebar_layout_message(empty_state(14)));
        assert_eq!(app.detail_panel_height, 14);

        app.apply_server_message(sidebar_layout_message(empty_state(9)));
        assert_eq!(app.detail_panel_height, 9);
    }

    #[test]
    fn agent_panel_scope_comes_from_server_state_and_toggle_emits_command() {
        let mut state = empty_state(10);
        state.agent_panel_scope = AgentPanelScope::All;
        let mut app = App::from_state(state);

        assert_eq!(app.agent_panel_scope, AgentPanelScope::All);

        app.toggle_agent_panel_scope();

        assert_eq!(app.agent_panel_scope, AgentPanelScope::Current);
        assert_eq!(
            app.drain_commands(),
            vec![ClientCommand::SetAgentPanelScope {
                scope: AgentPanelScope::Current,
            }]
        );
    }

    #[test]
    fn pending_agent_panel_scope_ignores_stale_server_echoes_until_ack() {
        let mut app = App::from_state(empty_state(10));

        app.toggle_agent_panel_scope();
        app.apply_server_message(settings_message(empty_state(10)));

        assert_eq!(app.agent_panel_scope, AgentPanelScope::All);

        let mut acknowledged = empty_state(10);
        acknowledged.agent_panel_scope = AgentPanelScope::All;
        app.apply_server_message(settings_message(acknowledged));
        assert_eq!(app.agent_panel_scope, AgentPanelScope::All);

        let mut later = empty_state(10);
        later.agent_panel_scope = AgentPanelScope::Current;
        app.apply_server_message(settings_message(later));
        assert_eq!(app.agent_panel_scope, AgentPanelScope::Current);
    }

    #[test]
    fn resizing_detail_panel_emits_shared_height_command() {
        let mut app = App::from_state(empty_state(10));

        app.resize_detail_panel(2);

        assert_eq!(app.detail_panel_height, 12);
        assert_eq!(
            app.drain_commands(),
            vec![ClientCommand::SetDetailPanelHeight { height: 12 }]
        );
    }

    #[test]
    fn x_on_worktree_group_opens_group_kill_confirmation() {
        let mut state = empty_state(10);
        state.sessions = vec![
            session("feature-a", "/repo/worktrees/feature-a", true),
            session("feature-b", "/repo/worktrees/feature-b", true),
        ];
        let mut app = App::from_state(state);

        app.handle_key_char('x');

        assert_eq!(
            app.modal,
            Modal::KillConfirm {
                target: KillTarget::WorktreeGroup("/repo/worktrees".to_string())
            }
        );
    }

    #[test]
    fn x_on_worktree_child_session_opens_session_kill_confirmation() {
        let mut state = empty_state(10);
        state.sessions = vec![
            session("feature-a", "/repo/worktrees/feature-a", true),
            session("feature-b", "/repo/worktrees/feature-b", true),
        ];
        let mut app = App::from_state(state);
        app.set_focused_session("feature-a");

        app.handle_key_char('x');

        assert_eq!(
            app.modal,
            Modal::KillConfirm {
                target: KillTarget::Session("feature-a".to_string())
            }
        );
    }

    #[test]
    fn x_on_worktree_child_session_falls_back_from_empty_agent_panel() {
        let mut state = empty_state(10);
        state.sessions = vec![
            session("feature-a", "/repo/worktrees/feature-a", true),
            session("feature-b", "/repo/worktrees/feature-b", true),
        ];
        let mut app = App::from_state(state);
        app.set_focused_session("feature-a");
        app.panel_focus = PanelFocus::Agents;

        app.handle_key_char('x');

        assert_eq!(
            app.modal,
            Modal::KillConfirm {
                target: KillTarget::Session("feature-a".to_string())
            }
        );
    }

    #[test]
    fn x_on_worktree_child_session_falls_back_from_agent_without_live_pane() {
        let mut state = empty_state(10);
        let mut feature = session("feature-a", "/repo/worktrees/feature-a", true);
        feature
            .agents
            .push(agent_without_pane("amp", AgentStatus::Done));
        state.sessions = vec![
            feature,
            session("feature-b", "/repo/worktrees/feature-b", true),
        ];
        let mut app = App::from_state(state);
        app.set_focused_session("feature-a");
        app.panel_focus = PanelFocus::Agents;

        app.handle_key_char('x');

        assert_eq!(
            app.modal,
            Modal::KillConfirm {
                target: KillTarget::Session("feature-a".to_string())
            }
        );
        assert!(app.drain_commands().is_empty());
    }

    #[test]
    fn confirming_worktree_group_kill_resolves_current_group_members() {
        let mut state = empty_state(10);
        state.sessions = vec![
            session("feature-a", "/repo/worktrees/feature-a", true),
            session("feature-b", "/repo/worktrees/feature-b", true),
            session("main", "/repo/main", false),
        ];
        let mut app = App::from_state(state);

        app.handle_key_char('x');
        app.sessions
            .push(session("feature-c", "/repo/worktrees/feature-c", true));
        app.confirm_kill_target();

        assert_eq!(
            app.drain_commands(),
            vec![
                ClientCommand::KillSession {
                    name: "feature-a".to_string()
                },
                ClientCommand::KillSession {
                    name: "feature-b".to_string()
                },
                ClientCommand::KillSession {
                    name: "feature-c".to_string()
                },
            ]
        );
    }

    #[test]
    fn reordering_worktree_group_moves_all_members_as_a_block() {
        let mut state = empty_state(10);
        state.sessions = vec![
            session("main", "/repo/main", false),
            session("feature-a", "/repo/worktrees/feature-a", true),
            session("feature-b", "/repo/worktrees/feature-b", true),
            session("docs", "/repo/docs", false),
        ];
        let app = App::from_state(state);

        assert_eq!(
            app.reordered_worktree_group_names("/repo/worktrees", 1),
            Some(vec![
                "main".to_string(),
                "docs".to_string(),
                "feature-a".to_string(),
                "feature-b".to_string(),
            ])
        );
        assert_eq!(
            app.reordered_worktree_group_names("/repo/worktrees", -1),
            Some(vec![
                "feature-a".to_string(),
                "feature-b".to_string(),
                "main".to_string(),
                "docs".to_string(),
            ])
        );
    }

    #[test]
    fn alt_reorder_on_worktree_group_emits_group_reorder_command() {
        let mut state = empty_state(10);
        state.sessions = vec![
            session("feature-a", "/repo/worktrees/feature-a", true),
            session("feature-b", "/repo/worktrees/feature-b", true),
        ];
        let mut app = App::from_state(state);

        app.reorder_focused_session(1);

        assert_eq!(
            app.drain_commands(),
            vec![ClientCommand::ReorderWorktreeGroup {
                key: "/repo/worktrees".to_string(),
                delta: 1,
            }]
        );
    }
}
