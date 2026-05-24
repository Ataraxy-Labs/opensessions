use std::collections::HashMap;
use std::process::Command;
use std::sync::Arc;

use crate::mux::{
    ActiveWindow, AgentPane, MuxProvider, MuxSessionInfo, SidebarPane, SidebarPosition,
};

const SEP: &str = "\t";
const STASH_SESSION: &str = "_os_stash";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn ok(&self) -> bool {
        self.exit_code == 0
    }
}

pub trait CommandRunner: Send + Sync {
    fn run(&self, args: &[String]) -> CommandOutput;
}

#[derive(Debug, Clone)]
pub struct StdCommandRunner {
    binary: String,
}

impl StdCommandRunner {
    pub fn new(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
        }
    }
}

impl Default for StdCommandRunner {
    fn default() -> Self {
        Self::new("tmux")
    }
}

impl CommandRunner for StdCommandRunner {
    fn run(&self, args: &[String]) -> CommandOutput {
        match Command::new(&self.binary).args(args).output() {
            Ok(output) => CommandOutput {
                exit_code: output.status.code().unwrap_or(1),
                stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            },
            Err(err) => CommandOutput {
                exit_code: 1,
                stdout: String::new(),
                stderr: err.to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub created_at: u64,
    pub attached_clients: u32,
    pub window_count: u32,
    pub dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: String,
    pub session_id: String,
    pub session_name: String,
    pub index: u32,
    pub name: String,
    pub active: bool,
    pub pane_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneInfo {
    pub id: String,
    pub session_name: String,
    pub window_id: String,
    pub window_index: u32,
    pub index: u32,
    pub active: bool,
    pub tty: String,
    pub pid: u32,
    pub cwd: String,
    pub command: String,
    pub title: String,
    pub width: u16,
    pub height: u16,
    pub left: u16,
    pub right: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInfo {
    pub name: String,
    pub tty: String,
    pub pid: u32,
    pub session_name: String,
    pub width: u16,
    pub height: u16,
}

#[derive(Clone)]
pub struct TmuxClient {
    runner: Arc<dyn CommandRunner>,
}

impl TmuxClient {
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self { runner }
    }

    pub fn run(&self, args: &[&str]) -> CommandOutput {
        let args = args
            .iter()
            .map(|arg| (*arg).to_string())
            .collect::<Vec<_>>();
        self.runner.run(&args)
    }

    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        parse_sessions(&self.run(&["list-sessions", "-F", session_format()]).stdout)
    }

    pub fn list_windows(&self) -> Vec<WindowInfo> {
        parse_windows(
            &self
                .run(&["list-windows", "-a", "-F", window_format()])
                .stdout,
        )
    }

    pub fn list_clients(&self) -> Vec<ClientInfo> {
        parse_clients(&self.run(&["list-clients", "-F", client_format()]).stdout)
    }

    pub fn list_panes(&self, scope: PaneScope<'_>) -> Vec<PaneInfo> {
        let mut args = vec!["list-panes"];
        match scope {
            PaneScope::All => args.push("-a"),
            PaneScope::Session(target) => {
                args.push("-s");
                args.push("-t");
                args.push(target);
            }
            PaneScope::Window(target) => {
                args.push("-t");
                args.push(target);
            }
        }
        args.push("-F");
        args.push(pane_format());
        parse_panes(&self.run(&args).stdout)
    }

    pub fn switch_client(&self, target: &str, client_tty: Option<&str>) {
        let mut args = vec!["switch-client"];
        if let Some(client_tty) = client_tty {
            args.push("-c");
            args.push(client_tty);
        }
        args.push("-t");
        args.push(target);
        self.run(&args);
    }

    pub fn new_session(&self, name: Option<&str>, cwd: Option<&str>) -> String {
        let mut args = vec!["new-session", "-d"];
        if let Some(name) = name {
            args.push("-s");
            args.push(name);
        }
        if let Some(cwd) = cwd {
            args.push("-c");
            args.push(cwd);
        }
        args.extend(["-P", "-F", "#{session_name}"]);
        self.run(&args).stdout
    }

    pub fn kill_session(&self, target: &str) {
        self.run(&["kill-session", "-t", target]);
    }

    pub fn kill_pane(&self, target: &str) {
        self.run(&["kill-pane", "-t", target]);
    }

    pub fn select_window(&self, target: &str) {
        self.run(&["select-window", "-t", target]);
    }

    pub fn select_pane(&self, target: &str) {
        self.run(&["select-pane", "-t", target]);
    }

    pub fn set_pane_title(&self, target: &str, title: &str) {
        self.run(&["select-pane", "-t", target, "-T", title]);
    }

    pub fn resize_pane_width(&self, target: &str, width: u16) {
        self.run(&["resize-pane", "-t", target, "-x", &width.to_string()]);
    }

    pub fn split_sidebar_pane(
        &self,
        target: &str,
        before: bool,
        width: u16,
        command: &str,
    ) -> Option<PaneInfo> {
        let size = width.to_string();
        let side = if before { "-hb" } else { "-h" };
        let output = self.run(&[
            "split-window",
            side,
            "-f",
            "-l",
            &size,
            "-t",
            target,
            "-P",
            "-F",
            pane_format(),
            command,
        ]);
        if !output.ok() || output.stdout.is_empty() {
            return None;
        }
        parse_panes(&output.stdout).into_iter().next()
    }

    pub fn display(&self, format: &str, target: Option<&str>) -> String {
        let mut args = vec!["display-message"];
        if let Some(target) = target {
            args.push("-t");
            args.push(target);
        }
        args.push("-p");
        args.push(format);
        self.run(&args).stdout
    }

    pub fn get_current_session(&self) -> Option<String> {
        self.list_clients()
            .into_iter()
            .find(|client| !client.tty.is_empty())
            .and_then(|client| (!client.session_name.is_empty()).then_some(client.session_name))
    }

    pub fn get_client_tty(&self) -> String {
        self.display("#{client_tty}", None)
    }

    pub fn get_current_window_id(&self) -> Option<String> {
        let window_id = self.display("#{window_id}", None);
        (!window_id.is_empty()).then_some(window_id)
    }

    pub fn get_session_dir(&self, target: &str) -> String {
        self.display("#{pane_current_path}", Some(target))
    }

    pub fn get_pane_count(&self, target: &str) -> u32 {
        self.list_panes(PaneScope::Session(target)).len() as u32
    }

    pub fn get_all_pane_counts(&self) -> HashMap<String, u32> {
        let mut counts = HashMap::new();
        for pane in self.list_panes(PaneScope::All) {
            *counts.entry(pane.session_name).or_insert(0) += 1;
        }
        counts
    }

    pub fn get_active_session_dirs(&self) -> HashMap<String, String> {
        let output = self.run(&[
            "list-panes",
            "-a",
            "-f",
            "#{&&:#{window_active},#{!=:#{pane_title},opensessions-sidebar}}",
            "-F",
            "#{session_name}\t#{pane_current_path}",
        ]);
        let mut dirs = HashMap::new();
        for line in output.stdout.lines() {
            let Some((session, cwd)) = line.split_once(SEP) else {
                continue;
            };
            dirs.entry(session.to_string())
                .or_insert_with(|| cwd.to_string());
        }
        dirs
    }

    pub fn set_global_hook(&self, name: &str, command: &str) {
        self.run(&["set-hook", "-g", name, command]);
    }

    pub fn unset_global_hook(&self, name: &str) {
        self.run(&["set-hook", "-gu", name]);
    }
}

pub enum PaneScope<'a> {
    All,
    Session(&'a str),
    Window(&'a str),
}

#[derive(Clone)]
pub struct TmuxProvider {
    name: String,
    client: TmuxClient,
}

impl TmuxProvider {
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            name: "tmux".to_string(),
            client: TmuxClient::new(runner),
        }
    }
}

impl MuxProvider for TmuxProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn list_sessions(&self) -> Vec<MuxSessionInfo> {
        let active_dirs = self.client.get_active_session_dirs();
        self.client
            .list_sessions()
            .into_iter()
            .filter(|session| session.name != STASH_SESSION)
            .map(|session| MuxSessionInfo {
                name: session.name.clone(),
                created_at: session.created_at,
                dir: active_dirs
                    .get(&session.name)
                    .cloned()
                    .unwrap_or(session.dir),
                windows: session.window_count,
            })
            .collect()
    }

    fn switch_session(&self, name: &str, client_tty: Option<&str>) {
        self.client.switch_client(name, client_tty);
    }

    fn get_current_session(&self) -> Option<String> {
        self.client.get_current_session()
    }

    fn get_session_dir(&self, name: &str) -> String {
        self.client.get_session_dir(name)
    }

    fn get_session_pane_pids(&self, name: &str) -> Vec<u32> {
        self.client
            .list_panes(PaneScope::Session(name))
            .into_iter()
            .map(|pane| pane.pid)
            .filter(|pid| *pid > 0)
            .collect()
    }

    fn get_pane_count(&self, name: &str) -> u32 {
        self.client.get_pane_count(name)
    }

    fn get_client_tty(&self) -> String {
        self.client.get_client_tty()
    }

    fn create_session(&self, name: Option<&str>, dir: Option<&str>) {
        self.client.new_session(name, dir);
    }

    fn kill_session(&self, name: &str) {
        self.client.kill_session(name);
    }

    fn cleanup_sidebar(&self) {
        self.client.kill_session(STASH_SESSION);
    }

    fn setup_hooks(&self, server_host: &str, server_port: u16) {
        let base = format!("http://{server_host}:{server_port}");
        let hook = |path: &str, data: Option<&str>| {
            let body = data.map(|data| format!(" -d '{data}'")).unwrap_or_default();
            format!(
                "run-shell -b \"curl -s -o /dev/null -m 0.2 --connect-timeout 0.1 -X POST {base}{path}{body} >/dev/null 2>&1 || true\""
            )
        };

        let focus_cmd = hook("/focus", Some("#{client_tty}|#{session_name}|#{window_id}"));
        let refresh_cmd = hook("/refresh", None);
        let ensure_cmd = hook(
            "/ensure-sidebar",
            Some("#{client_tty}|#{session_name}|#{window_id}"),
        );
        let client_resized_cmd = hook("/client-resized", None);
        let pane_exited_cmd = hook("/pane-exited", None);

        self.client.set_global_hook(
            "client-session-changed",
            &format!("{focus_cmd} ; {ensure_cmd}"),
        );
        self.client.set_global_hook("session-created", &refresh_cmd);
        self.client.set_global_hook("session-closed", &refresh_cmd);
        self.client
            .set_global_hook("after-select-window", &ensure_cmd);
        self.client.set_global_hook("after-new-window", &ensure_cmd);
        self.client
            .set_global_hook("client-resized", &client_resized_cmd);
        self.client.set_global_hook("pane-exited", &pane_exited_cmd);
    }

    fn cleanup_hooks(&self) {
        for hook in [
            "client-session-changed",
            "session-created",
            "session-closed",
            "after-select-window",
            "after-new-window",
            "client-resized",
            "pane-exited",
        ] {
            self.client.unset_global_hook(hook);
        }
    }

    fn is_window_capable(&self) -> bool {
        true
    }

    fn is_sidebar_capable(&self) -> bool {
        true
    }

    fn is_batch_capable(&self) -> bool {
        true
    }

    fn list_active_windows(&self) -> Vec<ActiveWindow> {
        self.client
            .list_windows()
            .into_iter()
            .filter(|window| window.session_name != STASH_SESSION)
            .map(|window| ActiveWindow {
                id: window.id,
                session_name: window.session_name,
                active: window.active,
            })
            .collect()
    }

    fn get_current_window_id(&self) -> Option<String> {
        self.client.get_current_window_id()
    }

    fn list_sidebar_panes(&self, session_name: Option<&str>) -> Vec<SidebarPane> {
        let panes = match session_name {
            Some(session_name) => self.client.list_panes(PaneScope::Session(session_name)),
            None => self.client.list_panes(PaneScope::All),
        };
        let mut window_widths = HashMap::new();
        for pane in &panes {
            let width = pane.right.saturating_add(1);
            window_widths
                .entry(pane.window_id.clone())
                .and_modify(|current: &mut u16| *current = (*current).max(width))
                .or_insert(width);
        }

        panes
            .into_iter()
            .filter(|pane| {
                pane.title == "opensessions-sidebar" && pane.session_name != STASH_SESSION
            })
            .map(|pane| SidebarPane {
                pane_id: pane.id,
                session_name: pane.session_name,
                window_id: pane.window_id.clone(),
                width: Some(pane.width),
                window_width: window_widths.get(&pane.window_id).copied(),
            })
            .collect()
    }

    fn list_agent_panes(&self, session_name: &str) -> Vec<AgentPane> {
        self.client
            .list_panes(PaneScope::Session(session_name))
            .into_iter()
            .filter(|pane| pane.title != "opensessions-sidebar")
            .filter_map(|pane| agent_from_pane(&pane).map(|agent| (pane, agent)))
            .map(|(pane, agent)| AgentPane {
                thread_name: thread_name_from_pane(&pane, &agent),
                agent,
                pane_id: pane.id,
                thread_id: None,
            })
            .collect()
    }

    fn hide_sidebar(&self, pane_id: &str) {
        self.client.kill_pane(pane_id);
    }

    fn kill_sidebar_pane(&self, pane_id: &str) {
        self.client.kill_pane(pane_id);
    }

    fn focus_pane(&self, pane_id: &str) {
        let window_id = self.client.display("#{window_id}", Some(pane_id));
        if !window_id.is_empty() {
            self.client.select_window(&window_id);
        }
        self.client.select_pane(pane_id);
    }

    fn kill_pane(&self, pane_id: &str) {
        self.client.kill_pane(pane_id);
    }

    fn resolve_agent_pane_id(
        &self,
        session: &str,
        agent: &str,
        _thread_id: Option<&str>,
        thread_name: Option<&str>,
    ) -> Option<String> {
        let panes = self
            .client
            .list_panes(PaneScope::Session(session))
            .into_iter()
            .filter(|pane| pane.title != "opensessions-sidebar")
            .collect::<Vec<_>>();

        if agent == "amp" {
            if let Some(thread_name) = thread_name {
                let matches = panes
                    .iter()
                    .filter(|pane| {
                        pane.title.to_lowercase().starts_with("amp - ")
                            && pane.title.contains(thread_name)
                    })
                    .collect::<Vec<_>>();
                if matches.len() == 1 {
                    return Some(matches[0].id.clone());
                }
            }
        }

        let patterns = match agent {
            "amp" => &["amp"][..],
            "claude-code" => &["claude"][..],
            "codex" => &["codex"][..],
            "opencode" => &["opencode"][..],
            _ => return None,
        };
        panes
            .into_iter()
            .find(|pane| {
                let title = pane.title.to_lowercase();
                patterns.iter().any(|pattern| title.contains(pattern))
            })
            .map(|pane| pane.id)
    }

    fn resize_sidebar_pane(&self, pane_id: &str, width: u16) {
        self.client.resize_pane_width(pane_id, width);
    }

    fn kill_orphaned_sidebar_panes(&self) {
        let panes = self.client.list_panes(PaneScope::All);
        let mut window_pane_counts: HashMap<String, u32> = HashMap::new();
        let mut sidebars_by_window: HashMap<String, Vec<String>> = HashMap::new();

        for pane in panes {
            if pane.session_name == STASH_SESSION {
                continue;
            }
            *window_pane_counts
                .entry(pane.window_id.clone())
                .or_insert(0) += 1;
            if pane.title == "opensessions-sidebar" {
                sidebars_by_window
                    .entry(pane.window_id)
                    .or_default()
                    .push(pane.id);
            }
        }

        for (window_id, sidebars) in sidebars_by_window {
            if window_pane_counts.get(&window_id) == Some(&1) {
                for pane_id in sidebars {
                    self.client.kill_pane(&pane_id);
                }
                continue;
            }
            for pane_id in sidebars.into_iter().skip(1) {
                self.client.kill_pane(&pane_id);
            }
        }
    }

    fn spawn_sidebar(
        &self,
        _session_name: &str,
        window_id: &str,
        width: u16,
        position: SidebarPosition,
        scripts_dir: &str,
    ) -> Option<String> {
        let panes = self.client.list_panes(PaneScope::Window(window_id));
        let target = match position {
            SidebarPosition::Left => panes.iter().min_by_key(|pane| pane.left),
            SidebarPosition::Right => panes.iter().max_by_key(|pane| pane.right),
        }?;
        // Resolve the script path against `$OPENSESSIONS_DIR` so the spawned
        // pane works even when the parent pane's cwd is unrelated to the
        // workspace (e.g. tmux sessions whose default cwd is `$HOME`). Falls
        // back to the literal path if the env is unset.
        let command = format!(
            "OPENSESSIONS_SESSION_NAME={} OPENSESSIONS_WINDOW_ID={window_id} REFOCUS_WINDOW={window_id} exec \"${{OPENSESSIONS_DIR:-.}}\"/{scripts_dir}/start.sh",
            target.session_name,
        );
        let new_pane = self.client.split_sidebar_pane(
            &target.id,
            position == SidebarPosition::Left,
            width,
            &command,
        )?;
        self.client
            .set_pane_title(&new_pane.id, "opensessions-sidebar");
        Some(new_pane.id)
    }

    fn get_all_pane_counts(&self) -> HashMap<String, u32> {
        self.client.get_all_pane_counts()
    }
}

fn session_format() -> &'static str {
    "#{session_id}\t#{session_name}\t#{session_created}\t#{session_attached}\t#{session_windows}\t#{session_path}"
}

fn window_format() -> &'static str {
    "#{window_id}\t#{session_id}\t#{session_name}\t#{window_index}\t#{window_name}\t#{window_active}\t#{window_panes}"
}

fn client_format() -> &'static str {
    "#{client_name}\t#{client_tty}\t#{client_pid}\t#{session_name}\t#{client_width}\t#{client_height}"
}

fn pane_format() -> &'static str {
    "#{pane_id}\t#{session_name}\t#{window_id}\t#{window_index}\t#{pane_index}\t#{pane_active}\t#{pane_tty}\t#{pane_pid}\t#{pane_current_path}\t#{pane_current_command}\t#{pane_title}\t#{pane_width}\t#{pane_height}\t#{pane_left}\t#{pane_right}"
}

fn agent_from_pane(pane: &PaneInfo) -> Option<String> {
    let title = pane.title.to_lowercase();
    let command = pane.command.to_lowercase();
    if title.starts_with("amp") || command.contains("amp") {
        return Some("amp".to_string());
    }
    if title.contains("claude") || command.contains("claude") {
        return Some("claude-code".to_string());
    }
    if title.contains("codex") || command.contains("codex") {
        return Some("codex".to_string());
    }
    if title.contains("opencode") || command.contains("opencode") {
        return Some("opencode".to_string());
    }
    None
}

fn thread_name_from_pane(pane: &PaneInfo, agent: &str) -> Option<String> {
    let title = pane.title.trim();
    if agent == "amp"
        && let Some((_, thread_name)) = title.split_once(" - ")
    {
        let thread_name = thread_name.trim();
        if !thread_name.is_empty() {
            return Some(thread_name.to_string());
        }
    }
    None
}

fn parse_sessions(raw: &str) -> Vec<SessionInfo> {
    raw.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let parts = split(line);
            SessionInfo {
                id: part(&parts, 0),
                name: part(&parts, 1),
                created_at: parse_u64(&parts, 2),
                attached_clients: parse_u32(&parts, 3),
                window_count: parse_u32(&parts, 4),
                dir: part(&parts, 5),
            }
        })
        .collect()
}

fn parse_windows(raw: &str) -> Vec<WindowInfo> {
    raw.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let parts = split(line);
            WindowInfo {
                id: part(&parts, 0),
                session_id: part(&parts, 1),
                session_name: part(&parts, 2),
                index: parse_u32(&parts, 3),
                name: part(&parts, 4),
                active: part(&parts, 5) == "1",
                pane_count: parse_u32(&parts, 6),
            }
        })
        .collect()
}

fn parse_clients(raw: &str) -> Vec<ClientInfo> {
    raw.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let parts = split(line);
            ClientInfo {
                name: part(&parts, 0),
                tty: part(&parts, 1),
                pid: parse_u32(&parts, 2),
                session_name: part(&parts, 3),
                width: parse_u16(&parts, 4),
                height: parse_u16(&parts, 5),
            }
        })
        .collect()
}

fn parse_panes(raw: &str) -> Vec<PaneInfo> {
    raw.lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let parts = split(line);
            if parts.len() < 15 {
                return None;
            }
            Some(PaneInfo {
                id: part(&parts, 0),
                session_name: part(&parts, 1),
                window_id: part(&parts, 2),
                window_index: parse_u32(&parts, 3),
                index: parse_u32(&parts, 4),
                active: part(&parts, 5) == "1",
                tty: part(&parts, 6),
                pid: parse_u32(&parts, 7),
                cwd: part(&parts, 8),
                command: part(&parts, 9),
                title: part(&parts, 10),
                width: parse_u16(&parts, 11),
                height: parse_u16(&parts, 12),
                left: parse_u16(&parts, 13),
                right: parse_u16(&parts, 14),
            })
        })
        .collect()
}

fn split(line: &str) -> Vec<&str> {
    line.split(SEP).collect()
}

fn part(parts: &[&str], index: usize) -> String {
    parts.get(index).copied().unwrap_or_default().to_string()
}

fn parse_u16(parts: &[&str], index: usize) -> u16 {
    parts
        .get(index)
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or_default()
}

fn parse_u32(parts: &[&str], index: usize) -> u32 {
    parts
        .get(index)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or_default()
}

fn parse_u64(parts: &[&str], index: usize) -> u64 {
    parts
        .get(index)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default()
}
