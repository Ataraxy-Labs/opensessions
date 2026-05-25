use serde_json::Value;

use crate::protocol::AgentStatus;

pub fn map_amp_state(amp_state: &str) -> Option<AgentStatus> {
    match amp_state {
        "working" | "streaming" | "running_tools" => Some(AgentStatus::Running),
        "tool_use" => Some(AgentStatus::ToolRunning),
        "awaiting_approval" => Some(AgentStatus::Waiting),
        "idle" => Some(AgentStatus::Done),
        "error" => Some(AgentStatus::Error),
        _ => None,
    }
}

pub fn determine_amp_message_status(last_msg: &Value) -> AgentStatus {
    let Some(role) = last_msg.get("role").and_then(Value::as_str) else {
        return AgentStatus::Idle;
    };

    match role {
        "user" => {
            if content_has_type_with_run_status(
                last_msg.get("content"),
                "tool_result",
                "in-progress",
            ) {
                AgentStatus::ToolRunning
            } else {
                AgentStatus::Running
            }
        }
        "assistant" => {
            let state_type = last_msg.pointer("/state/type").and_then(Value::as_str);
            match state_type {
                None => AgentStatus::Running,
                Some("streaming") => AgentStatus::Running,
                Some("cancelled") => AgentStatus::Interrupted,
                Some("complete") => match last_msg
                    .pointer("/state/stopReason")
                    .and_then(Value::as_str)
                {
                    Some("tool_use") => AgentStatus::Running,
                    Some("end_turn") => AgentStatus::Done,
                    _ => AgentStatus::Error,
                },
                _ => AgentStatus::Running,
            }
        }
        _ => AgentStatus::Idle,
    }
}

pub fn determine_claude_code_status(entry: &Value) -> Option<AgentStatus> {
    let message = entry.get("message")?;
    let role = message.get("role").and_then(Value::as_str)?;
    let content = message.get("content");

    match role {
        "assistant" => {
            if content_has_type(content, "tool_use") || content_has_type(content, "thinking") {
                return Some(AgentStatus::Running);
            }
            match message.get("stop_reason").and_then(Value::as_str) {
                None => Some(AgentStatus::Running),
                Some("end_turn") => Some(AgentStatus::Done),
                Some("tool_use") => Some(AgentStatus::Running),
                Some(_) => Some(AgentStatus::Done),
            }
        }
        "user" => {
            if let Some(text) = content_text(content) {
                if text.starts_with("[Request interrupted by user")
                    || text.starts_with("[Request interrupted")
                {
                    return Some(AgentStatus::Interrupted);
                }
                if text.contains("<command-name>/exit</command-name>") {
                    return Some(AgentStatus::Done);
                }
                if text.contains("<command-name>/") || is_noise_user_prefix(&text) {
                    return None;
                }
            }
            if content_has_type(content, "tool_result") {
                return Some(AgentStatus::Running);
            }
            Some(AgentStatus::Running)
        }
        _ => None,
    }
}

pub fn determine_codex_status(entry: &Value) -> Option<AgentStatus> {
    match entry.get("type").and_then(Value::as_str) {
        Some("event_msg") => match entry.pointer("/payload/type").and_then(Value::as_str) {
            Some("task_complete") => Some(AgentStatus::Done),
            Some("turn_aborted") => Some(AgentStatus::Interrupted),
            Some("task_started" | "user_message") => Some(AgentStatus::Running),
            Some("agent_message") => {
                match entry.pointer("/payload/phase").and_then(Value::as_str) {
                    Some("final_answer") => Some(AgentStatus::Done),
                    _ => Some(AgentStatus::Running),
                }
            }
            _ => None,
        },
        Some("response_item") => match entry.pointer("/payload/type").and_then(Value::as_str) {
            Some("message") => match entry.pointer("/payload/role").and_then(Value::as_str) {
                Some("developer") => None,
                Some("user") => Some(AgentStatus::Running),
                Some("assistant") => {
                    match entry.pointer("/payload/phase").and_then(Value::as_str) {
                        Some("final_answer") => Some(AgentStatus::Done),
                        _ => Some(AgentStatus::Running),
                    }
                }
                _ => None,
            },
            Some(
                "function_call"
                | "function_call_output"
                | "reasoning"
                | "custom_tool_call"
                | "custom_tool_call_output"
                | "web_search_call",
            ) => Some(AgentStatus::Running),
            _ => None,
        },
        Some("message") => match entry.get("role").and_then(Value::as_str) {
            Some("user" | "assistant") => Some(AgentStatus::Running),
            _ => None,
        },
        Some("function_call" | "function_call_output" | "reasoning") => Some(AgentStatus::Running),
        _ => None,
    }
}

pub fn determine_opencode_status(msg: &Value) -> AgentStatus {
    let Some(role) = msg.get("role").and_then(Value::as_str) else {
        return AgentStatus::Idle;
    };

    if let Some(error_name) = msg.pointer("/error/name").and_then(Value::as_str) {
        return if error_name == "MessageAbortedError" {
            AgentStatus::Interrupted
        } else {
            AgentStatus::Error
        };
    }

    match role {
        "assistant" => match msg.get("finish").and_then(Value::as_str) {
            Some("tool-calls") => AgentStatus::Running,
            Some("stop") => AgentStatus::Done,
            Some("error") => AgentStatus::Error,
            Some("unknown") => AgentStatus::Done,
            _ if msg
                .pointer("/time/completed")
                .and_then(Value::as_u64)
                .is_some() =>
            {
                AgentStatus::Done
            }
            _ => AgentStatus::Running,
        },
        "user" => AgentStatus::Running,
        _ => AgentStatus::Idle,
    }
}

pub fn determine_pi_status(entry: &Value) -> AgentStatus {
    if entry.get("type").and_then(Value::as_str) != Some("message") {
        return AgentStatus::Idle;
    }
    let Some(role) = entry.pointer("/message/role").and_then(Value::as_str) else {
        return AgentStatus::Idle;
    };

    match role {
        "user" | "toolResult" => AgentStatus::Running,
        "assistant" => match entry.pointer("/message/stopReason").and_then(Value::as_str) {
            Some("toolUse") => AgentStatus::Running,
            Some("stop") => AgentStatus::Done,
            Some("error") => AgentStatus::Error,
            Some("cancelled" | "aborted" | "interrupted") => AgentStatus::Interrupted,
            _ => AgentStatus::Waiting,
        },
        _ => AgentStatus::Idle,
    }
}

fn content_has_type(content: Option<&Value>, target_type: &str) -> bool {
    content.and_then(Value::as_array).is_some_and(|items| {
        items
            .iter()
            .any(|item| item.get("type").and_then(Value::as_str) == Some(target_type))
    })
}

fn content_has_type_with_run_status(
    content: Option<&Value>,
    target_type: &str,
    run_status: &str,
) -> bool {
    content.and_then(Value::as_array).is_some_and(|items| {
        items.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some(target_type)
                && item.pointer("/run/status").and_then(Value::as_str) == Some(run_status)
        })
    })
}

fn content_text(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => items
            .iter()
            .find(|item| {
                item.get("type").and_then(Value::as_str) == Some("text")
                    && item.get("text").is_some()
            })
            .and_then(|item| item.get("text").and_then(Value::as_str))
            .map(str::to_string),
        _ => None,
    }
}

fn is_noise_user_prefix(text: &str) -> bool {
    [
        "<local-command-caveat>",
        "<local-command-stdout>",
        "<local-command-stderr>",
        "<bash-input>",
        "<bash-stdout>",
        "<bash-stderr>",
        "<system-reminder>",
        "<task-notification>",
    ]
    .iter()
    .any(|prefix| text.starts_with(prefix))
}
