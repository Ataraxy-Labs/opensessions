use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchKind {
    File,
    RecursiveDirectory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchRoot {
    pub path: PathBuf,
    pub kind: WatchKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderWatchSpec {
    pub provider: String,
    pub roots: Vec<WatchRoot>,
    pub debounce: Duration,
    pub fallback_poll: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoalescedWatchRoot {
    pub path: PathBuf,
    pub kind: WatchKind,
    pub providers: Vec<String>,
}

pub fn builtin_provider_specs(home: &Path) -> Vec<ProviderWatchSpec> {
    vec![
        ProviderWatchSpec {
            provider: "amp".to_string(),
            roots: vec![
                WatchRoot {
                    path: home.join(".config/amp/settings.json"),
                    kind: WatchKind::File,
                },
                WatchRoot {
                    path: home.join(".local/share/amp/secrets.json"),
                    kind: WatchKind::File,
                },
            ],
            debounce: Duration::from_millis(500),
            fallback_poll: Some(Duration::from_secs(10)),
        },
        ProviderWatchSpec {
            provider: "claude-code".to_string(),
            roots: vec![WatchRoot {
                path: home.join(".claude/projects"),
                kind: WatchKind::RecursiveDirectory,
            }],
            debounce: Duration::from_millis(150),
            fallback_poll: Some(Duration::from_secs(2)),
        },
        ProviderWatchSpec {
            provider: "codex".to_string(),
            roots: vec![
                WatchRoot {
                    path: home.join(".codex/sessions"),
                    kind: WatchKind::RecursiveDirectory,
                },
                WatchRoot {
                    path: home.join(".codex/session_index.jsonl"),
                    kind: WatchKind::File,
                },
            ],
            debounce: Duration::from_millis(150),
            fallback_poll: Some(Duration::from_secs(2)),
        },
        ProviderWatchSpec {
            provider: "opencode".to_string(),
            roots: vec![WatchRoot {
                path: home.join(".local/share/opencode/opencode.db"),
                kind: WatchKind::File,
            }],
            debounce: Duration::from_millis(300),
            fallback_poll: Some(Duration::from_secs(3)),
        },
        ProviderWatchSpec {
            provider: "pi".to_string(),
            roots: vec![WatchRoot {
                path: home.join(".pi/agent/sessions"),
                kind: WatchKind::RecursiveDirectory,
            }],
            debounce: Duration::from_millis(150),
            fallback_poll: Some(Duration::from_secs(2)),
        },
    ]
}

pub fn coalesce_watch_roots(input: Vec<(&str, PathBuf, WatchKind)>) -> Vec<CoalescedWatchRoot> {
    let mut roots: Vec<CoalescedWatchRoot> = Vec::new();

    let mut sorted = input;
    sorted.sort_by(|a, b| {
        path_len(&a.1)
            .cmp(&path_len(&b.1))
            .then_with(|| a.1.cmp(&b.1))
    });

    for (provider, path, kind) in sorted {
        if let Some(existing) = roots
            .iter_mut()
            .find(|root| root.path == path && root.kind == kind)
        {
            push_provider(&mut existing.providers, provider);
            continue;
        }

        if let Some(parent) = roots
            .iter_mut()
            .find(|root| root.kind == WatchKind::RecursiveDirectory && path.starts_with(&root.path))
        {
            push_provider(&mut parent.providers, provider);
            continue;
        }

        roots.push(CoalescedWatchRoot {
            path,
            kind,
            providers: vec![provider.to_string()],
        });
    }

    roots
}

fn push_provider(providers: &mut Vec<String>, provider: &str) {
    let mut set = providers.iter().cloned().collect::<BTreeSet<_>>();
    set.insert(provider.to_string());
    *providers = set.into_iter().collect();
}

fn path_len(path: &Path) -> usize {
    path.components().count()
}
