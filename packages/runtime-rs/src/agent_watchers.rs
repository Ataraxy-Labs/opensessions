use serde_json::Value;

use crate::agent_parsers::{
    determine_amp_message_status, determine_claude_code_status, determine_codex_status,
    determine_opencode_status,
};
use crate::protocol::AgentStatus;

const THREAD_NAME_MAX: usize = 80;
const TOOL_USE_WAIT_MS: u64 = 3_000;
const STUCK_MS: u64 = 15_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentWatcherSnapshot {
    pub agent: &'static str,
    pub thread_id: Option<String>,
    pub thread_name: Option<String>,
    pub project_dir: Option<String>,
    pub status: AgentStatus,
    pub ts: u64,
}

pub fn amp_snapshot_from_thread_json(raw: &str, ts: u64) -> Option<AgentWatcherSnapshot> {
    let thread: Value = serde_json::from_str(raw).ok()?;
    let thread_id = thread
        .get("id")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let thread_name = thread
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty())
        .map(ToString::to_string);
    let project_dir = extract_amp_project_dir(&thread);
    let status = thread
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.last())
        .map(determine_amp_message_status)
        .unwrap_or(AgentStatus::Idle);

    Some(AgentWatcherSnapshot {
        agent: "amp",
        thread_id,
        thread_name,
        project_dir,
        status,
        ts,
    })
}

pub fn claude_code_snapshot_from_jsonl(
    thread_id: &str,
    project_dir: &str,
    raw: &str,
    mtime_ms: u64,
    now_ms: u64,
) -> Option<AgentWatcherSnapshot> {
    let mut status = AgentStatus::Idle;
    let mut thread_name = None;
    let mut last_entry_is_tool_use = false;
    let mut saw_entry = false;

    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        saw_entry = true;

        if let Some(custom_title) = extract_claude_custom_title(&entry) {
            thread_name = Some(custom_title);
            continue;
        }
        if thread_name.is_none() {
            thread_name = extract_claude_thread_name(&entry);
        }

        if let Some(next_status) = determine_claude_code_status(&entry) {
            status = next_status;
        }
        last_entry_is_tool_use = is_claude_tool_use_entry(&entry);
    }

    if !saw_entry {
        return None;
    }

    let idle_for = now_ms.saturating_sub(mtime_ms);
    if status == AgentStatus::Running && last_entry_is_tool_use && idle_for >= TOOL_USE_WAIT_MS {
        status = AgentStatus::Waiting;
    }
    if matches!(status, AgentStatus::Running | AgentStatus::Waiting) && idle_for >= STUCK_MS {
        status = AgentStatus::Stale;
    }

    Some(AgentWatcherSnapshot {
        agent: "claude-code",
        thread_id: Some(thread_id.to_string()),
        thread_name,
        project_dir: Some(project_dir.to_string()),
        status,
        ts: mtime_ms,
    })
}

pub fn codex_snapshot_from_jsonl(
    thread_id: &str,
    raw: &str,
    indexed_thread_name: Option<&str>,
    mtime_ms: u64,
    now_ms: u64,
) -> Option<AgentWatcherSnapshot> {
    let mut status = AgentStatus::Idle;
    let mut project_dir = None;
    let mut thread_name = indexed_thread_name.map(ToString::to_string);
    let mut last_entry_is_tool_call = false;
    let mut saw_entry = false;

    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        saw_entry = true;

        if project_dir.is_none() {
            project_dir = extract_codex_project_dir(&entry);
        }
        if thread_name.is_none() {
            thread_name = extract_codex_thread_name(&entry);
        }
        if let Some(next_status) = determine_codex_status(&entry) {
            status = next_status;
            last_entry_is_tool_call = is_codex_tool_call_entry(&entry);
        }
    }

    if !saw_entry {
        return None;
    }

    let idle_for = now_ms.saturating_sub(mtime_ms);
    if status == AgentStatus::Running && last_entry_is_tool_call && idle_for >= TOOL_USE_WAIT_MS {
        status = AgentStatus::Waiting;
    }
    if matches!(status, AgentStatus::Running | AgentStatus::Waiting) && idle_for >= STUCK_MS {
        status = AgentStatus::Stale;
    }

    Some(AgentWatcherSnapshot {
        agent: "codex",
        thread_id: Some(thread_id.to_string()),
        thread_name,
        project_dir,
        status,
        ts: mtime_ms,
    })
}

pub fn opencode_snapshot_from_row(
    session_id: &str,
    title: Option<&str>,
    directory: &str,
    time_updated: u64,
    last_message_json: &str,
    now_ms: u64,
) -> Option<AgentWatcherSnapshot> {
    let message = serde_json::from_str::<Value>(last_message_json).ok()?;
    let mut status = determine_opencode_status(&message);
    if status == AgentStatus::Running && now_ms.saturating_sub(time_updated) >= STUCK_MS {
        status = AgentStatus::Stale;
    }

    Some(AgentWatcherSnapshot {
        agent: "opencode",
        thread_id: Some(session_id.to_string()),
        thread_name: title
            .filter(|title| !title.is_empty())
            .map(ToString::to_string),
        project_dir: (!directory.is_empty()).then(|| directory.to_string()),
        status,
        ts: now_ms,
    })
}

pub fn codex_thread_id_from_path(path: &str) -> String {
    let name = path
        .rsplit_once('/')
        .map(|(_, name)| name)
        .unwrap_or(path)
        .strip_suffix(".jsonl")
        .unwrap_or_else(|| path.rsplit_once('/').map(|(_, name)| name).unwrap_or(path));

    find_uuid_suffix(name).unwrap_or(name).to_string()
}

pub fn decode_claude_project_dir(encoded: &str, exists: impl Fn(&str) -> bool) -> String {
    let naive = encoded.replace('-', "/");
    if exists(&naive) {
        naive
    } else {
        format!("__encoded__:{encoded}")
    }
}

pub fn parse_codex_session_index(raw: &str) -> Vec<(String, String)> {
    raw.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|entry| {
            let id = entry.get("id")?.as_str()?;
            let name = entry.get("thread_name")?.as_str()?;
            Some((id.to_string(), name.to_string()))
        })
        .collect()
}

fn extract_amp_project_dir(thread: &Value) -> Option<String> {
    let uri = thread
        .pointer("/env/initial/trees/0/uri")
        .and_then(Value::as_str)?;
    uri.strip_prefix("file://").map(ToString::to_string)
}

fn extract_claude_custom_title(entry: &Value) -> Option<String> {
    if entry.get("type").and_then(Value::as_str) != Some("custom-title") {
        return None;
    }
    entry
        .get("customTitle")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty())
        .map(ToString::to_string)
}

fn extract_claude_thread_name(entry: &Value) -> Option<String> {
    if entry.pointer("/message/role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let text = content_text(entry.pointer("/message/content"))?;
    if text.starts_with('<') || text.starts_with('{') || text.starts_with("[Request") {
        return None;
    }
    Some(
        first_non_empty_line(&text)?
            .chars()
            .take(THREAD_NAME_MAX)
            .collect(),
    )
}

fn is_claude_tool_use_entry(entry: &Value) -> bool {
    entry.pointer("/message/role").and_then(Value::as_str) == Some("assistant")
        && content_has_type(entry.pointer("/message/content"), "tool_use")
}

fn extract_codex_project_dir(entry: &Value) -> Option<String> {
    match entry.get("type").and_then(Value::as_str) {
        Some("session_meta" | "turn_context") => entry
            .pointer("/payload/cwd")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        _ => None,
    }
}

fn extract_codex_thread_name(entry: &Value) -> Option<String> {
    if entry.get("type").and_then(Value::as_str) == Some("event_msg")
        && entry.pointer("/payload/type").and_then(Value::as_str) == Some("user_message")
    {
        let message = entry.pointer("/payload/message").and_then(Value::as_str)?;
        if message.starts_with("<codex reminder>") || message.starts_with('<') {
            return None;
        }
        return normalize_thread_name(message);
    }

    if entry.get("type").and_then(Value::as_str) == Some("response_item")
        && entry.pointer("/payload/type").and_then(Value::as_str) == Some("message")
        && entry.pointer("/payload/role").and_then(Value::as_str) == Some("user")
    {
        let text = entry
            .pointer("/payload/content")
            .and_then(Value::as_array)?
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("input_text"))
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        let candidate = normalize_thread_name(&text)?;
        if is_codex_internal_prompt(&candidate) {
            return None;
        }
        return Some(candidate);
    }

    if entry.get("type").and_then(Value::as_str) == Some("message")
        && entry.get("role").and_then(Value::as_str) == Some("user")
    {
        let text = entry
            .get("content")
            .and_then(Value::as_array)?
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("input_text"))
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        let candidate = normalize_thread_name(&text)?;
        if candidate.starts_with('<')
            || candidate.starts_with('{')
            || candidate.starts_with("# AGENTS.md")
        {
            return None;
        }
        return Some(candidate);
    }

    None
}

fn is_codex_tool_call_entry(entry: &Value) -> bool {
    matches!(
        entry.get("type").and_then(Value::as_str),
        Some("function_call")
    ) || entry.get("type").and_then(Value::as_str) == Some("response_item")
        && entry.pointer("/payload/type").and_then(Value::as_str) == Some("function_call")
}

fn is_codex_internal_prompt(candidate: &str) -> bool {
    candidate.starts_with("# AGENTS.md")
        || candidate.starts_with("<environment_context>")
        || candidate.starts_with("<codex reminder>")
        || candidate.starts_with("<permissions ")
        || candidate.starts_with("<app-context>")
        || candidate.starts_with("<collaboration_mode>")
        || candidate.starts_with("<turn_aborted>")
}

fn normalize_thread_name(text: &str) -> Option<String> {
    let line = first_non_empty_line(text)?;
    Some(line.chars().take(THREAD_NAME_MAX).collect())
}

fn first_non_empty_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
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
            .map(ToString::to_string),
        _ => None,
    }
}

fn content_has_type(content: Option<&Value>, target_type: &str) -> bool {
    content.and_then(Value::as_array).is_some_and(|items| {
        items
            .iter()
            .any(|item| item.get("type").and_then(Value::as_str) == Some(target_type))
    })
}

fn find_uuid_suffix(name: &str) -> Option<&str> {
    let bytes = name.as_bytes();
    let len = bytes.len();
    if len < 36 {
        return None;
    }
    for start in (0..=len - 36).rev() {
        let candidate = &name[start..start + 36];
        if is_uuid(candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_uuid(candidate: &str) -> bool {
    candidate.char_indices().all(|(idx, ch)| match idx {
        8 | 13 | 18 | 23 => ch == '-',
        _ => ch.is_ascii_hexdigit(),
    })
}
