use std::collections::{BTreeSet, HashMap};

use crate::protocol::{
    MetadataLogEntry, MetadataProgress, MetadataStatus, MetadataTone, SessionMetadata,
};

const MAX_LOGS: usize = 50;
const MAX_MESSAGE_LENGTH: usize = 500;

#[derive(Debug, Clone, Default)]
pub struct SessionMetadataStore {
    store: HashMap<String, SessionMetadata>,
}

impl SessionMetadataStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, session: &str) -> Option<SessionMetadata> {
        let meta = self.store.get(session)?;
        if meta.status.is_none() && meta.progress.is_none() && meta.logs.is_empty() {
            return None;
        }
        Some(meta.clone())
    }

    pub fn set_status(&mut self, session: &str, status: Option<(String, Option<MetadataTone>)>) {
        match status {
            Some((text, tone)) => {
                let meta = self.get_or_create(session);
                meta.status = Some(MetadataStatus {
                    text: truncate(&text, 100),
                    tone,
                    ts: now_ms(),
                });
            }
            None => {
                if let Some(meta) = self.store.get_mut(session) {
                    meta.status = None;
                }
            }
        }
    }

    pub fn set_progress(
        &mut self,
        session: &str,
        progress: Option<(Option<u64>, Option<u64>, Option<f64>, Option<String>)>,
    ) {
        match progress {
            Some((current, total, percent, label)) => {
                let meta = self.get_or_create(session);
                meta.progress = Some(MetadataProgress {
                    current,
                    total,
                    percent,
                    label: label.map(|label| truncate(&label, 100)),
                    ts: now_ms(),
                });
            }
            None => {
                if let Some(meta) = self.store.get_mut(session) {
                    meta.progress = None;
                }
            }
        }
    }

    pub fn append_log(
        &mut self,
        session: &str,
        message: String,
        tone: Option<MetadataTone>,
        source: Option<String>,
    ) {
        let meta = self.get_or_create(session);
        meta.logs.push(MetadataLogEntry {
            message: truncate(&message, MAX_MESSAGE_LENGTH),
            tone,
            source: source.map(|source| truncate(&source, 50)),
            ts: now_ms(),
        });
        if meta.logs.len() > MAX_LOGS {
            meta.logs.drain(0..meta.logs.len() - MAX_LOGS);
        }
    }

    pub fn clear_logs(&mut self, session: &str) {
        if let Some(meta) = self.store.get_mut(session) {
            meta.logs.clear();
        }
    }

    pub fn prune_sessions(&mut self, valid_names: impl IntoIterator<Item = String>) {
        let valid = valid_names.into_iter().collect::<BTreeSet<_>>();
        self.store.retain(|name, _| valid.contains(name));
    }

    fn get_or_create(&mut self, session: &str) -> &mut SessionMetadata {
        self.store.entry(session.to_string()).or_default()
    }
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    value.chars().take(max - 1).collect::<String>() + "…"
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
