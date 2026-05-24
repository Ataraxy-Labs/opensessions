use opensessions_runtime::agent_watchers::{
    amp_snapshot_from_thread_json, claude_code_snapshot_from_jsonl, codex_snapshot_from_jsonl,
    opencode_snapshot_from_row,
};
use opensessions_runtime::protocol::AgentStatus;

#[test]
fn amp_thread_snapshot_extracts_project_title_and_status() {
    let raw = r#"{
        "id":"T-amp",
        "title":"Production server-rendered sidebar architecture",
        "env":{"initial":{"trees":[{"uri":"file:///repo/opensessions"}]}},
        "messages":[{"role":"user","content":[{"type":"text","text":"continue"}]}]
    }"#;

    let snapshot = amp_snapshot_from_thread_json(raw, 123).expect("snapshot should parse");

    assert_eq!(snapshot.agent, "amp");
    assert_eq!(snapshot.thread_id.as_deref(), Some("T-amp"));
    assert_eq!(
        snapshot.thread_name.as_deref(),
        Some("Production server-rendered sidebar architecture")
    );
    assert_eq!(snapshot.project_dir.as_deref(), Some("/repo/opensessions"));
    assert_eq!(snapshot.status, AgentStatus::Running);
    assert_eq!(snapshot.ts, 123);
}

#[test]
fn claude_code_snapshot_uses_custom_title_and_ignores_control_entries() {
    let raw = concat!(
        "{\"type\":\"custom-title\",\"customTitle\":\"Fix sidebar streaming\"}\n",
        "{\"type\":\"queue-operation\"}\n",
        "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"stop_reason\":\"end_turn\",\"content\":[{\"type\":\"text\"}]}}\n"
    );

    let snapshot =
        claude_code_snapshot_from_jsonl("claude-thread", "/repo/opensessions", raw, 200, 200)
            .expect("snapshot should parse");

    assert_eq!(snapshot.agent, "claude-code");
    assert_eq!(snapshot.thread_id.as_deref(), Some("claude-thread"));
    assert_eq!(
        snapshot.thread_name.as_deref(),
        Some("Fix sidebar streaming")
    );
    assert_eq!(snapshot.project_dir.as_deref(), Some("/repo/opensessions"));
    assert_eq!(snapshot.status, AgentStatus::Done);
}

#[test]
fn codex_snapshot_prefers_session_index_name_and_extracts_cwd() {
    let raw = concat!(
        "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/repo/opensessions\"}}\n",
        "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"Implement watchers\"}}\n",
        "{\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"phase\":\"final_answer\"}}\n"
    );

    let snapshot = codex_snapshot_from_jsonl("codex-thread", raw, Some("Indexed name"), 300, 300)
        .expect("snapshot should parse");

    assert_eq!(snapshot.agent, "codex");
    assert_eq!(snapshot.thread_id.as_deref(), Some("codex-thread"));
    assert_eq!(snapshot.thread_name.as_deref(), Some("Indexed name"));
    assert_eq!(snapshot.project_dir.as_deref(), Some("/repo/opensessions"));
    assert_eq!(snapshot.status, AgentStatus::Done);
}

#[test]
fn opencode_snapshot_maps_last_message_json() {
    let snapshot = opencode_snapshot_from_row(
        "opencode-thread",
        Some("Review renderer"),
        "/repo/opensessions",
        400,
        r#"{"role":"assistant","finish":"tool-calls","time":{"created":1,"completed":2}}"#,
        401,
    )
    .expect("snapshot should parse");

    assert_eq!(snapshot.agent, "opencode");
    assert_eq!(snapshot.thread_id.as_deref(), Some("opencode-thread"));
    assert_eq!(snapshot.thread_name.as_deref(), Some("Review renderer"));
    assert_eq!(snapshot.project_dir.as_deref(), Some("/repo/opensessions"));
    assert_eq!(snapshot.status, AgentStatus::Running);
    assert_eq!(snapshot.ts, 401);
}
