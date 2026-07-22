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
    pub last_user_prompt: Option<String>,
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
    let last_user_prompt = extract_amp_last_user_prompt(&thread);
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
        last_user_prompt,
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
    let mut last_user_prompt = None;
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
        if let Some(prompt) = extract_claude_user_prompt(&entry) {
            if thread_name.is_none() {
                thread_name = normalize_thread_name(&prompt);
            }
            last_user_prompt = Some(prompt);
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
        last_user_prompt,
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
    let mut last_user_prompt = None;
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
        if let Some(prompt) = extract_codex_user_prompt(&entry) {
            if thread_name.is_none() {
                thread_name = normalize_thread_name(&prompt);
            }
            last_user_prompt = Some(prompt);
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
        last_user_prompt,
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
    last_user_prompt_json: Option<&str>,
    now_ms: u64,
) -> Option<AgentWatcherSnapshot> {
    let message = serde_json::from_str::<Value>(last_message_json).ok()?;
    let mut status = determine_opencode_status(&message);
    if status == AgentStatus::Running && now_ms.saturating_sub(time_updated) >= STUCK_MS {
        status = AgentStatus::Stale;
    }
    let last_user_prompt = last_user_prompt_json.and_then(extract_opencode_user_prompt_json);

    Some(AgentWatcherSnapshot {
        agent: "opencode",
        thread_id: Some(session_id.to_string()),
        thread_name: title
            .filter(|title| !title.is_empty())
            .map(ToString::to_string),
        last_user_prompt,
        project_dir: (!directory.is_empty()).then(|| directory.to_string()),
        status,
        ts: now_ms,
    })
}

pub fn pi_snapshot_from_jsonl(
    thread_id: &str,
    raw: &str,
    mtime_ms: u64,
    now_ms: u64,
) -> Option<AgentWatcherSnapshot> {
    let mut session_id = Some(thread_id.to_string());
    let mut project_dir = None;
    let mut thread_name = None;
    let mut last_user_prompt = None;
    let mut status = AgentStatus::Idle;
    let mut saw_entry = false;

    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        saw_entry = true;

        if entry.get("type").and_then(Value::as_str) == Some("session") {
            if let Some(id) = entry.get("id").and_then(Value::as_str) {
                session_id = Some(id.to_string());
            }
            if project_dir.is_none() {
                project_dir = entry
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
            }
            continue;
        }

        if let Some(name) = extract_pi_session_name(&entry) {
            thread_name = Some(name);
        }
        if let Some(prompt) = extract_pi_user_prompt(&entry) {
            if thread_name.is_none() {
                thread_name = normalize_thread_name(&prompt);
            }
            last_user_prompt = Some(prompt);
        }
        let next_status = crate::agent_parsers::determine_pi_status(&entry);
        if next_status != AgentStatus::Idle {
            status = next_status;
        }
    }

    if !saw_entry {
        return None;
    }

    let idle_for = now_ms.saturating_sub(mtime_ms);
    if matches!(status, AgentStatus::Running | AgentStatus::Waiting) && idle_for >= STUCK_MS {
        status = AgentStatus::Stale;
    }

    Some(AgentWatcherSnapshot {
        agent: "pi",
        thread_id: session_id,
        thread_name,
        last_user_prompt,
        project_dir,
        status,
        ts: mtime_ms,
    })
}

pub fn droid_snapshot_from_jsonl(
    thread_id: &str,
    raw: &str,
    mtime_ms: u64,
    now_ms: u64,
) -> Option<AgentWatcherSnapshot> {
    let mut session_id = Some(thread_id.to_string());
    let mut project_dir = None;
    let mut thread_name = None;
    let mut last_user_prompt = None;
    let mut status = AgentStatus::Idle;
    let mut saw_entry = false;

    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        saw_entry = true;

        if let Some(id) = entry
            .get("session_id")
            .or_else(|| entry.get("sessionId"))
            .or_else(|| entry.get("id"))
            .and_then(Value::as_str)
        {
            session_id = Some(id.to_string());
        }
        if project_dir.is_none() {
            project_dir = entry
                .get("cwd")
                .or_else(|| entry.get("directory"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
        }
        if thread_name.is_none() {
            thread_name = entry
                .get("title")
                .or_else(|| entry.get("name"))
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(ToString::to_string);
        }

        if let Some(prompt) = extract_droid_user_prompt(&entry) {
            if thread_name.is_none() {
                thread_name = normalize_thread_name(&prompt);
            }
            last_user_prompt = Some(prompt);
        }
        if let Some(next_status) = determine_droid_status(&entry) {
            status = next_status;
        }
    }

    if !saw_entry {
        return None;
    }

    let idle_for = now_ms.saturating_sub(mtime_ms);
    if matches!(status, AgentStatus::Running | AgentStatus::Waiting) && idle_for >= STUCK_MS {
        status = AgentStatus::Stale;
    }

    Some(AgentWatcherSnapshot {
        agent: "droid",
        thread_id: session_id,
        thread_name,
        last_user_prompt,
        project_dir,
        status,
        ts: mtime_ms,
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

fn extract_amp_last_user_prompt(thread: &Value) -> Option<String> {
    thread
        .get("messages")
        .and_then(Value::as_array)?
        .iter()
        .rev()
        .find_map(|message| {
            (message.get("role").and_then(Value::as_str) == Some("user"))
                .then(|| content_text(message.get("content")))?
        })
        .and_then(|prompt| normalize_prompt(&prompt))
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

fn extract_claude_user_prompt(entry: &Value) -> Option<String> {
    if entry.pointer("/message/role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let text = content_text(entry.pointer("/message/content"))?;
    if text.starts_with('<')
        || text.starts_with('{')
        || text.starts_with("[Request")
        || text.contains("<command-name>/")
    {
        return None;
    }
    normalize_prompt(&text)
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
        let prompt = normalize_codex_user_prompt(message)?;
        return normalize_thread_name(&prompt);
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

fn extract_codex_user_prompt(entry: &Value) -> Option<String> {
    if entry.get("type").and_then(Value::as_str) == Some("event_msg")
        && entry.pointer("/payload/type").and_then(Value::as_str) == Some("user_message")
    {
        let message = entry.pointer("/payload/message").and_then(Value::as_str)?;
        return normalize_codex_user_prompt(message);
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
        return normalize_codex_user_prompt(&text);
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
        return normalize_codex_user_prompt(&text);
    }

    None
}

fn normalize_codex_user_prompt(text: &str) -> Option<String> {
    let prompt = text
        .strip_prefix("## My request for Codex:")
        .unwrap_or(text)
        .trim();
    let candidate = normalize_prompt(prompt)?;
    (!is_codex_internal_prompt(&candidate)).then_some(candidate)
}

fn extract_opencode_user_prompt_json(raw: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(raw).ok()?;
    let text = value
        .get("text")
        .or_else(|| value.pointer("/prompt/text"))
        .or_else(|| value.pointer("/data/text"))
        .and_then(Value::as_str)?;
    normalize_prompt(text)
}

fn extract_pi_session_name(entry: &Value) -> Option<String> {
    (entry.get("type").and_then(Value::as_str) == Some("session_info"))
        .then(|| entry.get("name")?.as_str())?
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
}

fn extract_pi_user_prompt(entry: &Value) -> Option<String> {
    if entry.get("type").and_then(Value::as_str) != Some("message")
        || entry.pointer("/message/role").and_then(Value::as_str) != Some("user")
    {
        return None;
    }
    normalize_prompt(&message_content_text(entry.pointer("/message/content")?)?)
}

fn extract_droid_user_prompt(entry: &Value) -> Option<String> {
    if entry.get("hook_event_name").and_then(Value::as_str) == Some("UserPromptSubmit") {
        return entry
            .get("prompt")
            .and_then(Value::as_str)
            .and_then(normalize_prompt);
    }

    if entry.get("role").and_then(Value::as_str) == Some("user") {
        return entry
            .get("content")
            .or_else(|| entry.get("message"))
            .and_then(message_content_text)
            .and_then(|text| normalize_prompt(&text));
    }

    if entry.pointer("/message/role").and_then(Value::as_str) == Some("user") {
        return entry
            .pointer("/message/content")
            .and_then(message_content_text)
            .and_then(|text| normalize_prompt(&text));
    }

    None
}

fn determine_droid_status(entry: &Value) -> Option<AgentStatus> {
    match entry.get("hook_event_name").and_then(Value::as_str) {
        Some("UserPromptSubmit") => return Some(AgentStatus::Running),
        Some("Stop" | "SessionEnd") => return Some(AgentStatus::Done),
        Some("Notification") => return Some(AgentStatus::Waiting),
        _ => {}
    }

    match entry.get("role").and_then(Value::as_str) {
        Some("user") => Some(AgentStatus::Running),
        Some("assistant") => Some(AgentStatus::Done),
        _ => match entry.pointer("/message/role").and_then(Value::as_str) {
            Some("user") => Some(AgentStatus::Running),
            Some("assistant") => Some(AgentStatus::Done),
            _ => None,
        },
    }
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

fn normalize_prompt(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
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

fn message_content_text(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            (!text.trim().is_empty()).then_some(text)
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_code_snapshot_tracks_latest_real_user_prompt() {
        let raw = r#"
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Implement auth"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Done"}],"stop_reason":"end_turn"}}
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Add tests too"}]}}
"#;
        let snapshot = claude_code_snapshot_from_jsonl("thread", "/repo", raw, 1_000, 1_100)
            .expect("snapshot");
        assert_eq!(snapshot.thread_name.as_deref(), Some("Implement auth"));
        assert_eq!(snapshot.last_user_prompt.as_deref(), Some("Add tests too"));
        assert_eq!(snapshot.status, AgentStatus::Running);
    }

    #[test]
    fn codex_snapshot_strips_request_prefix_and_ignores_internal_prompts() {
        let raw = r###"
{"type":"session_meta","payload":{"id":"abc","cwd":"/repo"}}
{"type":"event_msg","payload":{"type":"user_message","message":"<codex reminder>ignore"}}
{"type":"event_msg","payload":{"type":"user_message","message":"## My request for Codex:\nFix the flaky watcher"}}
{"type":"event_msg","payload":{"type":"task_complete"}}
"###;
        let snapshot = codex_snapshot_from_jsonl("abc", raw, None, 1_000, 1_100).expect("snapshot");
        assert_eq!(
            snapshot.thread_name.as_deref(),
            Some("Fix the flaky watcher")
        );
        assert_eq!(
            snapshot.last_user_prompt.as_deref(),
            Some("Fix the flaky watcher")
        );
        assert_eq!(snapshot.status, AgentStatus::Done);
    }

    #[test]
    fn pi_snapshot_reads_session_header_name_and_latest_user_prompt() {
        let raw = r#"
{"type":"session","version":3,"id":"pi-session","cwd":"/repo"}
{"type":"session_info","id":"n","parentId":null,"name":"Nice title"}
{"type":"message","id":"u1","parentId":null,"message":{"role":"user","content":[{"type":"text","text":"First task"}]}}
{"type":"message","id":"a1","parentId":"u1","message":{"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"Done"}]}}
{"type":"message","id":"u2","parentId":"a1","message":{"role":"user","content":[{"type":"text","text":"Follow up"}]}}
"#;
        let snapshot = pi_snapshot_from_jsonl("file", raw, 1_000, 1_100).expect("snapshot");
        assert_eq!(snapshot.thread_id.as_deref(), Some("pi-session"));
        assert_eq!(snapshot.project_dir.as_deref(), Some("/repo"));
        assert_eq!(snapshot.thread_name.as_deref(), Some("Nice title"));
        assert_eq!(snapshot.last_user_prompt.as_deref(), Some("Follow up"));
        assert_eq!(snapshot.status, AgentStatus::Running);
    }

    #[test]
    fn pi_snapshot_stales_a_waiting_thread_after_idle_timeout() {
        let raw = r#"
{"type":"session","version":3,"id":"pi-session","cwd":"/repo"}
{"type":"message","id":"u1","parentId":null,"message":{"role":"user","content":[{"type":"text","text":"Task"}]}}
{"type":"message","id":"a1","parentId":"u1","message":{"role":"assistant","content":[{"type":"text","text":"Which option?"}]}}
"#;
        let waiting = pi_snapshot_from_jsonl("file", raw, 1_000, 1_100).expect("snapshot");
        assert_eq!(waiting.status, AgentStatus::Waiting);

        let stale = pi_snapshot_from_jsonl("file", raw, 1_000, 1_000 + STUCK_MS).expect("snapshot");
        assert_eq!(stale.status, AgentStatus::Stale);
    }

    #[test]
    fn droid_snapshot_reads_hook_prompt_and_stop_status() {
        let raw = r#"
{"session_id":"droid-session","transcript_path":"/tmp/session.jsonl","cwd":"/repo","hook_event_name":"SessionStart","source":"startup"}
{"session_id":"droid-session","cwd":"/repo","hook_event_name":"UserPromptSubmit","prompt":"Write a migration"}
{"session_id":"droid-session","cwd":"/repo","hook_event_name":"Stop","stop_hook_active":false}
"#;
        let snapshot = droid_snapshot_from_jsonl("file", raw, 1_000, 1_100).expect("snapshot");
        assert_eq!(snapshot.agent, "droid");
        assert_eq!(snapshot.thread_id.as_deref(), Some("droid-session"));
        assert_eq!(snapshot.project_dir.as_deref(), Some("/repo"));
        assert_eq!(snapshot.thread_name.as_deref(), Some("Write a migration"));
        assert_eq!(
            snapshot.last_user_prompt.as_deref(),
            Some("Write a migration")
        );
        assert_eq!(snapshot.status, AgentStatus::Done);
    }

    #[test]
    fn opencode_snapshot_reads_v2_user_prompt_json() {
        let last_message = r#"{"role":"assistant","finish":"stop"}"#;
        let last_user = r#"{"text":"Ship this sidebar"}"#;
        let snapshot = opencode_snapshot_from_row(
            "ses_1",
            Some("Sidebar"),
            "/repo",
            1_000,
            last_message,
            Some(last_user),
            1_100,
        )
        .expect("snapshot");
        assert_eq!(
            snapshot.last_user_prompt.as_deref(),
            Some("Ship this sidebar")
        );
        assert_eq!(snapshot.status, AgentStatus::Done);
    }
}
