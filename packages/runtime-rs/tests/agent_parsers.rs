use opensessions_runtime::agent_parsers::{
    determine_amp_message_status, determine_claude_code_status, determine_codex_status,
    determine_opencode_status, determine_pi_status, map_amp_state,
};
use opensessions_runtime::protocol::AgentStatus;

#[test]
fn amp_state_and_message_status_match_typescript_watcher() {
    assert_eq!(map_amp_state("working"), Some(AgentStatus::Running));
    assert_eq!(map_amp_state("streaming"), Some(AgentStatus::Running));
    assert_eq!(map_amp_state("running_tools"), Some(AgentStatus::Running));
    assert_eq!(map_amp_state("tool_use"), Some(AgentStatus::ToolRunning));
    assert_eq!(
        map_amp_state("awaiting_approval"),
        Some(AgentStatus::Waiting)
    );
    assert_eq!(map_amp_state("idle"), Some(AgentStatus::Done));
    assert_eq!(map_amp_state("error"), Some(AgentStatus::Error));
    assert_eq!(map_amp_state("future"), None);

    assert_eq!(
        determine_amp_message_status(&serde_json::json!({ "role": "user" })),
        AgentStatus::Running
    );
    assert_eq!(
        determine_amp_message_status(
            &serde_json::json!({ "role": "assistant", "state": { "type": "complete", "stopReason": "end_turn" } })
        ),
        AgentStatus::Done
    );
    assert_eq!(
        determine_amp_message_status(
            &serde_json::json!({ "role": "assistant", "state": { "type": "complete", "stopReason": "tool_use" } })
        ),
        AgentStatus::Running
    );
}

#[test]
fn claude_code_status_skips_control_noise_and_maps_lifecycle_entries() {
    assert_eq!(
        determine_claude_code_status(&serde_json::json!({ "type": "queue-operation" })),
        None
    );
    assert_eq!(
        determine_claude_code_status(
            &serde_json::json!({ "message": { "role": "assistant", "content": [{ "type": "tool_use" }], "stop_reason": "tool_use" } })
        ),
        Some(AgentStatus::Running)
    );
    assert_eq!(
        determine_claude_code_status(
            &serde_json::json!({ "message": { "role": "assistant", "content": [{ "type": "text" }], "stop_reason": "end_turn" } })
        ),
        Some(AgentStatus::Done)
    );
    assert_eq!(
        determine_claude_code_status(
            &serde_json::json!({ "message": { "role": "user", "content": "[Request interrupted by user]" } })
        ),
        Some(AgentStatus::Interrupted)
    );
    assert_eq!(
        determine_claude_code_status(
            &serde_json::json!({ "message": { "role": "user", "content": "<command-name>/clear</command-name>" } })
        ),
        None
    );
    assert_eq!(
        determine_claude_code_status(
            &serde_json::json!({ "message": { "role": "user", "content": [{ "type": "tool_result" }] } })
        ),
        Some(AgentStatus::Running)
    );
}

#[test]
fn codex_status_supports_new_and_old_transcript_formats() {
    assert_eq!(
        determine_codex_status(
            &serde_json::json!({ "type": "event_msg", "payload": { "type": "task_complete" } })
        ),
        Some(AgentStatus::Done)
    );
    assert_eq!(
        determine_codex_status(
            &serde_json::json!({ "type": "event_msg", "payload": { "type": "turn_aborted" } })
        ),
        Some(AgentStatus::Interrupted)
    );
    assert_eq!(
        determine_codex_status(
            &serde_json::json!({ "type": "response_item", "payload": { "type": "message", "role": "assistant", "phase": "final_answer" } })
        ),
        Some(AgentStatus::Done)
    );
    assert_eq!(
        determine_codex_status(
            &serde_json::json!({ "type": "response_item", "payload": { "type": "function_call" } })
        ),
        Some(AgentStatus::Running)
    );
    assert_eq!(
        determine_codex_status(&serde_json::json!({ "type": "message", "role": "assistant" })),
        Some(AgentStatus::Running)
    );
    assert_eq!(
        determine_codex_status(
            &serde_json::json!({ "type": "turn_context", "payload": { "cwd": "/repo" } })
        ),
        None
    );
}

#[test]
fn opencode_status_matches_last_message_rules() {
    assert_eq!(
        determine_opencode_status(&serde_json::json!(null)),
        AgentStatus::Idle
    );
    assert_eq!(
        determine_opencode_status(&serde_json::json!({ "role": "user" })),
        AgentStatus::Running
    );
    assert_eq!(
        determine_opencode_status(
            &serde_json::json!({ "role": "assistant", "finish": "tool-calls" })
        ),
        AgentStatus::Running
    );
    assert_eq!(
        determine_opencode_status(&serde_json::json!({ "role": "assistant", "finish": "stop" })),
        AgentStatus::Done
    );
    assert_eq!(
        determine_opencode_status(&serde_json::json!({ "role": "assistant", "finish": "unknown" })),
        AgentStatus::Done
    );
    assert_eq!(
        determine_opencode_status(
            &serde_json::json!({ "role": "assistant", "error": { "name": "MessageAbortedError" } })
        ),
        AgentStatus::Interrupted
    );
    assert_eq!(
        determine_opencode_status(
            &serde_json::json!({ "role": "assistant", "error": { "name": "APIError" } })
        ),
        AgentStatus::Error
    );
}

#[test]
fn pi_status_matches_message_stop_reasons() {
    assert_eq!(
        determine_pi_status(&serde_json::json!({ "type": "session", "cwd": "/repo" })),
        AgentStatus::Idle
    );
    assert_eq!(
        determine_pi_status(
            &serde_json::json!({ "type": "message", "message": { "role": "user" } })
        ),
        AgentStatus::Running
    );
    assert_eq!(
        determine_pi_status(
            &serde_json::json!({ "type": "message", "message": { "role": "toolResult" } })
        ),
        AgentStatus::Running
    );
    assert_eq!(
        determine_pi_status(
            &serde_json::json!({ "type": "message", "message": { "role": "assistant", "stopReason": "toolUse" } })
        ),
        AgentStatus::Running
    );
    assert_eq!(
        determine_pi_status(
            &serde_json::json!({ "type": "message", "message": { "role": "assistant", "stopReason": "stop" } })
        ),
        AgentStatus::Done
    );
    assert_eq!(
        determine_pi_status(
            &serde_json::json!({ "type": "message", "message": { "role": "assistant", "stopReason": "cancelled" } })
        ),
        AgentStatus::Interrupted
    );
    assert_eq!(
        determine_pi_status(
            &serde_json::json!({ "type": "message", "message": { "role": "assistant" } })
        ),
        AgentStatus::Waiting
    );
}
