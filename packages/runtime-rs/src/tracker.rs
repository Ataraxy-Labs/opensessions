use std::collections::{BTreeSet, HashMap, HashSet};

use crate::protocol::{AgentEvent, AgentLiveness, AgentStatus};

const MAX_EVENT_TIMESTAMPS: usize = 30;
const TERMINAL_PRUNE_MS: u64 = 5 * 60 * 1000;
const SYNTHETIC_PANE_MARKER: &str = ":pane:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanePresenceInput {
    pub agent: String,
    pub pane_id: String,
    pub active: bool,
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
            .iter()
            .filter(|(key, _)| !is_synthetic_pane_key(key))
            .map(|(key, event)| {
                let mut event = event.clone();
                if self
                    .unseen_instances
                    .contains(&self.unseen_key(session, key))
                {
                    event.unseen = Some(true);
                }
                event
            })
            .max_by_key(|event| status_priority(event.status))
    }

    pub fn get_agents(&self, session: &str) -> Vec<AgentEvent> {
        let Some(session_instances) = self.instances.get(session) else {
            return Vec::new();
        };

        let mut agents = session_instances
            .iter()
            .filter(|(key, _)| !is_synthetic_pane_key(key))
            .map(|(key, event)| {
                let mut event = event.clone();
                if self
                    .unseen_instances
                    .contains(&self.unseen_key(session, key))
                {
                    event.unseen = Some(true);
                }
                event
            })
            .collect::<Vec<_>>();
        agents.sort_by_key(|agent| std::cmp::Reverse(agent.ts));
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
        if self.unseen_instance_count(session) == 1 {
            return self.mark_single_unseen_seen(session);
        }
        false
    }

    pub fn mark_agent_seen(
        &mut self,
        session: &str,
        agent: &str,
        thread_id: Option<&str>,
        pane_id: Option<&str>,
    ) -> bool {
        let Some(session_instances) = self.instances.get(session) else {
            return false;
        };

        let mut keys = Vec::new();
        if let Some(thread_id) = thread_id {
            let exact_key = instance_key(agent, Some(thread_id));
            if session_instances.contains_key(&exact_key) {
                keys.push(exact_key);
            }
        }
        if let Some(pane_id) = pane_id {
            keys.extend(
                session_instances
                    .iter()
                    .filter(|(_, event)| {
                        event.agent == agent && event.pane_id.as_deref() == Some(pane_id)
                    })
                    .map(|(key, _)| key.clone()),
            );
        }
        if keys.is_empty() && thread_id.is_none() {
            let generic_key = instance_key(agent, None);
            if session_instances.contains_key(&generic_key) {
                keys.push(generic_key);
            }
        }

        keys.sort();
        keys.dedup();

        let mut changed = false;
        for key in keys {
            if let Some(event) = self
                .instances
                .get_mut(session)
                .and_then(|instances| instances.get_mut(&key))
                && event.unseen == Some(true)
            {
                event.unseen = None;
                changed = true;
            }
            changed = self
                .unseen_instances
                .remove(&self.unseen_key(session, &key))
                || changed;
        }
        changed
    }

    pub fn mark_pane_seen(&mut self, session: &str, pane_id: &str) -> bool {
        let Some(session_instances) = self.instances.get(session) else {
            return false;
        };
        let mut keys = session_instances
            .iter()
            .filter(|(_, event)| event.pane_id.as_deref() == Some(pane_id))
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();

        let pane_agents = session_instances
            .iter()
            .filter(|(key, event)| {
                is_synthetic_pane_key(key) && event.pane_id.as_deref() == Some(pane_id)
            })
            .map(|(_, event)| (event.agent.as_str(), event.thread_id.as_deref()))
            .collect::<Vec<_>>();
        for (agent, thread_id) in pane_agents {
            let matching_logical_keys = session_instances
                .iter()
                .filter(|(key, event)| {
                    !is_synthetic_pane_key(key)
                        && event.pane_id.is_none()
                        && event.agent == agent
                        && thread_id
                            .is_none_or(|thread_id| event.thread_id.as_deref() == Some(thread_id))
                })
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            if matching_logical_keys.len() == 1 {
                keys.extend(matching_logical_keys);
            }
        }

        keys.sort();
        keys.dedup();

        let mut changed = false;
        for key in keys {
            if let Some(event) = self
                .instances
                .get_mut(session)
                .and_then(|instances| instances.get_mut(&key))
                && event.unseen == Some(true)
            {
                event.unseen = None;
                changed = true;
            }
            changed = self
                .unseen_instances
                .remove(&self.unseen_key(session, &key))
                || changed;
        }
        changed
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

                let generic_synthetic_key = synthetic_pane_key(&pane.agent, &pane.pane_id, None);
                self.remove_instance(session, &generic_synthetic_key);
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

            let named_watcher_entries = self
                .instances
                .get(session)
                .expect("session instances exist")
                .iter()
                .filter(|(key, event)| {
                    event.agent == pane.agent
                        && !is_synthetic_pane_key(key)
                        && event.thread_name.is_some()
                        && event.thread_name == pane.thread_name
                })
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();

            if named_watcher_entries.len() == 1 {
                if self.stamp_alive(session, &named_watcher_entries[0], &pane.pane_id) {
                    changed = true;
                }
                let synthetic_key = synthetic_pane_key(&pane.agent, &pane.pane_id, None);
                changed = self.remove_instance(session, &synthetic_key) || changed;
                continue;
            }

            if pane.thread_name.is_some()
                && watcher_entries.len() == 1
                && self
                    .instances
                    .get(session)
                    .and_then(|instances| instances.get(&watcher_entries[0]))
                    .and_then(|event| event.thread_name.as_ref())
                    .is_some()
            {
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
                    continue;
                }
            }

            if watcher_entries.len() == 1 {
                let watcher_thread_name = self
                    .instances
                    .get(session)
                    .and_then(|instances| instances.get(&watcher_entries[0]))
                    .and_then(|event| event.thread_name.as_deref());
                if pane.thread_name.is_some()
                    && watcher_thread_name.is_some()
                    && watcher_thread_name != pane.thread_name.as_deref()
                {
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
                    }
                    continue;
                }
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
            }
        }

        changed
    }

    pub fn active_pane_ids(&self, session: &str) -> Vec<String> {
        self.instances
            .get(session)
            .map(|instances| {
                instances
                    .values()
                    .filter(|event| event.liveness == Some(AgentLiveness::Alive))
                    .filter_map(|event| event.pane_id.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default()
    }

    fn apply_event_with_options(&mut self, event: &mut AgentEvent, _seed: bool) {
        event.ts = event.ts.min(now_ms());
        let key = instance_key(&event.agent, event.thread_id.as_deref());
        let mut removed_unseen_keys = Vec::new();
        if is_terminal_status(event.status) {
            event.unseen = Some(true);
        } else {
            event.unseen = None;
        }

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
                if prev.last_user_prompt.is_some() && event.last_user_prompt.is_none() {
                    event.last_user_prompt = prev.last_user_prompt.clone();
                }
            }
            if event.thread_id.is_some()
                && let Some(thread_name) = event.thread_name.as_deref()
            {
                let matching_provisional_keys = session_instances
                    .iter()
                    .filter(|(candidate_key, candidate_event)| {
                        *candidate_key != &key
                            && candidate_event.agent == event.agent
                            && candidate_event.thread_id.is_none()
                            && candidate_event.thread_name.as_deref() == Some(thread_name)
                    })
                    .map(|(candidate_key, _)| candidate_key.clone())
                    .collect::<Vec<_>>();

                for provisional_key in matching_provisional_keys {
                    if let Some(provisional) = session_instances.remove(&provisional_key) {
                        if event.pane_id.is_none() {
                            event.pane_id = provisional.pane_id;
                        }
                        if event.liveness.is_none() {
                            event.liveness = provisional.liveness;
                        }
                        removed_unseen_keys.push(format!("{}\0{provisional_key}", event.session));
                    }
                }
            }
            session_instances.insert(key.clone(), event.clone());
        }

        for unseen_key in removed_unseen_keys {
            self.unseen_instances.remove(&unseen_key);
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
            self.unseen_instances.insert(unseen_key);
        } else {
            self.unseen_instances.remove(&unseen_key);
        }
    }

    fn unseen_instance_count(&self, session: &str) -> usize {
        let Some(session_instances) = self.instances.get(session) else {
            return 0;
        };
        session_instances
            .keys()
            .filter(|key| {
                self.unseen_instances
                    .contains(&self.unseen_key(session, key))
            })
            .count()
    }

    fn mark_single_unseen_seen(&mut self, session: &str) -> bool {
        let Some(session_instances) = self.instances.get(session) else {
            return false;
        };
        let unseen_keys = session_instances
            .keys()
            .filter(|key| {
                self.unseen_instances
                    .contains(&self.unseen_key(session, key))
            })
            .cloned()
            .collect::<Vec<_>>();
        if unseen_keys.len() != 1 {
            return false;
        }
        if let Some(event) = self
            .instances
            .get_mut(session)
            .and_then(|instances| instances.get_mut(&unseen_keys[0]))
        {
            event.unseen = None;
        }
        self.unseen_instances
            .remove(&self.unseen_key(session, &unseen_keys[0]))
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

        let pane_match = event.pane_id.as_deref().and_then(|pane_id| {
            exact_matches
                .iter()
                .chain(generic_matches.iter())
                .find(|(_, candidate_event)| candidate_event.pane_id.as_deref() == Some(pane_id))
                .cloned()
        });

        let match_to_merge = if pane_match.is_some() {
            pane_match
        } else if exact_matches.len() == 1 {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn event(
        agent: &str,
        session: &str,
        thread_id: Option<&str>,
        thread_name: Option<&str>,
    ) -> AgentEvent {
        AgentEvent {
            agent: agent.to_string(),
            session: session.to_string(),
            status: AgentStatus::Running,
            ts: 1,
            thread_id: thread_id.map(str::to_string),
            thread_name: thread_name.map(str::to_string),
            last_user_prompt: None,
            unseen: None,
            pane_id: None,
            liveness: None,
        }
    }

    fn terminal_event(
        agent: &str,
        session: &str,
        thread_id: Option<&str>,
        thread_name: Option<&str>,
        pane_id: Option<&str>,
    ) -> AgentEvent {
        let mut event = event(agent, session, thread_id, thread_name);
        event.status = AgentStatus::Done;
        event.pane_id = pane_id.map(str::to_string);
        event.liveness = pane_id.map(|_| AgentLiveness::Alive);
        event
    }

    #[test]
    fn future_event_timestamp_is_clamped_to_receive_time() {
        let before = now_ms();
        let mut tracker = AgentTracker::new();
        let mut future = event("amp", "work", Some("T-future"), Some("Future"));
        future.ts = u64::MAX;

        tracker.apply_event(future);

        let stored = tracker.get_agents("work").pop().expect("tracked agent");
        assert!(stored.ts >= before);
        assert!(stored.ts < u64::MAX);
    }

    #[test]
    fn pane_presence_attaches_exact_pane_to_threaded_agent_event() {
        let mut tracker = AgentTracker::new();
        tracker.apply_event(event("amp", "work", Some("T-1"), Some("Fix focus")));

        tracker.apply_pane_presence(
            "work",
            vec![PanePresenceInput {
                agent: "amp".to_string(),
                pane_id: "%7".to_string(),
                active: false,
                thread_id: Some("T-1".to_string()),
                thread_name: Some("Fix focus".to_string()),
            }],
        );

        let agent = tracker
            .get_agents("work")
            .into_iter()
            .find(|agent| agent.thread_id.as_deref() == Some("T-1"))
            .expect("tracked agent");
        assert_eq!(agent.pane_id.as_deref(), Some("%7"));
        assert_eq!(agent.liveness, Some(AgentLiveness::Alive));
    }

    #[test]
    fn pane_presence_uses_thread_name_to_attach_pane_when_thread_id_is_missing() {
        let mut tracker = AgentTracker::new();
        tracker.apply_event(event("amp", "work", Some("T-2"), Some("Review PR")));

        tracker.apply_pane_presence(
            "work",
            vec![PanePresenceInput {
                agent: "amp".to_string(),
                pane_id: "%9".to_string(),
                active: false,
                thread_id: None,
                thread_name: Some("Review PR".to_string()),
            }],
        );

        let agent = tracker
            .get_agents("work")
            .into_iter()
            .find(|agent| agent.thread_id.as_deref() == Some("T-2"))
            .expect("tracked agent");
        assert_eq!(agent.pane_id.as_deref(), Some("%9"));
        assert_eq!(agent.liveness, Some(AgentLiveness::Alive));
    }

    #[test]
    fn pane_presence_without_matching_events_does_not_create_fake_agent_instances() {
        let mut tracker = AgentTracker::new();

        tracker.apply_pane_presence(
            "work",
            vec![
                PanePresenceInput {
                    agent: "amp".to_string(),
                    pane_id: "%7".to_string(),
                    active: false,
                    thread_id: None,
                    thread_name: Some("Roadmap".to_string()),
                },
                PanePresenceInput {
                    agent: "amp".to_string(),
                    pane_id: "%8".to_string(),
                    active: true,
                    thread_id: None,
                    thread_name: Some("Release".to_string()),
                },
            ],
        );

        assert!(tracker.get_agents("work").is_empty());
        assert!(tracker.get_state("work").is_none());
    }

    #[test]
    fn focusing_session_clears_single_unseen_agent() {
        let mut tracker = AgentTracker::new();
        tracker.apply_event(terminal_event(
            "amp",
            "work",
            Some("T-1"),
            Some("Fix focus"),
            Some("%7"),
        ));

        assert!(tracker.is_unseen("work"));
        assert!(tracker.handle_focus("work"));

        assert!(!tracker.is_unseen("work"));
        assert_eq!(tracker.get_agents("work")[0].unseen, None);
    }

    #[test]
    fn focusing_session_preserves_multiple_unseen_agents_for_agent_level_review() {
        let mut tracker = AgentTracker::new();
        tracker.apply_event(terminal_event(
            "amp",
            "work",
            Some("T-1"),
            Some("Fix focus"),
            Some("%7"),
        ));
        tracker.apply_event(terminal_event(
            "codex",
            "work",
            Some("C-1"),
            Some("Polish UI"),
            Some("%8"),
        ));

        assert!(!tracker.handle_focus("work"));

        let agents = tracker.get_agents("work");
        assert_eq!(agents.len(), 2);
        assert!(agents.iter().all(|agent| agent.unseen == Some(true)));
    }

    #[test]
    fn focusing_agent_pane_clears_only_matching_unseen_agent() {
        let mut tracker = AgentTracker::new();
        tracker.apply_event(terminal_event(
            "amp",
            "work",
            Some("T-1"),
            Some("Fix focus"),
            Some("%7"),
        ));
        tracker.apply_event(terminal_event(
            "codex",
            "work",
            Some("C-1"),
            Some("Polish UI"),
            Some("%8"),
        ));

        assert!(tracker.mark_agent_seen("work", "amp", Some("T-1"), Some("%7")));

        let agents = tracker.get_agents("work");
        let amp = agents
            .iter()
            .find(|agent| agent.agent == "amp")
            .expect("amp agent");
        let codex = agents
            .iter()
            .find(|agent| agent.agent == "codex")
            .expect("codex agent");
        assert_eq!(amp.unseen, None);
        assert_eq!(codex.unseen, Some(true));
        assert!(tracker.is_unseen("work"));
    }

    #[test]
    fn focusing_one_pane_preserves_unseen_for_other_threads_of_same_agent() {
        let mut tracker = AgentTracker::new();
        tracker.apply_event(terminal_event(
            "amp",
            "work",
            Some("seen-thread"),
            Some("Seen"),
            Some("%7"),
        ));
        tracker.apply_event(terminal_event(
            "amp",
            "work",
            Some("unseen-thread"),
            Some("Unseen"),
            Some("%8"),
        ));

        assert!(tracker.mark_agent_seen("work", "amp", Some("seen-thread"), Some("%7")));

        let agents = tracker.get_agents("work");
        let seen = agents
            .iter()
            .find(|agent| agent.thread_id.as_deref() == Some("seen-thread"))
            .expect("seen thread");
        let unseen = agents
            .iter()
            .find(|agent| agent.thread_id.as_deref() == Some("unseen-thread"))
            .expect("unseen thread");
        assert_eq!(seen.unseen, None);
        assert_eq!(unseen.unseen, Some(true));
    }

    #[test]
    fn focusing_one_pane_preserves_unseen_for_other_threads_after_pane_presence() {
        let mut tracker = AgentTracker::new();
        tracker.apply_pane_presence(
            "work",
            vec![
                PanePresenceInput {
                    agent: "amp".to_string(),
                    pane_id: "%7".to_string(),
                    active: false,
                    thread_id: None,
                    thread_name: Some("Seen - amp - focused".to_string()),
                },
                PanePresenceInput {
                    agent: "amp".to_string(),
                    pane_id: "%8".to_string(),
                    active: false,
                    thread_id: None,
                    thread_name: Some("Unseen - amp - background".to_string()),
                },
            ],
        );
        tracker.apply_event(terminal_event(
            "amp",
            "work",
            Some("seen-thread"),
            Some("seen-thread"),
            Some("%7"),
        ));
        tracker.apply_event(terminal_event(
            "amp",
            "work",
            Some("unseen-thread"),
            Some("unseen-thread"),
            Some("%8"),
        ));

        assert!(tracker.mark_agent_seen("work", "amp", Some("seen-thread"), Some("%7")));

        let agents = tracker.get_agents("work");
        let seen = agents
            .iter()
            .find(|agent| agent.thread_id.as_deref() == Some("seen-thread"))
            .expect("seen thread");
        let unseen = agents
            .iter()
            .find(|agent| agent.thread_id.as_deref() == Some("unseen-thread"))
            .expect("unseen thread");
        assert_eq!(seen.unseen, None);
        assert_eq!(unseen.unseen, Some(true));
    }

    #[test]
    fn aggregate_state_reports_same_unseen_flag_as_agent_list() {
        let mut tracker = AgentTracker::new();
        tracker.apply_event(terminal_event(
            "amp",
            "work",
            Some("T-1"),
            Some("Fix focus"),
            Some("%7"),
        ));

        assert_eq!(tracker.get_state("work").unwrap().unseen, Some(true));
        assert_eq!(tracker.get_agents("work")[0].unseen, Some(true));

        assert!(tracker.mark_pane_seen("work", "%7"));

        assert_eq!(tracker.get_state("work").unwrap().unseen, None);
        assert_eq!(tracker.get_agents("work")[0].unseen, None);
    }

    #[test]
    fn focusing_pane_does_not_guess_for_unattached_logical_event() {
        let mut tracker = AgentTracker::new();
        tracker.apply_pane_presence(
            "work",
            vec![PanePresenceInput {
                agent: "amp".to_string(),
                pane_id: "%7".to_string(),
                active: false,
                thread_id: None,
                thread_name: Some("Amp running here".to_string()),
            }],
        );
        tracker.apply_event(terminal_event("amp", "work", Some("T-1"), None, None));

        assert!(tracker.is_unseen("work"));
        assert!(!tracker.mark_pane_seen("work", "%7"));

        assert!(tracker.is_unseen("work"));
        assert_eq!(tracker.get_agents("work")[0].unseen, Some(true));
    }

    #[test]
    fn focusing_pane_does_not_guess_when_multiple_unattached_events_match_live_agent() {
        let mut tracker = AgentTracker::new();
        tracker.apply_pane_presence(
            "work",
            vec![PanePresenceInput {
                agent: "amp".to_string(),
                pane_id: "%7".to_string(),
                active: false,
                thread_id: None,
                thread_name: Some("Amp running here".to_string()),
            }],
        );
        tracker.apply_event(terminal_event("amp", "work", Some("T-1"), None, None));
        tracker.apply_event(terminal_event("amp", "work", Some("T-2"), None, None));

        assert!(!tracker.mark_pane_seen("work", "%7"));

        let agents = tracker.get_agents("work");
        assert_eq!(agents.len(), 2);
        assert!(agents.iter().all(|agent| agent.unseen == Some(true)));
    }

    #[test]
    fn terminal_event_in_active_session_still_becomes_unseen_until_agent_is_focused() {
        let mut tracker = AgentTracker::new();
        tracker.handle_focus("work");

        tracker.apply_event(terminal_event(
            "amp",
            "work",
            Some("T-1"),
            Some("Fix focus"),
            Some("%7"),
        ));

        assert!(tracker.is_unseen("work"));
        assert_eq!(tracker.get_agents("work")[0].unseen, Some(true));
    }
}
