use core::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolHello {
    pub protocol: u16,
    pub server_version: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ServerMessage {
    Hello(ProtocolHello),
    Invalidate {
        keys: Vec<ServerQueryKey>,
        ts: u64,
    },
    QueryResult {
        key: ServerQueryKey,
        data: ServerQueryData,
        ts: u64,
    },
    Quit,
    YourSession {
        name: String,
        client_tty: Option<String>,
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        ui_focus: Option<ClientUiFocus>,
    },
    UiFocus {
        client_tty: String,
        focus: ClientUiFocus,
    },
    ActivateSession {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_pane_id: Option<String>,
    },
    ReIdentify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServerQueryKey {
    Sessions,
    Agents,
    Focus,
    SidebarLayout,
    Settings,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ServerQueryData {
    Sessions {
        sessions: Vec<SessionData>,
    },
    Agents {
        sessions: Vec<SessionAgentsData>,
    },
    Focus {
        focused_session: Option<String>,
        current_session: Option<String>,
    },
    SidebarLayout {
        sidebar_width: u32,
        detail_panel_height: u32,
        initializing: bool,
        init_label: Option<String>,
        collapsed_worktree_groups: Vec<String>,
    },
    Settings {
        theme: Option<String>,
        session_filter: Option<SessionFilterMode>,
        agent_panel_scope: AgentPanelScope,
    },
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionAgentsData {
    pub session: String,
    pub unseen: bool,
    pub agent_state: Option<AgentEvent>,
    pub agents: Vec<AgentEvent>,
    pub event_timestamps: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerState {
    pub sessions: Vec<SessionData>,
    pub focused_session: Option<String>,
    pub current_session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_filter: Option<SessionFilterMode>,
    #[serde(default)]
    pub agent_panel_scope: AgentPanelScope,
    pub sidebar_width: u32,
    pub detail_panel_height: u32,
    pub initializing: bool,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub init_label: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub collapsed_worktree_groups: Vec<String>,
    pub ts: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionData {
    pub name: String,
    pub created_at: u64,
    pub dir: String,
    pub branch: String,
    pub dirty: bool,
    #[serde(default)]
    pub changed_files: u32,
    #[serde(default)]
    pub insertions: u32,
    #[serde(default)]
    pub deletions: u32,
    pub is_worktree: bool,
    pub unseen: bool,
    pub panes: u32,
    pub ports: Vec<u32>,
    pub local_links: Vec<LocalLink>,
    pub windows: u32,
    pub uptime: String,
    pub agent_state: Option<AgentEvent>,
    pub agents: Vec<AgentEvent>,
    pub event_timestamps: Vec<u64>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<SessionMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LocalLink {
    pub kind: LocalLinkKind,
    pub port: u32,
    pub url: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LocalLinkKind {
    Direct,
    Portless,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentStatus {
    Idle,
    Running,
    ToolRunning,
    Done,
    Error,
    Waiting,
    Interrupted,
    Stale,
}

impl fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::ToolRunning => "tool-running",
            Self::Done => "done",
            Self::Error => "error",
            Self::Waiting => "waiting",
            Self::Interrupted => "interrupted",
            Self::Stale => "stale",
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentPanelScope {
    #[default]
    Current,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentLiveness {
    Alive,
    Exited,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    pub agent: String,
    pub session: String,
    pub status: AgentStatus,
    pub ts: u64,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_name: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_user_prompt: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unseen: Option<bool>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub liveness: Option<AgentLiveness>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPaneDiagnostic {
    pub session: String,
    pub pane_id: String,
    pub window_id: String,
    pub active: bool,
    pub command: String,
    pub title: String,
    pub mapped_agent: Option<String>,
    pub mapping_reason: String,
    pub parsed_status: Option<AgentStatus>,
    pub thread_name: Option<String>,
    pub visible_tail: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionDiagnostic {
    pub session: String,
    pub focused: bool,
    pub current: bool,
    pub pane_candidates: Vec<AgentPaneDiagnostic>,
    pub tracker_agent_state: Option<AgentEvent>,
    pub tracker_agents: Vec<AgentEvent>,
    pub projected_current_panel_count: usize,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDiagnostics {
    pub focused_session: Option<String>,
    pub current_session: Option<String>,
    pub agent_panel_scope: AgentPanelScope,
    pub sessions: Vec<AgentSessionDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MetadataTone {
    Neutral,
    Info,
    Success,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct MetadataStatus {
    pub text: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tone: Option<MetadataTone>,
    pub ts: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct MetadataProgress {
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<u64>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<f64>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub ts: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct MetadataLogEntry {
    pub message: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tone: Option<MetadataTone>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub ts: u64,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
pub struct SessionMetadata {
    #[serde(default)]
    pub status: Option<MetadataStatus>,
    #[serde(default)]
    pub progress: Option<MetadataProgress>,
    #[serde(default)]
    pub logs: Vec<MetadataLogEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionFilterMode {
    #[default]
    All,
    Active,
    Running,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ClientCommand {
    FetchQuery {
        key: ServerQueryKey,
    },
    SwitchSession {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        client_tty: Option<String>,
    },
    NewSession,
    HideSession {
        name: String,
    },
    ShowAllSessions,
    KillSession {
        name: String,
    },
    ReorderSession {
        name: String,
        delta: i8,
    },
    ReorderWorktreeGroup {
        key: String,
        delta: i8,
    },
    Refresh,
    MarkSeen {
        name: String,
    },
    DismissAgent {
        session: String,
        agent: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
    },
    SetTheme {
        theme: String,
    },
    SetSidebarWidth {
        width: u32,
    },
    SetDetailPanelHeight {
        height: u32,
    },
    SetAgentPanelScope {
        scope: AgentPanelScope,
    },
    SetUiFocus {
        focus: ClientUiFocus,
    },
    RepairWidth,
    SetFilter {
        filter: SessionFilterMode,
    },
    ToggleWorktreeGroup {
        key: String,
    },
    Quit,
    IdentifyPane {
        pane_id: String,
        session_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        window_id: Option<String>,
    },
    FocusAgentPane {
        session: String,
        agent: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pane_id: Option<String>,
    },
    KillAgentPane {
        session: String,
        agent: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pane_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientUiFocus {
    pub panel: ClientUiPanel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidebar: Option<ClientSidebarFocus>,
    pub agent_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientUiPanel {
    Sessions,
    Agents,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ClientSidebarFocus {
    Session { name: String },
    WorktreeGroup { key: String },
}
