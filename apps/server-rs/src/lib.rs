use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::path::PathBuf;
use std::process;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime};

use base64::{Engine, engine::general_purpose::STANDARD};
use convex::{ConvexClient, FunctionResult, Value as ConvexValue};
use futures_util::{SinkExt, StreamExt};
use opensessions_runtime::agent_watchers::{
    AgentWatcherSnapshot, amp_snapshot_from_thread_json, claude_code_snapshot_from_jsonl,
    codex_snapshot_from_jsonl, codex_thread_id_from_path, decode_claude_project_dir,
    droid_snapshot_from_jsonl, opencode_snapshot_from_row, parse_codex_session_index,
    pi_snapshot_from_jsonl,
};
use opensessions_runtime::config::{
    OpensessionsConfig, SshNodeConfig, config_path_from_home, load_config_from_home,
    save_config_to_home,
};
use opensessions_runtime::git_info::{GitInfo, parse_git_info_output};
use opensessions_runtime::metadata_store::SessionMetadataStore;
use opensessions_runtime::mux::{ActiveWindow, MuxProvider, SidebarPosition};
use opensessions_runtime::pi_runtime_registry::{PiRuntimeRegistry, parse_pi_runtime_info};
use opensessions_runtime::port_discovery::{PortDiscoveryInput, discover_session_ports};
use opensessions_runtime::project_dir_session::{
    build_dir_session_map, resolve_session_for_project_dir,
};
use opensessions_runtime::protocol::{
    AgentDiagnostics, AgentEvent, AgentLiveness, AgentPanelScope, AgentSessionDiagnostic,
    AgentStatus, ClientUiFocus, MetadataTone, ServerMessage, ServerQueryData, ServerQueryKey,
    ServerState, SessionAgentsData, SessionFilterMode,
};
use opensessions_runtime::server_state::{ReadOnlyStateInput, build_read_only_state};
use opensessions_runtime::session_order::SessionOrder;
use opensessions_runtime::session_projection::{
    SessionProjectionOptions, display_session_names, display_sessions, reordered_session_names,
    reordered_worktree_group_names,
};
use opensessions_runtime::sidebar_coordinator::SidebarCoordinator;
use opensessions_runtime::sidebar_width_sync::clamp_sidebar_width;
use opensessions_runtime::tmux_provider::{StdCommandRunner, TmuxProvider};
use opensessions_runtime::tracker::{AgentTracker, PanePresenceInput};
use serde::Deserialize;
use serde_json::Value;
use sha1_smol::Sha1;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio::time::{Duration, MissedTickBehavior};
use tokio_websockets::{Message, ServerBuilder};

pub const SERVER_VERSION: &str = "0.2.0-alpha.12";
pub const PROTOCOL_VERSION: u16 = 1;
pub const HELLO_JSON: &str = r#"{"type":"hello","protocol":1,"serverVersion":"0.2.0-alpha.12"}"#;
pub const QUIT_JSON: &str = r#"{"type":"quit"}"#;
pub const VERSION_HTTP_BODY: &str = "opensessions-server 0.2.0-alpha.12 protocol 1";

const MAX_HTTP_HEADER_BYTES: usize = 16 * 1024;
const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const SIDEBAR_SCRIPTS_DIR: &str = "apps/tui/scripts";
const GIT_CACHE_TTL_MS: u64 = 5_000;
const PORT_POLL_INTERVAL_MS: u64 = 10_000;
const ENABLE_JSONL_AGENT_WATCHERS: bool = true;
const AGENT_WATCHER_POLL_MS: u64 = 2_000;
const TMUX_STATE_POLL_MS: u64 = 2_000;
const SIDEBAR_WARMUP_MS: u64 = 1_200;
const SIDEBAR_STAGGER_MS: u64 = 35;
const SERVER_SHUTDOWN_DRAIN_MS: u64 = 120;
const AGENT_WATCHER_RECENT_MS: u64 = 5 * 60 * 1000;
const OPENCODE_SQL_TIMEOUT_MS: u64 = 500;
const OPENCODE_SQL_SEP: char = '\u{1f}';
const DEFAULT_DETAIL_PANEL_HEIGHT: u16 = 10;
const MIN_DETAIL_PANEL_HEIGHT: u16 = 4;
const MAX_DETAIL_PANEL_HEIGHT: u16 = 60;

#[derive(Debug, Default)]
struct ShutdownAnnouncement {
    announced: AtomicBool,
}

impl ShutdownAnnouncement {
    fn announce_once(
        &self,
        state_source: &Option<Arc<dyn StateSource>>,
        state_updates: &broadcast::Sender<String>,
    ) {
        if self.announced.swap(true, Ordering::SeqCst) {
            return;
        }
        announce_shutdown(state_source, state_updates);
    }
}

/// Append a single debug line. Temporarily defaults to `/tmp/opensessions-debug.log`
/// so live focus/agent-state issues can be diagnosed without extra env setup;
/// `OPENSESSIONS_DEBUG_LOG` still overrides the path when set.
fn debug_log(line: impl AsRef<str>) {
    use std::io::Write;
    let path = std::env::var("OPENSESSIONS_DEBUG_LOG")
        .ok()
        .unwrap_or_else(|| "/tmp/opensessions-debug.log".to_string());
    if path.is_empty() {
        return;
    }
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(
            file,
            "[{now}] [server pid={}] {}",
            std::process::id(),
            line.as_ref()
        );
    }
}

pub trait StateSource: Send + Sync + 'static {
    fn query_result_json(&self, key: ServerQueryKey) -> Option<String>;

    fn invalidate_queries_json(&self, keys: Vec<ServerQueryKey>) -> String {
        invalidate_queries_json(keys)
    }

    fn invalidate_all_queries_json(&self) -> String {
        self.invalidate_queries_json(all_query_keys())
    }

    fn setup_mux_hooks(&self, _server_host: &str, _server_port: u16) {}

    fn cleanup_mux_hooks(&self) {}

    fn start_background_tasks(
        self: Arc<Self>,
        _state_updates: broadcast::Sender<String>,
        _shutdown: broadcast::Sender<()>,
    ) -> Vec<JoinHandle<()>> {
        Vec::new()
    }

    fn handle_client_command(&self, _command: &Value) -> Option<String> {
        None
    }

    fn handle_client_command_with_context(
        &self,
        command: &Value,
        _context: Option<&ClientConnectionContext>,
    ) -> Option<String> {
        self.handle_client_command(command)
    }

    fn handle_sender_command(&self, _command: &Value) -> Option<SenderCommandOutcome> {
        None
    }

    fn handle_sender_command_with_context(
        &self,
        command: &Value,
        _context: &mut ClientConnectionContext,
    ) -> Option<SenderCommandOutcome> {
        self.handle_sender_command(command)
    }

    fn handle_http_json(&self, _path: &str, _body: &Value) -> Option<String> {
        None
    }

    fn handle_http_text(&self, _path: &str, _body: &str) -> Option<String> {
        None
    }

    fn debug_agents_json(&self, _session: Option<&str>) -> Option<String> {
        None
    }

    fn handle_http_hook(&self, _path: &str, _body: &str) -> Option<String> {
        None
    }

    fn handle_switch_index(&self, _index: u32, _body: &str) -> Option<String> {
        None
    }

    fn handle_agent_event_json(&self, _body: &Value) -> Result<String, AgentEventError> {
        Err(AgentEventError::CouldNotResolveSession)
    }

    fn handle_pi_runtime_upsert(&self, _body: &Value) -> Result<(), PiRuntimeError> {
        Err(PiRuntimeError::InvalidPayload)
    }

    fn handle_pi_runtime_delete(&self, _body: &Value) -> Result<(), PiRuntimeError> {
        Err(PiRuntimeError::MissingPid)
    }

    fn begin_shutdown(&self) -> Option<String> {
        None
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ClientConnectionContext {
    client_tty: Option<String>,
    pane_id: Option<String>,
    session_name: Option<String>,
    window_id: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SenderCommandOutcome {
    reply: Option<String>,
    broadcast: Option<String>,
}

#[derive(Default)]
struct FocusObservation {
    agents_changed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentEventError {
    MissingAgent,
    InvalidStatus,
    CouldNotResolveSession,
}

impl AgentEventError {
    fn status_and_body(self) -> (&'static str, &'static str) {
        match self {
            Self::MissingAgent => ("400 Bad Request", "missing agent"),
            Self::InvalidStatus => ("400 Bad Request", "invalid status"),
            // Agent events are intentionally broadcast to every opensessions
            // server in every tmux namespace. A server that cannot map the
            // event's projectDir/tmuxSession to one of its sessions should
            // no-op with a non-error status so the plugin can publish once and
            // let each server decide folder ownership locally. Use 202 (not
            // 204) so the plugin can distinguish "ignored by this server" from
            // "applied by an owning server" when deciding whether to retry
            // during owner-server restarts.
            Self::CouldNotResolveSession => ("202 Accepted", ""),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiRuntimeError {
    InvalidPayload,
    MissingPid,
}

impl PiRuntimeError {
    fn body(self) -> &'static str {
        match self {
            Self::InvalidPayload => "invalid pi runtime payload",
            Self::MissingPid => "missing pid",
        }
    }
}

impl<F> StateSource for F
where
    F: Fn(ServerQueryKey) -> Option<String> + Send + Sync + 'static,
{
    fn query_result_json(&self, key: ServerQueryKey) -> Option<String> {
        self(key)
    }
}

pub trait PortCommandRunner: Send + Sync + 'static {
    fn process_rows(&self) -> Vec<(u32, u32)>;
    fn lsof_fields(&self) -> String;
}

pub trait GitCommandRunner: Send + Sync + 'static {
    fn git_info_output(&self, dir: &str) -> String;
}

#[derive(Debug, Default)]
struct SystemPortCommandRunner;

#[derive(Debug, Default)]
struct SystemGitCommandRunner;

impl PortCommandRunner for SystemPortCommandRunner {
    fn process_rows(&self) -> Vec<(u32, u32)> {
        let Ok(output) = process::Command::new("ps")
            .args(["-eo", "pid=,ppid="])
            .output()
        else {
            return Vec::new();
        };
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(parse_process_row)
            .collect()
    }

    fn lsof_fields(&self) -> String {
        let Ok(output) = process::Command::new("/usr/sbin/lsof")
            .args(["-iTCP", "-sTCP:LISTEN", "-nP", "-F", "pn"])
            .output()
        else {
            return String::new();
        };
        if !output.status.success() {
            return String::new();
        }
        String::from_utf8_lossy(&output.stdout).to_string()
    }
}

impl GitCommandRunner for SystemGitCommandRunner {
    fn git_info_output(&self, dir: &str) -> String {
        if dir.is_empty() {
            return String::new();
        }

        let Ok(rev_parse) = process::Command::new("git")
            .current_dir(dir)
            .args(["rev-parse", "--abbrev-ref", "HEAD", "--git-dir"])
            .output()
        else {
            return String::new();
        };
        if !rev_parse.status.success() {
            return String::new();
        }

        let Ok(status) = process::Command::new("git")
            .current_dir(dir)
            .args(["status", "--porcelain"])
            .output()
        else {
            return String::new();
        };

        let Ok(numstat) = process::Command::new("git")
            .current_dir(dir)
            .args(["diff", "--numstat", "HEAD", "--"])
            .output()
        else {
            return String::new();
        };

        format!(
            "{}\n---\n{}\n---NUMSTAT---\n{}",
            String::from_utf8_lossy(&rev_parse.stdout).trim(),
            String::from_utf8_lossy(&status.stdout).trim(),
            String::from_utf8_lossy(&numstat.stdout).trim()
        )
    }
}

#[derive(Debug, Clone)]
struct CachedGitInfo {
    info: GitInfo,
    ts: u64,
}

#[derive(Debug, Clone)]
struct CachedPortSnapshot {
    session_names: Vec<String>,
    ports_by_session: HashMap<String, Vec<u16>>,
    ts: u64,
}

#[derive(Debug, Clone)]
struct CachedQueryJson {
    generation: u64,
    json: String,
}

#[derive(Debug, Default)]
struct QueryJsonCache {
    generation: u64,
    results_by_key: HashMap<ServerQueryKey, CachedQueryJson>,
}

#[derive(Debug, Clone, Default)]
struct CloudGraphCache {
    sessions: Vec<opensessions_runtime::protocol::SessionData>,
    ui_state: Option<CloudWorkspaceUiState>,
    command_intents: Vec<CloudCommandIntent>,
    handled_command_intents: HashSet<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CloudWorkspaceUiState {
    #[serde(default)]
    sidebar_visible: Option<bool>,
    #[serde(default)]
    focused_session: Option<String>,
    #[serde(default)]
    provider_filter: Option<String>,
    #[serde(default)]
    session_filter: Option<String>,
    #[serde(default)]
    agent_panel_scope: Option<String>,
    #[serde(default)]
    visible_session_order: Vec<String>,
    #[serde(default)]
    hidden_sessions: Vec<String>,
    #[serde(default)]
    collapsed_worktree_groups: Vec<String>,
    #[serde(default)]
    sidebar_width: Option<u32>,
    #[serde(default)]
    detail_panel_height: Option<u32>,
    #[serde(default)]
    updated_by_node: Option<String>,
    #[serde(default)]
    ts: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CloudCommandIntent {
    id: String,
    action: String,
    #[serde(default)]
    node_id: Option<String>,
    #[serde(default)]
    provider_id: Option<String>,
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    payload: Value,
    #[serde(default)]
    ts: u64,
}

pub struct ReadOnlyMuxStateSource {
    providers: Vec<Arc<dyn MuxProvider>>,
    port_command_runner: Arc<dyn PortCommandRunner>,
    port_snapshot_cache: Mutex<Option<CachedPortSnapshot>>,
    git_command_runner: Arc<dyn GitCommandRunner>,
    git_info_cache: Mutex<HashMap<String, CachedGitInfo>>,
    query_json_cache: Mutex<QueryJsonCache>,
    cloud_graph_cache: Mutex<CloudGraphCache>,
    // The sidebar coordinator owns the single source of truth for the current
    // width (`SidebarCoordinator::state().width`), so there is no separate
    // mirror field to drift out of sync.
    sidebar_coordinator: Mutex<SidebarCoordinator>,
    detail_panel_height: Mutex<u16>,
    agent_panel_scope: Mutex<AgentPanelScope>,
    focused_session: Mutex<Option<String>>,
    focused_pane_by_session: Mutex<HashMap<String, String>>,
    focused_client_tty: Mutex<Option<String>>,
    ui_focus_by_client_tty: Mutex<HashMap<String, ClientUiFocus>>,
    workspace_ui_updated_at: Mutex<u64>,
    theme: Mutex<Option<String>>,
    session_filter: Mutex<Option<SessionFilterMode>>,
    provider_filter: Mutex<Option<String>>,
    collapsed_worktree_groups: Mutex<HashSet<String>>,
    session_order: Mutex<SessionOrder>,
    metadata_store: Mutex<SessionMetadataStore>,
    agent_tracker: Mutex<AgentTracker>,
    pi_runtime_registry: Mutex<PiRuntimeRegistry>,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
}

pub fn default_state_source_from_env(
    env: impl Fn(&str) -> Option<String>,
) -> Option<ReadOnlyMuxStateSource> {
    let has_explicit_tmux_provider = env("OPENSESSIONS_TMUX_SOCKETS")
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty());
    if env("TMUX").is_some() || has_explicit_tmux_provider {
        let node_id = env("OPENSESSIONS_NODE_ID")
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "local".to_string());
        let providers = tmux_providers_from_env(&env, &node_id);
        let mut source = ReadOnlyMuxStateSource::new(providers);
        let config = env("HOME")
            .map(PathBuf::from)
            .map(|home| load_config_from_home(&home));
        if let Some(width) = config.as_ref().and_then(|config| config.sidebar_width) {
            source = source.with_sidebar_width(clamp_sidebar_width(width) as u32);
        }
        if let Some(height) = config.and_then(|config| config.detail_panel_height) {
            source = source.with_detail_panel_height(height);
        }
        return Some(source);
    }

    None
}

fn tmux_providers_from_env(
    env: &impl Fn(&str) -> Option<String>,
    node_id: &str,
) -> Vec<Arc<dyn MuxProvider>> {
    let Some(raw) = env("OPENSESSIONS_TMUX_SOCKETS") else {
        return vec![Arc::new(TmuxProvider::with_identity(
            node_id,
            "tmux",
            Arc::new(StdCommandRunner::default()),
        ))];
    };
    let providers = raw
        .split(',')
        .filter_map(|entry| {
            let entry = entry.trim();
            if entry.is_empty() {
                return None;
            }
            let (name, socket) = entry.split_once('=').unwrap_or((entry, entry));
            let name = name.trim();
            let socket = socket.trim();
            if name.is_empty() {
                return None;
            }
            let runner: Arc<dyn opensessions_runtime::tmux_provider::CommandRunner> =
                if socket.is_empty() {
                    Arc::new(StdCommandRunner::default())
                } else {
                    Arc::new(StdCommandRunner::with_prefix_args(
                        "tmux",
                        vec!["-L".to_string(), socket.to_string()],
                    ))
                };
            Some(Arc::new(TmuxProvider::with_identity(node_id, name, runner))
                as Arc<dyn MuxProvider>)
        })
        .collect::<Vec<_>>();
    if providers.is_empty() {
        vec![Arc::new(TmuxProvider::with_identity(
            node_id,
            "tmux",
            Arc::new(StdCommandRunner::default()),
        ))]
    } else {
        providers
    }
}

impl ReadOnlyMuxStateSource {
    pub fn new(providers: Vec<Arc<dyn MuxProvider>>) -> Self {
        Self {
            providers,
            port_command_runner: Arc::new(SystemPortCommandRunner),
            port_snapshot_cache: Mutex::new(None),
            git_command_runner: Arc::new(SystemGitCommandRunner),
            git_info_cache: Mutex::new(HashMap::new()),
            query_json_cache: Mutex::new(QueryJsonCache::default()),
            cloud_graph_cache: Mutex::new(CloudGraphCache::default()),
            sidebar_coordinator: Mutex::new(SidebarCoordinator::new(26)),
            detail_panel_height: Mutex::new(DEFAULT_DETAIL_PANEL_HEIGHT),
            agent_panel_scope: Mutex::new(AgentPanelScope::Current),
            focused_session: Mutex::new(None),
            focused_pane_by_session: Mutex::new(HashMap::new()),
            focused_client_tty: Mutex::new(None),
            ui_focus_by_client_tty: Mutex::new(HashMap::new()),
            workspace_ui_updated_at: Mutex::new(0),
            theme: Mutex::new(None),
            session_filter: Mutex::new(None),
            provider_filter: Mutex::new(None),
            collapsed_worktree_groups: Mutex::new(HashSet::new()),
            session_order: Mutex::new(SessionOrder::new(None)),
            metadata_store: Mutex::new(SessionMetadataStore::new()),
            agent_tracker: Mutex::new(AgentTracker::new()),
            pi_runtime_registry: Mutex::new(PiRuntimeRegistry::with_default_ttl()),
            now_ms: Arc::new(current_time_ms),
        }
    }

    pub fn with_sidebar_width(mut self, sidebar_width: u32) -> Self {
        self.sidebar_coordinator = Mutex::new(SidebarCoordinator::new(sidebar_width));
        self
    }

    pub fn with_detail_panel_height(mut self, height: u16) -> Self {
        self.detail_panel_height = Mutex::new(clamp_detail_panel_height(height));
        self
    }

    /// Current sidebar width from the coordinator (single source of truth),
    /// clamped to `u16` for the tmux resize APIs.
    fn current_sidebar_width_u16(&self) -> u16 {
        self.sidebar_coordinator
            .lock()
            .unwrap()
            .state()
            .width
            .min(u16::MAX as u32) as u16
    }

    fn is_sidebar_visible(&self) -> bool {
        self.sidebar_coordinator.lock().unwrap().state().visible
    }

    fn persist_sidebar_width(&self, width: u16) {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            debug_log("set-sidebar-width: skipped config save because HOME is unset");
            return;
        };
        if let Err(err) = save_config_to_home(
            &home,
            OpensessionsConfig {
                sidebar_width: Some(width),
                ..OpensessionsConfig::default()
            },
        ) {
            debug_log(format!(
                "set-sidebar-width: failed to save sidebarWidth={width}: {err}"
            ));
        }
    }

    fn persist_detail_panel_height(&self, height: u16) {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            debug_log("set-detail-panel-height: skipped config save because HOME is unset");
            return;
        };
        if let Err(err) = save_config_to_home(
            &home,
            OpensessionsConfig {
                detail_panel_height: Some(height),
                ..OpensessionsConfig::default()
            },
        ) {
            debug_log(format!(
                "set-detail-panel-height: failed to save detailPanelHeight={height}: {err}"
            ));
        }
    }

    pub fn with_now_ms(mut self, now_ms: impl Fn() -> u64 + Send + Sync + 'static) -> Self {
        self.now_ms = Arc::new(now_ms);
        self
    }

    pub fn with_port_command_runner(mut self, runner: Arc<dyn PortCommandRunner>) -> Self {
        self.port_command_runner = runner;
        self
    }

    pub fn with_git_command_runner(mut self, runner: Arc<dyn GitCommandRunner>) -> Self {
        self.git_command_runner = runner;
        self
    }

    fn sync_agent_pane_presence(&self) -> bool {
        let mut presence_by_session = Vec::new();
        let mut focused_agent_panes = HashMap::<String, String>::new();
        let focused_client_tty = self.focused_client_tty.lock().unwrap().clone();
        for provider in &self.providers {
            let client_focus = provider.get_client_focus(focused_client_tty.as_deref());
            for session in provider.list_sessions() {
                let pane_agents = provider
                    .list_agent_panes(&session.name)
                    .into_iter()
                    .map(|pane| {
                        if client_focus.as_ref().is_some_and(|focus| {
                            focus.session_name == session.name && focus.pane_id == pane.pane_id
                        }) {
                            focused_agent_panes.insert(session.name.clone(), pane.pane_id.clone());
                        }
                        PanePresenceInput {
                            agent: pane.agent,
                            node_id: pane.node_id,
                            provider_id: pane.provider_id,
                            pane_id: pane.pane_id,
                            active: pane.active,
                            status: pane.status,
                            thread_id: pane.thread_id,
                            thread_name: pane.thread_name,
                        }
                    })
                    .collect::<Vec<_>>();
                if !pane_agents.is_empty() {
                    debug_log(format!(
                        "agent-pane-presence session={} panes={:?}",
                        session.name, pane_agents,
                    ));
                }
                presence_by_session.push((session.name, pane_agents));
            }
        }

        let mut changed = false;
        let mut tracker = self.agent_tracker.lock().unwrap();
        for (session, pane_agents) in presence_by_session {
            changed = tracker.apply_pane_presence(&session, pane_agents) || changed;
        }
        for (session, pane_id) in focused_agent_panes {
            let previous = self
                .focused_pane_by_session
                .lock()
                .unwrap()
                .insert(session.clone(), pane_id.clone());
            let seen_changed = tracker.mark_pane_seen(&session, &pane_id);
            debug_log(format!(
                "current-agent-pane-seen session={session} pane={pane_id} previous={previous:?} changed={seen_changed}",
            ));
            changed = seen_changed || changed;
        }
        changed
    }

    fn mark_focused_agent_panes_seen(&self) -> bool {
        let focused = self.focused_pane_by_session.lock().unwrap().clone();
        if focused.is_empty() {
            return false;
        }
        let mut tracker = self.agent_tracker.lock().unwrap();
        let mut changed = false;
        for (session, pane_id) in focused {
            let pane_changed = tracker.mark_pane_seen(&session, &pane_id);
            debug_log(format!(
                "focused-pane-seen-check session={session} pane={pane_id} changed={pane_changed}",
            ));
            changed = pane_changed || changed;
        }
        changed
    }

    fn observe_tmux_agent_state(&self) -> bool {
        self.sync_agent_pane_presence() || self.mark_focused_agent_panes_seen()
    }

    fn local_node_ids(&self) -> HashSet<String> {
        self.providers
            .iter()
            .map(|provider| provider.node_id().to_string())
            .collect()
    }

    fn local_node_id(&self) -> String {
        self.providers
            .first()
            .map(|provider| provider.node_id().to_string())
            .unwrap_or_else(|| "local".to_string())
    }

    fn workspace_actor_id(&self) -> String {
        std::env::var("OPENSESSIONS_SERVER_KEY")
            .ok()
            .filter(|value| !value.is_empty())
            .map(|server_key| format!("{}:{server_key}", self.local_node_id()))
            .unwrap_or_else(|| self.local_node_id())
    }

    fn workspace_ui_state(&self, state: &ServerState) -> CloudWorkspaceUiState {
        let sidebar_state = self.sidebar_coordinator.lock().unwrap().state();
        let session_order = self.session_order.lock().unwrap();
        let hidden_sessions = session_order.hidden_sessions();
        CloudWorkspaceUiState {
            sidebar_visible: Some(sidebar_state.visible),
            focused_session: self.focused_session.lock().unwrap().clone(),
            provider_filter: self.provider_filter.lock().unwrap().clone(),
            session_filter: self
                .session_filter
                .lock()
                .unwrap()
                .map(|filter| serde_json::to_value(filter).unwrap_or(Value::Null))
                .and_then(|value| value.as_str().map(str::to_string)),
            agent_panel_scope: Some(
                serde_json::to_value(*self.agent_panel_scope.lock().unwrap())
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_string))
                    .unwrap_or_else(|| "current".to_string()),
            ),
            visible_session_order: state
                .sessions
                .iter()
                .filter(|session| !hidden_sessions.contains(&session.name))
                .map(|session| session.name.clone())
                .collect(),
            hidden_sessions,
            collapsed_worktree_groups: self
                .collapsed_worktree_groups
                .lock()
                .unwrap()
                .iter()
                .cloned()
                .collect(),
            sidebar_width: Some(sidebar_state.width),
            detail_panel_height: Some(state.detail_panel_height),
            updated_by_node: Some(self.workspace_actor_id()),
            ts: *self.workspace_ui_updated_at.lock().unwrap(),
        }
    }

    fn mark_workspace_ui_updated(&self) {
        *self.workspace_ui_updated_at.lock().unwrap() = (self.now_ms)();
    }

    fn update_cloud_graph_sessions(
        &self,
        sessions: Vec<opensessions_runtime::protocol::SessionData>,
    ) -> bool {
        let mut cache = self.cloud_graph_cache.lock().unwrap();
        if cache.sessions == sessions {
            return false;
        }
        cache.sessions = sessions;
        true
    }

    fn update_cloud_workspace_cache(
        &self,
        ui_state: Option<CloudWorkspaceUiState>,
        command_intents: Vec<CloudCommandIntent>,
    ) -> bool {
        let mut cache = self.cloud_graph_cache.lock().unwrap();
        if cache.ui_state == ui_state && cache.command_intents == command_intents {
            return false;
        }
        cache.ui_state = ui_state;
        cache.command_intents = command_intents;
        true
    }

    fn apply_cloud_command_intents(
        &self,
        intents: &[CloudCommandIntent],
    ) -> Vec<(String, &'static str, String)> {
        let local_node_ids = self.local_node_ids();
        let mut results = Vec::new();
        for intent in intents {
            if !intent
                .node_id
                .as_deref()
                .is_some_and(|node_id| local_node_ids.contains(node_id))
            {
                continue;
            }
            {
                let mut cache = self.cloud_graph_cache.lock().unwrap();
                if !cache.handled_command_intents.insert(intent.id.clone()) {
                    continue;
                }
            }
            match intent.action.as_str() {
                "switchSession" => {
                    let Some(node_id) = intent.node_id.as_deref() else {
                        results.push((intent.id.clone(), "failed", "missing nodeId".to_string()));
                        continue;
                    };
                    let Some(provider_id) = intent.provider_id.as_deref() else {
                        results.push((intent.id.clone(), "failed", "missing providerId".to_string()));
                        continue;
                    };
                    let Some(session) = intent.session.as_deref() else {
                        results.push((intent.id.clone(), "failed", "missing session".to_string()));
                        continue;
                    };
                    let client_tty = intent.payload.get("clientTty").and_then(Value::as_str);
                    let return_command = self
                        .providers
                        .iter()
                        .find(|provider| provider.node_id() == node_id && provider.name() == provider_id)
                        .and_then(|provider| provider.attach_session_command(session));
                    terminate_managed_ssh_bridges(return_command.as_deref());
                    self.switch_session_on_provider(node_id, provider_id, session, client_tty);
                    results.push((intent.id.clone(), "completed", "switched".to_string()));
                }
                other => results.push((
                    intent.id.clone(),
                    "failed",
                    format!("unsupported action {other}"),
                )),
            }
        }
        results
    }

    fn apply_cloud_ui_state(&self, ui_state: &CloudWorkspaceUiState) -> Vec<ServerQueryKey> {
        let local_ui_revision = *self.workspace_ui_updated_at.lock().unwrap();
        if ui_state.ts < local_ui_revision {
            return Vec::new();
        }
        if ui_state.ts > local_ui_revision {
            *self.workspace_ui_updated_at.lock().unwrap() = ui_state.ts;
        }
        let mut changed = Vec::new();
        if let Some(provider_filter) = &ui_state.provider_filter
            && self.provider_filter.lock().unwrap().as_ref() != Some(provider_filter)
        {
            *self.provider_filter.lock().unwrap() = Some(provider_filter.clone());
            changed.extend([ServerQueryKey::Settings, ServerQueryKey::Sessions]);
        } else if ui_state.provider_filter.is_none()
            && self.provider_filter.lock().unwrap().is_some()
        {
            *self.provider_filter.lock().unwrap() = None;
            changed.extend([ServerQueryKey::Settings, ServerQueryKey::Sessions]);
        }
        if let Some(session_filter) = &ui_state.session_filter
            && let Ok(filter) =
                serde_json::from_value::<SessionFilterMode>(Value::String(session_filter.clone()))
            && *self.session_filter.lock().unwrap() != Some(filter)
        {
            *self.session_filter.lock().unwrap() = Some(filter);
            changed.extend([ServerQueryKey::Settings, ServerQueryKey::Sessions]);
        }
        if let Some(scope) = &ui_state.agent_panel_scope
            && let Some(scope) = parse_agent_panel_scope(scope)
            && *self.agent_panel_scope.lock().unwrap() != scope
        {
            *self.agent_panel_scope.lock().unwrap() = scope;
            changed.push(ServerQueryKey::Settings);
        }
        if let Some(width) = ui_state.sidebar_width {
            let width = clamp_sidebar_width(width.min(u16::MAX as u32) as u16);
            let width_changed = {
                let mut coordinator = self.sidebar_coordinator.lock().unwrap();
                if coordinator.state().width != u32::from(width) {
                    coordinator.set_width(u32::from(width));
                    true
                } else {
                    false
                }
            };
            if width_changed {
                changed.push(ServerQueryKey::SidebarLayout);
            }
            for provider in &self.providers {
                provider.set_sidebar_width_hint(width);
            }
            self.enforce_sidebar_width(width);
        }
        if let Some(height) = ui_state.detail_panel_height {
            let height = clamp_detail_panel_height(height.min(u16::MAX as u32) as u16);
            if *self.detail_panel_height.lock().unwrap() != height {
                *self.detail_panel_height.lock().unwrap() = height;
                changed.push(ServerQueryKey::SidebarLayout);
            }
        }
        {
            let mut collapsed = self.collapsed_worktree_groups.lock().unwrap();
            let incoming = ui_state
                .collapsed_worktree_groups
                .iter()
                .cloned()
                .collect::<HashSet<_>>();
            if *collapsed != incoming {
                *collapsed = incoming;
                changed.push(ServerQueryKey::SidebarLayout);
                changed.push(ServerQueryKey::Sessions);
            }
        }
        if !ui_state.visible_session_order.is_empty() {
            self.session_order
                .lock()
                .unwrap()
                .set_visible_order(ui_state.visible_session_order.clone());
            changed.push(ServerQueryKey::Sessions);
        }
        {
            let mut order = self.session_order.lock().unwrap();
            if order.hidden_sessions() != ui_state.hidden_sessions {
                order.set_hidden(ui_state.hidden_sessions.clone());
                changed.push(ServerQueryKey::Sessions);
            }
        }
        if let Some(focused) = &ui_state.focused_session
            && self.focused_session.lock().unwrap().as_ref() != Some(focused)
        {
            *self.focused_session.lock().unwrap() = Some(focused.clone());
            changed.push(ServerQueryKey::Focus);
        }
        if let Some(visible) = ui_state.sidebar_visible {
            let mut coordinator = self.sidebar_coordinator.lock().unwrap();
            if visible && !coordinator.state().visible {
                coordinator.mark_ready();
                changed.push(ServerQueryKey::SidebarLayout);
                drop(coordinator);
                let _ = self.ensure_sidebar("");
            } else if !visible && coordinator.state().visible {
                coordinator.hide();
                changed.push(ServerQueryKey::SidebarLayout);
            }
        }
        changed
    }

    fn remember_focused_pane(&self, context: &HttpContext) -> FocusObservation {
        if context.pane_active == Some(false) {
            debug_log(format!(
                "focus-pane ignored inactive session={} pane={:?}",
                context.session, context.pane_id,
            ));
            return FocusObservation::default();
        }
        let Some(pane_id) = context
            .pane_id
            .as_deref()
            .filter(|pane_id| !pane_id.is_empty())
        else {
            return FocusObservation::default();
        };
        if context.client_tty.is_some() {
            *self.focused_client_tty.lock().unwrap() = context.client_tty.clone();
        }
        let previous_pane = self
            .focused_pane_by_session
            .lock()
            .unwrap()
            .insert(context.session.clone(), pane_id.to_string());
        let changed = self
            .agent_tracker
            .lock()
            .unwrap()
            .mark_pane_seen(&context.session, pane_id);
        debug_log(format!(
            "focus-pane session={} pane={} changed={changed}",
            context.session, pane_id,
        ));
        let _pane_changed = previous_pane.as_deref() != Some(pane_id);
        FocusObservation {
            agents_changed: changed,
        }
    }

    fn snapshot_state(&self) -> ServerState {
        let providers = self
            .providers
            .iter()
            .map(|provider| provider.as_ref())
            .collect::<Vec<_>>();
        let visible_session_names = self.visible_session_names();
        let metadata_by_session = visible_session_names.as_ref().map(|names| {
            names
                .iter()
                .filter_map(|name| {
                    self.metadata_store
                        .lock()
                        .unwrap()
                        .get(name)
                        .map(|metadata| (name.clone(), metadata))
                })
                .collect()
        });
        let git_by_session = self.git_info_by_session(visible_session_names.as_deref());
        let (agent_state_by_session, agents_by_session, event_timestamps_by_session) =
            visible_session_names
                .as_ref()
                .map(|names| {
                    let tracker = self.agent_tracker.lock().unwrap();
                    let mut states = HashMap::new();
                    let mut agents = HashMap::new();
                    let mut timestamps = HashMap::new();
                    for name in names {
                        if let Some(state) = tracker.get_state(name) {
                            states.insert(name.clone(), state);
                        }
                        let session_agents = tracker.get_agents(name);
                        if !session_agents.is_empty() {
                            agents.insert(name.clone(), session_agents);
                        }
                        let session_timestamps = tracker.get_event_timestamps(name);
                        if !session_timestamps.is_empty() {
                            timestamps.insert(name.clone(), session_timestamps);
                        }
                    }
                    (Some(states), Some(agents), Some(timestamps))
                })
                .unwrap_or((None, None, None));
        let ports_by_session = self.discover_live_ports(visible_session_names.as_deref());
        let sidebar_state = self.sidebar_coordinator.lock().unwrap().state();
        debug_log(format!(
            "snapshot_state mode={} init={} width={}",
            sidebar_state.mode, sidebar_state.initializing, sidebar_state.width,
        ));
        let mut state = build_read_only_state(ReadOnlyStateInput {
            providers,
            visible_session_names,
            metadata_by_session,
            git_by_session,
            agent_state_by_session,
            agents_by_session,
            event_timestamps_by_session,
            unseen_sessions: Some(self.agent_tracker.lock().unwrap().get_unseen()),
            ports_by_session,
            portless_state: None,
            focused_session: self.focused_session.lock().unwrap().clone(),
            current_session_override: None,
            theme: self.theme.lock().unwrap().clone(),
            session_filter: *self.session_filter.lock().unwrap(),
            agent_panel_scope: *self.agent_panel_scope.lock().unwrap(),
            collapsed_worktree_groups: self
                .collapsed_worktree_groups
                .lock()
                .unwrap()
                .iter()
                .cloned()
                .collect(),
            sidebar_width: sidebar_state.width,
            detail_panel_height: u32::from(*self.detail_panel_height.lock().unwrap()),
            initializing: sidebar_state.initializing,
            init_label: (!sidebar_state.init_label.is_empty()).then_some(sidebar_state.init_label),
            now_ms: (self.now_ms)(),
        });
        state
            .sessions
            .extend(self.cloud_graph_cache.lock().unwrap().sessions.clone());
        apply_global_session_order(&mut state.sessions, &self.session_order.lock().unwrap());
        state
    }

    fn query_result_json_for_key(&self, key: ServerQueryKey) -> String {
        loop {
            let generation = {
                let cache = self.query_json_cache.lock().unwrap();
                if let Some(cached) = cache
                    .results_by_key
                    .get(&key)
                    .filter(|cached| cached.generation == cache.generation)
                {
                    return cached.json.clone();
                }
                cache.generation
            };

            let json = self.compute_query_result_json(key);
            let mut cache = self.query_json_cache.lock().unwrap();
            if cache.generation == generation {
                cache.results_by_key.insert(
                    key,
                    CachedQueryJson {
                        generation,
                        json: json.clone(),
                    },
                );
                return json;
            }
        }
    }

    fn compute_query_result_json(&self, key: ServerQueryKey) -> String {
        let ts = current_time_ms();
        match key {
            ServerQueryKey::Sessions => {
                query_result_json_from_state(key, self.snapshot_state(), ts)
            }
            ServerQueryKey::Agents => query_result_json_from_state(key, self.snapshot_state(), ts),
            ServerQueryKey::Focus => query_result_json_from_data(
                key,
                ServerQueryData::Focus {
                    focused_session: self.focused_session.lock().unwrap().clone(),
                    current_session: self
                        .providers
                        .first()
                        .and_then(|provider| provider.get_current_session()),
                },
                ts,
            ),
            ServerQueryKey::SidebarLayout => {
                let sidebar_state = self.sidebar_coordinator.lock().unwrap().state();
                query_result_json_from_data(
                    key,
                    ServerQueryData::SidebarLayout {
                        sidebar_width: sidebar_state.width,
                        detail_panel_height: u32::from(*self.detail_panel_height.lock().unwrap()),
                        initializing: sidebar_state.initializing,
                        init_label: (!sidebar_state.init_label.is_empty())
                            .then_some(sidebar_state.init_label),
                        collapsed_worktree_groups: self
                            .collapsed_worktree_groups
                            .lock()
                            .unwrap()
                            .iter()
                            .cloned()
                            .collect(),
                    },
                    ts,
                )
            }
            ServerQueryKey::Settings => query_result_json_from_data(
                key,
                ServerQueryData::Settings {
                    theme: self.theme.lock().unwrap().clone(),
                    session_filter: *self.session_filter.lock().unwrap(),
                    provider_filter: self.provider_filter.lock().unwrap().clone(),
                    agent_panel_scope: *self.agent_panel_scope.lock().unwrap(),
                },
                ts,
            ),
        }
    }

    fn invalidate_cached_query_json(&self, _keys: &[ServerQueryKey]) {
        let mut cache = self.query_json_cache.lock().unwrap();
        cache.generation = cache.generation.wrapping_add(1);
        cache.results_by_key.clear();
    }

    fn debug_agent_diagnostics_json(&self, only_session: Option<&str>) -> String {
        let focused_session = self.focused_session.lock().unwrap().clone();
        let current_session = self
            .providers
            .first()
            .and_then(|provider| provider.get_current_session());
        let agent_panel_scope = *self.agent_panel_scope.lock().unwrap();
        let tracker = self.agent_tracker.lock().unwrap();
        let sessions = self
            .sorted_session_names()
            .into_iter()
            .filter(|session| only_session.is_none_or(|only| only == session))
            .map(|session| {
                let pane_candidates = self
                    .providers
                    .iter()
                    .flat_map(|provider| provider.list_agent_pane_diagnostics(&session))
                    .collect::<Vec<_>>();
                let tracker_agents = tracker.get_agents(&session);
                let projected_current_panel_count = focused_session
                    .as_deref()
                    .filter(|focused| *focused == session)
                    .map(|_| tracker_agents.len())
                    .unwrap_or(0);
                AgentSessionDiagnostic {
                    node_id: "local".to_string(),
                    provider_id: "tmux".to_string(),
                    focused: focused_session.as_deref() == Some(session.as_str()),
                    current: current_session.as_deref() == Some(session.as_str()),
                    tracker_agent_state: tracker.get_state(&session),
                    tracker_agents,
                    projected_current_panel_count,
                    pane_candidates,
                    session,
                }
            })
            .collect();
        serde_json::to_string_pretty(&AgentDiagnostics {
            focused_session,
            current_session,
            agent_panel_scope,
            sessions,
        })
        .expect("agent diagnostics must serialize")
    }
}

impl StateSource for ReadOnlyMuxStateSource {
    fn setup_mux_hooks(&self, server_host: &str, server_port: u16) {
        let width = self.current_sidebar_width_u16();
        for provider in &self.providers {
            provider.set_sidebar_width_hint(width);
            provider.setup_hooks(server_host, server_port);
        }
    }

    fn cleanup_mux_hooks(&self) {
        for provider in &self.providers {
            provider.cleanup_hooks();
        }
    }

    fn start_background_tasks(
        self: Arc<Self>,
        state_updates: broadcast::Sender<String>,
        shutdown: broadcast::Sender<()>,
    ) -> Vec<JoinHandle<()>> {
        let mut tasks = Vec::new();
        if ENABLE_JSONL_AGENT_WATCHERS {
            tasks.push(tokio::spawn(run_agent_watcher_loop(
                self.clone(),
                state_updates.clone(),
                shutdown.clone(),
            )));
        }
        tasks.extend([
            tokio::spawn(run_sidebar_lifecycle_loop(
                self.clone(),
                state_updates.clone(),
                shutdown.clone(),
            )),
            tokio::spawn(run_tmux_state_poll_loop(self, state_updates, shutdown)),
        ]);
        tasks
    }

    fn query_result_json(&self, key: ServerQueryKey) -> Option<String> {
        Some(self.query_result_json_for_key(key))
    }

    fn invalidate_queries_json(&self, keys: Vec<ServerQueryKey>) -> String {
        self.invalidate_cached_query_json(&keys);
        invalidate_queries_json(keys)
    }

    fn invalidate_all_queries_json(&self) -> String {
        self.invalidate_queries_json(all_query_keys())
    }

    fn debug_agents_json(&self, session: Option<&str>) -> Option<String> {
        Some(self.debug_agent_diagnostics_json(session))
    }

    fn begin_shutdown(&self) -> Option<String> {
        {
            let mut coordinator = self.sidebar_coordinator.lock().unwrap();
            coordinator.begin_closing();
        }
        Some(self.invalidate_all_queries_json())
    }

    fn handle_client_command(&self, command: &Value) -> Option<String> {
        self.handle_client_command_with_context(command, None)
    }

    fn handle_client_command_with_context(
        &self,
        command: &Value,
        context: Option<&ClientConnectionContext>,
    ) -> Option<String> {
        let provider = self.providers.first()?;
        match command.get("type").and_then(Value::as_str)? {
            "new-session" => {
                provider.create_session(None, None);
                Some(self.invalidate_all_queries_json())
            }
            "switch-session" => {
                let name = command.get("name")?.as_str()?;
                let node_id = command
                    .get("nodeId")
                    .and_then(Value::as_str)
                    .unwrap_or("local");
                let provider_id = command
                    .get("providerId")
                    .and_then(Value::as_str)
                    .unwrap_or("tmux");
                let client_tty = command
                    .get("clientTty")
                    .and_then(Value::as_str)
                    .or_else(|| context.and_then(|context| context.client_tty.as_deref()));
                self.switch_session_on_provider(node_id, provider_id, name, client_tty);
                None
            }
            "register-ssh-node" => {
                let node = ssh_node_config_from_value(command)?;
                persist_ssh_node(&node).ok()?;
                Some(self.invalidate_queries_json(vec![ServerQueryKey::Settings]))
            }
            "deregister-ssh-node" => {
                let node_id = command.get("nodeId")?.as_str()?;
                remove_ssh_node(node_id).ok()?;
                Some(self.invalidate_queries_json(vec![ServerQueryKey::Settings]))
            }
            "switch-index" => {
                let index = command.get("index")?.as_u64()?.min(u32::MAX as u64) as u32;
                self.switch_visible_index(index, None)
            }
            "kill-session" => {
                let name = command.get("name")?.as_str()?;
                if provider.get_current_session().as_deref() == Some(name)
                    && let Some(next) = self
                        .session_before(name)
                        .or_else(|| self.session_after(name))
                {
                    provider.switch_session(&next, None);
                    *self.focused_session.lock().unwrap() = Some(next);
                }
                provider.kill_session(name);
                Some(self.invalidate_all_queries_json())
            }
            "hide-session" => {
                let name = command.get("name")?.as_str()?;
                self.session_order.lock().unwrap().hide(name);
                self.mark_workspace_ui_updated();
                Some(self.invalidate_all_queries_json())
            }
            "show-all-sessions" => {
                self.session_order.lock().unwrap().show_all();
                self.mark_workspace_ui_updated();
                Some(self.invalidate_all_queries_json())
            }
            "reorder-session" => {
                let name = command.get("name")?.as_str()?;
                let delta = command.get("delta")?.as_i64()? as i8;
                if let Some(names) = self.sidebar_reordered_session_names(name, delta) {
                    self.session_order.lock().unwrap().set_visible_order(names);
                    self.mark_workspace_ui_updated();
                }
                Some(self.invalidate_all_queries_json())
            }
            "reorder-worktree-group" => {
                let key = command.get("key")?.as_str()?;
                let delta = command.get("delta")?.as_i64()? as i8;
                if let Some(names) = self.sidebar_reordered_worktree_group_names(key, delta) {
                    self.session_order.lock().unwrap().set_visible_order(names);
                    self.mark_workspace_ui_updated();
                }
                Some(self.invalidate_all_queries_json())
            }
            "set-theme" => {
                let theme = command.get("theme")?.as_str()?.to_string();
                *self.theme.lock().unwrap() = Some(theme);
                Some(self.invalidate_queries_json(vec![ServerQueryKey::Settings]))
            }
            "set-sidebar-width" => {
                let width = command.get("width")?.as_u64()?.min(u16::MAX as u64) as u16;
                let width = clamp_sidebar_width(width);
                self.persist_sidebar_width(width);
                self.sidebar_coordinator
                    .lock()
                    .unwrap()
                    .set_width(u32::from(width));
                for provider in &self.providers {
                    provider.set_sidebar_width_hint(width);
                }
                self.enforce_sidebar_width(width);
                self.mark_workspace_ui_updated();
                Some(self.invalidate_queries_json(vec![ServerQueryKey::SidebarLayout]))
            }
            "set-detail-panel-height" => {
                let height = command.get("height")?.as_u64()?.min(u16::MAX as u64) as u16;
                let height = clamp_detail_panel_height(height);
                *self.detail_panel_height.lock().unwrap() = height;
                self.persist_detail_panel_height(height);
                self.mark_workspace_ui_updated();
                Some(self.invalidate_queries_json(vec![ServerQueryKey::SidebarLayout]))
            }
            "set-agent-panel-scope" => {
                let scope = parse_agent_panel_scope(command.get("scope")?.as_str()?)?;
                *self.agent_panel_scope.lock().unwrap() = scope;
                self.mark_workspace_ui_updated();
                Some(self.invalidate_queries_json(vec![ServerQueryKey::Settings]))
            }
            "set-provider-filter" => {
                let provider = command
                    .get("provider")
                    .and_then(Value::as_str)
                    .filter(|provider| !provider.is_empty())
                    .map(str::to_string);
                *self.provider_filter.lock().unwrap() = provider;
                self.mark_workspace_ui_updated();
                Some(self.invalidate_queries_json(vec![
                    ServerQueryKey::Sessions,
                    ServerQueryKey::Settings,
                ]))
            }
            "set-ui-focus" => {
                let client_tty = context
                    .and_then(|context| context.client_tty.as_deref())
                    .filter(|client_tty| !client_tty.is_empty())?;
                let focus: ClientUiFocus =
                    serde_json::from_value(command.get("focus")?.clone()).ok()?;
                let mut focuses = self.ui_focus_by_client_tty.lock().unwrap();
                if focuses.get(client_tty) == Some(&focus) {
                    return None;
                }
                focuses.insert(client_tty.to_string(), focus.clone());
                Some(ui_focus_json(client_tty.to_string(), focus))
            }
            "repair-width" => {
                if self.is_sidebar_visible() {
                    let width = self.current_sidebar_width_u16();
                    if !self.repair_context_sidebar_width(context, width) {
                        self.enforce_sidebar_width(width);
                    }
                }
                None
            }
            "set-filter" => {
                let filter = match command.get("filter")?.as_str()? {
                    "all" => SessionFilterMode::All,
                    "active" => SessionFilterMode::Active,
                    "running" => SessionFilterMode::Running,
                    _ => return None,
                };
                *self.session_filter.lock().unwrap() = Some(filter);
                self.mark_workspace_ui_updated();
                Some(self.invalidate_queries_json(vec![ServerQueryKey::Settings]))
            }
            "toggle-worktree-group" => {
                let key = command.get("key")?.as_str()?.to_string();
                let mut collapsed = self.collapsed_worktree_groups.lock().unwrap();
                if !collapsed.insert(key) {
                    collapsed.remove(command.get("key")?.as_str()?);
                }
                drop(collapsed);
                self.mark_workspace_ui_updated();
                Some(self.invalidate_queries_json(vec![ServerQueryKey::SidebarLayout]))
            }
            "focus-agent-pane" => {
                let session = command.get("session")?.as_str()?;
                let agent = command.get("agent")?.as_str()?;
                let thread_id = command.get("threadId").and_then(Value::as_str);
                let thread_name = command.get("threadName").and_then(Value::as_str);
                let pane_id = command.get("paneId").and_then(Value::as_str);
                let mut seen_changed = self
                    .agent_tracker
                    .lock()
                    .unwrap()
                    .mark_agent_seen(session, agent, thread_id, pane_id);
                if let Some((provider, pane_id)) =
                    self.resolve_agent_pane(session, agent, thread_id, thread_name, pane_id)
                {
                    seen_changed = self.agent_tracker.lock().unwrap().mark_agent_seen(
                        session,
                        agent,
                        thread_id,
                        Some(&pane_id),
                    ) || seen_changed;
                    provider.focus_pane(&pane_id);
                }
                seen_changed.then(|| self.invalidate_queries_json(vec![ServerQueryKey::Agents]))
            }
            "kill-agent-pane" => {
                let session = command.get("session")?.as_str()?;
                let agent = command.get("agent")?.as_str()?;
                let thread_id = command.get("threadId").and_then(Value::as_str);
                let thread_name = command.get("threadName").and_then(Value::as_str);
                let pane_id = command.get("paneId").and_then(Value::as_str);
                if let Some((provider, pane_id)) =
                    self.resolve_agent_pane(session, agent, thread_id, thread_name, pane_id)
                {
                    provider.kill_pane(&pane_id);
                }
                None
            }
            _ => None,
        }
    }

    fn handle_sender_command(&self, command: &Value) -> Option<SenderCommandOutcome> {
        self.handle_sender_command_with_context(command, &mut ClientConnectionContext::default())
    }

    fn handle_sender_command_with_context(
        &self,
        command: &Value,
        context: &mut ClientConnectionContext,
    ) -> Option<SenderCommandOutcome> {
        if command.get("type").and_then(Value::as_str)? != "identify-pane" {
            return None;
        }
        let session_name = command.get("sessionName")?.as_str()?;
        if session_name == "_os_stash" {
            return None;
        }
        context.pane_id = command
            .get("paneId")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        context.session_name = Some(session_name.to_string());
        context.window_id = command
            .get("windowId")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        debug_log(format!(
            "identify-pane session={:?} pane={:?} window={:?} -> acknowledge_sidebar_connected",
            context.session_name, context.pane_id, context.window_id,
        ));
        let lifecycle_changed = self
            .sidebar_coordinator
            .lock()
            .unwrap()
            .acknowledge_sidebar_window_connected(context.window_id.as_deref());
        if let Some(window_id) = context.window_id.as_deref() {
            for provider in &self.providers {
                provider.prepare_sidebar_window(window_id);
            }
        }
        if self.is_sidebar_visible() {
            let width = self.current_sidebar_width_u16();
            if !self.repair_context_sidebar_width(Some(context), width) {
                self.enforce_sidebar_width(width);
            }
        }
        let client_tty = self.providers.first()?.get_client_tty();
        let client_tty = (!client_tty.is_empty()).then_some(client_tty);
        context.client_tty = client_tty.clone();
        let ui_focus = client_tty.as_deref().and_then(|client_tty| {
            self.ui_focus_by_client_tty
                .lock()
                .unwrap()
                .get(client_tty)
                .cloned()
        });
        let reply = serde_json::to_string(&ServerMessage::YourSession {
            name: session_name.to_string(),
            client_tty,
            ui_focus,
        })
        .ok();
        Some(SenderCommandOutcome {
            reply,
            broadcast: lifecycle_changed
                .then(|| self.invalidate_queries_json(vec![ServerQueryKey::SidebarLayout])),
        })
    }

    fn handle_http_json(&self, path: &str, body: &Value) -> Option<String> {
        match path {
            "/api/ssh-nodes/register" => {
                let node = ssh_node_config_from_value(body)?;
                persist_ssh_node(&node).ok()?;
                return Some(self.invalidate_queries_json(vec![ServerQueryKey::Settings]));
            }
            "/api/ssh-nodes/deregister" => {
                let node_id = body.get("nodeId")?.as_str()?;
                remove_ssh_node(node_id).ok()?;
                return Some(self.invalidate_queries_json(vec![ServerQueryKey::Settings]));
            }
            "/set-status" => {
                let session = body.get("session")?.as_str()?;
                let tone = body
                    .get("tone")
                    .and_then(Value::as_str)
                    .and_then(parse_metadata_tone);
                match body.get("text") {
                    Some(Value::String(text)) => self
                        .metadata_store
                        .lock()
                        .unwrap()
                        .set_status(session, Some((text.clone(), tone))),
                    Some(Value::Null) | None => self
                        .metadata_store
                        .lock()
                        .unwrap()
                        .set_status(session, None),
                    _ => return None,
                }
            }
            "/set-progress" => {
                let session = body.get("session")?.as_str()?;
                if body.get("clear").and_then(Value::as_bool).unwrap_or(false) {
                    self.metadata_store
                        .lock()
                        .unwrap()
                        .set_progress(session, None);
                } else {
                    self.metadata_store.lock().unwrap().set_progress(
                        session,
                        Some((
                            body.get("current").and_then(Value::as_u64),
                            body.get("total").and_then(Value::as_u64),
                            body.get("percent").and_then(Value::as_f64),
                            body.get("label")
                                .and_then(Value::as_str)
                                .map(ToString::to_string),
                        )),
                    );
                }
            }
            "/log" | "/notify" => {
                let session = body.get("session")?.as_str()?;
                let message = body.get("message")?.as_str()?.to_string();
                let tone = body
                    .get("tone")
                    .and_then(Value::as_str)
                    .and_then(parse_metadata_tone);
                let source = body
                    .get("source")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                self.metadata_store
                    .lock()
                    .unwrap()
                    .append_log(session, message, tone, source);
            }
            "/clear-log" => {
                let session = body.get("session")?.as_str()?;
                self.metadata_store.lock().unwrap().clear_logs(session);
            }
            _ => return None,
        }
        Some(self.invalidate_queries_json(vec![ServerQueryKey::Sessions]))
    }

    fn handle_agent_event_json(&self, body: &Value) -> Result<String, AgentEventError> {
        self.apply_agent_event(body)?;
        Ok(self.invalidate_queries_json(vec![ServerQueryKey::Agents]))
    }

    fn handle_pi_runtime_upsert(&self, body: &Value) -> Result<(), PiRuntimeError> {
        let info =
            parse_pi_runtime_info(body, (self.now_ms)()).ok_or(PiRuntimeError::InvalidPayload)?;
        self.pi_runtime_registry.lock().unwrap().upsert(info);
        Ok(())
    }

    fn handle_pi_runtime_delete(&self, body: &Value) -> Result<(), PiRuntimeError> {
        let pid = body
            .get("pid")
            .and_then(Value::as_u64)
            .filter(|pid| *pid > 0 && *pid <= u32::MAX as u64)
            .ok_or(PiRuntimeError::MissingPid)? as u32;
        self.pi_runtime_registry.lock().unwrap().delete(pid);
        Ok(())
    }

    fn handle_http_text(&self, path: &str, body: &str) -> Option<String> {
        if path != "/focus" {
            return None;
        }
        let context = parse_context(body)?;
        let name = context.session.clone();
        let current_session = self
            .providers
            .iter()
            .find_map(|provider| provider.get_current_session());
        if current_session.as_deref() != Some(name.as_str()) {
            let observation = self.remember_focused_pane(&context);
            return observation
                .agents_changed
                .then(|| self.invalidate_queries_json(vec![ServerQueryKey::Agents]));
        }
        let previous_session = self.focused_session.lock().unwrap().replace(name.clone());
        let observation = self.remember_focused_pane(&context);
        let mut keys = Vec::new();
        if previous_session.as_deref() != Some(name.as_str()) {
            keys.push(ServerQueryKey::Focus);
        }
        if observation.agents_changed {
            keys.push(ServerQueryKey::Agents);
        }
        (!keys.is_empty()).then(|| self.invalidate_queries_json(keys))
    }

    fn handle_http_hook(&self, path: &str, body: &str) -> Option<String> {
        match path {
            "/toggle" => {
                self.toggle_sidebar(body);
                self.mark_workspace_ui_updated();
                Some(self.invalidate_queries_json(vec![ServerQueryKey::SidebarLayout]))
            }
            "/ensure-sidebar" => {
                let spawned = self.ensure_sidebar(body);
                parse_context_session(body)
                    .map(|name| activate_session_json(name, None))
                    .or_else(|| spawned.then(|| self.invalidate_all_queries_json()))
            }
            "/pane-exited" => {
                let display_names = self.sidebar_display_session_names().unwrap_or_default();
                let sidebar_sessions = self
                    .providers
                    .iter()
                    .flat_map(|provider| provider.list_sidebar_panes(None))
                    .map(|pane| pane.session_name)
                    .collect::<HashSet<_>>();
                let fallback_sessions =
                    fallback_sessions_for_sidebar_sessions(&display_names, sidebar_sessions);
                for provider in &self.providers {
                    provider.kill_orphaned_sidebar_panes_with_fallbacks(&fallback_sessions);
                }
                None
            }
            "/pane-layout-changed" | "/client-resized" => {
                if self.is_sidebar_visible() {
                    self.enforce_sidebar_width(self.current_sidebar_width_u16());
                }
                None
            }
            _ => None,
        }
    }

    fn handle_switch_index(&self, index: u32, body: &str) -> Option<String> {
        let client_tty = parse_context(body).and_then(|context| context.client_tty);
        self.switch_visible_index(index, client_tty.as_deref())
    }
}

fn fallback_sessions_for_sidebar_sessions(
    display_names: &[String],
    sidebar_sessions: HashSet<String>,
) -> HashMap<String, String> {
    sidebar_sessions
        .into_iter()
        .filter_map(|session| {
            let index = display_names.iter().position(|name| name == &session)?;
            let fallback = index
                .checked_sub(1)
                .and_then(|idx| display_names.get(idx))
                .or_else(|| display_names.get(index + 1))
                .cloned()?;
            Some((session, fallback))
        })
        .collect()
}

impl ReadOnlyMuxStateSource {
    fn apply_agent_event(&self, body: &Value) -> Result<(), AgentEventError> {
        let agent = body
            .get("agent")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or(AgentEventError::MissingAgent)?;
        let status = body
            .get("status")
            .and_then(Value::as_str)
            .and_then(parse_agent_status)
            .ok_or(AgentEventError::InvalidStatus)?;
        let session = self
            .resolve_agent_event_session(body)
            .ok_or(AgentEventError::CouldNotResolveSession)?;
        let ts = body
            .get("ts")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| (self.now_ms)());
        let pane_id = body
            .get("paneId")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let event_pane_id = pane_id.clone();
        let event_session = session.clone();
        self.agent_tracker.lock().unwrap().apply_event(AgentEvent {
            agent,
            node_id: "local".to_string(),
            provider_id: "tmux".to_string(),
            session,
            status,
            ts,
            thread_id: body
                .get("threadId")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            thread_name: body
                .get("threadName")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            last_user_prompt: body
                .get("lastUserPrompt")
                .or_else(|| body.get("last_user_prompt"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
            unseen: None,
            liveness: pane_id.as_ref().map(|_| AgentLiveness::Alive),
            pane_id,
        });
        if let Some(pane_id) = event_pane_id
            && self.is_focused_pane(&event_session, &pane_id)
        {
            debug_log(format!(
                "agent-event-focused-pane session={} pane={} -> mark seen",
                event_session, pane_id,
            ));
            self.agent_tracker
                .lock()
                .unwrap()
                .mark_pane_seen(&event_session, &pane_id);
        }
        Ok(())
    }

    fn is_focused_pane(&self, session: &str, pane_id: &str) -> bool {
        if self
            .focused_pane_by_session
            .lock()
            .unwrap()
            .get(session)
            .is_some_and(|focused_pane| focused_pane == pane_id)
        {
            return true;
        }
        let focused_client_tty = self.focused_client_tty.lock().unwrap().clone();
        self.providers.iter().any(|provider| {
            provider
                .get_client_focus(focused_client_tty.as_deref())
                .is_some_and(|focus| focus.session_name == session && focus.pane_id == pane_id)
        })
    }

    fn apply_agent_watcher_snapshot(&self, snapshot: AgentWatcherSnapshot) -> bool {
        if snapshot.status == AgentStatus::Idle {
            debug_log(format!(
                "watcher-snapshot ignored idle agent={} thread_id={:?} thread_name={:?} project_dir={:?}",
                snapshot.agent, snapshot.thread_id, snapshot.thread_name, snapshot.project_dir,
            ));
            return false;
        }
        let Some(session) = self.resolve_agent_watcher_session(&snapshot) else {
            debug_log(format!(
                "watcher-snapshot unresolved agent={} status={:?} thread_id={:?} thread_name={:?} project_dir={:?}",
                snapshot.agent,
                snapshot.status,
                snapshot.thread_id,
                snapshot.thread_name,
                snapshot.project_dir,
            ));
            return false;
        };
        let focused_pane = self
            .focused_pane_by_session
            .lock()
            .unwrap()
            .get(&session)
            .cloned();
        debug_log(format!(
            "watcher-snapshot applying session={} focused_pane={:?} agent={} status={:?} thread_id={:?} thread_name={:?} project_dir={:?}",
            session,
            focused_pane,
            snapshot.agent,
            snapshot.status,
            snapshot.thread_id,
            snapshot.thread_name,
            snapshot.project_dir,
        ));
        let event = AgentEvent {
            agent: snapshot.agent.to_string(),
            node_id: "local".to_string(),
            provider_id: "tmux".to_string(),
            session: session.clone(),
            status: snapshot.status,
            ts: snapshot.ts,
            thread_id: snapshot.thread_id.clone(),
            thread_name: snapshot.thread_name.clone(),
            last_user_prompt: snapshot.last_user_prompt.clone(),
            unseen: None,
            pane_id: None,
            liveness: None,
        };
        self.agent_tracker.lock().unwrap().apply_event(event);
        if let Some(pane_id) = focused_pane {
            let changed = self
                .agent_tracker
                .lock()
                .unwrap()
                .mark_pane_seen(&session, &pane_id);
            debug_log(format!(
                "watcher-snapshot-focused-pane-seen session={} pane={} agent={} thread_id={:?} thread_name={:?} changed={changed}",
                session, pane_id, snapshot.agent, snapshot.thread_id, snapshot.thread_name,
            ));
        }
        true
    }

    fn resolve_agent_watcher_session(&self, snapshot: &AgentWatcherSnapshot) -> Option<String> {
        let sessions = self
            .providers
            .iter()
            .flat_map(|provider| provider.list_sessions())
            .collect::<Vec<_>>();
        let project_dir = snapshot.project_dir.as_deref()?;

        if let Some(encoded) = project_dir.strip_prefix("__encoded__:") {
            return sessions
                .iter()
                .find(|session| encode_agent_project_dir(&session.dir) == encoded)
                .map(|session| session.name.clone());
        }

        let dir_session_map = build_dir_session_map(
            sessions
                .into_iter()
                .map(|session| (session.name, session.dir)),
        );
        resolve_session_for_project_dir(project_dir, &dir_session_map)
    }

    fn resolve_agent_event_session(&self, body: &Value) -> Option<String> {
        let sessions = self
            .providers
            .iter()
            .flat_map(|provider| provider.list_sessions())
            .collect::<Vec<_>>();

        if let Some(project_dir) = body.get("projectDir").and_then(Value::as_str) {
            let dir_session_map = build_dir_session_map(
                sessions
                    .iter()
                    .map(|session| (session.name.clone(), session.dir.clone())),
            );
            if let Some(session) = resolve_session_for_project_dir(project_dir, &dir_session_map) {
                return Some(session);
            }
        }

        body.get("tmuxSession")
            .and_then(Value::as_str)
            .filter(|tmux_session| sessions.iter().any(|session| session.name == *tmux_session))
            .map(ToString::to_string)
    }

    fn resolve_agent_pane(
        &self,
        session: &str,
        agent: &str,
        thread_id: Option<&str>,
        thread_name: Option<&str>,
        pane_id: Option<&str>,
    ) -> Option<(Arc<dyn MuxProvider>, String)> {
        let provider = self.provider_for_session(session)?;
        if let Some(pane_id) = pane_id {
            return Some((provider, pane_id.to_string()));
        }
        self.sync_agent_pane_presence();
        if let Some(pane_id) = self.resolve_tracked_agent_pane(session, agent, thread_id) {
            return Some((provider, pane_id));
        }
        let pane_id = provider.resolve_agent_pane_id(session, agent, thread_id, thread_name)?;
        Some((provider, pane_id))
    }

    fn resolve_tracked_agent_pane(
        &self,
        session: &str,
        agent: &str,
        thread_id: Option<&str>,
    ) -> Option<String> {
        let thread_id = thread_id?;
        self.agent_tracker
            .lock()
            .unwrap()
            .get_agents(session)
            .into_iter()
            .find(|event| {
                event.agent == agent
                    && event.thread_id.as_deref() == Some(thread_id)
                    && event.liveness == Some(AgentLiveness::Alive)
                    && event.pane_id.is_some()
            })
            .and_then(|event| event.pane_id)
    }

    fn sidebar_panes_to_resize(&self, width: u16) -> Vec<String> {
        let mut pane_ids = Vec::new();
        for provider in &self.providers {
            if !provider.is_sidebar_capable() {
                continue;
            }
            for pane in provider.list_sidebar_panes(None) {
                if pane.width == Some(width) {
                    continue;
                }
                pane_ids.push(pane.pane_id);
            }
        }
        pane_ids.reverse();
        pane_ids
    }

    fn repair_context_sidebar_width(
        &self,
        context: Option<&ClientConnectionContext>,
        width: u16,
    ) -> bool {
        let Some(pane_id) = context.and_then(|context| context.pane_id.as_deref()) else {
            return false;
        };
        debug_log(format!(
            "width-repair: resize context pane={pane_id} to={width}"
        ));
        for provider in &self.providers {
            provider.resize_sidebar_pane(pane_id, width);
        }
        true
    }

    fn enforce_sidebar_width(&self, width: u16) {
        let panes = self.sidebar_panes_to_resize(width);
        for pane_id in panes {
            debug_log(format!("width-repair: resize pane={pane_id} to={width}",));
            for provider in &self.providers {
                provider.resize_sidebar_pane(&pane_id, width);
            }
        }
    }

    fn provider_for_session(&self, session: &str) -> Option<Arc<dyn MuxProvider>> {
        self.providers
            .iter()
            .find(|provider| {
                provider
                    .list_sessions()
                    .iter()
                    .any(|mux_session| mux_session.name == session)
            })
            .cloned()
            .or_else(|| self.providers.first().cloned())
    }

    fn git_info_by_session(
        &self,
        visible_session_names: Option<&[String]>,
    ) -> Option<HashMap<String, GitInfo>> {
        let visible =
            visible_session_names.map(|names| names.iter().cloned().collect::<HashSet<_>>());
        let mut git_by_session = HashMap::new();
        for provider in &self.providers {
            for session in provider.list_sessions() {
                if visible
                    .as_ref()
                    .is_some_and(|visible| !visible.contains(&session.name))
                {
                    continue;
                }
                git_by_session.insert(session.name, self.git_info_for_dir(&session.dir));
            }
        }
        Some(git_by_session)
    }

    fn git_info_for_dir(&self, dir: &str) -> GitInfo {
        if dir.is_empty() {
            return GitInfo::empty();
        }

        let now = (self.now_ms)();
        if let Some(cached) = self.git_info_cache.lock().unwrap().get(dir).cloned()
            && now.saturating_sub(cached.ts) < GIT_CACHE_TTL_MS
        {
            return cached.info;
        }

        let output = self.git_command_runner.git_info_output(dir);
        if output.is_empty() {
            return GitInfo::empty();
        }
        let info = parse_git_info_output(&output);
        self.git_info_cache.lock().unwrap().insert(
            dir.to_string(),
            CachedGitInfo {
                info: info.clone(),
                ts: now,
            },
        );
        info
    }

    fn discover_live_ports(
        &self,
        visible_session_names: Option<&[String]>,
    ) -> Option<HashMap<String, Vec<u16>>> {
        let session_names = visible_session_names
            .map(|names| names.to_vec())
            .unwrap_or_else(|| self.sorted_session_names());
        let now = (self.now_ms)();
        if let Some(cached) = self.port_snapshot_cache.lock().unwrap().clone()
            && cached.session_names == session_names
            && now.saturating_sub(cached.ts) < PORT_POLL_INTERVAL_MS
        {
            return Some(cached.ports_by_session);
        }

        if session_names.is_empty() {
            return Some(HashMap::new());
        }

        let session_filter = session_names.iter().cloned().collect::<HashSet<_>>();
        let mut pane_pids_by_session = HashMap::new();
        for provider in &self.providers {
            for session in provider.list_sessions() {
                if !session_filter.contains(&session.name) {
                    continue;
                }
                let pids = provider.get_session_pane_pids(&session.name);
                if !pids.is_empty() {
                    pane_pids_by_session.insert(session.name, pids);
                }
            }
        }

        if pane_pids_by_session.is_empty() {
            return Some(discover_session_ports(PortDiscoveryInput {
                session_names,
                pane_pids_by_session,
                process_rows: Vec::new(),
                lsof_fields: "",
            }));
        }

        let lsof_fields = self.port_command_runner.lsof_fields();
        let cache_session_names = session_names.clone();
        let ports_by_session = discover_session_ports(PortDiscoveryInput {
            session_names,
            pane_pids_by_session,
            process_rows: self.port_command_runner.process_rows(),
            lsof_fields: &lsof_fields,
        });
        self.port_snapshot_cache
            .lock()
            .unwrap()
            .replace(CachedPortSnapshot {
                session_names: cache_session_names,
                ports_by_session: ports_by_session.clone(),
                ts: now,
            });
        Some(ports_by_session)
    }

    fn toggle_sidebar(&self, body: &str) {
        let context = parse_context(body);
        let providers = self
            .providers
            .iter()
            .filter(|provider| provider.is_full_sidebar_capable())
            .cloned()
            .collect::<Vec<_>>();
        let panes_by_provider = providers
            .iter()
            .map(|provider| (provider.clone(), provider.list_sidebar_panes(None)))
            .collect::<Vec<_>>();

        if panes_by_provider.iter().any(|(_, panes)| !panes.is_empty()) {
            for (provider, panes) in panes_by_provider {
                for pane in panes {
                    provider.hide_sidebar(&pane.pane_id);
                }
            }
            self.sidebar_coordinator.lock().unwrap().hide();
            return;
        }

        let warmup_until = (self.now_ms)().saturating_add(SIDEBAR_WARMUP_MS);
        let width = self.current_sidebar_width_u16();
        for provider in providers {
            let windows = sidebar_launch_plan(provider.as_ref(), context.as_ref());
            self.sidebar_coordinator
                .lock()
                .unwrap()
                .begin_warmup_for_windows(
                    windows.iter().map(|window| window.id.clone()),
                    warmup_until,
                );

            let Some((first, rest)) = windows.split_first() else {
                continue;
            };
            spawn_sidebar_window(provider.as_ref(), first, width, "toggle_sidebar: immediate");
            if !rest.is_empty() {
                spawn_staggered_sidebars(
                    provider,
                    rest.to_vec(),
                    width,
                    "toggle_sidebar: staggered",
                );
            }
        }
    }

    fn ensure_sidebar(&self, body: &str) -> bool {
        let context = parse_context(body);
        let width = self.current_sidebar_width_u16();
        if !self.is_sidebar_visible() {
            debug_log("ensure_sidebar: ignored spawn while sidebar is hidden");
            return false;
        }
        // A window switch / new window can make tmux proportionally redistribute
        // panes in that window, so repair existing sidebars before spawning any
        // missing ones. This is event-driven, not a per-tick width scan.
        self.enforce_sidebar_width(width);
        let mut spawned = false;
        for provider in &self.providers {
            if !provider.is_full_sidebar_capable() {
                continue;
            }
            let session_name = context
                .as_ref()
                .map(|context| context.session.clone())
                .or_else(|| provider.get_current_session());
            let window_id = context
                .as_ref()
                .map(|context| context.window_id.clone())
                .or_else(|| provider.get_current_window_id());
            let (Some(session_name), Some(window_id)) = (session_name, window_id) else {
                continue;
            };
            if provider
                .list_sidebar_panes(Some(&session_name))
                .iter()
                .any(|pane| pane.window_id == window_id)
            {
                continue;
            }
            let warmup_until = (self.now_ms)().saturating_add(SIDEBAR_WARMUP_MS);
            self.sidebar_coordinator
                .lock()
                .unwrap()
                .begin_warmup_for_windows([window_id.clone()], warmup_until);
            spawn_sidebar_window(
                provider.as_ref(),
                &ActiveWindow {
                    id: window_id,
                    session_name,
                    active: true,
                },
                width,
                "ensure_sidebar",
            );
            spawned = true;
        }
        spawned
    }

    fn switch_visible_index(&self, index: u32, client_tty: Option<&str>) -> Option<String> {
        let target_index = index.checked_sub(1).map(|index| index as usize)?;
        let state = self.snapshot_state();
        let collapsed_groups = collapsed_worktree_group_set(&state);
        let provider_filter = self.provider_filter.lock().unwrap().clone();
        let session = display_sessions(
            &state.sessions,
            session_projection_options(&state, &collapsed_groups, provider_filter.as_deref()),
        )
        .get(target_index)
        .copied()?;
        self.switch_session_on_provider(
            &session.node_id,
            &session.provider_id,
            &session.name,
            client_tty,
        );
        None
    }

    fn switch_session_on_provider(
        &self,
        node_id: &str,
        provider_id: &str,
        name: &str,
        client_tty: Option<&str>,
    ) {
        let source_provider = client_tty
            .and_then(|tty| {
                self.providers
                    .iter()
                    .find(|provider| provider.get_client_focus(Some(tty)).is_some())
            })
            .or_else(|| self.providers.first());
        let Some(source_provider) = source_provider else {
            return;
        };
        let resolved_client_tty = client_tty
            .filter(|tty| !tty.is_empty())
            .map(str::to_string)
            .or_else(|| self.focused_client_tty.lock().unwrap().clone())
            .or_else(|| {
                source_provider
                    .get_client_focus(None)
                    .and_then(|focus| focus.client_tty)
            });
        let client_tty = resolved_client_tty.as_deref();
        let Some(target_provider) = self
            .providers
            .iter()
            .find(|provider| provider.node_id() == node_id && provider.name() == provider_id)
        else {
            let enqueued = enqueue_switch_intent_if_configured(
                &self.workspace_actor_id(),
                node_id,
                provider_id,
                name,
                client_tty,
                (self.now_ms)(),
            );
            let client_size = source_provider.get_client_size(client_tty);
            if let Some(command) = registered_ssh_attach_command(node_id, provider_id, name, client_size) {
                let wrapped = opensessions_managed_ssh_attach_command(node_id, &command, client_size);
                source_provider.detach_client_and_run(client_tty, &wrapped);
                return;
            }
            if enqueued {
                return;
            }
            debug_log(format!(
                "switch-session ignored unavailable provider node={node_id} provider={provider_id} session={name}",
            ));
            return;
        };
        if source_provider.node_id() == target_provider.node_id()
            && source_provider.name() == target_provider.name()
        {
            target_provider.switch_session(name, client_tty);
            return;
        }
        let Some(command) = target_provider.attach_session_command(name) else {
            debug_log(format!(
                "switch-session target provider cannot build attach command node={node_id} provider={provider_id} session={name}",
            ));
            return;
        };
        source_provider.detach_client_and_run(client_tty, &command);
    }

    fn session_before(&self, name: &str) -> Option<String> {
        let names = self.sidebar_display_session_names()?;
        let index = names.iter().position(|candidate| candidate == name)?;
        index
            .checked_sub(1)
            .and_then(|previous| names.get(previous).cloned())
    }

    fn session_after(&self, name: &str) -> Option<String> {
        let names = self.sidebar_display_session_names()?;
        let index = names.iter().position(|candidate| candidate == name)?;
        names.get(index + 1).cloned()
    }

    fn sidebar_display_session_names(&self) -> Option<Vec<String>> {
        let state = self.snapshot_state();
        let collapsed_groups = collapsed_worktree_group_set(&state);
        let provider_filter = self.provider_filter.lock().unwrap().clone();
        Some(display_session_names(
            &state.sessions,
            session_projection_options(&state, &collapsed_groups, provider_filter.as_deref()),
        ))
    }

    fn sidebar_reordered_session_names(&self, name: &str, delta: i8) -> Option<Vec<String>> {
        let state = self.snapshot_state();
        let collapsed_groups = collapsed_worktree_group_set(&state);
        let provider_filter = self.provider_filter.lock().unwrap().clone();
        reordered_session_names(
            &state.sessions,
            session_projection_options(&state, &collapsed_groups, provider_filter.as_deref()),
            name,
            delta,
        )
    }

    fn sidebar_reordered_worktree_group_names(&self, key: &str, delta: i8) -> Option<Vec<String>> {
        let state = self.snapshot_state();
        let collapsed_groups = collapsed_worktree_group_set(&state);
        let provider_filter = self.provider_filter.lock().unwrap().clone();
        reordered_worktree_group_names(
            &state.sessions,
            session_projection_options(&state, &collapsed_groups, provider_filter.as_deref()),
            key,
            delta,
        )
    }

    fn visible_session_names(&self) -> Option<Vec<String>> {
        let names = self.sorted_session_names();
        let mut session_order = self.session_order.lock().unwrap();
        if let Some(current_session) = self
            .providers
            .iter()
            .find_map(|provider| provider.get_current_session())
        {
            session_order.show(&current_session);
        }
        Some(session_order.apply(names))
    }

    fn sorted_session_names(&self) -> Vec<String> {
        let mut sessions = self
            .providers
            .iter()
            .flat_map(|provider| provider.list_sessions())
            .collect::<Vec<_>>();
        sessions.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.name.cmp(&b.name))
        });
        sessions.into_iter().map(|session| session.name).collect()
    }
}

/// Background ticker that advances sidebar lifecycle timers. This keeps
/// user-visible lifecycle states like `warming up…` stable long enough to be
/// perceived, then broadcasts the transition back to ready without relying on
/// unrelated tmux or websocket traffic.
async fn run_sidebar_lifecycle_loop(
    source: Arc<ReadOnlyMuxStateSource>,
    state_updates: broadcast::Sender<String>,
    shutdown: broadcast::Sender<()>,
) {
    let mut shutdown_rx = shutdown.subscribe();
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => return,
            _ = interval.tick() => {
                let now = (source.now_ms)();
                let changed = {
                    let mut coordinator = source.sidebar_coordinator.lock().unwrap();
                    coordinator.tick_timers(now)
                };
                if changed {
                    debug_log("sidebar_lifecycle_loop: lifecycle changed, invalidating queries");
                    let _ = state_updates
                        .send(source.invalidate_queries_json(vec![ServerQueryKey::SidebarLayout]));
                }
            }
        }
    }
}

/// Poll tmux state on a fixed cadence and broadcast invalidation whenever the
/// read model differs from the last poll, so sidebars refetch only their active
/// typed queries without requiring an explicit hook.
async fn run_tmux_state_poll_loop(
    source: Arc<ReadOnlyMuxStateSource>,
    state_updates: broadcast::Sender<String>,
    shutdown: broadcast::Sender<()>,
) {
    let mut shutdown_rx = shutdown.subscribe();
    let mut interval = tokio::time::interval(Duration::from_millis(TMUX_STATE_POLL_MS));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    // Seed `last_hash` from the current state so the first tick does not
    // broadcast an unprovoked snapshot. Subsequent broadcasts only happen
    // when something other than `ts` actually changes.
    source.observe_tmux_agent_state();
    let mut last_hashes = query_hashes_from_state(&source.snapshot_state());
    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => return,
            _ = interval.tick() => {
                // Hooks correct tmux layout churn immediately; this slower poll
                // is only a backstop for missed external tmux changes.

                // Hash the state ignoring the per-tick `ts` field so that
                // identical state on consecutive ticks does not trigger a
                // wasteful invalidation. Anything else changing (sessions,
                // panes, widths, init state, focus) flips the hash and the
                // sidebar refetches its active typed queries.
                let agent_observation_changed = source.observe_tmux_agent_state();
                publish_opensessions_snapshot_if_configured(&source, &source.snapshot_state()).await;
                let cloud_changed_keys = sync_opensessions_graph_if_configured(&source).await;
                let next_state = source.snapshot_state();
                let next_hashes = query_hashes_from_state(&next_state);
                let mut changed_keys = [ServerQueryKey::Sessions, ServerQueryKey::Focus, ServerQueryKey::SidebarLayout]
                    .into_iter()
                    .filter(|key| last_hashes.get(key) != next_hashes.get(key))
                    .collect::<Vec<_>>();
                if agent_observation_changed {
                    changed_keys.push(ServerQueryKey::Agents);
                }
                changed_keys.extend(cloud_changed_keys);
                if !changed_keys.is_empty() {
                    let mut deduped_keys = Vec::new();
                    for key in changed_keys {
                        if !deduped_keys.contains(&key) {
                            deduped_keys.push(key);
                        }
                    }
                    let changed_keys = deduped_keys;
                    last_hashes = next_hashes;
                    debug_log(format!(
                        "tmux_state_poll_loop: state changed, invalidating {changed_keys:?}",
                    ));
                    let _ = state_updates.send(source.invalidate_queries_json(changed_keys));
                }
            }
        }
    }
}

async fn publish_opensessions_snapshot_if_configured(
    source: &ReadOnlyMuxStateSource,
    state: &ServerState,
) {
    let Some(base_url) = std::env::var("OPENSESSIONS_CLOUD_URL")
        .ok()
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    let Some(api_key) = std::env::var("OPENSESSIONS_CLOUD_API_KEY")
        .ok()
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    let node_id = source
        .providers
        .first()
        .map(|provider| provider.node_id().to_string())
        .unwrap_or_else(|| "local".to_string());
    let providers = source
        .providers
        .iter()
        .map(|provider| {
            serde_json::json!({
                "providerId": provider.name(),
                "kind": provider.name(),
                "health": "connected",
            })
        })
        .collect::<Vec<_>>();
    let windows = source
        .providers
        .iter()
        .flat_map(|provider| {
            provider.list_active_windows().into_iter().map(|window| {
                serde_json::json!({
                    "nodeId": provider.node_id(),
                    "providerId": provider.name(),
                    "id": window.id,
                    "sessionName": window.session_name,
                    "active": window.active,
                })
            })
        })
        .collect::<Vec<_>>();
    let panes = source
        .providers
        .iter()
        .flat_map(|provider| {
            let sidebar_panes = provider.list_sidebar_panes(None).into_iter().map(|pane| {
                serde_json::json!({
                    "kind": "sidebar",
                    "nodeId": provider.node_id(),
                    "providerId": provider.name(),
                    "paneId": pane.pane_id,
                    "sessionName": pane.session_name,
                    "windowId": pane.window_id,
                    "width": pane.width,
                    "windowWidth": pane.window_width,
                })
            });
            let agent_panes = state
                .sessions
                .iter()
                .filter_map(move |session| {
                    (session.node_id == provider.node_id()
                        && session.provider_id == provider.name())
                    .then_some(session.name.clone())
                })
                .flat_map(move |session_name| {
                    provider
                        .list_agent_panes(&session_name)
                        .into_iter()
                        .map(move |pane| {
                            serde_json::json!({
                                "kind": "agent",
                                "nodeId": pane.node_id,
                                "providerId": pane.provider_id,
                                "paneId": pane.pane_id,
                                "sessionName": session_name,
                                "agent": pane.agent,
                                "active": pane.active,
                                "status": pane.status.to_string(),
                                "threadId": pane.thread_id,
                                "threadName": pane.thread_name,
                            })
                        })
                });
            sidebar_panes.chain(agent_panes).collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let local_node_ids = source.local_node_ids();
    let local_sessions = state
        .sessions
        .iter()
        .filter(|session| local_node_ids.contains(&session.node_id))
        .cloned()
        .collect::<Vec<_>>();
    let snapshot = serde_json::json!({
        "nodeId": node_id,
        "providers": providers,
        "windows": windows,
        "panes": panes,
        "sessions": local_sessions,
        "agents": local_sessions.iter().flat_map(|session| session.agents.clone()).collect::<Vec<_>>(),
        "health": "connected",
        "liveness": "alive",
        "uiState": source.workspace_ui_state(state),
        "commandIntents": [],
        "ts": state.ts,
    });
    if let Err(err) = post_opensessions_snapshot(&base_url, &api_key, &snapshot).await {
        debug_log(format!("opensessions-cloud publish failed: {err}"));
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpensessionsGraphSnapshot {
    nodes: Vec<OpensessionsNodeSnapshot>,
    #[serde(default)]
    ui_state: Option<CloudWorkspaceUiState>,
    #[serde(default)]
    command_intents: Vec<CloudCommandIntent>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpensessionsNodeSnapshot {
    node_id: String,
    #[serde(default)]
    sessions: Vec<opensessions_runtime::protocol::SessionData>,
}

async fn sync_opensessions_graph_if_configured(
    source: &ReadOnlyMuxStateSource,
) -> Vec<ServerQueryKey> {
    let Some(base_url) = std::env::var("OPENSESSIONS_CLOUD_URL")
        .ok()
        .filter(|value| !value.is_empty())
    else {
        return Vec::new();
    };
    let Some(api_key) = std::env::var("OPENSESSIONS_CLOUD_API_KEY")
        .ok()
        .filter(|value| !value.is_empty())
    else {
        return Vec::new();
    };
    let local_node_ids = source.local_node_ids();
    let graph = match get_opensessions_graph(&base_url, &api_key).await {
        Ok(graph) => graph,
        Err(err) => {
            debug_log(format!("opensessions-cloud graph fetch failed: {err}"));
            return Vec::new();
        }
    };
    let ui_state = graph.ui_state.clone();
    let command_intents = graph.command_intents.clone();
    let mut sessions = graph
        .nodes
        .into_iter()
        .filter(|node| !local_node_ids.contains(&node.node_id))
        .flat_map(|node| node.sessions)
        .collect::<Vec<_>>();
    sessions.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.provider_id.cmp(&b.provider_id))
            .then_with(|| a.name.cmp(&b.name))
    });
    let mut changed_keys = Vec::new();
    if source.update_cloud_graph_sessions(sessions) {
        changed_keys.extend([ServerQueryKey::Sessions, ServerQueryKey::Agents]);
    }
    if source.update_cloud_workspace_cache(ui_state.clone(), command_intents) {
        changed_keys.extend([ServerQueryKey::Settings]);
    }
    for (intent_id, status, result) in source.apply_cloud_command_intents(&graph.command_intents) {
        if let Err(err) = complete_command_intent(&base_url, &api_key, &intent_id, status, Some(&result)).await {
            debug_log(format!(
                "opensessions-cloud complete intent failed id={intent_id}: {err}"
            ));
        }
    }
    if let Some(ui_state) = &ui_state {
        changed_keys.extend(source.apply_cloud_ui_state(ui_state));
    }
    changed_keys
}

async fn get_opensessions_graph(
    base_url: &str,
    api_key: &str,
) -> Result<OpensessionsGraphSnapshot, String> {
    if let Some(convex_url) = convex_url_from_cloud_url(base_url) {
        return get_opensessions_graph_from_convex(convex_url, api_key).await;
    }
    let body = get_json_from_sem_cloud(base_url, api_key, "/v1/opensessions/graph").await?;
    serde_json::from_slice(&body).map_err(|err| err.to_string())
}

async fn post_opensessions_snapshot(
    base_url: &str,
    api_key: &str,
    body: &serde_json::Value,
) -> Result<(), String> {
    if let Some(convex_url) = convex_url_from_cloud_url(base_url) {
        return post_opensessions_snapshot_to_convex(convex_url, api_key, body).await;
    }
    post_json_to_sem_cloud(base_url, api_key, body).await
}

async fn post_command_intent(
    base_url: &str,
    api_key: &str,
    intent: &serde_json::Value,
) -> Result<(), String> {
    if let Some(convex_url) = convex_url_from_cloud_url(base_url) {
        return post_command_intent_to_convex(convex_url, api_key, intent).await;
    }
    post_json_to_sem_cloud_path(base_url, api_key, "/v1/opensessions/command-intent", intent).await
}

async fn complete_command_intent(
    base_url: &str,
    api_key: &str,
    intent_id: &str,
    status: &str,
    result: Option<&str>,
) -> Result<(), String> {
    if let Some(convex_url) = convex_url_from_cloud_url(base_url) {
        return complete_command_intent_to_convex(convex_url, api_key, intent_id, status, result)
            .await;
    }
    post_json_to_sem_cloud_path(
        base_url,
        api_key,
        "/v1/opensessions/command-intent/complete",
        &serde_json::json!({
            "intentId": intent_id,
            "status": status,
            "result": result,
        }),
    )
    .await
}

fn convex_url_from_cloud_url(base_url: &str) -> Option<&str> {
    base_url
        .strip_prefix("convex:")
        .or_else(|| base_url.strip_prefix("convex+"))
}

async fn post_opensessions_snapshot_to_convex(
    deployment_url: &str,
    api_key: &str,
    snapshot: &serde_json::Value,
) -> Result<(), String> {
    let mut client = ConvexClient::new(deployment_url)
        .await
        .map_err(|err| err.to_string())?;
    let snapshot = ConvexValue::try_from(snapshot.clone()).map_err(|err| err.to_string())?;
    let result = client
        .mutation(
            "opensessions:publishSnapshot",
            BTreeMap::from([
                ("apiKey".to_string(), api_key.to_string().into()),
                ("snapshot".to_string(), snapshot),
            ]),
        )
        .await
        .map_err(|err| err.to_string())?;
    match result {
        FunctionResult::Value(_) => Ok(()),
        FunctionResult::ErrorMessage(message) => Err(message),
        FunctionResult::ConvexError(err) => Err(format!("{err:?}")),
    }
}

async fn post_command_intent_to_convex(
    deployment_url: &str,
    api_key: &str,
    intent: &serde_json::Value,
) -> Result<(), String> {
    let mut client = ConvexClient::new(deployment_url)
        .await
        .map_err(|err| err.to_string())?;
    let intent = ConvexValue::try_from(intent.clone()).map_err(|err| err.to_string())?;
    let result = client
        .mutation(
            "opensessions:enqueueCommandIntent",
            BTreeMap::from([
                ("apiKey".to_string(), api_key.to_string().into()),
                ("intent".to_string(), intent),
            ]),
        )
        .await
        .map_err(|err| err.to_string())?;
    match result {
        FunctionResult::Value(_) => Ok(()),
        FunctionResult::ErrorMessage(message) => Err(message),
        FunctionResult::ConvexError(err) => Err(format!("{err:?}")),
    }
}

async fn complete_command_intent_to_convex(
    deployment_url: &str,
    api_key: &str,
    intent_id: &str,
    status: &str,
    result_message: Option<&str>,
) -> Result<(), String> {
    let mut client = ConvexClient::new(deployment_url)
        .await
        .map_err(|err| err.to_string())?;
    let mut args = BTreeMap::from([
        ("apiKey".to_string(), api_key.to_string().into()),
        ("intentId".to_string(), intent_id.to_string().into()),
        ("status".to_string(), status.to_string().into()),
    ]);
    if let Some(result_message) = result_message {
        args.insert("result".to_string(), result_message.to_string().into());
    }
    let result = client
        .mutation("opensessions:completeCommandIntent", args)
        .await
        .map_err(|err| err.to_string())?;
    match result {
        FunctionResult::Value(_) => Ok(()),
        FunctionResult::ErrorMessage(message) => Err(message),
        FunctionResult::ConvexError(err) => Err(format!("{err:?}")),
    }
}

async fn get_opensessions_graph_from_convex(
    deployment_url: &str,
    api_key: &str,
) -> Result<OpensessionsGraphSnapshot, String> {
    let mut client = ConvexClient::new(deployment_url)
        .await
        .map_err(|err| err.to_string())?;
    let result = client
        .query(
            "opensessions:getGraph",
            BTreeMap::from([("apiKey".to_string(), api_key.to_string().into())]),
        )
        .await
        .map_err(|err| err.to_string())?;
    match result {
        FunctionResult::Value(value) => {
            let mut json = value.export();
            normalize_integral_json_numbers(&mut json);
            serde_json::from_value(json).map_err(|err| err.to_string())
        }
        FunctionResult::ErrorMessage(message) => Err(message),
        FunctionResult::ConvexError(err) => Err(format!("{err:?}")),
    }
}

fn normalize_integral_json_numbers(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                normalize_integral_json_numbers(value);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values_mut() {
                normalize_integral_json_numbers(value);
            }
        }
        serde_json::Value::Number(number) => {
            let Some(float) = number.as_f64() else {
                return;
            };
            if !float.is_finite() || float.fract() != 0.0 {
                return;
            }
            if float >= 0.0 && float <= u64::MAX as f64 {
                *value = serde_json::Value::Number(serde_json::Number::from(float as u64));
            } else if float >= i64::MIN as f64 && float <= i64::MAX as f64 {
                *value = serde_json::Value::Number(serde_json::Number::from(float as i64));
            }
        }
        _ => {}
    }
}

fn apply_global_session_order(
    sessions: &mut Vec<opensessions_runtime::protocol::SessionData>,
    session_order: &SessionOrder,
) {
    let hidden = session_order.hidden_sessions();
    sessions.retain(|session| !hidden.contains(&session.name));
    let visible_order = session_order.visible_order();
    sessions.sort_by(|a, b| {
        let a_position = visible_order
            .iter()
            .position(|name| name == &a.name)
            .unwrap_or(usize::MAX);
        let b_position = visible_order
            .iter()
            .position(|name| name == &b.name)
            .unwrap_or(usize::MAX);
        a_position
            .cmp(&b_position)
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.provider_id.cmp(&b.provider_id))
            .then_with(|| a.name.cmp(&b.name))
    });
}

async fn post_json_to_sem_cloud(
    base_url: &str,
    api_key: &str,
    body: &serde_json::Value,
) -> Result<(), String> {
    post_json_to_sem_cloud_path(base_url, api_key, "/v1/opensessions/snapshot", body).await
}

async fn post_json_to_sem_cloud_path(
    base_url: &str,
    api_key: &str,
    path: &str,
    body: &serde_json::Value,
) -> Result<(), String> {
    let target = http_target(base_url, path)?;
    let mut stream = TcpStream::connect((target.host.as_str(), target.port))
        .await
        .map_err(|err| err.to_string())?;
    let body = serde_json::to_string(body).map_err(|err| err.to_string())?;
    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        target.path,
        target.host,
        api_key,
        body.len(),
        body,
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|err| err.to_string())?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .map_err(|err| err.to_string())?;
    let response = String::from_utf8_lossy(&response);
    if response.starts_with("HTTP/1.1 2") || response.starts_with("HTTP/1.0 2") {
        Ok(())
    } else {
        Err(response
            .lines()
            .next()
            .unwrap_or("empty response")
            .to_string())
    }
}

async fn get_json_from_sem_cloud(
    base_url: &str,
    api_key: &str,
    path: &str,
) -> Result<Vec<u8>, String> {
    let target = http_target(base_url, path)?;
    let mut stream = TcpStream::connect((target.host.as_str(), target.port))
        .await
        .map_err(|err| err.to_string())?;
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
        target.path, target.host, api_key,
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|err| err.to_string())?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .map_err(|err| err.to_string())?;
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .ok_or_else(|| "malformed http response".to_string())?;
    let status_line = String::from_utf8_lossy(&response[..header_end])
        .lines()
        .next()
        .unwrap_or("empty response")
        .to_string();
    if status_line.starts_with("HTTP/1.1 2") || status_line.starts_with("HTTP/1.0 2") {
        Ok(response[header_end..].to_vec())
    } else {
        Err(status_line)
    }
}

struct HttpTarget {
    host: String,
    port: u16,
    path: String,
}

fn http_target(base_url: &str, path: &str) -> Result<HttpTarget, String> {
    let rest = base_url
        .strip_prefix("http://")
        .ok_or_else(|| "only http:// OPENSESSIONS_CLOUD_URL is supported".to_string())?;
    let (authority, base_path) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port) = authority
        .rsplit_once(':')
        .map(|(host, port)| {
            port.parse::<u16>()
                .map(|port| (host.to_string(), port))
                .map_err(|err| err.to_string())
        })
        .unwrap_or_else(|| Ok((authority.to_string(), 80)))?;
    let base_path = base_path.trim_end_matches('/');
    let path = if base_path.is_empty() {
        path.to_string()
    } else {
        format!("/{base_path}{path}")
    };
    Ok(HttpTarget { host, port, path })
}

fn query_hashes_from_state(state: &ServerState) -> HashMap<ServerQueryKey, u64> {
    ALL_QUERY_KEYS
        .into_iter()
        .map(|key| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            let data = query_data_from_state(key, state.clone());
            serde_json::to_string(&data)
                .expect("query hash serialization must succeed")
                .hash(&mut hasher);
            (key, hasher.finish())
        })
        .collect()
}

async fn run_agent_watcher_loop(
    source: Arc<ReadOnlyMuxStateSource>,
    state_updates: broadcast::Sender<String>,
    shutdown: broadcast::Sender<()>,
) {
    let mut shutdown_rx = shutdown.subscribe();
    let mut interval = tokio::time::interval(Duration::from_millis(AGENT_WATCHER_POLL_MS));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_seen = HashMap::<String, AgentWatcherFingerprint>::new();

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => return,
            _ = interval.tick() => {
                let now = current_time_ms();
                let snapshots = tokio::task::spawn_blocking(move || scan_agent_watcher_snapshots(now))
                    .await
                    .unwrap_or_default();
                debug_log(format!(
                    "agent_watcher_loop: tick scanned {} snapshots",
                    snapshots.len()
                ));
                for snapshot in snapshots {
                    if snapshot.status == AgentStatus::Idle {
                        continue;
                    }
                    let key = agent_watcher_key(&snapshot);
                    let fingerprint = AgentWatcherFingerprint::from(&snapshot);
                    if last_seen.get(&key) == Some(&fingerprint) {
                        continue;
                    }
                    let agent = snapshot.agent.to_string();
                    let status = snapshot.status;
                    let thread_name = snapshot.thread_name.clone();
                    if source.apply_agent_watcher_snapshot(snapshot) {
                        debug_log(format!(
                            "agent_watcher_loop: applied snapshot agent={agent} status={status:?} thread={thread_name:?}",
                        ));
                        last_seen.insert(key, fingerprint);
                        let _ = state_updates
                            .send(source.invalidate_queries_json(vec![ServerQueryKey::Agents]));
                    } else {
                        debug_log(format!(
                            "agent_watcher_loop: dropped snapshot agent={agent} status={status:?} (no matching session)",
                        ));
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentWatcherFingerprint {
    status: AgentStatus,
    thread_name: Option<String>,
    last_user_prompt: Option<String>,
    project_dir: Option<String>,
}

impl From<&AgentWatcherSnapshot> for AgentWatcherFingerprint {
    fn from(snapshot: &AgentWatcherSnapshot) -> Self {
        Self {
            status: snapshot.status,
            thread_name: snapshot.thread_name.clone(),
            last_user_prompt: snapshot.last_user_prompt.clone(),
            project_dir: snapshot.project_dir.clone(),
        }
    }
}

fn agent_watcher_key(snapshot: &AgentWatcherSnapshot) -> String {
    format!(
        "{}\0{}",
        snapshot.agent,
        snapshot
            .thread_id
            .as_deref()
            .or(snapshot.project_dir.as_deref())
            .unwrap_or_default(),
    )
}

fn scan_agent_watcher_snapshots(now_ms: u64) -> Vec<AgentWatcherSnapshot> {
    let mut snapshots = Vec::new();
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return snapshots;
    };

    scan_amp_threads(&home, now_ms, &mut snapshots);
    scan_claude_code_projects(&home, now_ms, &mut snapshots);
    scan_codex_sessions(&home, now_ms, &mut snapshots);
    scan_opencode_sessions(&home, now_ms, &mut snapshots);
    scan_pi_sessions(&home, now_ms, &mut snapshots);
    scan_droid_sessions(&home, now_ms, &mut snapshots);
    snapshots
}

fn scan_amp_threads(home: &Path, now_ms: u64, snapshots: &mut Vec<AgentWatcherSnapshot>) {
    let threads_dir = home.join(".local/share/amp/threads");
    let Ok(entries) = fs::read_dir(threads_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(mtime_ms) = file_mtime_ms(&path) else {
            continue;
        };
        if now_ms.saturating_sub(mtime_ms) > AGENT_WATCHER_RECENT_MS {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        if let Some(snapshot) = amp_snapshot_from_thread_json(&raw, mtime_ms) {
            snapshots.push(snapshot);
        }
    }
}

fn scan_claude_code_projects(home: &Path, now_ms: u64, snapshots: &mut Vec<AgentWatcherSnapshot>) {
    let projects_dir = home.join(".claude/projects");
    let Ok(projects) = fs::read_dir(projects_dir) else {
        return;
    };

    for project in projects.flatten() {
        let project_path = project.path();
        if !project_path.is_dir() {
            continue;
        }
        let encoded = project.file_name().to_string_lossy().to_string();
        let project_dir = decode_claude_project_dir(&encoded, |path| Path::new(path).is_dir());
        let Ok(files) = fs::read_dir(project_path) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(mtime_ms) = file_mtime_ms(&path) else {
                continue;
            };
            if now_ms.saturating_sub(mtime_ms) > AGENT_WATCHER_RECENT_MS {
                continue;
            }
            let Some(thread_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            if let Some(snapshot) =
                claude_code_snapshot_from_jsonl(thread_id, &project_dir, &raw, mtime_ms, now_ms)
            {
                snapshots.push(snapshot);
            }
        }
    }
}

fn scan_codex_sessions(home: &Path, now_ms: u64, snapshots: &mut Vec<AgentWatcherSnapshot>) {
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    let sessions_dir = codex_home.join("sessions");
    let names = fs::read_to_string(codex_home.join("session_index.jsonl"))
        .ok()
        .map(|raw| {
            parse_codex_session_index(&raw)
                .into_iter()
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();

    for path in collect_jsonl_files(&sessions_dir) {
        let Some(mtime_ms) = file_mtime_ms(&path) else {
            continue;
        };
        if now_ms.saturating_sub(mtime_ms) > AGENT_WATCHER_RECENT_MS {
            continue;
        }
        let Some(path_text) = path.to_str() else {
            continue;
        };
        let thread_id = codex_thread_id_from_path(path_text);
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        if let Some(snapshot) = codex_snapshot_from_jsonl(
            &thread_id,
            &raw,
            names.get(&thread_id).map(String::as_str),
            mtime_ms,
            now_ms,
        ) {
            snapshots.push(snapshot);
        }
    }
}

fn scan_opencode_sessions(home: &Path, now_ms: u64, snapshots: &mut Vec<AgentWatcherSnapshot>) {
    let db_path = std::env::var_os("OPENCODE_DB_PATH")
        .or_else(|| std::env::var_os("OPENCODE_DB"))
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share/opencode/opencode.db"));
    if !db_path.exists() {
        return;
    }

    let stale_threshold = now_ms.saturating_sub(AGENT_WATCHER_RECENT_MS);
    let query = format!(
        "WITH recent AS MATERIALIZED (SELECT id, title, directory, time_updated FROM session WHERE time_updated > {stale_threshold} ORDER BY time_updated DESC LIMIT 50) SELECT r.id, ifnull(r.title,''), r.directory, r.time_updated, ifnull((SELECT m.data FROM message m WHERE m.session_id = r.id ORDER BY m.time_created DESC LIMIT 1),''), ifnull((SELECT sm.data FROM session_message sm WHERE sm.session_id = r.id AND sm.type = 'user' ORDER BY sm.seq DESC LIMIT 1),'') FROM recent r ORDER BY r.time_updated DESC;"
    );
    let run_query = |query: String| {
        let mut command = process::Command::new("sqlite3");
        command
            .arg("-readonly")
            .arg("-separator")
            .arg(OPENCODE_SQL_SEP.to_string())
            .arg(&db_path)
            .arg(query);
        run_process_with_timeout(command, Duration::from_millis(OPENCODE_SQL_TIMEOUT_MS))
    };
    let output = run_query(query).or_else(|| {
        let legacy_query = format!(
            "WITH recent AS MATERIALIZED (SELECT id, title, directory, time_updated FROM session WHERE time_updated > {stale_threshold} ORDER BY time_updated DESC LIMIT 50) SELECT r.id, ifnull(r.title,''), r.directory, r.time_updated, ifnull((SELECT m.data FROM message m WHERE m.session_id = r.id ORDER BY m.time_created DESC LIMIT 1),'') FROM recent r ORDER BY r.time_updated DESC;"
        );
        run_query(legacy_query)
    });
    let Some(mut output) = output else {
        return;
    };
    if !output.status.success() {
        let legacy_query = format!(
            "WITH recent AS MATERIALIZED (SELECT id, title, directory, time_updated FROM session WHERE time_updated > {stale_threshold} ORDER BY time_updated DESC LIMIT 50) SELECT r.id, ifnull(r.title,''), r.directory, r.time_updated, ifnull((SELECT m.data FROM message m WHERE m.session_id = r.id ORDER BY m.time_created DESC LIMIT 1),'') FROM recent r ORDER BY r.time_updated DESC;"
        );
        let Some(legacy_output) = run_query(legacy_query) else {
            return;
        };
        if !legacy_output.status.success() {
            return;
        }
        output = legacy_output;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let parts = line.split(OPENCODE_SQL_SEP).collect::<Vec<_>>();
        if parts.len() < 5 || parts[4].is_empty() {
            continue;
        }
        let time_updated = parts[3].parse::<u64>().unwrap_or(now_ms);
        if let Some(snapshot) = opencode_snapshot_from_row(
            parts[0],
            (!parts[1].is_empty()).then_some(parts[1]),
            parts[2],
            time_updated,
            parts[4],
            parts.get(5).copied().filter(|value| !value.is_empty()),
            now_ms,
        ) {
            snapshots.push(snapshot);
        }
    }
}

fn scan_pi_sessions(home: &Path, now_ms: u64, snapshots: &mut Vec<AgentWatcherSnapshot>) {
    let sessions_dir = std::env::var_os("PI_CODING_AGENT_SESSION_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("PI_CODING_AGENT_DIR")
                .map(PathBuf::from)
                .map(|dir| dir.join("sessions"))
        })
        .unwrap_or_else(|| home.join(".pi/agent/sessions"));

    for path in collect_jsonl_files(&sessions_dir) {
        let Some(mtime_ms) = file_mtime_ms(&path) else {
            continue;
        };
        if now_ms.saturating_sub(mtime_ms) > AGENT_WATCHER_RECENT_MS {
            continue;
        }
        let Some(thread_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        if let Some(snapshot) = pi_snapshot_from_jsonl(thread_id, &raw, mtime_ms, now_ms) {
            snapshots.push(snapshot);
        }
    }
}

fn scan_droid_sessions(home: &Path, now_ms: u64, snapshots: &mut Vec<AgentWatcherSnapshot>) {
    let projects_dir = std::env::var_os("FACTORY_PROJECTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".factory/projects"));

    for path in collect_jsonl_files(&projects_dir) {
        let Some(mtime_ms) = file_mtime_ms(&path) else {
            continue;
        };
        if now_ms.saturating_sub(mtime_ms) > AGENT_WATCHER_RECENT_MS {
            continue;
        }
        let Some(thread_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        if let Some(snapshot) = droid_snapshot_from_jsonl(thread_id, &raw, mtime_ms, now_ms) {
            snapshots.push(snapshot);
        }
    }
}

fn run_process_with_timeout(
    mut command: process::Command,
    timeout: Duration,
) -> Option<process::Output> {
    let mut child = command
        .stdout(process::Stdio::piped())
        .stderr(process::Stdio::piped())
        .spawn()
        .ok()?;
    let started = Instant::now();

    loop {
        if child.try_wait().ok()?.is_some() {
            return child.wait_with_output().ok();
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn collect_jsonl_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(collect_jsonl_files(&path));
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
    files
}

fn file_mtime_ms(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
}

fn encode_agent_project_dir(path: &str) -> String {
    path.chars()
        .map(|ch| match ch {
            '/' | '.' | '_' => '-',
            ch => ch,
        })
        .collect()
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn activate_session_json(name: String, source_pane_id: Option<&str>) -> String {
    serde_json::to_string(&ServerMessage::ActivateSession {
        name,
        source_pane_id: source_pane_id.map(str::to_string),
    })
    .expect("activate-session must serialize")
}

fn ui_focus_json(client_tty: String, focus: ClientUiFocus) -> String {
    serde_json::to_string(&ServerMessage::UiFocus { client_tty, focus })
        .expect("ui-focus must serialize")
}

const ALL_QUERY_KEYS: [ServerQueryKey; 5] = [
    ServerQueryKey::Sessions,
    ServerQueryKey::Agents,
    ServerQueryKey::Focus,
    ServerQueryKey::SidebarLayout,
    ServerQueryKey::Settings,
];

fn all_query_keys() -> Vec<ServerQueryKey> {
    ALL_QUERY_KEYS.to_vec()
}

fn invalidate_queries_json(keys: Vec<ServerQueryKey>) -> String {
    serde_json::to_string(&ServerMessage::Invalidate {
        keys,
        ts: current_time_ms(),
    })
    .expect("invalidate must serialize")
}

fn sidebar_launch_plan(
    provider: &dyn MuxProvider,
    context: Option<&HttpContext>,
) -> Vec<ActiveWindow> {
    let existing_sidebar_windows = provider
        .list_sidebar_panes(None)
        .into_iter()
        .map(|pane| pane.window_id)
        .collect::<HashSet<_>>();
    let mut windows = Vec::<ActiveWindow>::new();
    for window in provider.list_sidebar_target_windows() {
        if existing_sidebar_windows.contains(&window.id) {
            continue;
        }
        if windows.iter().any(|current| current.id == window.id) {
            continue;
        }
        windows.push(window);
    }
    windows.sort_by_key(|window| sidebar_launch_rank(window, context));
    windows
}

fn sidebar_launch_rank(window: &ActiveWindow, context: Option<&HttpContext>) -> (u8, bool) {
    let Some(context) = context else {
        return (u8::from(!window.active), !window.active);
    };
    if window.id == context.window_id {
        return (0, false);
    }
    if window.session_name == context.session {
        return (1, !window.active);
    }
    (2, !window.active)
}

fn spawn_sidebar_window(
    provider: &dyn MuxProvider,
    window: &ActiveWindow,
    width: u16,
    reason: &str,
) {
    debug_log(format!(
        "{reason}: spawning in session={} window={} width={width}",
        window.session_name, window.id,
    ));
    provider.spawn_sidebar(
        &window.session_name,
        &window.id,
        width,
        SidebarPosition::Left,
        SIDEBAR_SCRIPTS_DIR,
    );
}

fn spawn_staggered_sidebars(
    provider: Arc<dyn MuxProvider>,
    windows: Vec<ActiveWindow>,
    width: u16,
    reason: &'static str,
) {
    std::thread::spawn(move || {
        for window in windows {
            std::thread::sleep(Duration::from_millis(SIDEBAR_STAGGER_MS));
            spawn_sidebar_window(provider.as_ref(), &window, width, reason);
        }
    });
}

fn query_result_json_from_state(key: ServerQueryKey, state: ServerState, ts: u64) -> String {
    query_result_json_from_data(key, query_data_from_state(key, state), ts)
}

fn query_result_json_from_data(key: ServerQueryKey, data: ServerQueryData, ts: u64) -> String {
    serde_json::to_string(&ServerMessage::QueryResult { key, data, ts })
        .expect("query result must serialize")
}

fn query_data_from_state(key: ServerQueryKey, state: ServerState) -> ServerQueryData {
    match key {
        ServerQueryKey::Sessions => ServerQueryData::Sessions {
            sessions: state
                .sessions
                .into_iter()
                .map(|mut session| {
                    session.unseen = false;
                    session.agent_state = None;
                    session.agents.clear();
                    session.event_timestamps.clear();
                    session
                })
                .collect(),
        },
        ServerQueryKey::Agents => ServerQueryData::Agents {
            sessions: state
                .sessions
                .into_iter()
                .map(|session| SessionAgentsData {
                    node_id: session.node_id,
                    provider_id: session.provider_id,
                    session: session.name,
                    unseen: session.unseen,
                    agent_state: session.agent_state,
                    agents: session.agents,
                    event_timestamps: session.event_timestamps,
                })
                .collect(),
        },
        ServerQueryKey::Focus => ServerQueryData::Focus {
            focused_session: state.focused_session,
            current_session: state.current_session,
        },
        ServerQueryKey::SidebarLayout => ServerQueryData::SidebarLayout {
            sidebar_width: state.sidebar_width,
            detail_panel_height: state.detail_panel_height,
            initializing: state.initializing,
            init_label: state.init_label,
            collapsed_worktree_groups: state.collapsed_worktree_groups,
        },
        ServerQueryKey::Settings => ServerQueryData::Settings {
            theme: state.theme,
            session_filter: state.session_filter,
            provider_filter: None,
            agent_panel_scope: state.agent_panel_scope,
        },
    }
}

fn parse_metadata_tone(value: &str) -> Option<MetadataTone> {
    match value {
        "neutral" => Some(MetadataTone::Neutral),
        "info" => Some(MetadataTone::Info),
        "success" => Some(MetadataTone::Success),
        "warn" => Some(MetadataTone::Warn),
        "error" => Some(MetadataTone::Error),
        _ => None,
    }
}

fn parse_agent_status(value: &str) -> Option<AgentStatus> {
    match value {
        "idle" => Some(AgentStatus::Idle),
        "running" => Some(AgentStatus::Running),
        "tool-running" => Some(AgentStatus::ToolRunning),
        "done" => Some(AgentStatus::Done),
        "error" => Some(AgentStatus::Error),
        "waiting" => Some(AgentStatus::Waiting),
        "interrupted" => Some(AgentStatus::Interrupted),
        "stale" => Some(AgentStatus::Stale),
        _ => None,
    }
}

fn parse_agent_panel_scope(value: &str) -> Option<AgentPanelScope> {
    match value {
        "current" => Some(AgentPanelScope::Current),
        "all" => Some(AgentPanelScope::All),
        _ => None,
    }
}

fn parse_process_row(line: &str) -> Option<(u32, u32)> {
    let mut parts = line.split_whitespace();
    let pid = parts.next()?.parse::<u32>().ok()?;
    let ppid = parts.next()?.parse::<u32>().ok()?;
    Some((pid, ppid))
}

struct HttpContext {
    client_tty: Option<String>,
    session: String,
    window_id: String,
    pane_id: Option<String>,
    pane_active: Option<bool>,
}

fn parse_context(body: &str) -> Option<HttpContext> {
    let trimmed = trim_context_quotes(body);
    let pipe_parts = trimmed.split('|').collect::<Vec<_>>();
    if pipe_parts.len() == 5 && !pipe_parts[1].is_empty() && !pipe_parts[2].is_empty() {
        return Some(HttpContext {
            client_tty: (!pipe_parts[0].is_empty()).then(|| pipe_parts[0].to_string()),
            session: pipe_parts[1].to_string(),
            window_id: pipe_parts[2].to_string(),
            pane_id: Some(pipe_parts[3].to_string()),
            pane_active: Some(pipe_parts[4] == "1"),
        });
    }
    if pipe_parts.len() == 4 && !pipe_parts[1].is_empty() && !pipe_parts[2].is_empty() {
        return Some(HttpContext {
            client_tty: (!pipe_parts[0].is_empty()).then(|| pipe_parts[0].to_string()),
            session: pipe_parts[1].to_string(),
            window_id: pipe_parts[2].to_string(),
            pane_id: Some(pipe_parts[3].to_string()),
            pane_active: None,
        });
    }
    if pipe_parts.len() == 3 && !pipe_parts[1].is_empty() && !pipe_parts[2].is_empty() {
        return Some(HttpContext {
            client_tty: (!pipe_parts[0].is_empty()).then(|| pipe_parts[0].to_string()),
            session: pipe_parts[1].to_string(),
            window_id: pipe_parts[2].to_string(),
            pane_id: None,
            pane_active: None,
        });
    }

    let colon_idx = trimmed.find(':')?;
    if colon_idx < 1 {
        return None;
    }
    let session = &trimmed[..colon_idx];
    let window_id = &trimmed[colon_idx + 1..];
    (!session.is_empty() && !window_id.is_empty()).then(|| HttpContext {
        client_tty: None,
        session: session.to_string(),
        window_id: window_id.to_string(),
        pane_id: None,
        pane_active: None,
    })
}

fn parse_context_session(body: &str) -> Option<String> {
    parse_context(body).map(|context| context.session)
}

fn trim_context_quotes(value: &str) -> &str {
    trim_single_quotes(trim_double_quotes(value.trim()))
}

fn trim_double_quotes(value: &str) -> &str {
    value.trim_matches('"')
}

fn trim_single_quotes(value: &str) -> &str {
    value.trim_matches('\'')
}

#[derive(Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub pid_file: PathBuf,
    state_source: Option<Arc<dyn StateSource>>,
}

impl ServerConfig {
    pub fn new(host: impl Into<String>, port: u16, pid_file: impl Into<PathBuf>) -> Self {
        Self {
            host: host.into(),
            port,
            pid_file: pid_file.into(),
            state_source: None,
        }
    }

    pub fn with_state_source(mut self, source: impl StateSource) -> Self {
        self.state_source = Some(Arc::new(source));
        self
    }
}

#[derive(Debug)]
pub struct ServerHandle {
    addr: SocketAddr,
    shutdown: broadcast::Sender<()>,
    task: JoinHandle<Result<(), ServerError>>,
}

impl ServerHandle {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub async fn shutdown(self) -> Result<(), ServerError> {
        let _ = self.shutdown.send(());
        self.wait_shutdown().await
    }

    pub async fn wait_shutdown(self) -> Result<(), ServerError> {
        self.task.await.map_err(ServerError::from)?
    }
}

#[derive(Debug, Clone)]
pub struct ServerError {
    message: String,
}

impl ServerError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ServerError {}

impl From<std::io::Error> for ServerError {
    fn from(value: std::io::Error) -> Self {
        Self::new(value.to_string())
    }
}

impl From<tokio_websockets::Error> for ServerError {
    fn from(value: tokio_websockets::Error) -> Self {
        Self::new(value.to_string())
    }
}

impl From<tokio::task::JoinError> for ServerError {
    fn from(value: tokio::task::JoinError) -> Self {
        Self::new(value.to_string())
    }
}

pub async fn start_server(config: ServerConfig) -> Result<ServerHandle, ServerError> {
    let bind_addr = (config.host.as_str(), config.port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| ServerError::new("server bind address did not resolve"))?;
    let listener = TcpListener::bind(bind_addr).await?;
    let addr = listener.local_addr()?;
    fs::write(&config.pid_file, process::id().to_string())?;

    let (shutdown, shutdown_rx) = broadcast::channel(1);
    let (state_updates, _) = broadcast::channel(16);
    let shutdown_announcement = Arc::new(ShutdownAnnouncement::default());
    if let Some(source) = config.state_source.clone() {
        let _background_tasks = source
            .clone()
            .start_background_tasks(state_updates.clone(), shutdown.clone());
        source.setup_mux_hooks(&config.host, addr.port());
    }
    let task_shutdown = shutdown.clone();
    let state_source = config.state_source.clone();
    let cleanup_state_source = state_source.clone();
    let loop_shutdown_announcement = Arc::clone(&shutdown_announcement);
    let task = tokio::spawn(async move {
        let result = run_accept_loop(
            listener,
            task_shutdown,
            shutdown_rx,
            state_source,
            state_updates,
            loop_shutdown_announcement,
        )
        .await;
        if let Some(source) = cleanup_state_source.as_ref() {
            source.cleanup_mux_hooks();
        }
        let cleanup_result = fs::remove_file(&config.pid_file);
        match (result, cleanup_result) {
            (Err(err), _) => Err(err),
            (Ok(()), Err(err)) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
            _ => Ok(()),
        }
    });

    Ok(ServerHandle {
        addr,
        shutdown,
        task,
    })
}

async fn run_accept_loop(
    listener: TcpListener,
    shutdown: broadcast::Sender<()>,
    mut shutdown_rx: broadcast::Receiver<()>,
    state_source: Option<Arc<dyn StateSource>>,
    state_updates: broadcast::Sender<String>,
    shutdown_announcement: Arc<ShutdownAnnouncement>,
) -> Result<(), ServerError> {
    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                shutdown_announcement.announce_once(&state_source, &state_updates);
                tokio::time::sleep(Duration::from_millis(SERVER_SHUTDOWN_DRAIN_MS)).await;
                return Ok(());
            }
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let connection_shutdown = shutdown.clone();
                let connection_state_source = state_source.clone();
                let connection_state_updates = state_updates.clone();
                let connection_shutdown_announcement = Arc::clone(&shutdown_announcement);
                tokio::spawn(async move {
                    let _ = handle_connection(
                        stream,
                        connection_shutdown,
                        connection_state_source,
                        connection_state_updates,
                        connection_shutdown_announcement,
                    )
                    .await;
                });
            }

        }
    }
}

fn announce_shutdown(
    state_source: &Option<Arc<dyn StateSource>>,
    state_updates: &broadcast::Sender<String>,
) {
    if let Some(payload) = state_source
        .as_ref()
        .and_then(|source| source.begin_shutdown())
    {
        let _ = state_updates.send(payload);
    }
    let _ = state_updates.send(QUIT_JSON.to_string());
}

fn request_shutdown(
    state_source: &Option<Arc<dyn StateSource>>,
    state_updates: &broadcast::Sender<String>,
    shutdown: &broadcast::Sender<()>,
    shutdown_announcement: &ShutdownAnnouncement,
) {
    shutdown_announcement.announce_once(state_source, state_updates);
    let _ = shutdown.send(());
}

async fn handle_connection(
    mut stream: TcpStream,
    shutdown: broadcast::Sender<()>,
    state_source: Option<Arc<dyn StateSource>>,
    state_updates: broadcast::Sender<String>,
    shutdown_announcement: Arc<ShutdownAnnouncement>,
) -> Result<(), ServerError> {
    let mut request = read_http_header(&mut stream).await?;
    let parsed = parse_http_request(&request)?;
    read_remaining_http_body(&mut stream, &mut request, parsed.content_length()).await?;

    if parsed.method == "GET" && parsed.path == "/version" {
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                    VERSION_HTTP_BODY.len(),
                    VERSION_HTTP_BODY
                )
                .as_bytes(),
            )
            .await?;
        let _ = stream.shutdown().await;
        return Ok(());
    }

    if parsed.method == "GET" && parsed.path == "/debug/agents" {
        let body = state_source
            .as_ref()
            .and_then(|state_source| state_source.debug_agents_json(parsed.query_param("session")))
            .unwrap_or_else(|| "{\"error\":\"state source unavailable\"}".to_string());
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                )
                .as_bytes(),
            )
            .await?;
        let _ = stream.shutdown().await;
        return Ok(());
    }

    if parsed.method == "POST" && parsed.path == "/refresh" {
        if let Some(state_source) = state_source.as_ref() {
            let _ = state_updates.send(state_source.invalidate_all_queries_json());
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .await?;
        let _ = stream.shutdown().await;
        return Ok(());
    }

    if parsed.method == "POST" && parsed.path == "/focus" {
        let body = String::from_utf8_lossy(http_body(&request));
        if let Some(payload) = state_source
            .as_ref()
            .and_then(|state_source| state_source.handle_http_text(&parsed.path, &body))
        {
            let _ = state_updates.send(payload);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .await?;
        let _ = stream.shutdown().await;
        return Ok(());
    }

    if parsed.method == "POST" && parsed.path == "/switch-index" {
        let Some(index) = parsed
            .query_param("index")
            .and_then(|index| index.parse::<u32>().ok())
        else {
            stream
                .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 13\r\n\r\nmissing index")
                .await?;
            let _ = stream.shutdown().await;
            return Ok(());
        };
        let body = String::from_utf8_lossy(http_body(&request));
        if let Some(state_source) = &state_source {
            let _ = state_source.handle_switch_index(index, &body);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .await?;
        let _ = stream.shutdown().await;
        return Ok(());
    }

    if parsed.method == "POST" && is_ok_hook_path(&parsed.path) {
        let body = String::from_utf8_lossy(http_body(&request));
        if let Some(payload) = state_source
            .as_ref()
            .and_then(|state_source| state_source.handle_http_hook(&parsed.path, &body))
        {
            let _ = state_updates.send(payload);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .await?;
        let _ = stream.shutdown().await;
        return Ok(());
    }

    if parsed.method == "POST" && parsed.path == "/api/agent-event" {
        let Ok(body) = serde_json::from_slice::<Value>(http_body(&request)) else {
            stream
                .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 12\r\n\r\ninvalid json")
                .await?;
            let _ = stream.shutdown().await;
            return Ok(());
        };
        match state_source
            .as_ref()
            .ok_or(AgentEventError::CouldNotResolveSession)
            .and_then(|state_source| state_source.handle_agent_event_json(&body))
        {
            Ok(payload) => {
                let _ = state_updates.send(payload);
                stream
                    .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                    .await?;
            }
            Err(err) => {
                let (status, body) = err.status_and_body();
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await?;
            }
        }
        let _ = stream.shutdown().await;
        return Ok(());
    }

    if parsed.method == "POST" && parsed.path == "/api/runtime/pi/upsert" {
        let Ok(body) = serde_json::from_slice::<Value>(http_body(&request)) else {
            stream
                .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 12\r\n\r\ninvalid json")
                .await?;
            let _ = stream.shutdown().await;
            return Ok(());
        };
        if let Some(state_source) = &state_source
            && let Err(err) = state_source.handle_pi_runtime_upsert(&body)
        {
            let body = err.body();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await?;
            let _ = stream.shutdown().await;
            return Ok(());
        }
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
            .await?;
        let _ = stream.shutdown().await;
        return Ok(());
    }

    if parsed.method == "POST" && parsed.path == "/api/runtime/pi/delete" {
        let Ok(body) = serde_json::from_slice::<Value>(http_body(&request)) else {
            stream
                .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 12\r\n\r\ninvalid json")
                .await?;
            let _ = stream.shutdown().await;
            return Ok(());
        };
        if let Some(state_source) = &state_source
            && let Err(err) = state_source.handle_pi_runtime_delete(&body)
        {
            let body = err.body();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await?;
            let _ = stream.shutdown().await;
            return Ok(());
        }
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
            .await?;
        let _ = stream.shutdown().await;
        return Ok(());
    }

    if parsed.method == "POST"
        && let Ok(body) = serde_json::from_slice::<Value>(http_body(&request))
        && is_metadata_path(&parsed.path)
        && !body.get("session").is_some_and(Value::is_string)
    {
        stream
            .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 15\r\n\r\nmissing session")
            .await?;
        let _ = stream.shutdown().await;
        return Ok(());
    }

    if parsed.method == "POST"
        && let Ok(body) = serde_json::from_slice::<Value>(http_body(&request))
        && let Some(payload) = state_source
            .as_ref()
            .and_then(|state_source| state_source.handle_http_json(&parsed.path, &body))
    {
        let _ = state_updates.send(payload);
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
            .await?;
        let _ = stream.shutdown().await;
        return Ok(());
    }

    if parsed.method == "POST" && parsed.path == "/quit" {
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .await?;
        let _ = stream.shutdown().await;
        request_shutdown(
            &state_source,
            &state_updates,
            &shutdown,
            &shutdown_announcement,
        );
        return Ok(());
    }

    if parsed.is_websocket_upgrade() {
        let Some(key) = parsed.header("sec-websocket-key") else {
            stream
                .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
                .await?;
            return Ok(());
        };
        let accept = websocket_accept(key);
        stream
            .write_all(
                format!(
                    "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
                )
                .as_bytes(),
            )
            .await?;

        let mut websocket = ServerBuilder::new().serve(stream);
        debug_log("ws: client connected, sending hello");
        websocket.send(Message::text(HELLO_JSON)).await?;

        let mut connection_shutdown = shutdown.subscribe();
        let mut state_update_rx = state_updates.subscribe();
        let mut client_context = ClientConnectionContext::default();
        loop {
            tokio::select! {
                biased;

                _ = connection_shutdown.recv() => {
                    let _ = websocket.send(Message::text(QUIT_JSON)).await;
                    return Ok(());
                }
                message = websocket.next() => {
                    match message {
                        Some(Ok(message)) if message.is_close() => return Ok(()),
                        Some(Ok(message)) => {
                            if is_quit_command(&message) {
                                request_shutdown(
                                    &state_source,
                                    &state_updates,
                                    &shutdown,
                                    &shutdown_announcement,
                                );
                                return Ok(());
                            }
                            if is_command_type(&message, "refresh")
                                && let Some(state_source) = state_source.as_ref()
                            {
                                let _ = state_updates.send(state_source.invalidate_all_queries_json());
                            }
                            if let Some(command) = parse_command(&message) {
                                if let Some(reply) = state_source
                                    .as_ref()
                                    .and_then(|state_source| fetch_query_reply(state_source.as_ref(), &command))
                                {
                                    websocket.send(Message::text(reply)).await?;
                                    continue;
                                }
                                if let Some(outcome) = state_source
                                    .as_ref()
                                    .and_then(|state_source| state_source.handle_sender_command_with_context(&command, &mut client_context))
                                {
                                    if let Some(reply) = outcome.reply {
                                        websocket.send(Message::text(reply)).await?;
                                    }
                                    if let Some(payload) = outcome.broadcast {
                                        let _ = state_updates.send(payload);
                                    }
                                }
                                if let Some(name) = switch_session_target(&command) {
                                    let _ = state_updates.send(activate_session_json(
                                        name,
                                        client_context.pane_id.as_deref(),
                                    ));
                                    tokio::task::yield_now().await;
                                }
                                if let Some(payload) = state_source
                                    .as_ref()
                                    .and_then(|state_source| state_source.handle_client_command_with_context(&command, Some(&client_context)))
                                {
                                    if is_client_view_command(&command) {
                                        websocket.send(Message::text(payload)).await?;
                                    } else {
                                        let _ = state_updates.send(payload);
                                    }
                                }
                            }
                        }
                        Some(Err(err)) => return Err(err.into()),
                        None => return Ok(()),
                    }
                }
                payload = state_update_rx.recv() => {
                    match payload {
                        Ok(payload) => {
                            if payload == QUIT_JSON {
                                let _ = websocket.send(Message::text(QUIT_JSON)).await;
                                return Ok(());
                            }
                            websocket.send(Message::text(payload)).await?;
                        }
                        Err(broadcast::error::RecvError::Closed) => return Ok(()),
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            debug_log(format!("ws: state_update_rx lagged by {n} messages"));
                        }
                    }
                }
            }
        }
    }

    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 19\r\n\r\nopensessions server")
        .await?;
    Ok(())
}

fn collapsed_worktree_group_set(state: &ServerState) -> HashSet<String> {
    state.collapsed_worktree_groups.iter().cloned().collect()
}

fn session_projection_options<'a>(
    state: &ServerState,
    collapsed_groups: &'a HashSet<String>,
    provider_filter: Option<&'a str>,
) -> SessionProjectionOptions<'a> {
    SessionProjectionOptions {
        filter: state.session_filter.unwrap_or_default(),
        collapsed_groups,
        provider_filter,
    }
}

async fn read_http_header(stream: &mut TcpStream) -> Result<Vec<u8>, ServerError> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];

    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            return Err(ServerError::new("client closed before sending request"));
        }
        request.extend_from_slice(&buffer[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(request);
        }
        if request.len() > MAX_HTTP_HEADER_BYTES {
            return Err(ServerError::new("http request headers exceeded limit"));
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct HttpRequest {
    method: String,
    path: String,
    query: Option<String>,
    headers: Vec<(String, String)>,
}

impl HttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header_name, _)| header_name == name)
            .map(|(_, value)| value.as_str())
    }

    fn is_websocket_upgrade(&self) -> bool {
        self.header("upgrade")
            .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
            && self
                .header("connection")
                .is_some_and(|value| contains_token_ignore_ascii_case(value, "upgrade"))
    }

    fn content_length(&self) -> usize {
        self.header("content-length")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0)
    }

    fn query_param(&self, name: &str) -> Option<&str> {
        self.query.as_deref()?.split('&').find_map(|part| {
            let (key, value) = part.split_once('=')?;
            (key == name).then_some(value)
        })
    }
}

fn parse_http_request(bytes: &[u8]) -> Result<HttpRequest, ServerError> {
    let header_end = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| ServerError::new("http request missing header terminator"))?;
    let text = std::str::from_utf8(&bytes[..header_end])
        .map_err(|_| ServerError::new("http request headers were not utf-8"))?;
    let mut lines = text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| ServerError::new("http request missing request line"))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| ServerError::new("http request missing method"))?
        .to_string();
    let target = request_parts
        .next()
        .ok_or_else(|| ServerError::new("http request missing target"))?;
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path.to_string(), Some(query.to_string())),
        None => (target.to_string(), None),
    };

    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();

    Ok(HttpRequest {
        method,
        path,
        query,
        headers,
    })
}

fn contains_token_ignore_ascii_case(value: &str, needle: &str) -> bool {
    value
        .split(',')
        .any(|token| token.trim().eq_ignore_ascii_case(needle))
}

fn is_metadata_path(path: &str) -> bool {
    matches!(
        path,
        "/set-status" | "/set-progress" | "/log" | "/notify" | "/clear-log"
    )
}

fn is_ok_hook_path(path: &str) -> bool {
    matches!(
        path,
        "/pane-exited" | "/pane-layout-changed" | "/client-resized" | "/ensure-sidebar" | "/toggle"
    )
}

async fn read_remaining_http_body(
    stream: &mut TcpStream,
    request: &mut Vec<u8>,
    content_length: usize,
) -> Result<(), ServerError> {
    let remaining = content_length.saturating_sub(http_body(request).len());
    if remaining == 0 {
        return Ok(());
    }

    let start_len = request.len();
    request.resize(start_len + remaining, 0);
    stream.read_exact(&mut request[start_len..]).await?;
    Ok(())
}

fn http_body(request: &[u8]) -> &[u8] {
    let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
        return &[];
    };
    &request[header_end + 4..]
}

fn websocket_accept(key: &str) -> String {
    let mut sha1 = Sha1::new();
    sha1.update(key.as_bytes());
    sha1.update(WEBSOCKET_GUID.as_bytes());
    STANDARD.encode(sha1.digest().bytes())
}

fn is_quit_command(message: &Message) -> bool {
    is_command_type(message, "quit")
}

fn is_command_type(message: &Message, command_type: &str) -> bool {
    parse_command(message)
        .and_then(|value| {
            value
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .as_deref()
        == Some(command_type)
}

fn is_client_view_command(command: &Value) -> bool {
    matches!(
        command.get("type").and_then(Value::as_str),
        Some("switch-session" | "switch-index")
    )
}

fn switch_session_target(command: &Value) -> Option<String> {
    (command.get("type").and_then(Value::as_str) == Some("switch-session"))
        .then(|| command.get("name")?.as_str().map(str::to_string))?
}

fn ssh_node_config_from_value(value: &Value) -> Option<SshNodeConfig> {
    let node_id = value.get("nodeId")?.as_str()?.trim().to_string();
    let host = value.get("host")?.as_str()?.trim().to_string();
    if node_id.is_empty() || host.is_empty() {
        return None;
    }
    Some(SshNodeConfig {
        node_id,
        host,
        user: value
            .get("user")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        identity_file: value
            .get("identityFile")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        port: value
            .get("port")
            .and_then(Value::as_u64)
            .and_then(|port| u16::try_from(port).ok()),
        provider_id: value
            .get("providerId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        tmux_socket: value
            .get("tmuxSocket")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    })
}

fn persist_ssh_node(node: &SshNodeConfig) -> std::io::Result<()> {
    update_ssh_nodes_config(|nodes| {
        nodes.retain(|candidate| candidate.node_id != node.node_id);
        nodes.push(node.clone());
    })
}

fn remove_ssh_node(node_id: &str) -> std::io::Result<()> {
    update_ssh_nodes_config(|nodes| nodes.retain(|candidate| candidate.node_id != node_id))
}

fn update_ssh_nodes_config(update: impl FnOnce(&mut Vec<SshNodeConfig>)) -> std::io::Result<()> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME is unset"))?;
    let path = config_path_from_home(&home);
    let mut config = fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|value| serde_json::from_value::<OpensessionsConfig>(value).ok())
        .unwrap_or_default();
    update(&mut config.ssh_nodes);
    config.ssh_nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&config).map_err(std::io::Error::other)?
        ),
    )
}

fn registered_ssh_attach_command(
    node_id: &str,
    provider_id: &str,
    session_name: &str,
    client_size: Option<(u32, u32)>,
) -> Option<String> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let config = load_config_from_home(&home);
    let node = config.ssh_nodes.into_iter().find(|node| node.node_id == node_id)?;
    let configured_provider = node.provider_id.as_deref().unwrap_or("default");
    if configured_provider != provider_id && provider_id != "default" {
        return None;
    }
    Some(ssh_attach_command(&node, session_name, client_size))
}

fn ssh_attach_command(
    node: &SshNodeConfig,
    session_name: &str,
    client_size: Option<(u32, u32)>,
) -> String {
    let mut words = vec!["ssh".to_string()];
    if let Some(identity_file) = &node.identity_file {
        words.extend(["-i".to_string(), shell_quote(identity_file)]);
    }
    if let Some(port) = node.port {
        words.extend(["-p".to_string(), port.to_string()]);
    }
    words.push("-tt".to_string());
    let destination = match &node.user {
        Some(user) => format!("{user}@{}", node.host),
        None => node.host.clone(),
    };
    words.push(shell_quote(&destination));
    let mut tmux = vec!["tmux".to_string()];
    if let Some(socket) = &node.tmux_socket {
        tmux.extend(["-L".to_string(), shell_quote(socket)]);
    }
    tmux.extend([
        "attach-session".to_string(),
        "-t".to_string(),
        shell_quote(session_name),
    ]);
    let (cols, rows) = client_size.unwrap_or((80, 24));
    let remote_command = format!(
        "stty cols {cols} rows {rows} 2>/dev/null; TERM=xterm-256color exec {}",
        tmux.join(" "),
    );
    words.push(shell_quote(&remote_command));
    words.join(" ")
}

fn opensessions_managed_ssh_attach_command(
    node_id: &str,
    attach_command: &str,
    client_size: Option<(u32, u32)>,
) -> String {
    if let Some(bridge) = opensessions_bridge_command_path() {
        let mut words = vec![
            shell_quote(&bridge),
            "--node-id".to_string(),
            shell_quote(node_id),
            "--attach-command".to_string(),
            shell_quote(attach_command),
        ];
        if let Some((cols, rows)) = client_size {
            words.extend([
                "--cols".to_string(),
                cols.to_string(),
                "--rows".to_string(),
                rows.to_string(),
            ]);
        }
        return words.join(" ");
    }
    let managed_script = std::env::var("OPENSESSIONS_SSH_ATTACH_WRAPPER")
        .ok()
        .filter(|value| !value.is_empty());
    match managed_script {
        Some(script) => {
            let size_env = client_size
                .map(|(cols, rows)| {
                    format!(
                        "OPENSESSIONS_ATTACH_COLS={} OPENSESSIONS_ATTACH_ROWS={} ",
                        cols, rows
                    )
                })
                .unwrap_or_default();
            format!(
                "{}{} {} {}",
                size_env,
                shell_quote(&script),
                shell_quote(node_id),
                shell_quote(attach_command),
            )
        }
        None => attach_command.to_string(),
    }
}

fn opensessions_bridge_command_path() -> Option<String> {
    if let Some(path) = std::env::var("OPENSESSIONS_SSH_BRIDGE")
        .ok()
        .filter(|value| !value.is_empty())
    {
        return Some(path);
    }
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.join("opensessions-bridge");
    candidate.exists().then(|| candidate.to_string_lossy().into_owned())
}

fn terminate_managed_ssh_bridges(return_command: Option<&str>) {
    let state_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("opensessions-ssh-bridges");
    let Ok(entries) = fs::read_dir(state_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("pid") {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(pid) = raw.trim().parse::<u32>() else {
            continue;
        };
        if let Some(return_command) = return_command {
            let _ = fs::write(path.with_extension("pid.return"), return_command);
        }
        let _ = process::Command::new("kill").arg(pid.to_string()).status();
    }
}

fn enqueue_switch_intent_if_configured(
    actor_id: &str,
    node_id: &str,
    provider_id: &str,
    session_name: &str,
    client_tty: Option<&str>,
    ts: u64,
) -> bool {
    let Some(base_url) = std::env::var("OPENSESSIONS_CLOUD_URL")
        .ok()
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    let Some(api_key) = std::env::var("OPENSESSIONS_CLOUD_API_KEY")
        .ok()
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    let intent = serde_json::json!({
        "id": format!("{actor_id}:{node_id}:{provider_id}:{session_name}:{ts}"),
        "action": "switchSession",
        "nodeId": node_id,
        "providerId": provider_id,
        "session": session_name,
        "payload": {
            "requestedBy": actor_id,
            "clientTty": client_tty,
        },
        "ts": ts,
    });
    tokio::spawn(async move {
        if let Err(err) = post_command_intent(&base_url, &api_key, &intent).await {
            debug_log(format!("opensessions-cloud enqueue intent failed: {err}"));
        }
    });
    true
}

#[cfg(test)]
fn remote_attach_command_from_spec(
    raw: &str,
    node_id: &str,
    provider_id: &str,
    session_name: &str,
) -> Option<String> {
    raw.split(',').find_map(|entry| {
        let (entry_node_id, template) = entry.split_once('=')?;
        (entry_node_id.trim() == node_id).then(|| {
            template
                .trim()
                .replace("{node}", &shell_quote(node_id))
                .replace("{provider}", &shell_quote(provider_id))
                .replace("{session}", &shell_quote(session_name))
        })
    })
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '/' | '.' | ':' | '@'))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn fetch_query_reply(state_source: &dyn StateSource, command: &Value) -> Option<String> {
    if command.get("type").and_then(Value::as_str)? != "fetch-query" {
        return None;
    }
    let key = serde_json::from_value(command.get("key")?.clone()).ok()?;
    state_source.query_result_json(key)
}

fn clamp_detail_panel_height(height: u16) -> u16 {
    height.clamp(MIN_DETAIL_PANEL_HEIGHT, MAX_DETAIL_PANEL_HEIGHT)
}

fn parse_command(message: &Message) -> Option<Value> {
    serde_json::from_str::<Value>(message.as_text()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use opensessions_runtime::mux::{ClientFocus, MuxSessionInfo};
    use opensessions_runtime::protocol::{AgentStatus, LocalLink, SessionData};
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    #[derive(Default)]
    struct CountingProvider {
        list_sessions_calls: AtomicUsize,
    }

    impl MuxProvider for CountingProvider {
        fn name(&self) -> &str {
            "counting"
        }

        fn list_sessions(&self) -> Vec<MuxSessionInfo> {
            self.list_sessions_calls
                .fetch_add(1, AtomicOrdering::Relaxed);
            vec![MuxSessionInfo {
                name: "work".to_string(),
                created_at: 1,
                dir: "/repo".to_string(),
                windows: 1,
            }]
        }

        fn switch_session(&self, _name: &str, _client_tty: Option<&str>) {}

        fn get_current_session(&self) -> Option<String> {
            Some("work".to_string())
        }

        fn get_session_dir(&self, _name: &str) -> String {
            "/repo".to_string()
        }

        fn get_pane_count(&self, _name: &str) -> u32 {
            1
        }

        fn get_client_tty(&self) -> String {
            "/dev/ttys001".to_string()
        }

        fn create_session(&self, _name: Option<&str>, _dir: Option<&str>) {}
        fn kill_session(&self, _name: &str) {}
        fn setup_hooks(&self, _server_host: &str, _server_port: u16) {}
        fn cleanup_hooks(&self) {}
    }

    #[derive(Default)]
    struct JumpProvider {
        node_id: &'static str,
        name: &'static str,
        current_session: Option<&'static str>,
        attach_command: Option<&'static str>,
        detach_calls: Mutex<Vec<(Option<String>, String)>>,
        switch_calls: Mutex<Vec<(String, Option<String>)>>,
    }

    impl MuxProvider for JumpProvider {
        fn node_id(&self) -> &str {
            self.node_id
        }

        fn name(&self) -> &str {
            self.name
        }

        fn list_sessions(&self) -> Vec<MuxSessionInfo> {
            self.current_session
                .map(|name| {
                    vec![MuxSessionInfo {
                        name: name.to_string(),
                        created_at: 1,
                        dir: "/repo".to_string(),
                        windows: 1,
                    }]
                })
                .unwrap_or_default()
        }

        fn switch_session(&self, name: &str, client_tty: Option<&str>) {
            self.switch_calls
                .lock()
                .unwrap()
                .push((name.to_string(), client_tty.map(str::to_string)));
        }

        fn attach_session_command(&self, _name: &str) -> Option<String> {
            self.attach_command.map(str::to_string)
        }

        fn detach_client_and_run(&self, client_tty: Option<&str>, command: &str) {
            self.detach_calls
                .lock()
                .unwrap()
                .push((client_tty.map(str::to_string), command.to_string()));
        }

        fn get_current_session(&self) -> Option<String> {
            self.current_session.map(str::to_string)
        }

        fn get_session_dir(&self, _name: &str) -> String {
            "/repo".to_string()
        }

        fn get_pane_count(&self, _name: &str) -> u32 {
            1
        }

        fn get_client_tty(&self) -> String {
            "/dev/ttys001".to_string()
        }

        fn get_client_focus(&self, client_tty: Option<&str>) -> Option<ClientFocus> {
            (self.name == "default").then(|| ClientFocus {
                client_tty: Some(client_tty.unwrap_or("/dev/ttys001").to_string()),
                session_name: "local".to_string(),
                window_id: "@1".to_string(),
                pane_id: "%1".to_string(),
            })
        }

        fn create_session(&self, _name: Option<&str>, _dir: Option<&str>) {}
        fn kill_session(&self, _name: &str) {}
        fn setup_hooks(&self, _server_host: &str, _server_port: u16) {}
        fn cleanup_hooks(&self) {}
    }

    struct EmptyPortCommandRunner;

    impl PortCommandRunner for EmptyPortCommandRunner {
        fn process_rows(&self) -> Vec<(u32, u32)> {
            Vec::new()
        }

        fn lsof_fields(&self) -> String {
            String::new()
        }
    }

    struct EmptyGitCommandRunner;

    impl GitCommandRunner for EmptyGitCommandRunner {
        fn git_info_output(&self, _dir: &str) -> String {
            String::new()
        }
    }

    fn state() -> ServerState {
        ServerState {
            sessions: vec![SessionData {
                node_id: "local".to_string(),
                provider_id: "tmux".to_string(),
                name: "work".to_string(),
                created_at: 1,
                dir: "/repo".to_string(),
                branch: "main".to_string(),
                dirty: false,
                changed_files: 0,
                insertions: 0,
                deletions: 0,
                is_worktree: false,
                unseen: true,
                panes: 1,
                ports: Vec::new(),
                local_links: Vec::<LocalLink>::new(),
                windows: 1,
                uptime: "0m".to_string(),
                agent_state: Some(AgentEvent {
                    agent: "amp".to_string(),
                    node_id: "local".to_string(),
                    provider_id: "tmux".to_string(),
                    session: "work".to_string(),
                    status: AgentStatus::Running,
                    ts: 10,
                    thread_id: None,
                    thread_name: None,
                    last_user_prompt: None,
                    unseen: Some(true),
                    pane_id: None,
                    liveness: None,
                }),
                agents: Vec::new(),
                event_timestamps: vec![10],
                metadata: None,
            }],
            focused_session: Some("work".to_string()),
            current_session: Some("work".to_string()),
            theme: Some("dark".to_string()),
            session_filter: Some(SessionFilterMode::Running),
            agent_panel_scope: AgentPanelScope::All,
            sidebar_width: 44,
            detail_panel_height: 12,
            initializing: false,
            init_label: None,
            collapsed_worktree_groups: vec!["/repo".to_string()],
            ts: 99,
        }
    }

    #[test]
    fn query_data_from_state_returns_typed_read_models() {
        match query_data_from_state(ServerQueryKey::Settings, state()) {
            ServerQueryData::Settings {
                theme,
                session_filter,
                provider_filter,
                agent_panel_scope,
            } => {
                assert_eq!(theme.as_deref(), Some("dark"));
                assert_eq!(session_filter, Some(SessionFilterMode::Running));
                assert_eq!(provider_filter, None);
                assert_eq!(agent_panel_scope, AgentPanelScope::All);
            }
            other => panic!("expected settings query data, got {other:?}"),
        }

        match query_data_from_state(ServerQueryKey::Agents, state()) {
            ServerQueryData::Agents { sessions } => {
                assert_eq!(sessions.len(), 1);
                assert_eq!(sessions[0].session, "work");
                assert!(sessions[0].unseen);
                assert_eq!(sessions[0].event_timestamps, vec![10]);
            }
            other => panic!("expected agents query data, got {other:?}"),
        }
    }

    #[test]
    fn global_session_order_is_independent_of_local_node_append_order() {
        let mut order = SessionOrder::new(None);
        order.set_visible_order(vec!["local-a".to_string(), "local-b".to_string()]);

        let mut local_then_remote = vec![
            test_session_data("macbook", "default", "local-a", 30),
            test_session_data("macbook", "default", "local-b", 40),
            test_session_data("ovh", "default", "remote-old", 10),
            test_session_data("ovh", "default", "remote-new", 20),
        ];
        let mut remote_then_local = vec![
            test_session_data("ovh", "default", "remote-old", 10),
            test_session_data("ovh", "default", "remote-new", 20),
            test_session_data("macbook", "default", "local-a", 30),
            test_session_data("macbook", "default", "local-b", 40),
        ];

        apply_global_session_order(&mut local_then_remote, &order);
        apply_global_session_order(&mut remote_then_local, &order);

        let local_then_remote_names = local_then_remote
            .iter()
            .map(|session| session.name.as_str())
            .collect::<Vec<_>>();
        let remote_then_local_names = remote_then_local
            .iter()
            .map(|session| session.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(local_then_remote_names, remote_then_local_names);
        assert_eq!(
            local_then_remote_names,
            vec!["local-a", "local-b", "remote-old", "remote-new"]
        );
    }

    #[test]
    fn tmux_providers_from_env_names_multiple_local_sockets() {
        let providers = tmux_providers_from_env(
            &|key| match key {
                "OPENSESSIONS_TMUX_SOCKETS" => Some("default=default,test=os-test-b".to_string()),
                _ => None,
            },
            "macbook",
        );

        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0].node_id(), "macbook");
        assert_eq!(providers[0].name(), "default");
        assert_eq!(providers[1].node_id(), "macbook");
        assert_eq!(providers[1].name(), "test");
    }

    fn test_session_data(
        node_id: &str,
        provider_id: &str,
        name: &str,
        created_at: u64,
    ) -> SessionData {
        SessionData {
            node_id: node_id.to_string(),
            provider_id: provider_id.to_string(),
            name: name.to_string(),
            created_at,
            dir: "/repo".to_string(),
            branch: String::new(),
            dirty: false,
            changed_files: 0,
            insertions: 0,
            deletions: 0,
            is_worktree: false,
            unseen: false,
            panes: 1,
            ports: Vec::new(),
            local_links: Vec::new(),
            windows: 1,
            uptime: "0m".to_string(),
            agent_state: None,
            agents: Vec::new(),
            event_timestamps: Vec::new(),
            metadata: None,
        }
    }

    #[test]
    fn default_state_source_uses_explicit_tmux_sockets_without_tmux_env() {
        let source = default_state_source_from_env(|key| match key {
            "OPENSESSIONS_NODE_ID" => Some("ovh-palani".to_string()),
            "OPENSESSIONS_TMUX_SOCKETS" => Some("default=default".to_string()),
            _ => None,
        })
        .expect("state source");

        assert_eq!(source.local_node_id(), "ovh-palani");
        assert_eq!(
            source.local_node_ids(),
            HashSet::from(["ovh-palani".to_string()])
        );
    }

    #[test]
    fn http_target_appends_opensessions_path_to_cloud_base_url() {
        let target =
            http_target("http://127.0.0.1:8080/api", "/v1/opensessions/snapshot").expect("target");

        assert_eq!(target.host, "127.0.0.1");
        assert_eq!(target.port, 8080);
        assert_eq!(target.path, "/api/v1/opensessions/snapshot");
    }

    #[test]
    fn convex_url_from_cloud_url_accepts_convex_prefix() {
        assert_eq!(
            convex_url_from_cloud_url("convex:https://example.convex.cloud"),
            Some("https://example.convex.cloud")
        );
        assert_eq!(
            convex_url_from_cloud_url("convex+https://example.convex.cloud"),
            Some("https://example.convex.cloud")
        );
        assert_eq!(convex_url_from_cloud_url("http://127.0.0.1:8080"), None);
    }

    #[test]
    fn normalize_integral_json_numbers_converts_convex_float_exports() {
        let mut value = serde_json::json!({
            "ts": 1782063957253.0,
            "nested": [{ "createdAt": 42.0, "ratio": 1.5 }]
        });

        normalize_integral_json_numbers(&mut value);

        assert_eq!(value["ts"].as_u64(), Some(1782063957253));
        assert_eq!(value["nested"][0]["createdAt"].as_u64(), Some(42));
        assert_eq!(value["nested"][0]["ratio"].as_f64(), Some(1.5));
    }

    #[test]
    fn remote_attach_command_uses_matching_node_template() {
        let command = remote_attach_command_from_spec(
            "other=tmux attach -t {session},ovh-palani=ssh -t ubuntu@host tmux attach-session -t {session}",
            "ovh-palani",
            "default",
            "plane preview",
        )
        .expect("remote attach command");

        assert_eq!(
            command,
            "ssh -t ubuntu@host tmux attach-session -t 'plane preview'"
        );
    }

    #[test]
    fn registered_ssh_node_attach_command_uses_safe_tmux_attach() {
        let command = ssh_attach_command(
            &SshNodeConfig {
                node_id: "ovh-palani".to_string(),
                host: "148.113.49.189".to_string(),
                user: Some("ubuntu".to_string()),
                identity_file: Some("/Users/me/.ssh/ovh-palani".to_string()),
                port: Some(2222),
                provider_id: Some("default".to_string()),
                tmux_socket: None,
            },
            "plane preview",
            Some((186, 51)),
        );

        assert!(command.starts_with(
            "ssh -i /Users/me/.ssh/ovh-palani -p 2222 -tt ubuntu@148.113.49.189 '",
        ));
        assert!(command.contains(
            "stty cols 186 rows 51",
        ));
        assert!(command.contains("TERM=xterm-256color exec tmux attach-session -t"));
        assert!(command.contains("plane preview"));
    }

    #[test]
    fn switch_session_on_another_provider_detaches_client_into_target_attach() {
        let source_provider = Arc::new(JumpProvider {
            node_id: "macbook",
            name: "default",
            current_session: Some("local"),
            ..JumpProvider::default()
        });
        let target_provider = Arc::new(JumpProvider {
            node_id: "macbook",
            name: "extra",
            current_session: Some("extra-local"),
            attach_command: Some("tmux -L opensessions-live-extra attach-session -t extra-local"),
            ..JumpProvider::default()
        });
        let source = ReadOnlyMuxStateSource::new(vec![
            source_provider.clone() as Arc<dyn MuxProvider>,
            target_provider.clone() as Arc<dyn MuxProvider>,
        ]);

        let command = serde_json::json!({
            "type": "switch-session",
            "nodeId": "macbook",
            "providerId": "extra",
            "name": "extra-local",
            "clientTty": "/dev/ttys009",
        });
        let _ = source.handle_client_command(&command);

        assert_eq!(target_provider.switch_calls.lock().unwrap().as_slice(), []);
        assert_eq!(
            source_provider.detach_calls.lock().unwrap().as_slice(),
            [(
                Some("/dev/ttys009".to_string()),
                "tmux -L opensessions-live-extra attach-session -t extra-local".to_string(),
            )]
        );
    }

    #[test]
    fn switch_session_resolves_current_client_tty_before_detach() {
        let source_provider = Arc::new(JumpProvider {
            node_id: "macbook",
            name: "default",
            current_session: Some("local"),
            ..JumpProvider::default()
        });
        let target_provider = Arc::new(JumpProvider {
            node_id: "macbook",
            name: "extra",
            current_session: Some("extra-local"),
            attach_command: Some("tmux -L opensessions-live-extra attach-session -t extra-local"),
            ..JumpProvider::default()
        });
        let source = ReadOnlyMuxStateSource::new(vec![
            source_provider.clone() as Arc<dyn MuxProvider>,
            target_provider.clone() as Arc<dyn MuxProvider>,
        ]);

        let command = serde_json::json!({
            "type": "switch-session",
            "nodeId": "macbook",
            "providerId": "extra",
            "name": "extra-local",
        });
        let _ = source.handle_client_command(&command);

        assert_eq!(
            source_provider.detach_calls.lock().unwrap().as_slice(),
            [(
                Some("/dev/ttys001".to_string()),
                "tmux -L opensessions-live-extra attach-session -t extra-local".to_string(),
            )]
        );
    }

    #[test]
    fn query_results_are_shared_until_invalidation() {
        let provider = Arc::new(CountingProvider::default());
        let source = ReadOnlyMuxStateSource::new(vec![provider.clone()])
            .with_now_ms(|| 100_000)
            .with_port_command_runner(Arc::new(EmptyPortCommandRunner))
            .with_git_command_runner(Arc::new(EmptyGitCommandRunner));

        let _ = source.query_result_json(ServerQueryKey::Sessions);
        let calls_after_first_fetch = provider.list_sessions_calls.load(AtomicOrdering::Relaxed);
        let _ = source.query_result_json(ServerQueryKey::Sessions);

        assert_eq!(
            provider.list_sessions_calls.load(AtomicOrdering::Relaxed),
            calls_after_first_fetch,
            "same-generation query result should be reused across clients"
        );

        let _ = source.invalidate_queries_json(vec![ServerQueryKey::Sessions]);
        let _ = source.query_result_json(ServerQueryKey::Sessions);

        assert!(
            provider.list_sessions_calls.load(AtomicOrdering::Relaxed) > calls_after_first_fetch,
            "invalidating the query should force one fresh read"
        );
    }
}
