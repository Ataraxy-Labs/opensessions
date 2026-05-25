use std::collections::HashMap;

use crate::git_info::GitInfo;
use crate::mux::MuxProvider;
use crate::portless::{PortlessState, build_local_links};
use crate::protocol::{AgentEvent, ServerState, SessionData, SessionFilterMode, SessionMetadata};

pub struct ReadOnlyStateInput<'a> {
    pub providers: Vec<&'a dyn MuxProvider>,
    pub visible_session_names: Option<Vec<String>>,
    pub metadata_by_session: Option<HashMap<String, SessionMetadata>>,
    pub git_by_session: Option<HashMap<String, GitInfo>>,
    pub agent_state_by_session: Option<HashMap<String, AgentEvent>>,
    pub agents_by_session: Option<HashMap<String, Vec<AgentEvent>>>,
    pub event_timestamps_by_session: Option<HashMap<String, Vec<u64>>>,
    pub unseen_sessions: Option<Vec<String>>,
    pub ports_by_session: Option<HashMap<String, Vec<u16>>>,
    pub portless_state: Option<PortlessState>,
    pub focused_session: Option<String>,
    pub theme: Option<String>,
    pub session_filter: Option<SessionFilterMode>,
    pub sidebar_width: u32,
    pub initializing: bool,
    pub init_label: Option<String>,
    pub now_ms: u64,
}

pub fn build_read_only_state(input: ReadOnlyStateInput<'_>) -> ServerState {
    let now_secs = input.now_ms / 1_000;
    let current_session = input
        .providers
        .first()
        .and_then(|provider| provider.get_current_session());

    let mut mux_sessions = Vec::new();
    for provider in &input.providers {
        for session in provider.list_sessions() {
            mux_sessions.push((*provider, session));
        }
    }
    mux_sessions.sort_by(|(_, a), (_, b)| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.name.cmp(&b.name))
    });
    if let Some(visible_session_names) = &input.visible_session_names {
        mux_sessions.sort_by_key(|(_, session)| {
            visible_session_names
                .iter()
                .position(|name| name == &session.name)
                .unwrap_or(usize::MAX)
        });
        mux_sessions.retain(|(_, session)| visible_session_names.contains(&session.name));
    }

    let mut batch_pane_counts: HashMap<&str, HashMap<String, u32>> = HashMap::new();
    for provider in &input.providers {
        if provider.is_batch_capable() {
            batch_pane_counts.insert(provider.name(), provider.get_all_pane_counts());
        }
    }

    let sessions = mux_sessions
        .into_iter()
        .map(|(provider, session)| {
            let panes = batch_pane_counts
                .get(provider.name())
                .and_then(|counts| counts.get(&session.name).copied())
                .unwrap_or_else(|| provider.get_pane_count(&session.name));
            let metadata = input
                .metadata_by_session
                .as_ref()
                .and_then(|metadata| metadata.get(&session.name).cloned());
            let git = input
                .git_by_session
                .as_ref()
                .and_then(|git| git.get(&session.name).cloned())
                .unwrap_or_else(GitInfo::empty);
            let agent_state = input
                .agent_state_by_session
                .as_ref()
                .and_then(|agents| agents.get(&session.name).cloned());
            let agents = input
                .agents_by_session
                .as_ref()
                .and_then(|agents| agents.get(&session.name).cloned())
                .unwrap_or_default();
            let event_timestamps = input
                .event_timestamps_by_session
                .as_ref()
                .and_then(|timestamps| timestamps.get(&session.name).cloned())
                .unwrap_or_default();
            let unseen = input
                .unseen_sessions
                .as_ref()
                .is_some_and(|sessions| sessions.contains(&session.name));
            let ports = input
                .ports_by_session
                .as_ref()
                .and_then(|ports| ports.get(&session.name).cloned())
                .unwrap_or_default();
            let local_links =
                build_local_links(ports.iter().copied(), input.portless_state.as_ref());

            SessionData {
                name: session.name,
                created_at: session.created_at,
                dir: session.dir,
                branch: git.branch,
                dirty: git.dirty,
                is_worktree: git.is_worktree,
                unseen,
                panes,
                ports: ports.into_iter().map(u32::from).collect(),
                local_links,
                windows: session.windows,
                uptime: format_uptime(now_secs, session.created_at),
                agent_state,
                agents,
                event_timestamps,
                metadata,
            }
        })
        .collect::<Vec<_>>();

    let focused_session =
        resolve_focused_session(input.focused_session, current_session.as_deref(), &sessions);

    ServerState {
        sessions,
        focused_session,
        current_session,
        theme: input.theme,
        session_filter: input.session_filter,
        sidebar_width: input.sidebar_width,
        initializing: input.initializing,
        init_label: input.init_label,
        ts: input.now_ms,
    }
}

fn resolve_focused_session(
    focused_session: Option<String>,
    current_session: Option<&str>,
    sessions: &[SessionData],
) -> Option<String> {
    if sessions.is_empty() {
        return None;
    }

    if let Some(focused) = focused_session {
        if sessions.iter().any(|session| session.name == focused) {
            return Some(focused);
        }
    }

    if let Some(current) = current_session {
        if sessions.iter().any(|session| session.name == current) {
            return Some(current.to_string());
        }
    }

    sessions.first().map(|session| session.name.clone())
}

fn format_uptime(now_secs: u64, created_at: u64) -> String {
    let Some(diff) = now_secs.checked_sub(created_at) else {
        return String::new();
    };

    let days = diff / 86_400;
    let hours = (diff % 86_400) / 3_600;
    let mins = (diff % 3_600) / 60;
    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{mins}m")
    } else {
        format!("{mins}m")
    }
}
