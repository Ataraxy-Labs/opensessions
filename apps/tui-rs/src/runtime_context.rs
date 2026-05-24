use crate::generated::protocol::ClientCommand;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneIdentity {
    pub pane_id: String,
    pub session_name: String,
    pub window_id: Option<String>,
}

pub fn pane_identity_from_env<F>(env: F) -> Option<PaneIdentity>
where
    F: Fn(&str) -> Option<String>,
{
    pane_identity_resolve(env, |_, _| None)
}

/// Resolve the running pane's identity, mirroring
/// `apps/tui/src/index.tsx` (`getLocalSessionName` / `getLocalWindowId`).
///
/// `env` reads process environment variables. `tmux_query` invokes
/// `tmux display-message -p -t <target> <format>` and returns the trimmed
/// stdout. Tmux is only consulted when the corresponding `OPENSESSIONS_*`
/// env vars are absent, matching the OpenTUI client priority.
pub fn pane_identity_resolve<F, T>(env: F, tmux_query: T) -> Option<PaneIdentity>
where
    F: Fn(&str) -> Option<String>,
    T: Fn(&str, &str) -> Option<String>,
{
    let pane_id = env("TMUX_PANE")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;

    let session_name = env("OPENSESSIONS_SESSION_NAME")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            tmux_query("#{session_name}", &pane_id)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })?;

    let window_id = env("OPENSESSIONS_WINDOW_ID")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            tmux_query("#{window_id}", &pane_id)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        });

    Some(PaneIdentity {
        pane_id,
        session_name,
        window_id,
    })
}

pub fn should_report_width(local_session: Option<&str>, current_session: Option<&str>) -> bool {
    let Some(local) = local_session else {
        return false;
    };
    if local == "_os_stash" {
        return false;
    }
    match current_session {
        Some(current) => current == local,
        None => true,
    }
}

/// Plan describing which tmux pane should receive focus after the sidebar
/// finishes capability detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefocusPlan {
    pub select_pane: String,
}

/// Mirror of `apps/tui/src/index.tsx::refocusMainPane` as a pure function.
///
/// `tmux_query` is invoked with the argv slice that should be passed to
/// `tmux <args>`. It must return the trimmed stdout when the command succeeds
/// or `None` when it fails. The function is decoupled from process spawning to
/// keep red/green TDD on the sidebar refocus rules straightforward.
pub fn refocus_plan<F>(
    pane_id: &str,
    refocus_window_env: Option<&str>,
    tmux_query: F,
) -> Option<RefocusPlan>
where
    F: Fn(&[&str]) -> Option<String>,
{
    let window_id = match refocus_window_env.map(str::trim).filter(|s| !s.is_empty()) {
        Some(window) => window.to_string(),
        None => tmux_query(&["display-message", "-t", pane_id, "-p", "#{window_id}"])
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())?,
    };

    let panes = tmux_query(&[
        "list-panes",
        "-t",
        &window_id,
        "-F",
        "#{pane_id} #{pane_title}",
    ])?;
    let main_line = panes
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.contains("opensessions-sidebar"))?;
    let main_pane = main_line.split_whitespace().next()?;

    Some(RefocusPlan {
        select_pane: main_pane.to_string(),
    })
}

pub fn report_width_command(
    width: u32,
    local_session: Option<&str>,
    current_session: Option<&str>,
) -> Option<ClientCommand> {
    if !should_report_width(local_session, current_session) {
        return None;
    }
    Some(ClientCommand::ReportWidth { width })
}
