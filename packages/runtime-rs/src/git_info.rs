#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitInfo {
    pub branch: String,
    pub dirty: bool,
    pub is_worktree: bool,
}

impl GitInfo {
    pub fn empty() -> Self {
        Self {
            branch: String::new(),
            dirty: false,
            is_worktree: false,
        }
    }
}

pub fn parse_git_info_output(output: &str) -> GitInfo {
    let output = output.trim();
    if output.is_empty() {
        return GitInfo::empty();
    }

    let (header, status) = output
        .split_once("---")
        .map(|(header, status)| (header.trim(), status.trim()))
        .unwrap_or((output, ""));
    let mut lines = header.lines();
    let branch = lines.next().unwrap_or_default().trim().to_string();
    let git_dir = lines.next().unwrap_or_default().trim();

    GitInfo {
        branch,
        dirty: !status.is_empty(),
        is_worktree: git_dir.contains("/worktrees/"),
    }
}
