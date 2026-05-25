use std::collections::HashMap;

use serde_json::Value;

const DEFAULT_TTL_MS: u64 = 20_000;

#[derive(Debug, Clone, PartialEq)]
pub struct PiRuntimeInfo {
    pub pid: u32,
    pub ppid: Option<u32>,
    pub session_id: String,
    pub session_file: Option<String>,
    pub cwd: String,
    pub session_name: Option<String>,
    pub ts: u64,
}

#[derive(Debug, Clone)]
pub struct PiRuntimeRegistry {
    by_pid: HashMap<u32, PiRuntimeInfo>,
    ttl_ms: u64,
}

impl PiRuntimeRegistry {
    pub fn new(ttl_ms: u64) -> Self {
        Self {
            by_pid: HashMap::new(),
            ttl_ms,
        }
    }

    pub fn with_default_ttl() -> Self {
        Self::new(DEFAULT_TTL_MS)
    }

    pub fn upsert(&mut self, info: PiRuntimeInfo) {
        self.by_pid.insert(info.pid, info);
    }

    pub fn delete(&mut self, pid: u32) -> bool {
        self.by_pid.remove(&pid).is_some()
    }

    pub fn get(&mut self, pid: u32, now: u64) -> Option<PiRuntimeInfo> {
        let info = self.by_pid.get(&pid)?;
        if now.saturating_sub(info.ts) > self.ttl_ms {
            self.by_pid.remove(&pid);
            return None;
        }
        self.by_pid.get(&pid).cloned()
    }

    pub fn prune(&mut self, now: u64) -> bool {
        let before = self.by_pid.len();
        self.by_pid
            .retain(|_, info| now.saturating_sub(info.ts) <= self.ttl_ms);
        self.by_pid.len() != before
    }

    pub fn size(&mut self, now: u64) -> usize {
        self.prune(now);
        self.by_pid.len()
    }
}

pub fn parse_pi_runtime_info(value: &Value, now: u64) -> Option<PiRuntimeInfo> {
    let raw = value.as_object()?;
    let pid = positive_u32(raw.get("pid")?)?;
    let ppid = match raw.get("ppid") {
        Some(value) => Some(positive_u32(value)?),
        None => None,
    };
    let session_id = non_empty_string(raw.get("sessionId")?)?;
    let cwd = non_empty_string(raw.get("cwd")?)?;
    let session_file = optional_non_empty_string(raw.get("sessionFile"))?;
    let session_name = optional_non_empty_string(raw.get("sessionName"))?;
    let ts = raw.get("ts").and_then(Value::as_u64).unwrap_or(now);

    Some(PiRuntimeInfo {
        pid,
        ppid,
        session_id,
        session_file,
        cwd,
        session_name,
        ts,
    })
}

fn positive_u32(value: &Value) -> Option<u32> {
    let value = value.as_u64()?;
    (value > 0 && value <= u32::MAX as u64).then_some(value as u32)
}

fn non_empty_string(value: &Value) -> Option<String> {
    let value = value.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn optional_non_empty_string(value: Option<&Value>) -> Option<Option<String>> {
    match value {
        Some(Value::String(value)) if !value.is_empty() => Some(Some(value.clone())),
        Some(Value::String(_)) | None => Some(None),
        _ => None,
    }
}
