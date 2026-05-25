use std::collections::{HashMap, HashSet};
use std::fs;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::path::PathBuf;
use std::process;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Instant, SystemTime};

use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use opensessions_runtime::agent_watchers::{
    AgentWatcherSnapshot, amp_snapshot_from_thread_json, claude_code_snapshot_from_jsonl,
    codex_snapshot_from_jsonl, codex_thread_id_from_path, decode_claude_project_dir,
    opencode_snapshot_from_row, parse_codex_session_index,
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
    AgentEvent, AgentLiveness, AgentStatus, MetadataTone, ServerMessage, SessionFilterMode,
};
use opensessions_runtime::server_state::{ReadOnlyStateInput, build_read_only_state};
use opensessions_runtime::session_order::SessionOrder;
use opensessions_runtime::sidebar_coordinator::{SidebarCoordinator, SidebarWidthReportInput};
use opensessions_runtime::sidebar_width_sync::clamp_sidebar_width;
use opensessions_runtime::tmux_provider::{StdCommandRunner, TmuxProvider};
use opensessions_runtime::tracker::{AgentTracker, PanePresenceInput};
use opensessions_sidebar_core::app::App as SidebarApp;
use opensessions_sidebar_core::frame::{FrameDiff, RenderedRows, diff_rows, render_rows};
use opensessions_sidebar_core::generated::protocol::{
    ClientCommand as SidebarClientCommand, ServerMessage as SidebarServerMessage,
};
use opensessions_sidebar_core::input::{UiKey, apply_ui_key};
use opensessions_sidebar_protocol::{
    KeyCode as ShimKeyCode, KeyModifiers as ShimKeyModifiers, ServerToShim, ShimToServer,
    decode_shim_message, encode_server_message,
};
use serde_json::Value;
use sha1_smol::Sha1;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio::time::{Duration, MissedTickBehavior};
use tokio_websockets::{Message, ServerBuilder};

pub const SERVER_VERSION: &str = "0.2.0-alpha.5";
pub const PROTOCOL_VERSION: u16 = 1;
pub const HELLO_JSON: &str = r#"{"type":"hello","protocol":1,"serverVersion":"0.2.0-alpha.5"}"#;
pub const QUIT_JSON: &str = r#"{"type":"quit"}"#;

const MAX_HTTP_HEADER_BYTES: usize = 16 * 1024;
const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const SIDEBAR_SCRIPTS_DIR: &str = "apps/tui/scripts";
const GIT_CACHE_TTL_MS: u64 = 5_000;
const PORT_POLL_INTERVAL_MS: u64 = 10_000;
const RENDERED_SIDEBAR_FRAME_MS: u64 = 16;
const AGENT_WATCHER_POLL_MS: u64 = 2_000;
// Mirrors `USER_DRAG_SETTLE_MS` in `packages/runtime/src/server/index.ts`:
// once a width-report is accepted the coordinator stays in UserDrag for this
// many milliseconds, then the next snapshot tick clears it so the sidebar
// stops showing "adjusting…".
const USER_DRAG_SETTLE_MS: u64 = 600;
const AGENT_WATCHER_RECENT_MS: u64 = 5 * 60 * 1000;
const OPENCODE_SQL_TIMEOUT_MS: u64 = 500;
const OPENCODE_SQL_SEP: char = '\u{1f}';

/// Append a single debug line to the path in `OPENSESSIONS_DEBUG_LOG` (defaults
/// to `/tmp/opensessions-debug.log`). Use sparingly to trace state-machine
/// transitions in the live tmux A/B harness; the log is rotated by the user
/// (`: > /tmp/opensessions-debug.log`). Set `OPENSESSIONS_DEBUG_LOG=` (empty)
/// to silence.
fn debug_log(line: impl AsRef<str>) {
    use std::io::Write;
    let path = std::env::var("OPENSESSIONS_DEBUG_LOG")
        .unwrap_or_else(|_| "/tmp/opensessions-debug-rs.log".to_string());
    if path.is_empty() {
        return;
    }
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "[{now}] [server] {}", line.as_ref());
    }
}

pub trait StateSource: Send + Sync + 'static {
    fn snapshot_json(&self) -> String;

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

    fn handle_sender_command(&self, _command: &Value) -> Option<String> {
        None
    }

    fn handle_sender_command_with_context(
        &self,
        command: &Value,
        _context: &mut ClientConnectionContext,
    ) -> Option<String> {
        self.handle_sender_command(command)
    }

    fn handle_http_json(&self, _path: &str, _body: &Value) -> Option<String> {
        None
    }

    fn handle_http_text(&self, _path: &str, _body: &str) -> Option<String> {
        None
    }

    fn handle_http_hook(&self, _path: &str, _body: &str) {}

    fn should_report_sidebar_resize(&self, _context: &ClientConnectionContext) -> bool {
        false
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

    fn handle_switch_index(&self, _index: u32, _body: &str) {}
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ClientConnectionContext {
    client_tty: Option<String>,
    pane_id: Option<String>,
    session_name: Option<String>,
    window_id: Option<String>,
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
            Self::CouldNotResolveSession => ("404 Not Found", "could not resolve session"),
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
    F: Fn() -> String + Send + Sync + 'static,
{
    fn snapshot_json(&self) -> String {
        self()
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

        format!(
            "{}\n---\n{}",
            String::from_utf8_lossy(&rev_parse.stdout).trim(),
            String::from_utf8_lossy(&status.stdout).trim()
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

pub struct ReadOnlyMuxStateSource {
    providers: Vec<Arc<dyn MuxProvider>>,
    port_command_runner: Arc<dyn PortCommandRunner>,
    port_snapshot_cache: Mutex<Option<CachedPortSnapshot>>,
    git_command_runner: Arc<dyn GitCommandRunner>,
    git_info_cache: Mutex<HashMap<String, CachedGitInfo>>,
    sidebar_coordinator: Mutex<SidebarCoordinator>,
    sidebar_width: Mutex<u32>,
    focused_session: Mutex<Option<String>>,
    theme: Mutex<Option<String>>,
    session_filter: Mutex<Option<SessionFilterMode>>,
    session_order: Mutex<SessionOrder>,
    metadata_store: Mutex<SessionMetadataStore>,
    agent_tracker: Mutex<AgentTracker>,
    pi_runtime_registry: Mutex<PiRuntimeRegistry>,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
}

pub fn default_state_source_from_env(
    env: impl Fn(&str) -> Option<String>,
) -> Option<ReadOnlyMuxStateSource> {
    if env("TMUX").is_some() {
        let provider = Arc::new(TmuxProvider::new(Arc::new(StdCommandRunner::default())));
        let mut source = ReadOnlyMuxStateSource::new(vec![provider]);
        if let Some(width) = env("OPENSESSIONS_WIDTH").and_then(|width| width.parse::<u16>().ok()) {
            source = source.with_sidebar_width(clamp_sidebar_width(width) as u32);
        }
        return Some(source);
    }

    None
}

impl ReadOnlyMuxStateSource {
    pub fn new(providers: Vec<Arc<dyn MuxProvider>>) -> Self {
        Self {
            providers,
            port_command_runner: Arc::new(SystemPortCommandRunner),
            port_snapshot_cache: Mutex::new(None),
            git_command_runner: Arc::new(SystemGitCommandRunner),
            git_info_cache: Mutex::new(HashMap::new()),
            sidebar_coordinator: Mutex::new(SidebarCoordinator::new(26)),
            sidebar_width: Mutex::new(26),
            focused_session: Mutex::new(None),
            theme: Mutex::new(None),
            session_filter: Mutex::new(None),
            session_order: Mutex::new(SessionOrder::new(None)),
            metadata_store: Mutex::new(SessionMetadataStore::new()),
            agent_tracker: Mutex::new(AgentTracker::new()),
            pi_runtime_registry: Mutex::new(PiRuntimeRegistry::with_default_ttl()),
            now_ms: Arc::new(current_time_ms),
        }
    }

    pub fn with_sidebar_width(mut self, sidebar_width: u32) -> Self {
        self.sidebar_width = Mutex::new(sidebar_width);
        self.sidebar_coordinator = Mutex::new(SidebarCoordinator::new(sidebar_width));
        self
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
}

impl StateSource for ReadOnlyMuxStateSource {
    fn start_background_tasks(
        self: Arc<Self>,
        state_updates: broadcast::Sender<String>,
        shutdown: broadcast::Sender<()>,
    ) -> Vec<JoinHandle<()>> {
        vec![
            tokio::spawn(run_agent_watcher_loop(
                self.clone(),
                state_updates.clone(),
                shutdown.clone(),
            )),
            tokio::spawn(run_drag_settle_loop(
                self.clone(),
                state_updates.clone(),
                shutdown.clone(),
            )),
            tokio::spawn(run_tmux_state_poll_loop(self, state_updates, shutdown)),
        ]
    }

    fn snapshot_json(&self) -> String {
        let providers = self
            .providers
            .iter()
            .map(|provider| provider.as_ref())
            .collect::<Vec<_>>();
        let visible_session_names = self.visible_session_names();
        self.refresh_agent_pane_presence(visible_session_names.as_deref());
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
        let sidebar_state = {
            let mut coordinator = self.sidebar_coordinator.lock().unwrap();
            coordinator.tick_user_drag_settle((self.now_ms)(), USER_DRAG_SETTLE_MS);
            coordinator.state()
        };
        debug_log(format!(
            "snapshot_json mode={} init={} authority={:?} width={}",
            sidebar_state.mode,
            sidebar_state.initializing,
            sidebar_state.resize_authority,
            sidebar_state.width,
        ));
        let state = build_read_only_state(ReadOnlyStateInput {
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
            theme: self.theme.lock().unwrap().clone(),
            session_filter: *self.session_filter.lock().unwrap(),
            sidebar_width: *self.sidebar_width.lock().unwrap(),
            initializing: sidebar_state.initializing,
            init_label: (!sidebar_state.init_label.is_empty()).then_some(sidebar_state.init_label),
            now_ms: (self.now_ms)(),
        });

        serde_json::to_string(&ServerMessage::State(state)).expect("state must serialize")
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
                Some(self.snapshot_json())
            }
            "switch-session" => {
                let name = command.get("name")?.as_str()?;
                let client_tty = command
                    .get("clientTty")
                    .and_then(Value::as_str)
                    .or_else(|| context.and_then(|context| context.client_tty.as_deref()));
                provider.switch_session(name, client_tty);
                *self.focused_session.lock().unwrap() = Some(name.to_string());
                // Visiting a session clears its unseen agents (turns ● back
                // into ✓). Mirrors `tracker.handleFocus` in
                // `packages/runtime/src/server/index.ts:1964`.
                let had_unseen = self.agent_tracker.lock().unwrap().handle_focus(name);
                if had_unseen {
                    return Some(self.snapshot_json());
                }
                Some(format!(
                    r#"{{"type":"focus","focusedSession":"{name}","currentSession":"{name}"}}"#
                ))
            }
            "switch-index" => {
                let index = command.get("index")?.as_u64()?.min(u32::MAX as u64) as u32;
                self.switch_visible_index(index, None);
                None
            }
            "focus-session" => {
                let name = command.get("name")?.as_str()?;
                *self.focused_session.lock().unwrap() = Some(name.to_string());
                let had_unseen = self.agent_tracker.lock().unwrap().handle_focus(name);
                if had_unseen {
                    return Some(self.snapshot_json());
                }
                let current_session = provider.get_current_session();
                Some(format_focus_json(Some(name), current_session.as_deref()))
            }
            "move-focus" => {
                let delta = command.get("delta")?.as_i64()?;
                let current_session = provider.get_current_session();
                let focused = self.move_focus(delta, current_session.as_deref())?;
                Some(format_focus_json(
                    Some(&focused),
                    current_session.as_deref(),
                ))
            }
            "kill-session" => {
                let name = command.get("name")?.as_str()?;
                provider.kill_session(name);
                Some(self.snapshot_json())
            }
            "hide-session" => {
                let name = command.get("name")?.as_str()?;
                self.session_order.lock().unwrap().hide(name);
                Some(self.snapshot_json())
            }
            "show-all-sessions" => {
                self.session_order.lock().unwrap().show_all();
                Some(self.snapshot_json())
            }
            "reorder-session" => {
                let name = command.get("name")?.as_str()?;
                let delta = command.get("delta")?.as_i64()? as i8;
                self.session_order.lock().unwrap().reorder(name, delta);
                Some(self.snapshot_json())
            }
            "set-theme" => {
                let theme = command.get("theme")?.as_str()?.to_string();
                *self.theme.lock().unwrap() = Some(theme);
                Some(self.snapshot_json())
            }
            "set-filter" => {
                let filter = match command.get("filter")?.as_str()? {
                    "all" => SessionFilterMode::All,
                    "active" => SessionFilterMode::Active,
                    "running" => SessionFilterMode::Running,
                    _ => return None,
                };
                *self.session_filter.lock().unwrap() = Some(filter);
                Some(self.snapshot_json())
            }
            "report-width" => {
                let width = command.get("width")?.as_u64()?.min(u16::MAX as u64) as u16;
                let width = clamp_sidebar_width(width) as u32;
                let context = context?;
                let current_session = provider.get_current_session();
                let current_window_id = provider.get_current_window_id();
                let is_active_session =
                    context.session_name.as_deref() == current_session.as_deref();
                let is_current_window =
                    context.window_id.as_deref() == current_window_id.as_deref();
                let decision = self.sidebar_coordinator.lock().unwrap().apply_width_report(
                    SidebarWidthReportInput {
                        width,
                        session: context.session_name.clone(),
                        window_id: context.window_id.clone(),
                        is_active_session,
                        is_foreground_client: is_active_session && is_current_window,
                        is_current_window,
                        now: (self.now_ms)(),
                        suppress_ms: 500,
                    },
                );
                debug_log(format!(
                    "report-width width={width} session={:?} window={:?} \
                     active_session={is_active_session} current_window={is_current_window} \
                     accepted={} reason={}",
                    context.session_name, context.window_id, decision.accepted, decision.reason,
                ));
                if !decision.accepted {
                    return None;
                }
                *self.sidebar_width.lock().unwrap() = decision.next_width;
                self.enforce_sidebar_width(
                    decision.next_width.min(u16::MAX as u32) as u16,
                    context.pane_id.as_deref(),
                );
                Some(self.snapshot_json())
            }
            "focus-agent-pane" => {
                let session = command.get("session")?.as_str()?;
                let agent = command.get("agent")?.as_str()?;
                let thread_id = command.get("threadId").and_then(Value::as_str);
                let thread_name = command.get("threadName").and_then(Value::as_str);
                if let Some((provider, pane_id)) =
                    self.resolve_agent_pane(session, agent, thread_id, thread_name)
                {
                    provider.focus_pane(&pane_id);
                }
                None
            }
            "kill-agent-pane" => {
                let session = command.get("session")?.as_str()?;
                let agent = command.get("agent")?.as_str()?;
                let thread_id = command.get("threadId").and_then(Value::as_str);
                let thread_name = command.get("threadName").and_then(Value::as_str);
                if let Some((provider, pane_id)) =
                    self.resolve_agent_pane(session, agent, thread_id, thread_name)
                {
                    provider.kill_pane(&pane_id);
                }
                None
            }
            _ => None,
        }
    }

    fn handle_sender_command(&self, command: &Value) -> Option<String> {
        self.handle_sender_command_with_context(command, &mut ClientConnectionContext::default())
    }

    fn handle_sender_command_with_context(
        &self,
        command: &Value,
        context: &mut ClientConnectionContext,
    ) -> Option<String> {
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
            "identify-pane session={:?} pane={:?} window={:?} -> mark_ready",
            context.session_name, context.pane_id, context.window_id,
        ));
        self.sidebar_coordinator.lock().unwrap().mark_ready();
        let client_tty = self.providers.first()?.get_client_tty();
        Some(format!(
            r#"{{"type":"your-session","name":{},"clientTty":{}}}"#,
            json_string_or_null(Some(session_name)),
            json_string_or_null(Some(&client_tty)),
        ))
    }

    fn handle_http_json(&self, path: &str, body: &Value) -> Option<String> {
        match path {
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
        Some(self.snapshot_json())
    }

    fn handle_agent_event_json(&self, body: &Value) -> Result<String, AgentEventError> {
        self.apply_agent_event(body)?;
        Ok(self.snapshot_json())
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
        let name = parse_context_session(body).or_else(|| parse_legacy_focus_session(body))?;
        *self.focused_session.lock().unwrap() = Some(name.clone());
        // Visiting (focusing) a session clears its unseen agents — `●`
        // (notification) becomes `✓` (done). Mirrors `handleFocus` in
        // `packages/runtime/src/server/index.ts`.
        let had_unseen = self.agent_tracker.lock().unwrap().handle_focus(&name);
        if had_unseen {
            return Some(self.snapshot_json());
        }
        let current_session = self.providers.first()?.get_current_session();
        Some(format_focus_json(Some(&name), current_session.as_deref()))
    }

    fn handle_http_hook(&self, path: &str, body: &str) {
        match path {
            "/toggle" => self.toggle_sidebar(),
            "/ensure-sidebar" => self.ensure_sidebar(body),
            "/pane-exited" => {
                for provider in &self.providers {
                    provider.kill_orphaned_sidebar_panes();
                }
            }
            "/suppress-width-reports" => {
                self.sidebar_coordinator
                    .lock()
                    .unwrap()
                    .suppress_width_reports((self.now_ms)() + 500);
            }
            "/client-resized" => {
                let now = (self.now_ms)();
                self.sidebar_coordinator
                    .lock()
                    .unwrap()
                    .begin_client_resize_sync(now + 500, now + 700);
                let width = (*self.sidebar_width.lock().unwrap()).min(u16::MAX as u32) as u16;
                self.enforce_sidebar_width(width, None);
                self.sidebar_coordinator
                    .lock()
                    .unwrap()
                    .finish_client_resize_sync();
            }
            _ => {}
        }
    }

    fn should_report_sidebar_resize(&self, context: &ClientConnectionContext) -> bool {
        let Some(session_name) = context.session_name.as_deref() else {
            return false;
        };
        let Some(window_id) = context.window_id.as_deref() else {
            return false;
        };
        let Some(provider) = self.providers.first() else {
            return false;
        };
        provider.get_current_session().as_deref() == Some(session_name)
            && provider.get_current_window_id().as_deref() == Some(window_id)
    }

    fn handle_switch_index(&self, index: u32, body: &str) {
        let client_tty = parse_context(body).and_then(|context| context.client_tty);
        self.switch_visible_index(index, client_tty.as_deref());
    }
}

impl ReadOnlyMuxStateSource {
    fn refresh_agent_pane_presence(&self, visible_session_names: Option<&[String]>) {
        let visible =
            visible_session_names.map(|names| names.iter().cloned().collect::<HashSet<_>>());
        let mut tracker = self.agent_tracker.lock().unwrap();
        for provider in &self.providers {
            for session in provider.list_sessions() {
                if visible
                    .as_ref()
                    .is_some_and(|visible| !visible.contains(&session.name))
                {
                    continue;
                }
                let panes = provider
                    .list_agent_panes(&session.name)
                    .into_iter()
                    .map(|pane| PanePresenceInput {
                        agent: pane.agent,
                        pane_id: pane.pane_id,
                        thread_id: pane.thread_id,
                        thread_name: pane.thread_name,
                    })
                    .collect::<Vec<_>>();
                tracker.apply_pane_presence(&session.name, panes);
            }
        }
    }

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
        self.agent_tracker.lock().unwrap().apply_event(AgentEvent {
            agent,
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
            unseen: None,
            pane_id: None,
            liveness: None,
        });
        Ok(())
    }

    fn apply_agent_watcher_snapshot(&self, snapshot: AgentWatcherSnapshot) -> bool {
        if snapshot.status == AgentStatus::Idle {
            return false;
        }
        let Some(session) = self.resolve_agent_watcher_session(&snapshot) else {
            return false;
        };
        self.agent_tracker.lock().unwrap().apply_event(AgentEvent {
            agent: snapshot.agent.to_string(),
            session,
            status: snapshot.status,
            ts: snapshot.ts,
            thread_id: snapshot.thread_id,
            thread_name: snapshot.thread_name,
            unseen: None,
            pane_id: None,
            liveness: None,
        });
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

        if let Some(tmux_session) = body.get("tmuxSession").and_then(Value::as_str) {
            if sessions.iter().any(|session| session.name == tmux_session) {
                return Some(tmux_session.to_string());
            }
        }

        let project_dir = body.get("projectDir")?.as_str()?;
        let dir_session_map = build_dir_session_map(
            sessions
                .into_iter()
                .map(|session| (session.name, session.dir)),
        );
        resolve_session_for_project_dir(project_dir, &dir_session_map)
    }

    fn resolve_agent_pane(
        &self,
        session: &str,
        agent: &str,
        thread_id: Option<&str>,
        thread_name: Option<&str>,
    ) -> Option<(Arc<dyn MuxProvider>, String)> {
        let provider = self.provider_for_session(session)?;
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

    fn enforce_sidebar_width(&self, width: u16, except_pane_id: Option<&str>) {
        for provider in &self.providers {
            if !provider.is_sidebar_capable() {
                continue;
            }
            for pane in provider.list_sidebar_panes(None) {
                if except_pane_id == Some(pane.pane_id.as_str()) {
                    continue;
                }
                provider.resize_sidebar_pane(&pane.pane_id, width);
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
        if let Some(cached) = self.git_info_cache.lock().unwrap().get(dir).cloned() {
            if now.saturating_sub(cached.ts) < GIT_CACHE_TTL_MS {
                return cached.info;
            }
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
        if let Some(cached) = self.port_snapshot_cache.lock().unwrap().clone() {
            if cached.session_names == session_names
                && now.saturating_sub(cached.ts) < PORT_POLL_INTERVAL_MS
            {
                return Some(cached.ports_by_session);
            }
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

    fn toggle_sidebar(&self) {
        let providers = self
            .providers
            .iter()
            .filter(|provider| provider.is_full_sidebar_capable())
            .collect::<Vec<_>>();
        let panes_by_provider = providers
            .iter()
            .map(|provider| (*provider, provider.list_sidebar_panes(None)))
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

        self.sidebar_coordinator.lock().unwrap().begin_warmup();
        let width = (*self.sidebar_width.lock().unwrap()).min(u16::MAX as u32) as u16;
        for provider in providers {
            let mut unique_windows = Vec::<ActiveWindow>::new();
            for window in provider.list_active_windows() {
                if let Some(current) = unique_windows
                    .iter_mut()
                    .find(|current| current.id == window.id)
                {
                    if !current.active && window.active {
                        *current = window;
                    } else {
                        debug_log(format!(
                            "toggle_sidebar: skipping duplicate linked window session={} window={}",
                            window.session_name, window.id,
                        ));
                    }
                    continue;
                }
                unique_windows.push(window);
            }

            for window in unique_windows {
                debug_log(format!(
                    "toggle_sidebar: spawning in session={} window={} width={width}",
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
        }
    }

    fn ensure_sidebar(&self, body: &str) {
        let context = parse_context(body);
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
                .list_sidebar_panes(None)
                .iter()
                .any(|pane| pane.window_id == window_id)
            {
                continue;
            }
            self.sidebar_coordinator.lock().unwrap().begin_warmup();
            let width = (*self.sidebar_width.lock().unwrap()).min(u16::MAX as u32) as u16;
            provider.spawn_sidebar(
                &session_name,
                &window_id,
                width,
                SidebarPosition::Left,
                SIDEBAR_SCRIPTS_DIR,
            );
        }
    }

    fn switch_visible_index(&self, index: u32, client_tty: Option<&str>) {
        let Some(provider) = self.providers.first() else {
            return;
        };
        let Some(target_index) = index.checked_sub(1).map(|index| index as usize) else {
            return;
        };
        let Some(name) = self
            .visible_session_names()
            .and_then(|names| names.get(target_index).cloned())
        else {
            return;
        };
        provider.switch_session(&name, client_tty);
    }

    fn move_focus(&self, delta: i64, current_session: Option<&str>) -> Option<String> {
        let mut names = self.visible_session_names()?;
        if names.is_empty() {
            *self.focused_session.lock().unwrap() = None;
            return None;
        }

        let focused = self
            .focused_session
            .lock()
            .unwrap()
            .clone()
            .or_else(|| current_session.map(ToString::to_string));
        let current_idx = focused
            .and_then(|focused| names.iter().position(|name| name == &focused))
            .unwrap_or(0);
        let max_idx = names.len() - 1;
        let next_idx = (current_idx as i64 + delta).clamp(0, max_idx as i64) as usize;
        let next = names.swap_remove(next_idx);
        *self.focused_session.lock().unwrap() = Some(next.clone());
        Some(next)
    }

    fn visible_session_names(&self) -> Option<Vec<String>> {
        let names = self.sorted_session_names();
        let mut session_order = self.session_order.lock().unwrap();
        session_order.sync(names.clone());
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

/// Background ticker that polls the sidebar coordinator's UserDrag settle
/// state and broadcasts a fresh snapshot the moment the settle window
/// expires. Without this loop the websocket only sends an "adjusting…"
/// state when the width report is processed and never sends the cleared
/// "ready" state once the user stops resizing. Mirrors the
/// `setTimeout(USER_DRAG_SETTLE_MS)` callback in
/// `packages/runtime/src/server/index.ts::startTransientSidebarResize`.
async fn run_drag_settle_loop(
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
                let cleared = {
                    let mut coordinator = source.sidebar_coordinator.lock().unwrap();
                    let was_drag = coordinator.state().resize_authority
                        == opensessions_runtime::sidebar_coordinator::SidebarResizeAuthority::UserDrag;
                    coordinator.tick_user_drag_settle(now, USER_DRAG_SETTLE_MS);
                    let is_drag = coordinator.state().resize_authority
                        == opensessions_runtime::sidebar_coordinator::SidebarResizeAuthority::UserDrag;
                    was_drag && !is_drag
                };
                if cleared {
                    debug_log("drag_settle_loop: UserDrag cleared, broadcasting fresh state");
                    let _ = state_updates.send(source.snapshot_json());
                }
            }
        }
    }
}

/// Poll tmux state on a fixed cadence and broadcast a fresh snapshot whenever
/// the JSON differs from the last broadcast. Mirrors the periodic
/// session/window/pane refresh in `packages/runtime/src/server/index.ts`'s
/// `setInterval` so the sidebar picks up new sessions, agent panes, focus
/// changes, and width updates without requiring an explicit hook.
async fn run_tmux_state_poll_loop(
    source: Arc<ReadOnlyMuxStateSource>,
    state_updates: broadcast::Sender<String>,
    shutdown: broadcast::Sender<()>,
) {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut shutdown_rx = shutdown.subscribe();
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    // Seed `last_hash` from the current state so the first tick does not
    // broadcast an unprovoked snapshot. Subsequent broadcasts only happen
    // when something other than `ts` actually changes.
    let mut last_hash: u64 = {
        let mut hasher = DefaultHasher::new();
        strip_ts_field(&source.snapshot_json()).hash(&mut hasher);
        hasher.finish()
    };
    // Track the last observed current session so we can clear unseen-agent
    // flags whenever the user moves into a different tmux session externally
    // (e.g. via `tmux switch-client`). This complements the inline
    // `handle_focus` calls in switch-session / focus-session / `/focus`
    // command handlers.
    let mut last_current_session: Option<String> = source
        .providers
        .first()
        .and_then(|provider| provider.get_current_session());

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => return,
            _ = interval.tick() => {
                // Re-enforce the configured sidebar width on every tick so
                // tmux pane resizes by other clients are corrected. Mirrors
                // `enforceSidebarWidth` in the TS server's tick.
                let width = (*source.sidebar_width.lock().unwrap()).min(u16::MAX as u32) as u16;
                source.enforce_sidebar_width(width, None);

                // Visiting (= becoming the current tmux session) clears the
                // unseen-agents notification dot for that session, so `●`
                // turns back into `✓`. Mirrors `tracker.handleFocus` in TS
                // (`packages/runtime/src/server/index.ts`).
                let current_session = source
                    .providers
                    .first()
                    .and_then(|provider| provider.get_current_session());
                if current_session != last_current_session {
                    if let Some(name) = current_session.as_deref() {
                        source.agent_tracker.lock().unwrap().handle_focus(name);
                    }
                    last_current_session = current_session;
                }

                let snapshot = source.snapshot_json();
                // Hash the snapshot ignoring the per-tick `ts` field so that
                // identical state on consecutive ticks does not trigger a
                // wasteful re-broadcast. Anything else changing (sessions,
                // panes, widths, init state, focus) flips the hash and the
                // sidebar receives a fresh state.
                let stripped = strip_ts_field(&snapshot);
                let mut hasher = DefaultHasher::new();
                stripped.hash(&mut hasher);
                let hash = hasher.finish();
                if hash != last_hash {
                    last_hash = hash;
                    debug_log("tmux_state_poll_loop: state changed, broadcasting");
                    let _ = state_updates.send(snapshot);
                }
            }
        }
    }
}

/// Remove `,"ts":\d+` (or leading variant) from a JSON snapshot string so a
/// monotonically increasing timestamp does not defeat the change-detection
/// hash in `run_tmux_state_poll_loop`. Cheap byte scan; no full JSON parse.
fn strip_ts_field(snapshot: &str) -> String {
    let mut out = String::with_capacity(snapshot.len());
    let bytes = snapshot.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &snapshot[i..];
        let key = "\"ts\":";
        if rest.starts_with(key)
            || rest.starts_with(&format!(",{key}"))
            || rest.starts_with(&format!("{{{key}"))
        {
            // Preserve a leading `,` or `{` while dropping the rest of the
            // `"ts":<digits>` token.
            let mut prefix_len = 0;
            if rest.starts_with(',') || rest.starts_with('{') {
                prefix_len = 1;
                out.push(rest.chars().next().unwrap());
            }
            // Skip past `"ts":`
            let mut j = i + prefix_len + key.len();
            // Skip digits.
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            // If we left a leading `,`, also drop a trailing `,` to avoid
            // doubling separators when ts was sandwiched.
            if prefix_len == 1 && bytes.get(i) == Some(&b',') && bytes.get(j) == Some(&b',') {
                j += 1;
            }
            i = j;
            continue;
        }
        out.push(snapshot[i..].chars().next().unwrap());
        i += snapshot[i..].chars().next().unwrap().len_utf8();
    }
    out
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
                        let _ = state_updates.send(source.snapshot_json());
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
    project_dir: Option<String>,
}

impl From<&AgentWatcherSnapshot> for AgentWatcherFingerprint {
    fn from(snapshot: &AgentWatcherSnapshot) -> Self {
        Self {
            status: snapshot.status,
            thread_name: snapshot.thread_name.clone(),
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
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share/opencode/opencode.db"));
    if !db_path.exists() {
        return;
    }

    let stale_threshold = now_ms.saturating_sub(AGENT_WATCHER_RECENT_MS);
    let query = format!(
        "WITH recent AS MATERIALIZED (SELECT id, title, directory, time_updated FROM session WHERE time_updated > {stale_threshold} ORDER BY time_updated DESC LIMIT 50) SELECT r.id, ifnull(r.title,''), r.directory, r.time_updated, ifnull((SELECT m.data FROM message m WHERE m.session_id = r.id ORDER BY m.time_created DESC LIMIT 1),'') FROM recent r ORDER BY r.time_updated DESC;"
    );
    let mut command = process::Command::new("sqlite3");
    command
        .arg("-readonly")
        .arg("-separator")
        .arg(OPENCODE_SQL_SEP.to_string())
        .arg(&db_path)
        .arg(query);
    let Some(output) =
        run_process_with_timeout(command, Duration::from_millis(OPENCODE_SQL_TIMEOUT_MS))
    else {
        return;
    };
    if !output.status.success() {
        return;
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
            now_ms,
        ) {
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

fn format_focus_json(focused_session: Option<&str>, current_session: Option<&str>) -> String {
    format!(
        r#"{{"type":"focus","focusedSession":{},"currentSession":{}}}"#,
        json_string_or_null(focused_session),
        json_string_or_null(current_session),
    )
}

fn json_string_or_null(value: Option<&str>) -> String {
    value
        .map(|value| serde_json::to_string(value).expect("string must serialize"))
        .unwrap_or_else(|| "null".to_string())
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
}

fn parse_context(body: &str) -> Option<HttpContext> {
    let trimmed = trim_context_quotes(body);
    let pipe_parts = trimmed.split('|').collect::<Vec<_>>();
    if pipe_parts.len() == 3 && !pipe_parts[1].is_empty() && !pipe_parts[2].is_empty() {
        return Some(HttpContext {
            client_tty: (!pipe_parts[0].is_empty()).then(|| pipe_parts[0].to_string()),
            session: pipe_parts[1].to_string(),
            window_id: pipe_parts[2].to_string(),
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
    })
}

fn parse_context_session(body: &str) -> Option<String> {
    parse_context(body).map(|context| context.session)
}

fn parse_legacy_focus_session(body: &str) -> Option<String> {
    let name = trim_double_quotes(body.trim());
    (!name.is_empty()).then(|| name.to_string())
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
    shim_socket_path: Option<PathBuf>,
    state_source: Option<Arc<dyn StateSource>>,
}

impl ServerConfig {
    pub fn new(host: impl Into<String>, port: u16, pid_file: impl Into<PathBuf>) -> Self {
        Self {
            host: host.into(),
            port,
            pid_file: pid_file.into(),
            shim_socket_path: None,
            state_source: None,
        }
    }

    pub fn with_shim_socket_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.shim_socket_path = Some(path.into());
        self
    }

    pub fn with_state_source(mut self, source: impl StateSource) -> Self {
        self.state_source = Some(Arc::new(source));
        self
    }
}

#[derive(Debug)]
pub struct ServerHandle {
    addr: SocketAddr,
    shim_socket_path: Option<PathBuf>,
    shutdown: broadcast::Sender<()>,
    task: JoinHandle<Result<(), ServerError>>,
}

impl ServerHandle {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn shim_socket_path(&self) -> Option<&std::path::Path> {
        self.shim_socket_path.as_deref()
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
    let shim_socket_path = config
        .shim_socket_path
        .clone()
        .unwrap_or_else(|| default_shim_socket_path(&config.pid_file));
    if shim_socket_path.exists() {
        fs::remove_file(&shim_socket_path)?;
    }
    let shim_listener = UnixListener::bind(&shim_socket_path)?;

    fs::write(&config.pid_file, process::id().to_string())?;

    let (shutdown, shutdown_rx) = broadcast::channel(1);
    let (state_updates, _) = broadcast::channel(16);
    if let Some(source) = config.state_source.clone() {
        let _background_tasks =
            source.start_background_tasks(state_updates.clone(), shutdown.clone());
    }
    let task_shutdown = shutdown.clone();
    let state_source = config.state_source.clone();
    let cleanup_shim_socket_path = shim_socket_path.clone();
    let task = tokio::spawn(async move {
        let result = run_accept_loop(
            listener,
            task_shutdown,
            shutdown_rx,
            state_source,
            state_updates,
            shim_listener,
        )
        .await;
        let cleanup_result = fs::remove_file(&config.pid_file);
        let socket_cleanup_result = fs::remove_file(&cleanup_shim_socket_path);
        match (result, cleanup_result, socket_cleanup_result) {
            (Err(err), _, _) => Err(err),
            (Ok(()), Err(err), _) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
            (Ok(()), _, Err(err)) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
            _ => Ok(()),
        }
    });

    Ok(ServerHandle {
        addr,
        shim_socket_path: Some(shim_socket_path),
        shutdown,
        task,
    })
}

fn default_shim_socket_path(pid_file: &std::path::Path) -> PathBuf {
    let candidate = pid_file.with_extension("sock");
    if candidate.as_os_str().len() < 90 {
        return candidate;
    }

    let mut hash = 0_u32;
    for (idx, byte) in pid_file.to_string_lossy().bytes().enumerate() {
        hash = (hash + u32::from(byte) * (idx as u32 + 1)) % 100_000;
    }
    std::env::temp_dir().join(format!("opensessions-{hash}.sock"))
}

async fn run_accept_loop(
    listener: TcpListener,
    shutdown: broadcast::Sender<()>,
    mut shutdown_rx: broadcast::Receiver<()>,
    state_source: Option<Arc<dyn StateSource>>,
    state_updates: broadcast::Sender<String>,
    shim_listener: UnixListener,
) -> Result<(), ServerError> {
    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => return Ok(()),
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let connection_shutdown = shutdown.clone();
                let connection_state_source = state_source.clone();
                let connection_state_updates = state_updates.clone();
                tokio::spawn(async move {
                    let _ = handle_connection(
                        stream,
                        connection_shutdown,
                        connection_state_source,
                        connection_state_updates,
                    )
                    .await;
                });
            }
            accepted = shim_listener.accept() => {
                let (stream, _) = accepted?;
                let connection_shutdown = shutdown.clone();
                let connection_state_source = state_source.clone();
                let connection_state_updates = state_updates.clone();
                tokio::spawn(async move {
                    let _ = handle_shim_connection(
                        stream,
                        connection_shutdown,
                        connection_state_source,
                        connection_state_updates,
                    )
                    .await;
                });
            }
        }
    }
}

async fn handle_shim_connection(
    stream: UnixStream,
    shutdown: broadcast::Sender<()>,
    state_source: Option<Arc<dyn StateSource>>,
    state_updates: broadcast::Sender<String>,
) -> Result<(), ServerError> {
    let (mut reader, mut writer) = stream.into_split();
    let first = read_protocol_frame(&mut reader).await?;
    let ShimToServer::Hello(hello) = decode_shim_message(&first)
        .map_err(|err| ServerError::new(format!("invalid shim hello: {err}")))?
    else {
        return Err(ServerError::new("shim must send hello first"));
    };

    writer
        .write_all(&encode_server_message(&ServerToShim::Hello {
            protocol: PROTOCOL_VERSION,
        }))
        .await?;
    let (frames_tx, frames_rx) = watch::channel(Arc::new(Vec::<u8>::new()));
    let _writer_task = tokio::spawn(write_shim_frames(writer, frames_rx));

    let mut context = ClientConnectionContext {
        client_tty: hello.client_tty.clone(),
        pane_id: Some(hello.pane_id.clone()),
        session_name: Some(hello.session_name.clone()),
        window_id: hello.window_id.clone(),
    };
    let identify = serde_json::json!({
        "type": "identify-pane",
        "paneId": hello.pane_id,
        "sessionName": hello.session_name,
        "windowId": hello.window_id,
    });

    let mut app = state_source.as_ref().and_then(|state_source| {
        let _ = state_source.handle_sender_command_with_context(&identify, &mut context);
        app_from_state_json(&state_source.snapshot_json())
    });
    if let Some(state_source) = &state_source {
        let _ = state_updates.send(state_source.snapshot_json());
    }
    if let Some(app) = &mut app {
        app.my_session = context.session_name.clone();
    }

    let mut width = hello.width;
    let mut height = hello.height;
    let mut previous_rows = None::<RenderedRows>;
    let mut seq = 0_u32;
    if let Some(app) = &mut app {
        seq = seq.wrapping_add(1);
        let rows = render_rows(app, width, height);
        frames_tx.send_replace(Arc::new(encode_server_message(&ServerToShim::FullFrame {
            seq,
            width,
            height,
            rows: rows.rows.clone(),
        })));
        previous_rows = Some(rows);
    }

    let mut connection_shutdown = shutdown.subscribe();
    let mut state_rx = state_updates.subscribe();
    let mut render_tick = tokio::time::interval(Duration::from_millis(RENDERED_SIDEBAR_FRAME_MS));
    render_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut dirty = false;

    loop {
        tokio::select! {
            _ = connection_shutdown.recv() => {
                frames_tx.send_replace(Arc::new(encode_server_message(&ServerToShim::Quit)));
                return Ok(());
            }
            _ = render_tick.tick(), if dirty => {
                if let Some(app) = &mut app {
                    seq = seq.wrapping_add(1);
                    let rows = render_rows(app, width, height);
                    let message = match previous_rows.as_ref() {
                        Some(previous) => match diff_rows(previous, &rows) {
                            FrameDiff::Full(rows) => ServerToShim::FullFrame {
                                seq,
                                width: rows.width,
                                height: rows.height,
                                rows: rows.rows.clone(),
                            },
                            FrameDiff::Patch { width, height, changed_rows, clear_from_row } => {
                                ServerToShim::PatchFrame { seq, width, height, changed_rows, clear_from_row }
                            }
                        },
                        None => ServerToShim::FullFrame {
                            seq,
                            width: rows.width,
                            height: rows.height,
                            rows: rows.rows.clone(),
                        },
                    };
                    previous_rows = Some(rows);
                    frames_tx.send_replace(Arc::new(encode_server_message(&message)));
                }
                dirty = false;
            }
            state = state_rx.recv() => {
                match state {
                    Ok(state) => {
                        if let Ok(message) = serde_json::from_str::<SidebarServerMessage>(&state) {
                            if matches!(message, SidebarServerMessage::Quit) {
                                frames_tx.send_replace(Arc::new(encode_server_message(&ServerToShim::Quit)));
                                return Ok(());
                            }
                            apply_sidebar_server_message(&mut app, message);
                            dirty = true;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                }
            }
            frame = read_protocol_frame(&mut reader) => {
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(err) if err.message == "client closed" => return Ok(()),
                    Err(err) => return Err(err),
                };
                match decode_shim_message(&frame)
                    .map_err(|err| ServerError::new(format!("invalid shim frame: {err}")))? {
                    ShimToServer::Hello(_) => {}
                    ShimToServer::Close => return Ok(()),
                    ShimToServer::Resize { width: next_width, height: next_height } => {
                        if next_width != width && state_source.as_ref().is_some_and(|state_source| {
                            state_source.should_report_sidebar_resize(&context)
                        }) {
                            let command = serde_json::json!({
                                "type": "report-width",
                                "width": next_width,
                            });
                            if let Some(payload) = state_source
                                .as_ref()
                                .and_then(|state_source| {
                                    state_source.handle_client_command_with_context(
                                        &command,
                                        Some(&context),
                                    )
                                })
                            {
                                if let Ok(message) = serde_json::from_str::<SidebarServerMessage>(&payload) {
                                    apply_sidebar_server_message(&mut app, message);
                                }
                                let _ = state_updates.send(payload);
                            }
                        }
                        width = next_width;
                        height = next_height;
                        previous_rows = None;
                        dirty = true;
                    }
                    ShimToServer::Mouse(_) => {
                        // Mouse hit-testing is intentionally server-owned; the protocol carries
                        // coordinates now so clickable rows and drag resizing can be layered on
                        // the render model without adding Ratatui to the shim.
                    }
                    ShimToServer::Key(key) => {
                        if let Some(app) = &mut app {
                            if let Some(ui_key) = ui_key_from_shim(key.code, key.modifiers) {
                                apply_ui_key(app, ui_key);
                                if drain_sidebar_commands(
                                    app,
                                    &state_source,
                                    &state_updates,
                                    &mut context,
                                    &shutdown,
                                )? {
                                    frames_tx.send_replace(Arc::new(encode_server_message(&ServerToShim::Quit)));
                                    return Ok(());
                                }
                                dirty = true;
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn write_shim_frames(
    mut writer: tokio::net::unix::OwnedWriteHalf,
    mut frames_rx: watch::Receiver<Arc<Vec<u8>>>,
) -> Result<(), ServerError> {
    loop {
        if frames_rx.changed().await.is_err() {
            return Ok(());
        }
        let frame = frames_rx.borrow_and_update().clone();
        if frame.is_empty() {
            continue;
        }
        writer.write_all(&frame).await?;
    }
}

async fn read_protocol_frame<R>(reader: &mut R) -> Result<Vec<u8>, ServerError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut len = [0_u8; 4];
    if let Err(err) = reader.read_exact(&mut len).await {
        if err.kind() == std::io::ErrorKind::UnexpectedEof {
            return Err(ServerError::new("client closed"));
        }
        return Err(err.into());
    }
    let len = u32::from_le_bytes(len) as usize;
    let mut frame = Vec::with_capacity(4 + len);
    frame.extend_from_slice(&(len as u32).to_le_bytes());
    frame.resize(4 + len, 0);
    reader.read_exact(&mut frame[4..]).await?;
    Ok(frame)
}

fn ui_key_from_shim(code: ShimKeyCode, modifiers: ShimKeyModifiers) -> Option<UiKey> {
    if modifiers.contains(ShimKeyModifiers::ALT) {
        return match code {
            ShimKeyCode::Up => Some(UiKey::AltUp),
            ShimKeyCode::Down => Some(UiKey::AltDown),
            _ => None,
        };
    }
    if modifiers.contains(ShimKeyModifiers::CONTROL) {
        return match code {
            ShimKeyCode::Char('j') => Some(UiKey::CtrlJ),
            ShimKeyCode::Char('k') => Some(UiKey::CtrlK),
            _ => None,
        };
    }

    match code {
        ShimKeyCode::Char('j') | ShimKeyCode::Down => Some(UiKey::Down),
        ShimKeyCode::Char('k') | ShimKeyCode::Up => Some(UiKey::Up),
        ShimKeyCode::Char(ch) => Some(UiKey::Char(ch)),
        ShimKeyCode::Tab => Some(UiKey::Tab {
            shift: modifiers.contains(ShimKeyModifiers::SHIFT),
        }),
        ShimKeyCode::Enter => Some(UiKey::Enter),
        ShimKeyCode::Esc => Some(UiKey::Esc),
    }
}

fn drain_sidebar_commands(
    app: &mut SidebarApp,
    state_source: &Option<Arc<dyn StateSource>>,
    state_updates: &broadcast::Sender<String>,
    context: &mut ClientConnectionContext,
    shutdown: &broadcast::Sender<()>,
) -> Result<bool, ServerError> {
    for command in app.drain_commands() {
        if matches!(command, SidebarClientCommand::Quit) {
            let _ = state_updates.send(QUIT_JSON.to_string());
            let _ = shutdown.send(());
            return Ok(true);
        }
        let command = serde_json::to_value(command)
            .map_err(|err| ServerError::new(format!("serialize sidebar command: {err}")))?;
        if let Some(payload) = state_source.as_ref().and_then(|state_source| {
            state_source.handle_client_command_with_context(&command, Some(context))
        }) {
            if let Ok(message) = serde_json::from_str::<SidebarServerMessage>(&payload) {
                app.apply_server_message(message);
            }
            let _ = state_updates.send(payload);
        }
    }
    Ok(false)
}

async fn handle_connection(
    mut stream: TcpStream,
    shutdown: broadcast::Sender<()>,
    state_source: Option<Arc<dyn StateSource>>,
    state_updates: broadcast::Sender<String>,
) -> Result<(), ServerError> {
    let mut request = read_http_header(&mut stream).await?;
    let parsed = parse_http_request(&request)?;
    read_remaining_http_body(&mut stream, &mut request, parsed.content_length()).await?;

    if parsed.method == "POST" && parsed.path == "/refresh" {
        if let Some(state_source) = &state_source {
            let _ = state_updates.send(state_source.snapshot_json());
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
            state_source.handle_switch_index(index, &body);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .await?;
        let _ = stream.shutdown().await;
        return Ok(());
    }

    if parsed.method == "POST" && is_ok_hook_path(&parsed.path) {
        let body = String::from_utf8_lossy(http_body(&request));
        if let Some(state_source) = &state_source {
            state_source.handle_http_hook(&parsed.path, &body);
            let _ = state_updates.send(state_source.snapshot_json());
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
        if let Some(state_source) = &state_source {
            if let Err(err) = state_source.handle_pi_runtime_upsert(&body) {
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
        if let Some(state_source) = &state_source {
            if let Err(err) = state_source.handle_pi_runtime_delete(&body) {
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
        let _ = shutdown.send(());
        return Ok(());
    }

    if parsed.is_websocket_upgrade() && parsed.path == "/rendered-sidebar" {
        return handle_rendered_sidebar_connection(
            stream,
            parsed,
            shutdown,
            state_source,
            state_updates,
        )
        .await;
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
        debug_log("ws: client connected, sending hello + initial state");
        websocket.send(Message::text(HELLO_JSON)).await?;
        if let Some(state_source) = &state_source {
            websocket
                .send(Message::text(state_source.snapshot_json()))
                .await?;
        }

        let mut connection_shutdown = shutdown.subscribe();
        let mut state_rx = state_updates.subscribe();
        let mut client_context = ClientConnectionContext::default();
        loop {
            tokio::select! {
                _ = connection_shutdown.recv() => {
                    let _ = websocket.send(Message::text(QUIT_JSON)).await;
                    return Ok(());
                }
                state = state_rx.recv() => {
                    match state {
                        Ok(state) => {
                            debug_log(format!(
                                "ws: forwarding broadcast state ({} bytes) to client",
                                state.len()
                            ));
                            websocket.send(Message::text(state)).await?
                        }
                        Err(broadcast::error::RecvError::Closed) => return Ok(()),
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            debug_log(format!("ws: state_rx lagged by {n} messages"));
                        }
                    }
                }
                message = websocket.next() => {
                    match message {
                        Some(Ok(message)) if message.is_close() => return Ok(()),
                        Some(Ok(message)) => {
                            if is_quit_command(&message) {
                                let _ = state_updates.send(QUIT_JSON.to_string());
                                let _ = shutdown.send(());
                                return Ok(());
                            }
                            if is_command_type(&message, "refresh") {
                                if let Some(state_source) = &state_source {
                                    let _ = state_updates.send(state_source.snapshot_json());
                                }
                            }
                            if let Some(command) = parse_command(&message) {
                                if let Some(reply) = state_source
                                    .as_ref()
                                    .and_then(|state_source| state_source.handle_sender_command_with_context(&command, &mut client_context))
                                {
                                    websocket.send(Message::text(reply)).await?;
                                }
                                if let Some(payload) = state_source
                                    .as_ref()
                                    .and_then(|state_source| state_source.handle_client_command_with_context(&command, Some(&client_context)))
                                {
                                    let _ = state_updates.send(payload);
                                }
                            }
                        }
                        Some(Err(err)) => return Err(err.into()),
                        None => return Ok(()),
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

async fn handle_rendered_sidebar_connection(
    mut stream: TcpStream,
    parsed: HttpRequest,
    shutdown: broadcast::Sender<()>,
    state_source: Option<Arc<dyn StateSource>>,
    state_updates: broadcast::Sender<String>,
) -> Result<(), ServerError> {
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
    let mut width = parsed
        .query_param("width")
        .and_then(|width| width.parse::<u16>().ok())
        .unwrap_or(35);
    let mut height = parsed
        .query_param("height")
        .and_then(|height| height.parse::<u16>().ok())
        .unwrap_or(56);

    let mut app = state_source
        .as_ref()
        .and_then(|state_source| app_from_state_json(&state_source.snapshot_json()));
    if let Some(app) = &mut app {
        websocket
            .send(Message::text(render_sidebar_frame(app, width, height)))
            .await?;
    }

    let mut connection_shutdown = shutdown.subscribe();
    let mut state_rx = state_updates.subscribe();
    let mut render_tick = tokio::time::interval(Duration::from_millis(RENDERED_SIDEBAR_FRAME_MS));
    render_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut dirty = false;
    loop {
        tokio::select! {
            _ = connection_shutdown.recv() => {
                let _ = websocket.send(Message::text(QUIT_JSON)).await;
                return Ok(());
            }
            _ = render_tick.tick(), if dirty => {
                if let Some(app) = &mut app {
                    websocket.send(Message::text(render_sidebar_frame(app, width, height))).await?;
                }
                dirty = false;
            }
            state = state_rx.recv() => {
                match state {
                    Ok(state) => {
                        if let Ok(message) = serde_json::from_str::<SidebarServerMessage>(&state) {
                            if matches!(message, SidebarServerMessage::Quit) {
                                let _ = websocket.send(Message::text(QUIT_JSON)).await;
                                return Ok(());
                            }
                            apply_sidebar_server_message(&mut app, message);
                            dirty = true;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                }
            }
            message = websocket.next() => {
                match message {
                    Some(Ok(message)) if message.is_close() => return Ok(()),
                    Some(Ok(message)) => {
                        if is_quit_command(&message) {
                            let _ = state_updates.send(QUIT_JSON.to_string());
                            let _ = shutdown.send(());
                            return Ok(());
                        }

                        if let Some(command) = parse_command(&message) {
                            if command.get("type").and_then(Value::as_str) == Some("render-resize") {
                                width = command
                                    .get("width")
                                    .and_then(Value::as_u64)
                                    .map(|width| width.min(u16::MAX as u64) as u16)
                                    .unwrap_or(width);
                                height = command
                                    .get("height")
                                    .and_then(Value::as_u64)
                                    .map(|height| height.min(u16::MAX as u64) as u16)
                                    .unwrap_or(height);
                                dirty = true;
                                continue;
                            }

                            if command.get("type").and_then(Value::as_str) == Some("render-key") {
                                if let Some(app) = &mut app {
                                    apply_render_key(app, &command);
                                    for command in app.drain_commands() {
                                        if matches!(command, SidebarClientCommand::Quit) {
                                            let _ = state_updates.send(QUIT_JSON.to_string());
                                            let _ = shutdown.send(());
                                            return Ok(());
                                        }
                                        if let Ok(command) = serde_json::to_value(command) {
                                            if let Some(payload) = state_source
                                                .as_ref()
                                                .and_then(|state_source| state_source.handle_client_command(&command))
                                            {
                                                if let Ok(message) = serde_json::from_str::<SidebarServerMessage>(&payload) {
                                                    app.apply_server_message(message);
                                                }
                                                let _ = state_updates.send(payload);
                                            }
                                        }
                                    }
                                    dirty = true;
                                }
                                continue;
                            }

                            if let Some(payload) = state_source
                                .as_ref()
                                .and_then(|state_source| state_source.handle_client_command(&command))
                            {
                                if let Ok(message) = serde_json::from_str::<SidebarServerMessage>(&payload) {
                                    apply_sidebar_server_message(&mut app, message);
                                }
                                let _ = state_updates.send(payload);
                            }
                            dirty = true;
                        }
                    }
                    Some(Err(err)) => return Err(err.into()),
                    None => return Ok(()),
                }
            }
        }
    }
}

fn app_from_state_json(state_json: &str) -> Option<SidebarApp> {
    let SidebarServerMessage::State(state) =
        serde_json::from_str::<SidebarServerMessage>(state_json).ok()?
    else {
        return None;
    };
    Some(SidebarApp::from_state(state))
}

fn apply_sidebar_server_message(app: &mut Option<SidebarApp>, message: SidebarServerMessage) {
    match (app, message) {
        (slot @ None, SidebarServerMessage::State(state)) => {
            *slot = Some(SidebarApp::from_state(state))
        }
        (Some(app), message) => app.apply_server_message(message),
        (None, _) => {}
    }
}

fn render_sidebar_frame(app: &mut SidebarApp, width: u16, height: u16) -> String {
    let rows = render_rows(app, width, height);
    let mut frame = String::new();
    for row in rows.rows {
        frame.push_str(&String::from_utf8_lossy(&row));
        frame.push('\n');
    }
    frame
}

fn apply_render_key(app: &mut SidebarApp, command: &Value) {
    let key = command
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let alt = command.get("alt").and_then(Value::as_bool).unwrap_or(false);
    let ctrl = command
        .get("ctrl")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let shift = command
        .get("shift")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let code = match key {
        "up" => ShimKeyCode::Up,
        "down" => ShimKeyCode::Down,
        "tab" => ShimKeyCode::Tab,
        "enter" => ShimKeyCode::Enter,
        "esc" => ShimKeyCode::Esc,
        key if key.chars().count() == 1 => {
            ShimKeyCode::Char(key.chars().next().expect("single char key must exist"))
        }
        _ => return,
    };
    let mut modifiers = ShimKeyModifiers::empty();
    if alt {
        modifiers = modifiers | ShimKeyModifiers::ALT;
    }
    if ctrl {
        modifiers = modifiers | ShimKeyModifiers::CONTROL;
    }
    if shift {
        modifiers = modifiers | ShimKeyModifiers::SHIFT;
    }
    if let Some(key) = ui_key_from_shim(code, modifiers) {
        apply_ui_key(app, key);
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
        "/suppress-width-reports"
            | "/client-resized"
            | "/pane-exited"
            | "/ensure-sidebar"
            | "/toggle"
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

fn parse_command(message: &Message) -> Option<Value> {
    serde_json::from_str::<Value>(message.as_text()?).ok()
}
