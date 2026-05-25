use std::path::PathBuf;
use std::time::Duration;

use opensessions_runtime::watch_plan::{WatchKind, builtin_provider_specs, coalesce_watch_roots};

#[test]
fn builtin_provider_specs_cover_all_current_agent_providers() {
    let specs = builtin_provider_specs(&PathBuf::from("/Users/alice"));
    let names: Vec<_> = specs.iter().map(|spec| spec.provider.as_str()).collect();

    assert_eq!(names, vec!["amp", "claude-code", "codex", "opencode", "pi"]);
}

#[test]
fn filesystem_backed_providers_use_recursive_roots_with_debounce() {
    let specs = builtin_provider_specs(&PathBuf::from("/Users/alice"));

    let claude = specs
        .iter()
        .find(|spec| spec.provider == "claude-code")
        .unwrap();
    assert_eq!(
        claude.roots[0].path,
        PathBuf::from("/Users/alice/.claude/projects")
    );
    assert_eq!(claude.roots[0].kind, WatchKind::RecursiveDirectory);
    assert_eq!(claude.debounce, Duration::from_millis(150));

    let opencode = specs
        .iter()
        .find(|spec| spec.provider == "opencode")
        .unwrap();
    assert_eq!(
        opencode.roots[0].path,
        PathBuf::from("/Users/alice/.local/share/opencode/opencode.db")
    );
    assert_eq!(opencode.roots[0].kind, WatchKind::File);
}

#[test]
fn coalesces_duplicate_and_child_roots_for_more_efficient_watching() {
    let roots = coalesce_watch_roots(vec![
        (
            "claude-code",
            PathBuf::from("/Users/alice/.claude/projects"),
            WatchKind::RecursiveDirectory,
        ),
        (
            "claude-code",
            PathBuf::from("/Users/alice/.claude/projects/foo"),
            WatchKind::RecursiveDirectory,
        ),
        (
            "codex",
            PathBuf::from("/Users/alice/.codex/sessions"),
            WatchKind::RecursiveDirectory,
        ),
        (
            "pi",
            PathBuf::from("/Users/alice/.pi/agent/sessions"),
            WatchKind::RecursiveDirectory,
        ),
        (
            "pi",
            PathBuf::from("/Users/alice/.pi/agent/sessions"),
            WatchKind::RecursiveDirectory,
        ),
    ]);

    assert_eq!(roots.len(), 3);
    assert!(roots.iter().any(
        |root| root.path == PathBuf::from("/Users/alice/.claude/projects")
            && root.providers == vec!["claude-code"]
    ));
    assert!(roots.iter().any(
        |root| root.path == PathBuf::from("/Users/alice/.codex/sessions")
            && root.providers == vec!["codex"]
    ));
    assert!(roots.iter().any(
        |root| root.path == PathBuf::from("/Users/alice/.pi/agent/sessions")
            && root.providers == vec!["pi"]
    ));
}
