use std::collections::{HashMap, HashSet};

use crate::protocol::{AgentEvent, AgentStatus, SessionData, SessionFilterMode};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupSummary {
    pub running_agents: usize,
    pub unseen: usize,
    pub insertions: u32,
    pub deletions: u32,
    pub first_port: Option<u32>,
    pub extra_ports: usize,
}

#[derive(Debug, Clone)]
pub enum DisplaySessionEntry<'a> {
    Group {
        key: String,
        label: String,
        count: usize,
        collapsed: bool,
        summary: GroupSummary,
    },
    Session {
        index: usize,
        session: &'a SessionData,
        indented: bool,
    },
}

impl DisplaySessionEntry<'_> {
    pub fn row_height(&self) -> usize {
        match self {
            Self::Group { .. } => 1,
            Self::Session { .. } => 3,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SessionProjectionOptions<'a> {
    pub filter: SessionFilterMode,
    pub collapsed_groups: &'a HashSet<String>,
    pub provider_filter: Option<&'a str>,
}

pub fn filtered_sessions<'a>(
    sessions: &'a [SessionData],
    mode: SessionFilterMode,
) -> impl Iterator<Item = &'a SessionData> + 'a {
    sessions
        .iter()
        .filter(move |session| session_matches_filter(session, mode))
}

fn provider_filtered_sessions<'a>(
    sessions: impl Iterator<Item = &'a SessionData>,
    provider_filter: Option<&'a str>,
) -> Vec<&'a SessionData> {
    sessions
        .filter(|session| {
            provider_filter.is_none_or(|provider| session.provider_id.as_str() == provider)
        })
        .collect()
}

pub fn session_display_entries<'a>(
    sessions: Vec<&'a SessionData>,
    collapsed_groups: &HashSet<String>,
) -> Vec<DisplaySessionEntry<'a>> {
    let grouped_keys = grouped_worktree_keys(&sessions);
    let mut emitted = HashSet::<String>::new();
    let mut entries = Vec::with_capacity(sessions.len());
    let mut display_index = 0;

    for session in sessions.iter().copied() {
        let Some(key) = worktree_group_key(session).filter(|key| grouped_keys.contains(key)) else {
            display_index += 1;
            entries.push(DisplaySessionEntry::Session {
                index: display_index,
                session,
                indented: false,
            });
            continue;
        };

        if !emitted.insert(key.clone()) {
            continue;
        }

        let members = sessions
            .iter()
            .copied()
            .filter(|candidate| worktree_group_key(candidate).as_deref() == Some(key.as_str()))
            .collect::<Vec<_>>();
        entries.push(DisplaySessionEntry::Group {
            key: key.clone(),
            label: worktree_group_label(&key),
            count: members.len(),
            collapsed: collapsed_groups.contains(&key),
            summary: group_summary(&members),
        });
        if collapsed_groups.contains(&key) {
            continue;
        }
        for member in members {
            display_index += 1;
            entries.push(DisplaySessionEntry::Session {
                index: display_index,
                session: member,
                indented: true,
            });
        }
    }

    entries
}

pub fn project_sessions<'a>(
    sessions: &'a [SessionData],
    options: SessionProjectionOptions<'a>,
) -> Vec<DisplaySessionEntry<'a>> {
    session_display_entries(
        provider_filtered_sessions(
            filtered_sessions(sessions, options.filter),
            options.provider_filter,
        ),
        options.collapsed_groups,
    )
}

pub fn display_sessions<'a>(
    sessions: &'a [SessionData],
    options: SessionProjectionOptions<'a>,
) -> Vec<&'a SessionData> {
    project_sessions(sessions, options)
        .into_iter()
        .filter_map(|entry| match entry {
            DisplaySessionEntry::Session { session, .. } => Some(session),
            DisplaySessionEntry::Group { .. } => None,
        })
        .collect()
}

pub fn display_session_names<'a>(
    sessions: &'a [SessionData],
    options: SessionProjectionOptions<'a>,
) -> Vec<String> {
    display_sessions(sessions, options)
        .into_iter()
        .map(|session| session.name.clone())
        .collect()
}

pub fn reordered_session_names<'a>(
    sessions: &'a [SessionData],
    options: SessionProjectionOptions<'a>,
    name: &str,
    delta: i8,
) -> Option<Vec<String>> {
    let entries = project_sessions(sessions, options);
    let index = entries.iter().position(|entry| match entry {
        DisplaySessionEntry::Session { session, .. } => session.name == name,
        DisplaySessionEntry::Group { .. } => false,
    })?;
    let target_index = index as isize + delta as isize;
    if target_index < 0 || target_index >= entries.len() as isize {
        return None;
    }

    let mut names = display_session_names(sessions, options);
    match &entries[target_index as usize] {
        DisplaySessionEntry::Session {
            session, indented, ..
        } => {
            let current = names.iter().position(|candidate| candidate == name)?;
            if *indented && delta < 0 {
                let key = worktree_group_key(session)?;
                let group_indices = display_sessions(sessions, options)
                    .into_iter()
                    .filter_map(|session| {
                        (worktree_group_key(session).as_deref() == Some(key.as_str()))
                            .then(|| {
                                names
                                    .iter()
                                    .position(|candidate| candidate == &session.name)
                            })
                            .flatten()
                    })
                    .collect::<Vec<_>>();
                let name = names.remove(current);
                names.insert(group_indices.into_iter().min()?, name);
            } else {
                let target = names
                    .iter()
                    .position(|candidate| candidate == &session.name)?;
                names.swap(current, target);
            }
        }
        DisplaySessionEntry::Group { key, .. } => {
            let current = names.iter().position(|candidate| candidate == name)?;
            let name = names.remove(current);
            let group_indices = display_sessions(sessions, options)
                .into_iter()
                .filter_map(|session| {
                    (worktree_group_key(session).as_deref() == Some(key.as_str()))
                        .then(|| {
                            names
                                .iter()
                                .position(|candidate| candidate == &session.name)
                        })
                        .flatten()
                })
                .collect::<Vec<_>>();
            let insert_at = if delta < 0 {
                group_indices.into_iter().min()?
            } else {
                group_indices.into_iter().max()?.saturating_add(1)
            };
            names.insert(insert_at.min(names.len()), name);
        }
    }
    Some(names)
}

pub fn reordered_worktree_group_names<'a>(
    sessions: &'a [SessionData],
    options: SessionProjectionOptions<'a>,
    key: &str,
    delta: i8,
) -> Option<Vec<String>> {
    let mut blocks = session_order_blocks(sessions, options);
    let current = blocks
        .iter()
        .position(|block| block.group_key.as_deref() == Some(key))?;
    let target = (current as isize + delta as isize).clamp(0, blocks.len() as isize - 1) as usize;
    if current == target {
        return None;
    }

    let block = blocks.remove(current);
    blocks.insert(target, block);
    Some(
        blocks
            .into_iter()
            .flat_map(|block| block.names)
            .collect::<Vec<_>>(),
    )
}

pub fn worktree_group_session_names(sessions: &[SessionData], key: &str) -> Vec<String> {
    sessions
        .iter()
        .filter(|session| worktree_group_key(session).as_deref() == Some(key))
        .map(|session| session.name.clone())
        .collect()
}

pub fn worktree_group_key(session: &SessionData) -> Option<String> {
    if !session.is_worktree {
        return None;
    }
    let dir = session.dir.trim_end_matches('/');
    let (parent, _) = dir.rsplit_once('/')?;
    (!parent.is_empty()).then(|| parent.to_string())
}

pub fn worktree_group_label(key: &str) -> String {
    key.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(key)
        .to_string()
}

fn session_matches_filter(session: &SessionData, mode: SessionFilterMode) -> bool {
    if session.name == "_os_stash" {
        return false;
    }

    match mode {
        SessionFilterMode::All => true,
        SessionFilterMode::Active => !session.agents.is_empty() || session.agent_state.is_some(),
        SessionFilterMode::Running => {
            matches!(
                session.agent_state.as_ref().map(|agent| agent.status),
                Some(AgentStatus::Running | AgentStatus::ToolRunning | AgentStatus::Waiting),
            ) || session.agents.iter().any(|agent| {
                matches!(
                    agent.status,
                    AgentStatus::Running | AgentStatus::ToolRunning | AgentStatus::Waiting
                )
            })
        }
    }
}

fn group_summary(sessions: &[&SessionData]) -> GroupSummary {
    let mut summary = GroupSummary::default();
    for session in sessions {
        if session_has_active_agent(session) {
            summary.running_agents += 1;
        }
        if session.unseen {
            summary.unseen += 1;
        }
        summary.insertions = summary.insertions.saturating_add(session.insertions);
        summary.deletions = summary.deletions.saturating_add(session.deletions);
        for port in &session.ports {
            if summary.first_port.is_none() {
                summary.first_port = Some(*port);
            } else {
                summary.extra_ports += 1;
            }
        }
    }
    summary
}

fn session_has_active_agent(session: &SessionData) -> bool {
    session
        .agent_state
        .iter()
        .chain(session.agents.iter())
        .any(is_active_agent)
}

fn is_active_agent(agent: &AgentEvent) -> bool {
    matches!(
        agent.status,
        AgentStatus::Running | AgentStatus::ToolRunning | AgentStatus::Waiting
    )
}

fn grouped_worktree_keys(sessions: &[&SessionData]) -> HashSet<String> {
    let mut counts = HashMap::<String, usize>::new();
    for session in sessions {
        if let Some(key) = worktree_group_key(session) {
            *counts.entry(key).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .filter_map(|(key, count)| (count >= 2).then_some(key))
        .collect()
}

fn session_order_blocks<'a>(
    sessions: &'a [SessionData],
    options: SessionProjectionOptions<'a>,
) -> Vec<SessionOrderBlock> {
    let mut blocks = Vec::new();
    for entry in project_sessions(sessions, options) {
        match entry {
            DisplaySessionEntry::Group { key, .. } => {
                blocks.push(SessionOrderBlock {
                    group_key: Some(key.clone()),
                    names: worktree_group_session_names(sessions, &key),
                });
            }
            DisplaySessionEntry::Session {
                session, indented, ..
            } => {
                if !indented {
                    blocks.push(SessionOrderBlock {
                        group_key: None,
                        names: vec![session.name.clone()],
                    });
                }
            }
        }
    }
    blocks
}

struct SessionOrderBlock {
    group_key: Option<String>,
    names: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{AgentLiveness, LocalLink, SessionMetadata};

    fn session(name: &str, dir: &str, is_worktree: bool) -> SessionData {
        SessionData {
            node_id: "local".to_string(),
            provider_id: "tmux".to_string(),
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
            ports: Vec::<u32>::new(),
            local_links: Vec::<LocalLink>::new(),
            windows: 1,
            uptime: String::new(),
            agent_state: None,
            agents: Vec::new(),
            event_timestamps: Vec::new(),
            metadata: None::<SessionMetadata>,
        }
    }

    fn agent(status: AgentStatus) -> AgentEvent {
        AgentEvent {
            agent: "amp".to_string(),
            node_id: "local".to_string(),
            provider_id: "tmux".to_string(),
            session: String::new(),
            status,
            ts: 0,
            thread_id: None,
            thread_name: None,
            last_user_prompt: None,
            unseen: None,
            pane_id: None,
            liveness: Some(AgentLiveness::Alive),
        }
    }

    fn options(collapsed_groups: &HashSet<String>) -> SessionProjectionOptions<'_> {
        SessionProjectionOptions {
            filter: SessionFilterMode::All,
            collapsed_groups,
            provider_filter: None,
        }
    }

    #[test]
    fn projection_hides_stash_and_groups_worktree_siblings() {
        let collapsed = HashSet::new();
        let sessions = vec![
            session("_os_stash", "/tmp/stash", false),
            session("main", "/repo/main", false),
            session("feature-a", "/repo/worktrees/feature-a", true),
            session("feature-b", "/repo/worktrees/feature-b", true),
        ];

        let entries = project_sessions(&sessions, options(&collapsed));

        assert_eq!(entries.len(), 4);
        assert!(
            matches!(entries[0], DisplaySessionEntry::Session { session, .. } if session.name == "main")
        );
        assert!(
            matches!(entries[1], DisplaySessionEntry::Group { ref key, count: 2, .. } if key == "/repo/worktrees")
        );
        assert!(
            matches!(entries[2], DisplaySessionEntry::Session { session, indented: true, .. } if session.name == "feature-a")
        );
        assert!(
            matches!(entries[3], DisplaySessionEntry::Session { session, indented: true, .. } if session.name == "feature-b")
        );
    }

    #[test]
    fn running_filter_uses_aggregate_and_instance_agent_state() {
        let collapsed = HashSet::new();
        let mut aggregate = session("aggregate", "/repo/aggregate", false);
        aggregate.agent_state = Some(agent(AgentStatus::Running));
        let mut instance = session("instance", "/repo/instance", false);
        instance.agents.push(agent(AgentStatus::Waiting));
        let mut done = session("done", "/repo/done", false);
        done.agents.push(agent(AgentStatus::Done));
        let sessions = vec![aggregate, instance, done];

        assert_eq!(
            display_session_names(
                &sessions,
                SessionProjectionOptions {
                    filter: SessionFilterMode::Running,
                    collapsed_groups: &collapsed,
                    provider_filter: None,
                },
            ),
            vec!["aggregate".to_string(), "instance".to_string()],
        );
    }

    #[test]
    fn reordering_worktree_group_moves_all_members_as_a_block() {
        let collapsed = HashSet::new();
        let sessions = vec![
            session("main", "/repo/main", false),
            session("feature-a", "/repo/worktrees/feature-a", true),
            session("feature-b", "/repo/worktrees/feature-b", true),
            session("docs", "/repo/docs", false),
        ];

        assert_eq!(
            reordered_worktree_group_names(&sessions, options(&collapsed), "/repo/worktrees", 1),
            Some(vec![
                "main".to_string(),
                "docs".to_string(),
                "feature-a".to_string(),
                "feature-b".to_string(),
            ])
        );
        assert_eq!(
            reordered_worktree_group_names(&sessions, options(&collapsed), "/repo/worktrees", -1),
            Some(vec![
                "feature-a".to_string(),
                "feature-b".to_string(),
                "main".to_string(),
                "docs".to_string(),
            ])
        );
    }
}
