use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use opensessions_runtime::mux::{MuxProvider, SidebarPosition};
use opensessions_runtime::tmux_provider::{
    CommandOutput, CommandRunner, StdCommandRunner, TmuxClient, TmuxProvider,
};

#[test]
fn tmux_client_parses_sessions_clients_panes_and_counts() {
    let runner = RecordingRunner::new(HashMap::from([
        ("list-sessions".to_string(), "$1\talpha\t100\t1\t2\t/repo\n$2\tbeta\t200\t0\t1\t/tmp".to_string()),
        ("list-clients".to_string(), "control\t\t100\talpha\t80\t24\n/dev/ttys004\t/dev/ttys004\t101\tbeta\t160\t40".to_string()),
        ("list-panes".to_string(), "%1\talpha\t@1\t0\t0\t1\t/dev/ttys1\t123\t/repo\tbash\tmain\t80\t24\t0\t79\n%2\talpha\t@1\t0\t1\t0\t/dev/ttys2\t124\t/repo\tzsh\topensessions-sidebar\t26\t24\t0\t25".to_string()),
        ("display-message".to_string(), "/repo".to_string()),
    ]));
    let client = TmuxClient::new(Arc::new(runner));

    let sessions = client.list_sessions();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].name, "alpha");
    assert_eq!(sessions[0].window_count, 2);

    assert_eq!(client.get_current_session(), Some("beta".to_string()));
    assert_eq!(client.get_session_dir("alpha"), "/repo");
    assert_eq!(client.get_pane_count("alpha"), 2);
    assert_eq!(client.get_all_pane_counts().get("alpha"), Some(&2));
}

#[test]
fn tmux_provider_filters_stash_and_uses_active_session_dirs() {
    let runner = RecordingRunner::new(HashMap::from([
        (
            "list-sessions".to_string(),
            "$1\talpha\t100\t1\t2\t/old\n$2\t_os_stash\t200\t0\t1\t/tmp".to_string(),
        ),
        ("list-panes".to_string(), "alpha\t/new".to_string()),
    ]));
    let provider = TmuxProvider::new(Arc::new(runner));

    let sessions = provider.list_sessions();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].name, "alpha");
    assert_eq!(sessions[0].dir, "/new");
    assert_eq!(sessions[0].windows, 2);
}

#[test]
fn tmux_provider_sends_core_commands_to_runner() {
    let runner = Arc::new(RecordingRunner::new(HashMap::new()));
    let provider = TmuxProvider::new(runner.clone());

    provider.switch_session("alpha", Some("/dev/ttys004"));
    provider.create_session(Some("new"), Some("/repo"));
    provider.kill_session("old");
    provider.setup_hooks("127.0.0.1", 7391);
    provider.cleanup_hooks();

    let calls = runner.calls.lock().unwrap().clone();
    assert!(
        calls
            .iter()
            .any(|call| call == &vec!["switch-client", "-c", "/dev/ttys004", "-t", "alpha"])
    );
    assert!(calls.iter().any(|call| call
        == &vec![
            "new-session",
            "-d",
            "-s",
            "new",
            "-c",
            "/repo",
            "-P",
            "-F",
            "#{session_name}"
        ]));
    assert!(
        calls
            .iter()
            .any(|call| call == &vec!["kill-session", "-t", "old"])
    );
    assert!(
        calls
            .iter()
            .any(|call| call[0] == "set-hook" && call[2] == "client-session-changed")
    );
    assert!(
        calls
            .iter()
            .any(|call| call == &vec!["set-hook", "-gu", "pane-exited"])
    );
}

#[test]
fn tmux_provider_lists_active_windows_and_filters_stash_session() {
    let runner = RecordingRunner::new(HashMap::from([(
        "list-windows".to_string(),
        "@1\t$1\talpha\t0\tmain\t1\t2\n@2\t$2\t_os_stash\t0\tstash\t1\t1\n@3\t$3\tbeta\t1\teditor\t0\t1"
            .to_string(),
    )]));
    let provider = TmuxProvider::new(Arc::new(runner));

    let windows = provider.list_active_windows();
    assert_eq!(windows.len(), 2);
    assert_eq!(windows[0].id, "@1");
    assert_eq!(windows[0].session_name, "alpha");
    assert!(windows[0].active);
    assert_eq!(windows[1].id, "@3");
    assert_eq!(windows[1].session_name, "beta");
    assert!(!windows[1].active);
}

#[test]
fn tmux_provider_lists_sidebar_panes_with_window_widths() {
    let runner = RecordingRunner::new(HashMap::from([(
        "list-panes".to_string(),
        "%1\talpha\t@1\t0\t0\t1\t/dev/ttys1\t123\t/repo\tbash\tmain\t94\t24\t26\t119\n%2\talpha\t@1\t0\t1\t0\t/dev/ttys2\t124\t/repo\tzsh\topensessions-sidebar\t26\t24\t0\t25\n%3\t_os_stash\t@2\t0\t0\t1\t/dev/ttys3\t125\t/tmp\tzsh\topensessions-sidebar\t26\t24\t0\t25"
            .to_string(),
    )]));
    let provider = TmuxProvider::new(Arc::new(runner));

    let panes = provider.list_sidebar_panes(None);
    assert_eq!(panes.len(), 1);
    assert_eq!(panes[0].pane_id, "%2");
    assert_eq!(panes[0].session_name, "alpha");
    assert_eq!(panes[0].window_id, "@1");
    assert_eq!(panes[0].width, Some(26));
    assert_eq!(panes[0].window_width, Some(120));
}

#[test]
fn tmux_provider_returns_session_pane_pids() {
    let runner = RecordingRunner::new(HashMap::from([(
        "list-panes".to_string(),
        "%1\talpha\t@1\t0\t0\t1\t/dev/ttys1\t123\t/repo\tbash\tmain\t80\t24\t0\t79\n%2\talpha\t@1\t0\t1\t0\t/dev/ttys2\t124\t/repo\tzsh\topensessions-sidebar\t26\t24\t80\t105"
            .to_string(),
    )]));
    let provider = TmuxProvider::new(Arc::new(runner));

    assert_eq!(provider.get_session_pane_pids("alpha"), vec![123, 124]);
}

#[test]
fn tmux_provider_gets_current_window_id_from_display_message() {
    let runner = RecordingRunner::new(HashMap::from([(
        "display-message".to_string(),
        "@42".to_string(),
    )]));
    let provider = TmuxProvider::new(Arc::new(runner));

    assert_eq!(provider.get_current_window_id(), Some("@42".to_string()));
}

#[test]
fn tmux_provider_sends_sidebar_pane_commands_to_runner() {
    let runner = Arc::new(RecordingRunner::new(HashMap::new()));
    let provider = TmuxProvider::new(runner.clone());

    provider.hide_sidebar("%1");
    provider.kill_sidebar_pane("%2");
    provider.resize_sidebar_pane("%3", 34);
    provider.cleanup_sidebar();

    let calls = runner.calls.lock().unwrap().clone();
    assert!(
        calls
            .iter()
            .any(|call| call == &vec!["kill-pane", "-t", "%1"])
    );
    assert!(
        calls
            .iter()
            .any(|call| call == &vec!["kill-pane", "-t", "%2"])
    );
    assert!(
        calls
            .iter()
            .any(|call| call == &vec!["resize-pane", "-t", "%3", "-x", "34"])
    );
    assert!(
        calls
            .iter()
            .any(|call| call == &vec!["kill-session", "-t", "_os_stash"])
    );
}

#[test]
fn tmux_provider_resolves_focuses_and_kills_agent_panes() {
    let runner = Arc::new(RecordingRunner::new(HashMap::from([
        (
            "list-panes".to_string(),
            "%1\talpha\t@1\t0\t0\t1\t/dev/ttys1\t123\t/repo\tzsh\tamp - other thread\t80\t24\t0\t79\n%2\talpha\t@1\t0\t1\t0\t/dev/ttys2\t124\t/repo\tzsh\tamp - migrate server\t80\t24\t80\t159\n%3\talpha\t@1\t0\t2\t0\t/dev/ttys3\t125\t/repo\tzsh\topensessions-sidebar\t26\t24\t160\t185"
                .to_string(),
        ),
        ("display-message".to_string(), "@1".to_string()),
    ])));
    let provider = TmuxProvider::new(runner.clone());

    assert_eq!(
        provider.resolve_agent_pane_id("alpha", "amp", None, Some("migrate server")),
        Some("%2".to_string())
    );

    provider.focus_pane("%2");
    provider.kill_pane("%2");

    let calls = runner.calls.lock().unwrap().clone();
    assert!(
        calls
            .iter()
            .any(|call| call == &vec!["display-message", "-t", "%2", "-p", "#{window_id}"])
    );
    assert!(
        calls
            .iter()
            .any(|call| call == &vec!["select-window", "-t", "@1"])
    );
    assert!(
        calls
            .iter()
            .any(|call| call == &vec!["select-pane", "-t", "%2"])
    );
    assert!(
        calls
            .iter()
            .any(|call| call == &vec!["kill-pane", "-t", "%2"])
    );
}

#[test]
fn tmux_provider_kills_orphaned_and_duplicate_sidebar_panes() {
    let runner = Arc::new(RecordingRunner::new(HashMap::from([(
        "list-panes".to_string(),
        "%1\talpha\t@1\t0\t0\t1\t/dev/ttys1\t123\t/repo\tzsh\topensessions-sidebar\t26\t24\t0\t25\n%2\tbeta\t@2\t0\t0\t1\t/dev/ttys2\t124\t/repo\tbash\tmain\t94\t24\t26\t119\n%3\tbeta\t@2\t0\t1\t0\t/dev/ttys3\t125\t/repo\tzsh\topensessions-sidebar\t26\t24\t0\t25\n%4\tbeta\t@2\t0\t2\t0\t/dev/ttys4\t126\t/repo\tzsh\topensessions-sidebar\t26\t24\t0\t25\n%5\t_os_stash\t@3\t0\t0\t1\t/dev/ttys5\t127\t/tmp\tzsh\topensessions-sidebar\t26\t24\t0\t25"
            .to_string(),
    )])));
    let provider = TmuxProvider::new(runner.clone());

    provider.kill_orphaned_sidebar_panes();

    let calls = runner.calls.lock().unwrap().clone();
    assert!(
        calls
            .iter()
            .any(|call| call == &vec!["kill-pane", "-t", "%1"])
    );
    assert!(
        calls
            .iter()
            .any(|call| call == &vec!["kill-pane", "-t", "%4"])
    );
    assert!(
        !calls
            .iter()
            .any(|call| call == &vec!["kill-pane", "-t", "%3"]),
        "the first sidebar in a non-orphaned window should be kept"
    );
    assert!(
        !calls
            .iter()
            .any(|call| call == &vec!["kill-pane", "-t", "%5"]),
        "stash sidebars should be ignored"
    );
}

#[test]
fn tmux_provider_spawns_sidebar_against_edge_pane_and_titles_it() {
    let runner = Arc::new(RecordingRunner::new(HashMap::from([
        (
            "list-panes".to_string(),
            "%1\talpha\t@1\t0\t0\t1\t/dev/ttys1\t123\t/repo\tbash\tmain\t80\t24\t0\t79\n%2\talpha\t@1\t0\t1\t0\t/dev/ttys2\t124\t/repo\tzsh\tmain\t40\t24\t80\t119"
                .to_string(),
        ),
        (
            "split-window".to_string(),
            "%9\talpha\t@1\t0\t2\t0\t/dev/ttys9\t999\t/repo\tzsh\topensessions-sidebar\t26\t24\t0\t25"
                .to_string(),
        ),
    ])));
    let provider = TmuxProvider::new(runner.clone());

    let pane_id = provider.spawn_sidebar("alpha", "@1", 26, SidebarPosition::Left, "/scripts");

    assert_eq!(pane_id, Some("%9".to_string()));
    let calls = runner.calls.lock().unwrap().clone();
    let split_call = calls
        .iter()
        .find(|call| call.first().map(String::as_str) == Some("split-window"))
        .expect("split-window should be called");
    assert!(split_call.starts_with(&vec![
        "split-window".to_string(),
        "-hb".to_string(),
        "-f".to_string(),
        "-l".to_string(),
        "26".to_string(),
        "-t".to_string(),
        "%1".to_string(),
    ]));
    assert_eq!(
        split_call.last().map(String::as_str),
        Some(
            "OPENSESSIONS_SESSION_NAME=alpha OPENSESSIONS_WINDOW_ID=@1 REFOCUS_WINDOW=@1 exec \"${OPENSESSIONS_DIR:-.}\"//scripts/start.sh"
        )
    );
    assert!(
        calls
            .iter()
            .any(|call| call == &vec!["select-pane", "-t", "%9", "-T", "opensessions-sidebar"])
    );
}

#[test]
fn std_command_runner_executes_tmux_binary_and_captures_output() {
    let runner = StdCommandRunner::new("printf");
    let output = runner.run(&["hello".to_string()]);

    assert_eq!(output.exit_code, 0);
    assert_eq!(output.stdout, "hello");
    assert_eq!(output.stderr, "");
}

#[derive(Debug)]
struct RecordingRunner {
    outputs: HashMap<String, String>,
    calls: Mutex<Vec<Vec<String>>>,
}

impl RecordingRunner {
    fn new(outputs: HashMap<String, String>) -> Self {
        Self {
            outputs,
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl CommandRunner for RecordingRunner {
    fn run(&self, args: &[String]) -> CommandOutput {
        self.calls.lock().unwrap().push(args.to_vec());
        CommandOutput {
            exit_code: 0,
            stdout: self
                .outputs
                .get(args.first().map(String::as_str).unwrap_or_default())
                .cloned()
                .unwrap_or_default(),
            stderr: String::new(),
        }
    }
}
