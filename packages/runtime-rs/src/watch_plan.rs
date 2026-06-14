use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
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
            roots: claude_code_projects_dirs(home)
                .into_iter()
                .map(|path| WatchRoot {
                    path,
                    kind: WatchKind::RecursiveDirectory,
                })
                .collect(),
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

pub fn claude_code_projects_dirs(home: &Path) -> Vec<PathBuf> {
    claude_code_projects_dirs_with_config_dir(home, std::env::var_os("CLAUDE_CONFIG_DIR"))
}

fn claude_code_projects_dirs_with_config_dir(
    home: &Path,
    config_dir: Option<OsString>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    push_unique(&mut dirs, home.join(".claude/projects"));

    if let Some(config_dir) = config_dir.filter(|value| !value.is_empty()) {
        push_unique(
            &mut dirs,
            expand_home_path(home, PathBuf::from(config_dir)).join("projects"),
        );
    }

    if let Ok(entries) = fs::read_dir(home) {
        let mut sibling_projects_dirs = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name();
                let name = name.to_str()?;
                if !name.starts_with(".claude") {
                    return None;
                }
                let projects_dir = entry.path().join("projects");
                projects_dir.is_dir().then_some(projects_dir)
            })
            .collect::<Vec<_>>();
        sibling_projects_dirs.sort();
        for projects_dir in sibling_projects_dirs {
            push_unique(&mut dirs, projects_dir);
        }
    }

    dirs
}

fn expand_home_path(home: &Path, path: PathBuf) -> PathBuf {
    let Some(path_text) = path.to_str() else {
        return path;
    };
    if path_text == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = path_text.strip_prefix("~/") {
        return home.join(rest);
    }
    path
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn claude_code_projects_dirs_include_env_config_dir() {
        let home = unique_test_home("claude-env");

        let dirs = claude_code_projects_dirs_with_config_dir(
            &home,
            Some(OsString::from("~/.claude-personal")),
        );

        assert_eq!(
            dirs,
            vec![
                home.join(".claude/projects"),
                home.join(".claude-personal/projects"),
            ]
        );

        fs::remove_dir_all(&home).expect("clean test home");
    }

    #[test]
    fn claude_code_projects_dirs_scan_existing_sibling_config_dirs() {
        let home = unique_test_home("claude-siblings");
        fs::create_dir_all(home.join(".claude/projects")).expect("create default projects dir");
        fs::create_dir_all(home.join(".claude-personal/projects"))
            .expect("create personal projects dir");
        fs::create_dir_all(home.join(".claude-work/no-projects"))
            .expect("create non-project claude dir");
        fs::create_dir_all(home.join(".not-claude/projects")).expect("create unrelated dir");

        let dirs = claude_code_projects_dirs_with_config_dir(&home, None);

        assert_eq!(
            dirs,
            vec![
                home.join(".claude/projects"),
                home.join(".claude-personal/projects"),
            ]
        );

        fs::remove_dir_all(&home).expect("clean test home");
    }

    #[test]
    fn claude_code_projects_dirs_deduplicate_env_and_scan_matches() {
        let home = unique_test_home("claude-dedupe");
        fs::create_dir_all(home.join(".claude-personal/projects"))
            .expect("create personal projects dir");

        let dirs = claude_code_projects_dirs_with_config_dir(
            &home,
            Some(OsString::from(home.join(".claude-personal"))),
        );

        assert_eq!(
            dirs,
            vec![
                home.join(".claude/projects"),
                home.join(".claude-personal/projects"),
            ]
        );

        fs::remove_dir_all(&home).expect("clean test home");
    }

    fn unique_test_home(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time is after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("opensessions-{name}-{suffix}"));
        fs::create_dir_all(&path).expect("create test home");
        path
    }
}
