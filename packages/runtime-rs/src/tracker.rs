use std::collections::{BTreeSet, HashMap, HashSet};

use crate::protocol::{AgentEvent, AgentLiveness, AgentStatus};

const MAX_EVENT_TIMESTAMPS: usize = 30;
const TERMINAL_PRUNE_MS: u64 = 5 * 60 * 1000;
const SYNTHETIC_PANE_MARKER: &str = ":pane:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanePresenceInput {
    pub agent: String,
    pub pane_id: String,
    pub thread_id: Option<String>,
    pub thread_name: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AgentTracker {
    instances: HashMap<String, HashMap<String, AgentEvent>>,
    event_timestamps: HashMap<String, Vec<u64>>,
    unseen_instances: HashSet<String>,
    active: HashSet<String>,
}

impl AgentTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply_event(&mut self, mut event: AgentEvent) {
        self.apply_event_with_options(&mut event, false);
    }

    pub fn apply_seed_event(&mut self, mut event: AgentEvent) {
        self.apply_event_with_options(&mut event, true);
    }

    pub fn get_state(&self, session: &str) -> Option<AgentEvent> {
        let session_instances = self.instances.get(session)?;
        session_instances
            .values()
            .max_by_key(|event| status_priority(event.status))
            .cloned()
    }

    pub fn get_agents(&self, session: &str) -> Vec<AgentEvent> {
        let Some(session_instances) = self.instances.get(session) else {
            return Vec::new();
        };

        let mut agents = session_instances
            .values()
            .map(|event| {
                let mut event = event.clone();
                let key = instance_key(&event.agent, event.thread_id.as_deref());
                if self
                    .unseen_instances
                    .contains(&self.unseen_key(session, &key))
                {
                    event.unseen = Some(true);
                }
                event
            })
            .collect::<Vec<_>>();
        agents.sort_by(|a, b| b.ts.cmp(&a.ts));
        agents
    }

    pub fn get_event_timestamps(&self, session: &str) -> Vec<u64> {
        self.event_timestamps
            .get(session)
            .cloned()
            .unwrap_or_default()
    }

    pub fn mark_seen(&mut self, session: &str) -> bool {
        let had_unseen = self.is_unseen(session);
        if !had_unseen {
            return false;
        }

        if let Some(session_instances) = self.instances.get(session) {
            for key in session_instances.keys() {
                self.unseen_instances.remove(&self.unseen_key(session, key));
            }
        }
        true
    }

    pub fn dismiss(&mut self, session: &str, agent: &str, thread_id: Option<&str>) -> bool {
        let exact_key = instance_key(agent, thread_id);
        let mut removed_keys = Vec::new();
        let should_remove_session;

        {
            let Some(session_instances) = self.instances.get_mut(session) else {
                return false;
            };

            if session_instances.remove(&exact_key).is_some() {
                removed_keys.push(exact_key);
            }

            let synthetic_matches = session_instances
                .iter()
                .filter(|(key, event)| {
                    is_synthetic_pane_key(key)
                        && event.agent == agent
                        && event.thread_id.as_deref() == thread_id
                })
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();

            for key in synthetic_matches {
                session_instances.remove(&key);
                removed_keys.push(key);
            }

            should_remove_session = session_instances.is_empty();
        }

        if removed_keys.is_empty() {
            return false;
        }

        for key in removed_keys {
            self.unseen_instances
                .remove(&self.unseen_key(session, &key));
        }
        if should_remove_session {
            self.instances.remove(session);
        }
        true
    }

    pub fn dedupe_instance_to_session(
        &mut self,
        session: &str,
        agent: &str,
        thread_id: Option<&str>,
    ) -> bool {
        let Some(thread_id) = thread_id else {
            return false;
        };
        let key = instance_key(agent, Some(thread_id));
        let mut changed = false;
        let sessions = self.instances.keys().cloned().collect::<Vec<_>>();

        for other_session in sessions {
            if other_session == session {
                continue;
            }

            let mut removed = false;
            let mut empty = false;
            if let Some(session_instances) = self.instances.get_mut(&other_session) {
                removed = session_instances.remove(&key).is_some();
                empty = session_instances.is_empty();
            }

            if removed {
                self.unseen_instances
                    .remove(&self.unseen_key(&other_session, &key));
                if empty {
                    self.instances.remove(&other_session);
                }
                changed = true;
            }
        }

        changed
    }

    pub fn prune_stuck(&mut self, timeout_ms: u64) {
        let now = now_ms();
        let sessions = self.instances.keys().cloned().collect::<Vec<_>>();
        let mut unseen_to_remove = Vec::new();

        for session in sessions {
            let mut empty = false;
            if let Some(session_instances) = self.instances.get_mut(&session) {
                let keys = session_instances
                    .iter()
                    .filter(|(_, event)| {
                        matches!(
                            event.status,
                            AgentStatus::Running | AgentStatus::ToolRunning
                        ) && now.saturating_sub(event.ts) > timeout_ms
                            && event.liveness != Some(AgentLiveness::Alive)
                    })
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>();

                for key in keys {
                    session_instances.remove(&key);
                    unseen_to_remove.push(format!("{session}\0{key}"));
                }
                empty = session_instances.is_empty();
            }
            if empty {
                self.instances.remove(&session);
            }
        }

        for key in unseen_to_remove {
            self.unseen_instances.remove(&key);
        }
    }

    pub fn prune_terminal(&mut self) {
        let now = now_ms();
        let sessions = self.instances.keys().cloned().collect::<Vec<_>>();

        for session in sessions {
            let unseen_instances = self.unseen_instances.clone();
            let mut empty = false;
            if let Some(session_instances) = self.instances.get_mut(&session) {
                let keys = session_instances
                    .iter()
                    .filter(|(key, event)| {
                        is_terminal_status(event.status)
                            && !unseen_instances.contains(&format!("{session}\0{key}"))
                            && event.liveness != Some(AgentLiveness::Alive)
                            && now.saturating_sub(event.ts) > TERMINAL_PRUNE_MS
                    })
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>();
                for key in keys {
                    session_instances.remove(&key);
                }
                empty = session_instances.is_empty();
            }
            if empty {
                self.instances.remove(&session);
            }
        }
    }

    pub fn is_unseen(&self, session: &str) -> bool {
        let Some(session_instances) = self.instances.get(session) else {
            return false;
        };
        session_instances.keys().any(|key| {
            self.unseen_instances
                .contains(&self.unseen_key(session, key))
        })
    }

    pub fn get_unseen(&self) -> Vec<String> {
        let mut sessions = BTreeSet::new();
        for key in &self.unseen_instances {
            if let Some((session, _)) = key.split_once('\0') {
                sessions.insert(session.to_string());
            }
        }
        sessions.into_iter().collect()
    }

    pub fn handle_focus(&mut self, session: &str) -> bool {
        self.active.clear();
        self.active.insert(session.to_string());
        let had_unseen = self.is_unseen(session);
        if had_unseen {
            self.mark_seen(session);
        }
        had_unseen
    }

    pub fn set_active_sessions(&mut self, sessions: impl IntoIterator<Item = String>) {
        self.active.clear();
        self.active.extend(sessions);
    }

    pub fn apply_pane_presence(
        &mut self,
        session: &str,
        pane_agents: Vec<PanePresenceInput>,
    ) -> bool {
        let mut changed = false;

        let active_pane_ids = pane_agents
            .iter()
            .map(|pane| pane.pane_id.as_str())
            .collect::<HashSet<_>>();
        let agents_with_thread_ids = pane_agents
            .iter()
            .filter(|pane| pane.thread_id.is_some())
            .map(|pane| pane.agent.as_str())
            .collect::<HashSet<_>>();
        let alive_agent_threads = pane_agents
            .iter()
            .filter_map(|pane| {
                pane.thread_id
                    .as_ref()
                    .map(|thread_id| format!("{}\0{thread_id}", pane.agent))
            })
            .collect::<HashSet<_>>();

        let mut unseen_to_remove = Vec::new();
        if let Some(session_instances) = self.instances.get_mut(session) {
            let existing_keys = session_instances.keys().cloned().collect::<Vec<_>>();
            for key in existing_keys {
                let Some(event) = session_instances.get_mut(&key) else {
                    continue;
                };
                if event.liveness != Some(AgentLiveness::Alive) || event.pane_id.is_none() {
                    continue;
                }

                let is_alive = if let Some(thread_id) = event
                    .thread_id
                    .as_deref()
                    .filter(|_| agents_with_thread_ids.contains(event.agent.as_str()))
                {
                    alive_agent_threads.contains(&format!("{}\0{thread_id}", event.agent))
                } else {
                    event
                        .pane_id
                        .as_deref()
                        .is_some_and(|pane_id| active_pane_ids.contains(pane_id))
                };
                if is_alive {
                    continue;
                }

                if is_synthetic_pane_key(&key) {
                    session_instances.remove(&key);
                    unseen_to_remove.push(format!("{session}\0{key}"));
                } else {
                    event.liveness = Some(AgentLiveness::Exited);
                    event.pane_id = None;
                }
                changed = true;
            }
        }
        for key in unseen_to_remove {
            self.unseen_instances.remove(&key);
        }

        for pane in pane_agents {
            self.instances.entry(session.to_string()).or_default();

            if let Some(thread_id) = pane.thread_id.as_deref() {
                let exact_key = instance_key(&pane.agent, Some(thread_id));
                let exact_event_exists = self
                    .instances
                    .get(session)
                    .and_then(|instances| instances.get(&exact_key))
                    .is_some();

                if exact_event_exists {
                    if self.stamp_alive(session, &exact_key, &pane.pane_id) {
                        changed = true;
                    }

                    let generic_synthetic_key =
                        synthetic_pane_key(&pane.agent, &pane.pane_id, None);
                    let exact_synthetic_key =
                        synthetic_pane_key(&pane.agent, &pane.pane_id, Some(thread_id));
                    let removed_generic = self.remove_instance(session, &generic_synthetic_key);
                    let removed_exact = self.remove_instance(session, &exact_synthetic_key);
                    changed = changed || removed_generic || removed_exact;
                    continue;
                }

                let exact_synthetic_key =
                    synthetic_pane_key(&pane.agent, &pane.pane_id, Some(thread_id));
                if self
                    .instances
                    .get(session)
                    .and_then(|instances| instances.get(&exact_synthetic_key))
                    .is_some()
                {
                    if self.stamp_alive(session, &exact_synthetic_key, &pane.pane_id) {
                        changed = true;
                    }
                    continue;
                }

                let generic_synthetic_key = synthetic_pane_key(&pane.agent, &pane.pane_id, None);
                if self.remove_instance(session, &generic_synthetic_key) {
                    changed = true;
                }

                let synthetic = AgentEvent {
                    agent: pane.agent,
                    session: session.to_string(),
                    status: AgentStatus::Running,
                    ts: now_ms(),
                    thread_id: Some(thread_id.to_string()),
                    thread_name: pane.thread_name,
                    unseen: None,
                    pane_id: Some(pane.pane_id),
                    liveness: Some(AgentLiveness::Alive),
                };
                self.instances
                    .get_mut(session)
                    .expect("session instances exist")
                    .insert(exact_synthetic_key, synthetic);
                changed = true;
                continue;
            }

            let watcher_entries = self
                .instances
                .get(session)
                .expect("session instances exist")
                .iter()
                .filter(|(key, event)| event.agent == pane.agent && !is_synthetic_pane_key(key))
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();

            if watcher_entries.len() == 1 {
                if self.stamp_alive(session, &watcher_entries[0], &pane.pane_id) {
                    changed = true;
                }
                continue;
            }

            let synthetic_key = synthetic_pane_key(&pane.agent, &pane.pane_id, None);
            if self
                .instances
                .get(session)
                .and_then(|instances| instances.get(&synthetic_key))
                .is_some()
            {
                if self.stamp_alive(session, &synthetic_key, &pane.pane_id) {
                    changed = true;
                }
            } else {
                self.instances
                    .get_mut(session)
                    .expect("session instances exist")
                    .insert(
                        synthetic_key,
                        AgentEvent {
                            agent: pane.agent,
                            session: session.to_string(),
                            status: AgentStatus::Running,
                            ts: now_ms(),
                            thread_id: None,
                            thread_name: pane.thread_name,
                            unseen: None,
                            pane_id: Some(pane.pane_id),
                            liveness: Some(AgentLiveness::Alive),
                        },
                    );
                changed = true;
            }
        }

        changed
    }

    fn apply_event_with_options(&mut self, event: &mut AgentEvent, seed: bool) {
        let key = instance_key(&event.agent, event.thread_id.as_deref());

        {
            let session_instances = self.instances.entry(event.session.clone()).or_default();
            if let Some(prev) = session_instances.get(&key) {
                if prev.pane_id.is_some() {
                    if event.pane_id.is_none() {
                        event.pane_id = prev.pane_id.clone();
                    }
                    if event.liveness.is_none() {
                        event.liveness = prev.liveness;
                    }
                }
                if prev.thread_name.is_some() && event.thread_name.is_none() {
                    event.thread_name = prev.thread_name.clone();
                }
            }
            session_instances.insert(key.clone(), event.clone());
        }

        let event_session = event.session.clone();
        self.merge_matching_synthetic(&event_session, &key, event);

        let timestamps = self
            .event_timestamps
            .entry(event.session.clone())
            .or_default();
        timestamps.push(event.ts);
        if timestamps.len() > MAX_EVENT_TIMESTAMPS {
            timestamps.drain(0..timestamps.len() - MAX_EVENT_TIMESTAMPS);
        }

        let unseen_key = self.unseen_key(&event.session, &key);
        if is_terminal_status(event.status) {
            if seed || !self.active.contains(&event.session) {
                self.unseen_instances.insert(unseen_key);
            }
        } else {
            self.unseen_instances.remove(&unseen_key);
        }
    }

    fn merge_matching_synthetic(&mut self, session: &str, key: &str, event: &mut AgentEvent) {
        let Some(session_instances) = self.instances.get(session) else {
            return;
        };

        let mut exact_matches = Vec::new();
        let mut generic_matches = Vec::new();
        for (candidate_key, candidate_event) in session_instances {
            if candidate_key == key
                || candidate_event.agent != event.agent
                || !is_synthetic_pane_key(candidate_key)
            {
                continue;
            }
            if candidate_event.thread_id.is_some()
                && event.thread_id.is_some()
                && candidate_event.thread_id == event.thread_id
            {
                exact_matches.push((candidate_key.clone(), candidate_event.clone()));
            } else if candidate_event.thread_id.is_none() {
                generic_matches.push((candidate_key.clone(), candidate_event.clone()));
            }
        }

        let match_to_merge = if exact_matches.len() == 1 {
            exact_matches.pop()
        } else if exact_matches.is_empty() && generic_matches.len() == 1 {
            generic_matches.pop()
        } else {
            None
        };

        let Some((synthetic_key, synthetic_event)) = match_to_merge else {
            return;
        };

        if synthetic_event.pane_id.is_some() && event.pane_id.is_none() {
            event.pane_id = synthetic_event.pane_id;
            event.liveness = synthetic_event.liveness;
        }

        if let Some(session_instances) = self.instances.get_mut(session) {
            session_instances.insert(key.to_string(), event.clone());
            session_instances.remove(&synthetic_key);
        }
        self.unseen_instances
            .remove(&self.unseen_key(session, &synthetic_key));
    }

    fn remove_instance(&mut self, session: &str, key: &str) -> bool {
        let removed = self
            .instances
            .get_mut(session)
            .is_some_and(|instances| instances.remove(key).is_some());
        if removed {
            self.unseen_instances.remove(&self.unseen_key(session, key));
        }
        removed
    }

    fn stamp_alive(&mut self, session: &str, key: &str, pane_id: &str) -> bool {
        let Some(event) = self
            .instances
            .get_mut(session)
            .and_then(|instances| instances.get_mut(key))
        else {
            return false;
        };

        let was_different = event.pane_id.as_deref() != Some(pane_id)
            || event.liveness != Some(AgentLiveness::Alive);
        event.pane_id = Some(pane_id.to_string());
        event.liveness = Some(AgentLiveness::Alive);
        was_different
    }

    fn unseen_key(&self, session: &str, key: &str) -> String {
        format!("{session}\0{key}")
    }
}

pub fn instance_key(agent: &str, thread_id: Option<&str>) -> String {
    match thread_id {
        Some(thread_id) => format!("{agent}:{thread_id}"),
        None => agent.to_string(),
    }
}

fn synthetic_pane_key(agent: &str, pane_id: &str, thread_id: Option<&str>) -> String {
    match thread_id {
        Some(thread_id) => format!("{agent}:{thread_id}{SYNTHETIC_PANE_MARKER}{pane_id}"),
        None => format!("{agent}{SYNTHETIC_PANE_MARKER}{pane_id}"),
    }
}

fn is_synthetic_pane_key(key: &str) -> bool {
    key.contains(SYNTHETIC_PANE_MARKER)
}

fn is_terminal_status(status: AgentStatus) -> bool {
    matches!(
        status,
        AgentStatus::Done | AgentStatus::Error | AgentStatus::Interrupted | AgentStatus::Stale
    )
}

fn status_priority(status: AgentStatus) -> u8 {
    match status {
        AgentStatus::ToolRunning => 7,
        AgentStatus::Running => 6,
        AgentStatus::Error => 5,
        AgentStatus::Stale => 4,
        AgentStatus::Interrupted => 3,
        AgentStatus::Waiting => 2,
        AgentStatus::Done => 1,
        AgentStatus::Idle => 0,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
