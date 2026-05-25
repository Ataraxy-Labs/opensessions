use opensessions_runtime::protocol::{AgentEvent, AgentLiveness, AgentStatus};
use opensessions_runtime::tracker::{AgentTracker, PanePresenceInput};

#[test]
fn stores_state_by_session_and_selects_highest_priority_status() {
    let mut tracker = AgentTracker::new();

    tracker.apply_event(event("sess-1", "amp", AgentStatus::Done).with_thread("t1"));
    tracker.apply_event(event("sess-1", "codex", AgentStatus::ToolRunning).with_thread("t2"));

    assert_eq!(
        tracker.get_state("sess-1").unwrap().status,
        AgentStatus::ToolRunning
    );
    assert_eq!(tracker.get_state("unknown"), None);
}

#[test]
fn terminal_status_is_unseen_unless_session_is_active() {
    let mut tracker = AgentTracker::new();

    tracker.apply_event(event("sess-1", "amp", AgentStatus::Done));
    assert_eq!(tracker.get_unseen(), vec!["sess-1"]);

    assert!(tracker.mark_seen("sess-1"));
    assert_eq!(tracker.get_unseen(), Vec::<String>::new());

    tracker.set_active_sessions(["sess-1".to_string()]);
    tracker.apply_event(event("sess-1", "amp", AgentStatus::Error));
    assert!(!tracker.is_unseen("sess-1"));
}

#[test]
fn resuming_one_thread_does_not_clear_another_thread_unseen() {
    let mut tracker = AgentTracker::new();

    tracker.apply_event(event("sess-1", "amp", AgentStatus::Done).with_thread("t1"));
    tracker.apply_event(event("sess-1", "amp", AgentStatus::Done).with_thread("t2"));
    tracker.apply_event(event("sess-1", "amp", AgentStatus::Running).with_thread("t1"));

    assert!(tracker.is_unseen("sess-1"));
    let agents = tracker.get_agents("sess-1");
    assert_eq!(
        agents
            .iter()
            .find(|agent| agent.thread_id.as_deref() == Some("t2"))
            .unwrap()
            .unseen,
        Some(true)
    );
}

#[test]
fn get_agents_returns_newest_first_and_stamps_unseen() {
    let mut tracker = AgentTracker::new();

    tracker.apply_event(event_at("sess-1", "amp", AgentStatus::Done, 100).with_thread("t1"));
    tracker.apply_event(event_at("sess-1", "codex", AgentStatus::Running, 200).with_thread("t2"));

    let agents = tracker.get_agents("sess-1");
    assert_eq!(
        agents
            .iter()
            .map(|agent| agent.thread_id.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("t2"), Some("t1")]
    );
    assert_eq!(agents[1].unseen, Some(true));
}

#[test]
fn dismiss_removes_target_instance_and_synthetic_matches() {
    let mut tracker = AgentTracker::new();

    tracker.apply_pane_presence(
        "sess-1",
        vec![
            PanePresenceInput {
                agent: "pi".to_string(),
                pane_id: "%1".to_string(),
                thread_id: Some("dead".to_string()),
                thread_name: None,
            },
            PanePresenceInput {
                agent: "pi".to_string(),
                pane_id: "%1".to_string(),
                thread_id: Some("live".to_string()),
                thread_name: None,
            },
        ],
    );

    assert!(tracker.dismiss("sess-1", "pi", Some("dead")));
    let remaining = tracker.get_agents("sess-1");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].thread_id.as_deref(), Some("live"));
}

#[test]
fn dedupe_instance_to_session_removes_same_thread_from_other_sessions() {
    let mut tracker = AgentTracker::new();

    tracker.apply_event(event("sess-1", "pi", AgentStatus::Running).with_thread("shared"));
    tracker.apply_event(event("sess-2", "pi", AgentStatus::Running).with_thread("shared"));

    assert!(tracker.dedupe_instance_to_session("sess-2", "pi", Some("shared")));
    assert_eq!(tracker.get_agents("sess-1"), Vec::<AgentEvent>::new());
    assert_eq!(tracker.get_agents("sess-2").len(), 1);
}

#[test]
fn prune_stuck_removes_old_running_unless_alive() {
    let mut tracker = AgentTracker::new();

    tracker.apply_event(
        event_at(
            "sess-1",
            "claude-code",
            AgentStatus::Running,
            now_ms() - 600_000,
        )
        .with_thread("old"),
    );
    tracker.prune_stuck(180_000);
    assert_eq!(tracker.get_state("sess-1"), None);

    tracker.apply_event(
        event_at(
            "sess-2",
            "claude-code",
            AgentStatus::Running,
            now_ms() - 600_000,
        )
        .with_thread("alive"),
    );
    tracker.apply_pane_presence(
        "sess-2",
        vec![PanePresenceInput {
            agent: "claude-code".to_string(),
            pane_id: "%1".to_string(),
            thread_id: None,
            thread_name: None,
        }],
    );
    tracker.prune_stuck(180_000);
    assert_eq!(tracker.get_agents("sess-2").len(), 1);
}

#[test]
fn pane_presence_enriches_exact_thread_and_drops_missing_synthetic_threads() {
    let mut tracker = AgentTracker::new();

    tracker.apply_event(event("sess-1", "pi", AgentStatus::Running).with_thread("thread-a"));
    tracker.apply_event(event("sess-1", "pi", AgentStatus::Running).with_thread("thread-b"));

    assert!(tracker.apply_pane_presence(
        "sess-1",
        vec![PanePresenceInput {
            agent: "pi".to_string(),
            pane_id: "%31".to_string(),
            thread_id: Some("thread-b".to_string()),
            thread_name: None,
        }]
    ));
    let agents = tracker.get_agents("sess-1");
    assert_eq!(
        agents
            .iter()
            .find(|agent| agent.thread_id.as_deref() == Some("thread-a"))
            .unwrap()
            .pane_id,
        None
    );
    assert_eq!(
        agents
            .iter()
            .find(|agent| agent.thread_id.as_deref() == Some("thread-b"))
            .unwrap()
            .pane_id
            .as_deref(),
        Some("%31")
    );

    tracker.apply_pane_presence(
        "sess-2",
        vec![
            PanePresenceInput {
                agent: "pi".to_string(),
                pane_id: "%1".to_string(),
                thread_id: Some("old-dead".to_string()),
                thread_name: None,
            },
            PanePresenceInput {
                agent: "pi".to_string(),
                pane_id: "%1".to_string(),
                thread_id: Some("live".to_string()),
                thread_name: None,
            },
        ],
    );
    assert!(tracker.apply_pane_presence(
        "sess-2",
        vec![PanePresenceInput {
            agent: "pi".to_string(),
            pane_id: "%1".to_string(),
            thread_id: Some("live".to_string()),
            thread_name: None,
        }]
    ));
    let remaining = tracker.get_agents("sess-2");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].thread_id.as_deref(), Some("live"));
    assert_eq!(remaining[0].liveness, Some(AgentLiveness::Alive));
}

#[test]
fn synthetic_pane_entry_merges_when_watcher_event_arrives_for_same_thread() {
    let mut tracker = AgentTracker::new();

    tracker.apply_pane_presence(
        "sess-1",
        vec![PanePresenceInput {
            agent: "pi".to_string(),
            pane_id: "%21".to_string(),
            thread_id: Some("abc".to_string()),
            thread_name: None,
        }],
    );
    tracker.apply_event(event("sess-1", "pi", AgentStatus::Running).with_thread("abc"));

    let agents = tracker.get_agents("sess-1");
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].thread_id.as_deref(), Some("abc"));
    assert_eq!(agents[0].pane_id.as_deref(), Some("%21"));
    assert_eq!(agents[0].liveness, Some(AgentLiveness::Alive));
}

fn event(session: &str, agent: &str, status: AgentStatus) -> AgentEvent {
    event_at(session, agent, status, now_ms())
}

fn event_at(session: &str, agent: &str, status: AgentStatus, ts: u64) -> AgentEvent {
    AgentEvent {
        agent: agent.to_string(),
        session: session.to_string(),
        status,
        ts,
        thread_id: None,
        thread_name: None,
        unseen: None,
        pane_id: None,
        liveness: None,
    }
}

trait AgentEventExt {
    fn with_thread(self, thread_id: &str) -> Self;
}

impl AgentEventExt for AgentEvent {
    fn with_thread(mut self, thread_id: &str) -> Self {
        self.thread_id = Some(thread_id.to_string());
        self
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
