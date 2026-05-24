use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use http::Uri;
use opensessions_runtime::mux::{
    ActiveWindow, AgentPane, MuxProvider, MuxSessionInfo, SidebarPane, SidebarPosition,
};
use opensessions_server::{
    GitCommandRunner, PortCommandRunner, ReadOnlyMuxStateSource, StateSource,
    default_state_source_from_env,
};
use opensessions_server::{ServerConfig, start_server};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_websockets::{ClientBuilder, Message};

const EXPECTED_HELLO: &str = r#"{"type":"hello","protocol":1,"serverVersion":"0.2.0-alpha.5"}"#;
const EXPECTED_QUIT: &str = r#"{"type":"quit"}"#;

fn test_pid_file(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "opensessions-server-rs-{name}-{}-{stamp}.pid",
        std::process::id()
    ))
}

#[tokio::test(flavor = "current_thread")]
async fn writes_pid_file_without_newline() {
    let pid_file = test_pid_file("pid");
    let server = start_server(ServerConfig::new("127.0.0.1", 0, &pid_file))
        .await
        .expect("server should start");

    assert_eq!(
        fs::read_to_string(&pid_file).expect("pid file should be written"),
        std::process::id().to_string()
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn post_quit_returns_ok_stops_server_and_removes_pid_file() {
    let pid_file = test_pid_file("quit");
    let server = start_server(ServerConfig::new("127.0.0.1", 0, &pid_file))
        .await
        .expect("server should start");
    let addr = server.addr();

    let mut stream = TcpStream::connect(addr)
        .await
        .expect("server should accept http clients");
    stream
        .write_all(b"POST /quit HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n")
        .await
        .expect("quit request should write");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("quit response should read");

    assert!(
        String::from_utf8_lossy(&response).starts_with("HTTP/1.1 200 OK\r\n"),
        "response was {}",
        String::from_utf8_lossy(&response)
    );
    assert!(
        String::from_utf8_lossy(&response).ends_with("\r\n\r\nok"),
        "response was {}",
        String::from_utf8_lossy(&response)
    );

    server
        .wait_shutdown()
        .await
        .expect("/quit should stop the server");
    assert!(!pid_file.exists(), "/quit should remove the pid file");
}

#[tokio::test(flavor = "current_thread")]
async fn websocket_upgrade_immediately_sends_exact_hello_json() {
    let pid_file = test_pid_file("ws");
    let server = start_server(ServerConfig::new("127.0.0.1", 0, &pid_file))
        .await
        .expect("server should start");
    let uri: Uri = format!("ws://{}", server.addr())
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut client, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket clients");
    let message = client
        .next()
        .await
        .expect("server should send hello")
        .expect("hello should be a valid websocket message");

    assert_eq!(message.as_text(), Some(EXPECTED_HELLO));

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn websocket_sends_state_snapshot_after_hello_when_available() {
    let pid_file = test_pid_file("state");
    let state_json = r#"{"type":"state","sessions":[],"focusedSession":null,"currentSession":null,"sidebarWidth":26,"initializing":false,"ts":123}"#;
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(move || state_json.to_string()),
    )
    .await
    .expect("server should start");
    let uri: Uri = format!("ws://{}", server.addr())
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut client, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket clients");
    let hello = client
        .next()
        .await
        .expect("server should send hello")
        .expect("hello should be a valid websocket message");
    let state = client
        .next()
        .await
        .expect("server should send state after hello")
        .expect("state should be a valid websocket message");

    assert_eq!(hello.as_text(), Some(EXPECTED_HELLO));
    assert_eq!(state.as_text(), Some(state_json));

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn post_refresh_returns_ok_and_broadcasts_fresh_state_snapshot() {
    let pid_file = test_pid_file("refresh");
    let counter = Arc::new(AtomicUsize::new(1));
    let state_counter = Arc::clone(&counter);
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file).with_state_source(move || {
            format!(
                r#"{{"type":"state","sessions":[],"focusedSession":null,"currentSession":null,"sidebarWidth":26,"initializing":false,"ts":{}}}"#,
                state_counter.load(Ordering::SeqCst)
            )
        }),
    )
    .await
    .expect("server should start");
    let addr = server.addr();
    let uri: Uri = format!("ws://{addr}")
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut client, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket clients");
    let _ = client.next().await.expect("hello should arrive");
    assert_eq!(
        client
            .next()
            .await
            .expect("initial state should arrive")
            .expect("initial state should be valid")
            .as_text(),
        Some(
            r#"{"type":"state","sessions":[],"focusedSession":null,"currentSession":null,"sidebarWidth":26,"initializing":false,"ts":1}"#
        )
    );

    counter.store(2, Ordering::SeqCst);
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("server should accept http clients");
    stream
        .write_all(b"POST /refresh HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n")
        .await
        .expect("refresh request should write");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("refresh response should read");
    assert!(
        String::from_utf8_lossy(&response).ends_with("\r\n\r\nok"),
        "response was {}",
        String::from_utf8_lossy(&response)
    );

    let refreshed = timeout(Duration::from_secs(1), client.next())
        .await
        .expect("refresh should broadcast state before timeout")
        .expect("refreshed state should arrive")
        .expect("refreshed state should be valid");
    assert_eq!(
        refreshed.as_text(),
        Some(
            r#"{"type":"state","sessions":[],"focusedSession":null,"currentSession":null,"sidebarWidth":26,"initializing":false,"ts":2}"#
        )
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn websocket_refresh_command_broadcasts_fresh_state_snapshot() {
    let pid_file = test_pid_file("ws-refresh");
    let counter = Arc::new(AtomicUsize::new(1));
    let state_counter = Arc::clone(&counter);
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file).with_state_source(move || {
            format!(
                r#"{{"type":"state","sessions":[],"focusedSession":null,"currentSession":null,"sidebarWidth":26,"initializing":false,"ts":{}}}"#,
                state_counter.load(Ordering::SeqCst)
            )
        }),
    )
    .await
    .expect("server should start");
    let uri: Uri = format!("ws://{}", server.addr())
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut sender, _) = ClientBuilder::from_uri(uri.clone())
        .connect()
        .await
        .expect("server should upgrade websocket sender");
    let (mut receiver, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket receiver");
    let _ = sender.next().await.expect("sender hello should arrive");
    let _ = sender
        .next()
        .await
        .expect("sender initial state should arrive");
    let _ = receiver.next().await.expect("receiver hello should arrive");
    let _ = receiver
        .next()
        .await
        .expect("receiver initial state should arrive");

    counter.store(3, Ordering::SeqCst);
    sender
        .send(Message::text(r#"{"type":"refresh"}"#))
        .await
        .expect("refresh command should send");

    let refreshed = timeout(Duration::from_secs(1), receiver.next())
        .await
        .expect("refresh broadcast should arrive before timeout")
        .expect("refresh broadcast should arrive")
        .expect("refresh broadcast should be valid");
    assert_eq!(
        refreshed.as_text(),
        Some(
            r#"{"type":"state","sessions":[],"focusedSession":null,"currentSession":null,"sidebarWidth":26,"initializing":false,"ts":3}"#
        )
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn websocket_quit_command_broadcasts_quit_and_stops_server() {
    let pid_file = test_pid_file("ws-quit");
    let server = start_server(ServerConfig::new("127.0.0.1", 0, &pid_file))
        .await
        .expect("server should start");
    let uri: Uri = format!("ws://{}", server.addr())
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut sender, _) = ClientBuilder::from_uri(uri.clone())
        .connect()
        .await
        .expect("server should upgrade websocket sender");
    let (mut receiver, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket receiver");
    let _ = sender.next().await.expect("sender hello should arrive");
    let _ = receiver.next().await.expect("receiver hello should arrive");

    sender
        .send(Message::text(r#"{"type":"quit"}"#))
        .await
        .expect("quit command should send");

    let quit = timeout(Duration::from_secs(1), receiver.next())
        .await
        .expect("quit broadcast should arrive before timeout")
        .expect("quit broadcast should arrive")
        .expect("quit broadcast should be valid");
    assert_eq!(quit.as_text(), Some(EXPECTED_QUIT));

    server
        .wait_shutdown()
        .await
        .expect("websocket quit should stop the server");
    assert!(!pid_file.exists(), "quit should remove the pid file");
}

#[test]
fn read_only_mux_state_source_serializes_runtime_state() {
    let source = ReadOnlyMuxStateSource::new(vec![Arc::new(ServerMux {
        current: Some("api".to_string()),
        sessions: vec![MuxSessionInfo {
            name: "api".to_string(),
            created_at: 60,
            dir: "/repo/api".to_string(),
            windows: 2,
        }],
        panes: 5,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    })])
    .with_sidebar_width(33)
    .with_now_ms(|| 120_000);

    assert_eq!(
        source.snapshot_json(),
        r#"{"type":"state","sessions":[{"name":"api","createdAt":60,"dir":"/repo/api","branch":"","dirty":false,"isWorktree":false,"unseen":false,"panes":5,"ports":[],"localLinks":[],"windows":2,"uptime":"1m","agentState":null,"agents":[],"eventTimestamps":[]}],"focusedSession":"api","currentSession":"api","sidebarWidth":33,"initializing":false,"ts":120000}"#,
    );
}

#[test]
fn default_state_source_uses_tmux_provider_when_tmux_env_is_present() {
    assert!(
        default_state_source_from_env(|key| (key == "TMUX").then(|| "socket,1,0".to_string()))
            .is_some()
    );
    assert!(default_state_source_from_env(|_| None).is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn websocket_new_session_command_calls_mux_and_broadcasts_state() {
    let pid_file = test_pid_file("ws-new-session");
    let mux = Arc::new(ServerMux {
        current: None,
        sessions: vec![],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux.clone()]).with_now_ms(|| 456)),
    )
    .await
    .expect("server should start");
    let uri: Uri = format!("ws://{}", server.addr())
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut sender, _) = ClientBuilder::from_uri(uri.clone())
        .connect()
        .await
        .expect("server should upgrade websocket sender");
    let (mut receiver, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket receiver");
    let _ = sender.next().await.expect("sender hello should arrive");
    let _ = sender
        .next()
        .await
        .expect("sender initial state should arrive");
    let _ = receiver.next().await.expect("receiver hello should arrive");
    let _ = receiver
        .next()
        .await
        .expect("receiver initial state should arrive");

    sender
        .send(Message::text(r#"{"type":"new-session"}"#))
        .await
        .expect("new-session command should send");

    let state = timeout(Duration::from_secs(1), receiver.next())
        .await
        .expect("state broadcast should arrive before timeout")
        .expect("state broadcast should arrive")
        .expect("state broadcast should be valid");
    assert_eq!(*mux.create_calls.lock().unwrap(), 1);
    assert_eq!(
        state.as_text(),
        Some(
            r#"{"type":"state","sessions":[],"focusedSession":null,"currentSession":null,"sidebarWidth":26,"initializing":false,"ts":456}"#
        )
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn websocket_switch_session_calls_mux_and_broadcasts_focus_update() {
    let pid_file = test_pid_file("ws-switch-session");
    let mux = Arc::new(ServerMux {
        current: Some("api".to_string()),
        sessions: vec![MuxSessionInfo {
            name: "api".to_string(),
            created_at: 1,
            dir: "/repo/api".to_string(),
            windows: 1,
        }],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux.clone()]).with_now_ms(|| 456)),
    )
    .await
    .expect("server should start");
    let uri: Uri = format!("ws://{}", server.addr())
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut sender, _) = ClientBuilder::from_uri(uri.clone())
        .connect()
        .await
        .expect("server should upgrade websocket sender");
    let (mut receiver, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket receiver");
    let _ = sender.next().await.expect("sender hello should arrive");
    let _ = sender
        .next()
        .await
        .expect("sender initial state should arrive");
    let _ = receiver.next().await.expect("receiver hello should arrive");
    let _ = receiver
        .next()
        .await
        .expect("receiver initial state should arrive");

    sender
        .send(Message::text(
            r#"{"type":"switch-session","name":"api","clientTty":"/dev/ttys001"}"#,
        ))
        .await
        .expect("switch-session command should send");

    let focus = timeout(Duration::from_secs(1), receiver.next())
        .await
        .expect("focus broadcast should arrive before timeout")
        .expect("focus broadcast should arrive")
        .expect("focus broadcast should be valid");
    assert_eq!(
        *mux.switch_calls.lock().unwrap(),
        vec![("api".to_string(), Some("/dev/ttys001".to_string()))]
    );
    assert_eq!(
        focus.as_text(),
        Some(r#"{"type":"focus","focusedSession":"api","currentSession":"api"}"#)
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn websocket_switch_index_switches_to_visible_session() {
    let pid_file = test_pid_file("ws-switch-index");
    let mux = Arc::new(ServerMux {
        current: Some("api".to_string()),
        sessions: vec![
            MuxSessionInfo {
                name: "api".to_string(),
                created_at: 1,
                dir: "/repo/api".to_string(),
                windows: 1,
            },
            MuxSessionInfo {
                name: "worker".to_string(),
                created_at: 2,
                dir: "/repo/worker".to_string(),
                windows: 1,
            },
        ],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux.clone()]).with_now_ms(|| 789)),
    )
    .await
    .expect("server should start");
    let uri: Uri = format!("ws://{}", server.addr())
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut sender, _) = ClientBuilder::from_uri(uri.clone())
        .connect()
        .await
        .expect("server should upgrade websocket sender");
    let (mut receiver, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket receiver");
    let _ = sender.next().await.expect("sender hello should arrive");
    let _ = sender
        .next()
        .await
        .expect("sender initial state should arrive");
    let _ = receiver.next().await.expect("receiver hello should arrive");
    let _ = receiver
        .next()
        .await
        .expect("receiver initial state should arrive");

    sender
        .send(Message::text(r#"{"type":"switch-index","index":2}"#))
        .await
        .expect("switch-index command should send");
    sender
        .send(Message::text(r#"{"type":"refresh"}"#))
        .await
        .expect("refresh command should send");

    let _ = timeout(Duration::from_secs(1), receiver.next())
        .await
        .expect("refresh state should arrive before timeout")
        .expect("refresh state should arrive")
        .expect("refresh state should be valid");
    assert_eq!(
        *mux.switch_calls.lock().unwrap(),
        vec![("worker".to_string(), None)]
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn websocket_focus_session_broadcasts_focus_without_switching_mux() {
    let pid_file = test_pid_file("ws-focus-session");
    let mux = Arc::new(ServerMux {
        current: Some("api".to_string()),
        sessions: vec![MuxSessionInfo {
            name: "api".to_string(),
            created_at: 1,
            dir: "/repo/api".to_string(),
            windows: 1,
        }],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux.clone()]).with_now_ms(|| 456)),
    )
    .await
    .expect("server should start");
    let uri: Uri = format!("ws://{}", server.addr())
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut sender, _) = ClientBuilder::from_uri(uri.clone())
        .connect()
        .await
        .expect("server should upgrade websocket sender");
    let (mut receiver, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket receiver");
    let _ = sender.next().await.expect("sender hello should arrive");
    let _ = sender
        .next()
        .await
        .expect("sender initial state should arrive");
    let _ = receiver.next().await.expect("receiver hello should arrive");
    let _ = receiver
        .next()
        .await
        .expect("receiver initial state should arrive");

    sender
        .send(Message::text(r#"{"type":"focus-session","name":"api"}"#))
        .await
        .expect("focus-session command should send");

    let focus = timeout(Duration::from_secs(1), receiver.next())
        .await
        .expect("focus broadcast should arrive before timeout")
        .expect("focus broadcast should arrive")
        .expect("focus broadcast should be valid");
    assert!(mux.switch_calls.lock().unwrap().is_empty());
    assert_eq!(
        focus.as_text(),
        Some(r#"{"type":"focus","focusedSession":"api","currentSession":"api"}"#)
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn websocket_move_focus_moves_within_sorted_sessions_and_broadcasts_focus() {
    let pid_file = test_pid_file("ws-move-focus");
    let mux = Arc::new(ServerMux {
        current: Some("alpha".to_string()),
        sessions: vec![
            MuxSessionInfo {
                name: "charlie".to_string(),
                created_at: 30,
                dir: "/repo/charlie".to_string(),
                windows: 1,
            },
            MuxSessionInfo {
                name: "alpha".to_string(),
                created_at: 10,
                dir: "/repo/alpha".to_string(),
                windows: 1,
            },
            MuxSessionInfo {
                name: "bravo".to_string(),
                created_at: 20,
                dir: "/repo/bravo".to_string(),
                windows: 1,
            },
        ],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux]).with_now_ms(|| 456)),
    )
    .await
    .expect("server should start");
    let uri: Uri = format!("ws://{}", server.addr())
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut sender, _) = ClientBuilder::from_uri(uri.clone())
        .connect()
        .await
        .expect("server should upgrade websocket sender");
    let (mut receiver, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket receiver");
    let _ = sender.next().await.expect("sender hello should arrive");
    let _ = sender
        .next()
        .await
        .expect("sender initial state should arrive");
    let _ = receiver.next().await.expect("receiver hello should arrive");
    let _ = receiver
        .next()
        .await
        .expect("receiver initial state should arrive");

    sender
        .send(Message::text(r#"{"type":"move-focus","delta":1}"#))
        .await
        .expect("move-focus command should send");

    let focus = timeout(Duration::from_secs(1), receiver.next())
        .await
        .expect("focus broadcast should arrive before timeout")
        .expect("focus broadcast should arrive")
        .expect("focus broadcast should be valid");
    assert_eq!(
        focus.as_text(),
        Some(r#"{"type":"focus","focusedSession":"bravo","currentSession":"alpha"}"#)
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn websocket_kill_session_command_calls_mux_and_broadcasts_state() {
    let pid_file = test_pid_file("ws-kill-session");
    let mux = Arc::new(ServerMux {
        current: Some("api".to_string()),
        sessions: vec![MuxSessionInfo {
            name: "api".to_string(),
            created_at: 1,
            dir: "/repo/api".to_string(),
            windows: 1,
        }],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux.clone()]).with_now_ms(|| 789)),
    )
    .await
    .expect("server should start");
    let uri: Uri = format!("ws://{}", server.addr())
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut sender, _) = ClientBuilder::from_uri(uri.clone())
        .connect()
        .await
        .expect("server should upgrade websocket sender");
    let (mut receiver, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket receiver");
    let _ = sender.next().await.expect("sender hello should arrive");
    let _ = sender
        .next()
        .await
        .expect("sender initial state should arrive");
    let _ = receiver.next().await.expect("receiver hello should arrive");
    let _ = receiver
        .next()
        .await
        .expect("receiver initial state should arrive");

    sender
        .send(Message::text(r#"{"type":"kill-session","name":"api"}"#))
        .await
        .expect("kill-session command should send");

    let state = timeout(Duration::from_secs(1), receiver.next())
        .await
        .expect("state broadcast should arrive before timeout")
        .expect("state broadcast should arrive")
        .expect("state broadcast should be valid");
    assert_eq!(*mux.kill_calls.lock().unwrap(), vec!["api".to_string()]);
    assert_eq!(
        state.as_text(),
        Some(
            r#"{"type":"state","sessions":[{"name":"api","createdAt":1,"dir":"/repo/api","branch":"","dirty":false,"isWorktree":false,"unseen":false,"panes":1,"ports":[],"localLinks":[],"windows":1,"uptime":"","agentState":null,"agents":[],"eventTimestamps":[]}],"focusedSession":"api","currentSession":"api","sidebarWidth":26,"initializing":false,"ts":789}"#
        )
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn websocket_set_theme_updates_state_and_broadcasts() {
    let pid_file = test_pid_file("ws-set-theme");
    let mux = Arc::new(ServerMux {
        current: None,
        sessions: vec![],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux]).with_now_ms(|| 999)),
    )
    .await
    .expect("server should start");
    let uri: Uri = format!("ws://{}", server.addr())
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut sender, _) = ClientBuilder::from_uri(uri.clone())
        .connect()
        .await
        .expect("server should upgrade websocket sender");
    let (mut receiver, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket receiver");
    let _ = sender.next().await.expect("sender hello should arrive");
    let _ = sender
        .next()
        .await
        .expect("sender initial state should arrive");
    let _ = receiver.next().await.expect("receiver hello should arrive");
    let _ = receiver
        .next()
        .await
        .expect("receiver initial state should arrive");

    sender
        .send(Message::text(
            r#"{"type":"set-theme","theme":"catppuccin-mocha"}"#,
        ))
        .await
        .expect("set-theme command should send");

    let state = timeout(Duration::from_secs(1), receiver.next())
        .await
        .expect("state broadcast should arrive before timeout")
        .expect("state broadcast should arrive")
        .expect("state broadcast should be valid");
    assert_eq!(
        state.as_text(),
        Some(
            r#"{"type":"state","sessions":[],"focusedSession":null,"currentSession":null,"theme":"catppuccin-mocha","sidebarWidth":26,"initializing":false,"ts":999}"#
        )
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn websocket_set_filter_updates_state_and_broadcasts() {
    let pid_file = test_pid_file("ws-set-filter");
    let mux = Arc::new(ServerMux {
        current: None,
        sessions: vec![],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux]).with_now_ms(|| 1_001)),
    )
    .await
    .expect("server should start");
    let uri: Uri = format!("ws://{}", server.addr())
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut sender, _) = ClientBuilder::from_uri(uri.clone())
        .connect()
        .await
        .expect("server should upgrade websocket sender");
    let (mut receiver, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket receiver");
    let _ = sender.next().await.expect("sender hello should arrive");
    let _ = sender
        .next()
        .await
        .expect("sender initial state should arrive");
    let _ = receiver.next().await.expect("receiver hello should arrive");
    let _ = receiver
        .next()
        .await
        .expect("receiver initial state should arrive");

    sender
        .send(Message::text(r#"{"type":"set-filter","filter":"running"}"#))
        .await
        .expect("set-filter command should send");

    let state = timeout(Duration::from_secs(1), receiver.next())
        .await
        .expect("state broadcast should arrive before timeout")
        .expect("state broadcast should arrive")
        .expect("state broadcast should be valid");
    assert_eq!(
        state.as_text(),
        Some(
            r#"{"type":"state","sessions":[],"focusedSession":null,"currentSession":null,"sessionFilter":"running","sidebarWidth":26,"initializing":false,"ts":1001}"#
        )
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn websocket_report_width_updates_sidebar_width_and_broadcasts() {
    let pid_file = test_pid_file("ws-report-width");
    let mux = Arc::new(HookMux {
        sidebar_panes: Vec::new(),
        active_windows: vec![ActiveWindow {
            id: "@1".to_string(),
            session_name: "alpha".to_string(),
            active: true,
        }],
        spawn_calls: Mutex::new(Vec::new()),
        hide_calls: Mutex::new(Vec::new()),
        orphan_cleanup_calls: Mutex::new(0),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux]).with_now_ms(|| 1_002)),
    )
    .await
    .expect("server should start");
    let uri: Uri = format!("ws://{}", server.addr())
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut sender, _) = ClientBuilder::from_uri(uri.clone())
        .connect()
        .await
        .expect("server should upgrade websocket sender");
    let (mut receiver, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket receiver");
    let _ = sender.next().await.expect("sender hello should arrive");
    let _ = sender
        .next()
        .await
        .expect("sender initial state should arrive");
    let _ = receiver.next().await.expect("receiver hello should arrive");
    let _ = receiver
        .next()
        .await
        .expect("receiver initial state should arrive");

    sender
        .send(Message::text(
            r#"{"type":"identify-pane","paneId":"%1","sessionName":"alpha","windowId":"@1"}"#,
        ))
        .await
        .expect("identify-pane command should send");
    let _ = sender
        .next()
        .await
        .expect("your-session should arrive for sender");

    sender
        .send(Message::text(r#"{"type":"report-width","width":41}"#))
        .await
        .expect("report-width command should send");

    let state = timeout(Duration::from_secs(1), receiver.next())
        .await
        .expect("state broadcast should arrive before timeout")
        .expect("state broadcast should arrive")
        .expect("state broadcast should be valid");
    assert_eq!(
        state.as_text(),
        Some(
            r#"{"type":"state","sessions":[],"focusedSession":null,"currentSession":"alpha","sidebarWidth":41,"initializing":true,"initLabel":"adjusting…","ts":1002}"#
        )
    );

    sender
        .send(Message::text(r#"{"type":"report-width","width":3}"#))
        .await
        .expect("second report-width command should send");

    let state = timeout(Duration::from_secs(1), receiver.next())
        .await
        .expect("clamped state broadcast should arrive before timeout")
        .expect("clamped state broadcast should arrive")
        .expect("clamped state broadcast should be valid");
    assert_eq!(
        state.as_text(),
        Some(
            r#"{"type":"state","sessions":[],"focusedSession":null,"currentSession":"alpha","sidebarWidth":20,"initializing":true,"initLabel":"adjusting…","ts":1002}"#
        )
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn websocket_report_width_rejects_identified_background_sidebar() {
    let pid_file = test_pid_file("ws-report-width-background");
    let mux = Arc::new(HookMux {
        sidebar_panes: Vec::new(),
        active_windows: vec![ActiveWindow {
            id: "@1".to_string(),
            session_name: "alpha".to_string(),
            active: true,
        }],
        spawn_calls: Mutex::new(Vec::new()),
        hide_calls: Mutex::new(Vec::new()),
        orphan_cleanup_calls: Mutex::new(0),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux]).with_now_ms(|| 1_003)),
    )
    .await
    .expect("server should start");
    let uri: Uri = format!("ws://{}", server.addr())
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut sender, _) = ClientBuilder::from_uri(uri.clone())
        .connect()
        .await
        .expect("server should upgrade websocket sender");
    let (mut receiver, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket receiver");
    let _ = sender.next().await.expect("sender hello should arrive");
    let _ = sender
        .next()
        .await
        .expect("sender initial state should arrive");
    let _ = receiver.next().await.expect("receiver hello should arrive");
    let _ = receiver
        .next()
        .await
        .expect("receiver initial state should arrive");

    sender
        .send(Message::text(
            r#"{"type":"identify-pane","paneId":"%2","sessionName":"beta","windowId":"@2"}"#,
        ))
        .await
        .expect("identify-pane command should send");
    let _ = sender
        .next()
        .await
        .expect("your-session should arrive for sender");

    sender
        .send(Message::text(r#"{"type":"report-width","width":41}"#))
        .await
        .expect("report-width command should send");

    assert!(
        timeout(Duration::from_millis(50), receiver.next())
            .await
            .is_err(),
        "background sidebar width reports must not broadcast state"
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn websocket_hide_and_show_all_sessions_update_visible_state() {
    let pid_file = test_pid_file("ws-hide-show");
    let mux = Arc::new(ServerMux {
        current: Some("alpha".to_string()),
        sessions: vec![
            MuxSessionInfo {
                name: "alpha".to_string(),
                created_at: 1,
                dir: "/repo/alpha".to_string(),
                windows: 1,
            },
            MuxSessionInfo {
                name: "beta".to_string(),
                created_at: 2,
                dir: "/repo/beta".to_string(),
                windows: 1,
            },
        ],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux]).with_now_ms(|| 3_000)),
    )
    .await
    .expect("server should start");
    let uri: Uri = format!("ws://{}", server.addr())
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut sender, _) = ClientBuilder::from_uri(uri.clone())
        .connect()
        .await
        .expect("server should upgrade websocket sender");
    let (mut receiver, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket receiver");
    let _ = sender.next().await.expect("sender hello should arrive");
    let _ = sender
        .next()
        .await
        .expect("sender initial state should arrive");
    let _ = receiver.next().await.expect("receiver hello should arrive");
    let _ = receiver
        .next()
        .await
        .expect("receiver initial state should arrive");

    sender
        .send(Message::text(r#"{"type":"hide-session","name":"beta"}"#))
        .await
        .expect("hide-session command should send");
    let hidden = timeout(Duration::from_secs(1), receiver.next())
        .await
        .expect("hidden state should arrive before timeout")
        .expect("hidden state should arrive")
        .expect("hidden state should be valid");
    assert_eq!(
        session_names(hidden.as_text().unwrap()),
        vec!["alpha".to_string()]
    );

    sender
        .send(Message::text(r#"{"type":"show-all-sessions"}"#))
        .await
        .expect("show-all-sessions command should send");
    let shown = timeout(Duration::from_secs(1), receiver.next())
        .await
        .expect("shown state should arrive before timeout")
        .expect("shown state should arrive")
        .expect("shown state should be valid");
    assert_eq!(
        session_names(shown.as_text().unwrap()),
        vec!["alpha".to_string(), "beta".to_string()]
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn websocket_reorder_session_updates_visible_order() {
    let pid_file = test_pid_file("ws-reorder");
    let mux = Arc::new(ServerMux {
        current: Some("alpha".to_string()),
        sessions: vec![
            MuxSessionInfo {
                name: "alpha".to_string(),
                created_at: 1,
                dir: "/repo/alpha".to_string(),
                windows: 1,
            },
            MuxSessionInfo {
                name: "beta".to_string(),
                created_at: 2,
                dir: "/repo/beta".to_string(),
                windows: 1,
            },
        ],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux]).with_now_ms(|| 3_000)),
    )
    .await
    .expect("server should start");
    let uri: Uri = format!("ws://{}", server.addr())
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut sender, _) = ClientBuilder::from_uri(uri.clone())
        .connect()
        .await
        .expect("server should upgrade websocket sender");
    let (mut receiver, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket receiver");
    let _ = sender.next().await.expect("sender hello should arrive");
    let _ = sender
        .next()
        .await
        .expect("sender initial state should arrive");
    let _ = receiver.next().await.expect("receiver hello should arrive");
    let _ = receiver
        .next()
        .await
        .expect("receiver initial state should arrive");

    sender
        .send(Message::text(
            r#"{"type":"reorder-session","name":"beta","delta":-1}"#,
        ))
        .await
        .expect("reorder-session command should send");
    let reordered = timeout(Duration::from_secs(1), receiver.next())
        .await
        .expect("reordered state should arrive before timeout")
        .expect("reordered state should arrive")
        .expect("reordered state should be valid");
    assert_eq!(
        session_names(reordered.as_text().unwrap()),
        vec!["beta".to_string(), "alpha".to_string()]
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn websocket_identify_pane_replies_with_your_session_to_sender_only() {
    let pid_file = test_pid_file("ws-identify-pane");
    let mux = Arc::new(ServerMux {
        current: Some("alpha".to_string()),
        sessions: vec![],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux]).with_now_ms(|| 3_000)),
    )
    .await
    .expect("server should start");
    let uri: Uri = format!("ws://{}", server.addr())
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut sender, _) = ClientBuilder::from_uri(uri.clone())
        .connect()
        .await
        .expect("server should upgrade websocket sender");
    let (mut receiver, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket receiver");
    let _ = sender.next().await.expect("sender hello should arrive");
    let _ = sender
        .next()
        .await
        .expect("sender initial state should arrive");
    let _ = receiver.next().await.expect("receiver hello should arrive");
    let _ = receiver
        .next()
        .await
        .expect("receiver initial state should arrive");

    sender
        .send(Message::text(
            r#"{"type":"identify-pane","paneId":"%1","sessionName":"alpha","windowId":"@1"}"#,
        ))
        .await
        .expect("identify-pane command should send");

    let reply = timeout(Duration::from_secs(1), sender.next())
        .await
        .expect("your-session should arrive before timeout")
        .expect("your-session should arrive")
        .expect("your-session should be valid");
    assert_eq!(
        reply.as_text(),
        Some(r#"{"type":"your-session","name":"alpha","clientTty":"/dev/ttys-test"}"#)
    );
    assert!(
        timeout(Duration::from_millis(50), receiver.next())
            .await
            .is_err(),
        "identify-pane should not broadcast to other clients"
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn websocket_identify_pane_ignores_stash_session() {
    let pid_file = test_pid_file("ws-identify-pane-stash");
    let mux = Arc::new(ServerMux {
        current: None,
        sessions: vec![],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux]).with_now_ms(|| 3_000)),
    )
    .await
    .expect("server should start");
    let uri: Uri = format!("ws://{}", server.addr())
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut client, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket client");
    let _ = client.next().await.expect("hello should arrive");
    let _ = client.next().await.expect("initial state should arrive");

    client
        .send(Message::text(
            r#"{"type":"identify-pane","paneId":"%1","sessionName":"_os_stash","windowId":"@1"}"#,
        ))
        .await
        .expect("identify-pane command should send");
    assert!(
        timeout(Duration::from_millis(50), client.next())
            .await
            .is_err(),
        "stash identify-pane should not reply"
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn http_set_status_returns_204_and_broadcasts_metadata_state() {
    let pid_file = test_pid_file("http-set-status");
    let mux = Arc::new(ServerMux {
        current: Some("api".to_string()),
        sessions: vec![MuxSessionInfo {
            name: "api".to_string(),
            created_at: 1,
            dir: "/repo/api".to_string(),
            windows: 1,
        }],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux]).with_now_ms(|| 4_000)),
    )
    .await
    .expect("server should start");
    let addr = server.addr();
    let uri: Uri = format!("ws://{addr}")
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut client, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket client");
    let _ = client.next().await.expect("hello should arrive");
    let _ = client.next().await.expect("initial state should arrive");

    let body = r#"{"session":"api","text":"working","tone":"info"}"#;
    let response = post_json(addr, "/set-status", body).await;
    assert!(
        response.starts_with("HTTP/1.1 204 No Content\r\n"),
        "response was {response}"
    );

    let state = timeout(Duration::from_secs(1), client.next())
        .await
        .expect("metadata state should arrive before timeout")
        .expect("metadata state should arrive")
        .expect("metadata state should be valid");
    let parsed = serde_json::from_str::<serde_json::Value>(state.as_text().unwrap()).unwrap();
    let metadata = &parsed["sessions"][0]["metadata"];
    assert_eq!(metadata["status"]["text"], "working");
    assert_eq!(metadata["status"]["tone"], "info");
    assert!(metadata["status"]["ts"].as_u64().unwrap() > 0);

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn http_metadata_progress_log_and_clear_log_broadcast_state() {
    let pid_file = test_pid_file("http-metadata-more");
    let mux = Arc::new(ServerMux {
        current: Some("api".to_string()),
        sessions: vec![MuxSessionInfo {
            name: "api".to_string(),
            created_at: 1,
            dir: "/repo/api".to_string(),
            windows: 1,
        }],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux]).with_now_ms(|| 4_000)),
    )
    .await
    .expect("server should start");
    let addr = server.addr();
    let uri: Uri = format!("ws://{addr}")
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut client, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket client");
    let _ = client.next().await.expect("hello should arrive");
    let _ = client.next().await.expect("initial state should arrive");

    let response = post_json(
        addr,
        "/set-progress",
        r#"{"session":"api","current":2,"total":5,"percent":0.4,"label":"files"}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 204 No Content\r\n"));
    let state = timeout(Duration::from_secs(1), client.next())
        .await
        .expect("progress state should arrive before timeout")
        .expect("progress state should arrive")
        .expect("progress state should be valid");
    let parsed = serde_json::from_str::<serde_json::Value>(state.as_text().unwrap()).unwrap();
    assert_eq!(parsed["sessions"][0]["metadata"]["progress"]["current"], 2);
    assert_eq!(parsed["sessions"][0]["metadata"]["progress"]["total"], 5);
    assert_eq!(
        parsed["sessions"][0]["metadata"]["progress"]["percent"],
        0.4
    );
    assert_eq!(
        parsed["sessions"][0]["metadata"]["progress"]["label"],
        "files"
    );

    let response = post_json(
        addr,
        "/log",
        r#"{"session":"api","message":"built","tone":"success","source":"ci"}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 204 No Content\r\n"));
    let state = timeout(Duration::from_secs(1), client.next())
        .await
        .expect("log state should arrive before timeout")
        .expect("log state should arrive")
        .expect("log state should be valid");
    let parsed = serde_json::from_str::<serde_json::Value>(state.as_text().unwrap()).unwrap();
    assert_eq!(
        parsed["sessions"][0]["metadata"]["logs"][0]["message"],
        "built"
    );
    assert_eq!(
        parsed["sessions"][0]["metadata"]["logs"][0]["tone"],
        "success"
    );
    assert_eq!(parsed["sessions"][0]["metadata"]["logs"][0]["source"], "ci");

    let response = post_json(addr, "/clear-log", r#"{"session":"api"}"#).await;
    assert!(response.starts_with("HTTP/1.1 204 No Content\r\n"));
    let state = timeout(Duration::from_secs(1), client.next())
        .await
        .expect("clear-log state should arrive before timeout")
        .expect("clear-log state should arrive")
        .expect("clear-log state should be valid");
    let parsed = serde_json::from_str::<serde_json::Value>(state.as_text().unwrap()).unwrap();
    assert_eq!(
        parsed["sessions"][0]["metadata"]["logs"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn http_metadata_endpoints_reject_missing_session() {
    let pid_file = test_pid_file("http-metadata-invalid");
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file).with_state_source(
            ReadOnlyMuxStateSource::new(vec![Arc::new(ServerMux {
                current: None,
                sessions: vec![],
                panes: 1,
                create_calls: Mutex::new(0),
                switch_calls: Mutex::new(Vec::new()),
                kill_calls: Mutex::new(Vec::new()),
            })])
            .with_now_ms(|| 4_000),
        ),
    )
    .await
    .expect("server should start");

    let response = post_json(server.addr(), "/set-status", r#"{"text":"working"}"#).await;
    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request\r\n"),
        "response was {response}"
    );
    assert!(response.ends_with("\r\n\r\nmissing session"));

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn http_focus_context_returns_ok_and_broadcasts_focus_update() {
    let pid_file = test_pid_file("http-focus");
    let mux = Arc::new(ServerMux {
        current: Some("api".to_string()),
        sessions: vec![
            MuxSessionInfo {
                name: "api".to_string(),
                created_at: 1,
                dir: "/repo/api".to_string(),
                windows: 1,
            },
            MuxSessionInfo {
                name: "worker".to_string(),
                created_at: 2,
                dir: "/repo/worker".to_string(),
                windows: 1,
            },
        ],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux]).with_now_ms(|| 5_000)),
    )
    .await
    .expect("server should start");
    let addr = server.addr();
    let uri: Uri = format!("ws://{addr}")
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut client, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket client");
    let _ = client.next().await.expect("hello should arrive");
    let _ = client.next().await.expect("initial state should arrive");

    let response = post_text(addr, "/focus", "/dev/ttys-test|worker|@2").await;
    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n"),
        "response was {response}"
    );
    assert!(response.ends_with("\r\n\r\nok"));

    let focus = timeout(Duration::from_secs(1), client.next())
        .await
        .expect("focus broadcast should arrive before timeout")
        .expect("focus broadcast should arrive")
        .expect("focus broadcast should be valid");
    assert_eq!(
        focus.as_text(),
        Some(r#"{"type":"focus","focusedSession":"worker","currentSession":"api"}"#)
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn http_switch_index_switches_to_visible_session_with_context_tty() {
    let pid_file = test_pid_file("http-switch-index");
    let mux = Arc::new(ServerMux {
        current: Some("api".to_string()),
        sessions: vec![
            MuxSessionInfo {
                name: "api".to_string(),
                created_at: 1,
                dir: "/repo/api".to_string(),
                windows: 1,
            },
            MuxSessionInfo {
                name: "worker".to_string(),
                created_at: 2,
                dir: "/repo/worker".to_string(),
                windows: 1,
            },
        ],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file).with_state_source(
            ReadOnlyMuxStateSource::new(vec![mux.clone()]).with_now_ms(|| 5_000),
        ),
    )
    .await
    .expect("server should start");

    let response = post_text(
        server.addr(),
        "/switch-index?index=2",
        "/dev/ttys-test|api|@1",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n"),
        "response was {response}"
    );
    assert!(response.ends_with("\r\n\r\nok"));
    assert_eq!(
        *mux.switch_calls.lock().unwrap(),
        vec![("worker".to_string(), Some("/dev/ttys-test".to_string()))]
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn http_resize_hooks_return_ok_for_tmux_hook_compatibility() {
    let pid_file = test_pid_file("http-resize-hooks");
    let server = start_server(ServerConfig::new("127.0.0.1", 0, &pid_file))
        .await
        .expect("server should start");

    for path in ["/suppress-width-reports?ms=2500", "/client-resized"] {
        let response = post_text(server.addr(), path, "").await;
        assert!(
            response.starts_with("HTTP/1.1 200 OK\r\n"),
            "{path} response was {response}"
        );
        assert!(
            response.ends_with("\r\n\r\nok"),
            "{path} response was {response}"
        );
    }

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn http_pane_exited_returns_ok_and_kills_orphaned_sidebar_panes() {
    let pid_file = test_pid_file("http-pane-exited");
    let mux = Arc::new(HookMux {
        sidebar_panes: Vec::new(),
        active_windows: Vec::new(),
        spawn_calls: Mutex::new(Vec::new()),
        hide_calls: Mutex::new(Vec::new()),
        orphan_cleanup_calls: Mutex::new(0),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux.clone()])),
    )
    .await
    .expect("server should start");

    let response = post_text(server.addr(), "/pane-exited", "").await;
    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n"),
        "response was {response}"
    );
    assert!(response.ends_with("\r\n\r\nok"));
    assert_eq!(*mux.orphan_cleanup_calls.lock().unwrap(), 1);

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn http_ensure_sidebar_spawns_missing_sidebar_in_context_window() {
    let pid_file = test_pid_file("http-ensure-sidebar");
    let mux = Arc::new(HookMux {
        sidebar_panes: Vec::new(),
        active_windows: vec![ActiveWindow {
            id: "@2".to_string(),
            session_name: "worker".to_string(),
            active: true,
        }],
        spawn_calls: Mutex::new(Vec::new()),
        hide_calls: Mutex::new(Vec::new()),
        orphan_cleanup_calls: Mutex::new(0),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file).with_state_source(
            ReadOnlyMuxStateSource::new(vec![mux.clone()]).with_sidebar_width(33),
        ),
    )
    .await
    .expect("server should start");

    let response = post_text(server.addr(), "/ensure-sidebar", "/dev/ttys-test|worker|@2").await;
    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n"),
        "response was {response}"
    );
    assert!(response.ends_with("\r\n\r\nok"));
    assert_eq!(
        *mux.spawn_calls.lock().unwrap(),
        vec![EnsureSpawnCall {
            session_name: "worker".to_string(),
            window_id: "@2".to_string(),
            width: 33,
            position: SidebarPosition::Left,
        }]
    );

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn http_toggle_hides_existing_sidebar_panes() {
    let pid_file = test_pid_file("http-toggle-hide");
    let mux = Arc::new(HookMux {
        sidebar_panes: vec![SidebarPane {
            pane_id: "%2".to_string(),
            session_name: "worker".to_string(),
            window_id: "@2".to_string(),
            width: Some(26),
            window_width: Some(120),
        }],
        active_windows: Vec::new(),
        spawn_calls: Mutex::new(Vec::new()),
        hide_calls: Mutex::new(Vec::new()),
        orphan_cleanup_calls: Mutex::new(0),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux.clone()])),
    )
    .await
    .expect("server should start");

    let response = post_text(server.addr(), "/toggle", "/dev/ttys-test|worker|@2").await;
    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n"),
        "response was {response}"
    );
    assert!(response.ends_with("\r\n\r\nok"));
    assert_eq!(*mux.hide_calls.lock().unwrap(), vec!["%2".to_string()]);
    assert!(mux.spawn_calls.lock().unwrap().is_empty());

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn http_toggle_spawns_sidebar_in_active_windows_when_hidden() {
    let pid_file = test_pid_file("http-toggle-spawn");
    let mux = Arc::new(HookMux {
        sidebar_panes: Vec::new(),
        active_windows: vec![
            ActiveWindow {
                id: "@1".to_string(),
                session_name: "api".to_string(),
                active: true,
            },
            ActiveWindow {
                id: "@2".to_string(),
                session_name: "worker".to_string(),
                active: true,
            },
        ],
        spawn_calls: Mutex::new(Vec::new()),
        hide_calls: Mutex::new(Vec::new()),
        orphan_cleanup_calls: Mutex::new(0),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file).with_state_source(
            ReadOnlyMuxStateSource::new(vec![mux.clone()]).with_sidebar_width(31),
        ),
    )
    .await
    .expect("server should start");

    let response = post_text(server.addr(), "/toggle", "/dev/ttys-test|worker|@2").await;
    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n"),
        "response was {response}"
    );
    assert!(response.ends_with("\r\n\r\nok"));
    assert_eq!(
        *mux.spawn_calls.lock().unwrap(),
        vec![
            EnsureSpawnCall {
                session_name: "api".to_string(),
                window_id: "@1".to_string(),
                width: 31,
                position: SidebarPosition::Left,
            },
            EnsureSpawnCall {
                session_name: "worker".to_string(),
                window_id: "@2".to_string(),
                width: 31,
                position: SidebarPosition::Left,
            },
        ]
    );
    assert!(mux.hide_calls.lock().unwrap().is_empty());

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn http_agent_event_resolves_tmux_session_and_broadcasts_agent_state() {
    let pid_file = test_pid_file("http-agent-event");
    let mux = Arc::new(ServerMux {
        current: Some("api".to_string()),
        sessions: vec![MuxSessionInfo {
            name: "api".to_string(),
            created_at: 1,
            dir: "/repo/api".to_string(),
            windows: 1,
        }],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux]).with_now_ms(|| 8_000)),
    )
    .await
    .expect("server should start");
    let addr = server.addr();
    let uri: Uri = format!("ws://{addr}")
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut client, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket client");
    let _ = client.next().await.expect("hello should arrive");
    let _ = client.next().await.expect("initial state should arrive");

    let response = post_json(
        addr,
        "/api/agent-event",
        r#"{"agent":"amp","status":"running","threadId":"T-123","threadName":"Implement Rust","tmuxSession":"api","ts":7000}"#,
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 204 No Content\r\n"),
        "response was {response}"
    );

    let state = timeout(Duration::from_secs(1), client.next())
        .await
        .expect("agent state should arrive before timeout")
        .expect("agent state should arrive")
        .expect("agent state should be valid");
    let parsed = serde_json::from_str::<serde_json::Value>(state.as_text().unwrap()).unwrap();
    let session = &parsed["sessions"][0];
    assert_eq!(session["agentState"]["agent"], "amp");
    assert_eq!(session["agentState"]["status"], "running");
    assert_eq!(session["agentState"]["threadId"], "T-123");
    assert_eq!(session["agentState"]["threadName"], "Implement Rust");
    assert_eq!(session["agents"].as_array().unwrap().len(), 1);
    assert_eq!(session["eventTimestamps"], serde_json::json!([7000]));

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn http_agent_event_rejects_invalid_payloads() {
    let pid_file = test_pid_file("http-agent-event-invalid");
    let mux = Arc::new(ServerMux {
        current: Some("api".to_string()),
        sessions: vec![MuxSessionInfo {
            name: "api".to_string(),
            created_at: 1,
            dir: "/repo/api".to_string(),
            windows: 1,
        }],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux]).with_now_ms(|| 8_000)),
    )
    .await
    .expect("server should start");

    let missing_agent = post_json(
        server.addr(),
        "/api/agent-event",
        r#"{"status":"running","tmuxSession":"api"}"#,
    )
    .await;
    assert!(missing_agent.starts_with("HTTP/1.1 400 Bad Request\r\n"));
    assert!(missing_agent.ends_with("\r\n\r\nmissing agent"));

    let invalid_status = post_json(
        server.addr(),
        "/api/agent-event",
        r#"{"agent":"amp","status":"wat","tmuxSession":"api"}"#,
    )
    .await;
    assert!(invalid_status.starts_with("HTTP/1.1 400 Bad Request\r\n"));
    assert!(invalid_status.ends_with("\r\n\r\ninvalid status"));

    let unresolved = post_json(
        server.addr(),
        "/api/agent-event",
        r#"{"agent":"amp","status":"running","tmuxSession":"missing"}"#,
    )
    .await;
    assert!(unresolved.starts_with("HTTP/1.1 404 Not Found\r\n"));
    assert!(unresolved.ends_with("\r\n\r\ncould not resolve session"));

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn http_agent_event_resolves_session_from_project_dir() {
    let pid_file = test_pid_file("http-agent-event-project-dir");
    let mux = Arc::new(ServerMux {
        current: Some("api".to_string()),
        sessions: vec![MuxSessionInfo {
            name: "api".to_string(),
            created_at: 1,
            dir: "/repo/api".to_string(),
            windows: 1,
        }],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    });
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_state_source(ReadOnlyMuxStateSource::new(vec![mux]).with_now_ms(|| 8_000)),
    )
    .await
    .expect("server should start");
    let addr = server.addr();
    let uri: Uri = format!("ws://{addr}")
        .parse()
        .expect("server address should produce a websocket uri");

    let (mut client, _) = ClientBuilder::from_uri(uri)
        .connect()
        .await
        .expect("server should upgrade websocket client");
    let _ = client.next().await.expect("hello should arrive");
    let _ = client.next().await.expect("initial state should arrive");

    let response = post_json(
        addr,
        "/api/agent-event",
        r#"{"agent":"amp","status":"done","projectDir":"/repo/api/subdir","ts":7000}"#,
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 204 No Content\r\n"),
        "response was {response}"
    );

    let state = timeout(Duration::from_secs(1), client.next())
        .await
        .expect("agent state should arrive before timeout")
        .expect("agent state should arrive")
        .expect("agent state should be valid");
    let parsed = serde_json::from_str::<serde_json::Value>(state.as_text().unwrap()).unwrap();
    assert_eq!(parsed["sessions"][0]["agentState"]["status"], "done");

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[tokio::test(flavor = "current_thread")]
async fn http_pi_runtime_upsert_and_delete_validate_payloads() {
    let pid_file = test_pid_file("http-pi-runtime");
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file).with_state_source(
            ReadOnlyMuxStateSource::new(vec![Arc::new(ServerMux {
                current: None,
                sessions: Vec::new(),
                panes: 0,
                create_calls: Mutex::new(0),
                switch_calls: Mutex::new(Vec::new()),
                kill_calls: Mutex::new(Vec::new()),
            })])
            .with_now_ms(|| 9_000),
        ),
    )
    .await
    .expect("server should start");

    let upsert = post_json(
        server.addr(),
        "/api/runtime/pi/upsert",
        r#"{"pid":123,"sessionId":"thread-1","cwd":"/repo"}"#,
    )
    .await;
    assert!(
        upsert.starts_with("HTTP/1.1 204 No Content\r\n"),
        "response was {upsert}"
    );

    let invalid_upsert = post_json(
        server.addr(),
        "/api/runtime/pi/upsert",
        r#"{"pid":0,"sessionId":"thread-1","cwd":"/repo"}"#,
    )
    .await;
    assert!(invalid_upsert.starts_with("HTTP/1.1 400 Bad Request\r\n"));
    assert!(invalid_upsert.ends_with("\r\n\r\ninvalid pi runtime payload"));

    let delete = post_json(server.addr(), "/api/runtime/pi/delete", r#"{"pid":123}"#).await;
    assert!(
        delete.starts_with("HTTP/1.1 204 No Content\r\n"),
        "response was {delete}"
    );

    let invalid_delete = post_json(server.addr(), "/api/runtime/pi/delete", r#"{"pid":0}"#).await;
    assert!(invalid_delete.starts_with("HTTP/1.1 400 Bad Request\r\n"));
    assert!(invalid_delete.ends_with("\r\n\r\nmissing pid"));

    server.shutdown().await.expect("server should shut down");
    let _ = fs::remove_file(pid_file);
}

#[test]
fn state_source_focuses_and_kills_resolved_agent_panes() {
    let mux = Arc::new(AgentPaneMux {
        focus_calls: Mutex::new(Vec::new()),
        kill_pane_calls: Mutex::new(Vec::new()),
        resolve_calls: Mutex::new(Vec::new()),
    });
    let source = ReadOnlyMuxStateSource::new(vec![mux.clone()]);

    assert_eq!(
        source.handle_client_command(&serde_json::json!({
            "type": "focus-agent-pane",
            "session": "api",
            "agent": "amp",
            "threadName": "migrate server"
        })),
        None
    );
    assert_eq!(
        source.handle_client_command(&serde_json::json!({
            "type": "kill-agent-pane",
            "session": "api",
            "agent": "amp",
            "threadName": "migrate server"
        })),
        None
    );

    assert_eq!(
        *mux.resolve_calls.lock().unwrap(),
        vec![
            AgentPaneResolveCall {
                session: "api".to_string(),
                agent: "amp".to_string(),
                thread_id: None,
                thread_name: Some("migrate server".to_string()),
            },
            AgentPaneResolveCall {
                session: "api".to_string(),
                agent: "amp".to_string(),
                thread_id: None,
                thread_name: Some("migrate server".to_string()),
            },
        ]
    );
    assert_eq!(*mux.focus_calls.lock().unwrap(), vec!["%agent".to_string()]);
    assert_eq!(
        *mux.kill_pane_calls.lock().unwrap(),
        vec!["%agent".to_string()]
    );
}

#[test]
fn state_source_discovers_live_ports_from_pane_process_tree() {
    let source = ReadOnlyMuxStateSource::new(vec![Arc::new(PortMux)]).with_port_command_runner(
        Arc::new(StaticPortRunner {
            process_rows: vec![(101, 100), (102, 101)],
            lsof_fields: "p102\nn127.0.0.1:4549\n".to_string(),
        }),
    );

    let state = source.snapshot_json();
    let parsed = serde_json::from_str::<serde_json::Value>(&state).unwrap();

    assert_eq!(parsed["sessions"][0]["ports"], serde_json::json!([4549]));
    assert_eq!(
        parsed["sessions"][0]["localLinks"][0]["url"],
        "http://localhost:4549"
    );
}

#[test]
fn state_source_reuses_live_port_snapshot_for_ten_seconds() {
    let now = Arc::new(std::sync::atomic::AtomicU64::new(10_000));
    let now_for_source = Arc::clone(&now);
    let port_runner = Arc::new(CountingPortRunner {
        process_calls: AtomicUsize::new(0),
        lsof_calls: AtomicUsize::new(0),
    });
    let source = ReadOnlyMuxStateSource::new(vec![Arc::new(PortMux)])
        .with_now_ms(move || now_for_source.load(Ordering::SeqCst))
        .with_port_command_runner(port_runner.clone());

    let _ = source.snapshot_json();
    let _ = source.snapshot_json();

    assert_eq!(port_runner.process_calls.load(Ordering::SeqCst), 1);
    assert_eq!(port_runner.lsof_calls.load(Ordering::SeqCst), 1);

    now.store(20_001, Ordering::SeqCst);
    let _ = source.snapshot_json();

    assert_eq!(port_runner.process_calls.load(Ordering::SeqCst), 2);
    assert_eq!(port_runner.lsof_calls.load(Ordering::SeqCst), 2);
}

#[test]
fn state_source_populates_git_info_and_caches_by_dir_for_five_seconds() {
    let now = Arc::new(std::sync::atomic::AtomicU64::new(10_000));
    let now_for_source = Arc::clone(&now);
    let git_runner = Arc::new(StaticGitRunner {
        output: "main\n.git/worktrees/api\n---\n M src/lib.rs\n".to_string(),
        calls: Mutex::new(Vec::new()),
    });
    let source = ReadOnlyMuxStateSource::new(vec![Arc::new(PortMux)])
        .with_now_ms(move || now_for_source.load(Ordering::SeqCst))
        .with_git_command_runner(git_runner.clone());

    let first = serde_json::from_str::<serde_json::Value>(&source.snapshot_json()).unwrap();
    let second = serde_json::from_str::<serde_json::Value>(&source.snapshot_json()).unwrap();

    assert_eq!(first["sessions"][0]["branch"], "main");
    assert_eq!(first["sessions"][0]["dirty"], true);
    assert_eq!(first["sessions"][0]["isWorktree"], true);
    assert_eq!(second["sessions"][0]["branch"], "main");
    assert_eq!(
        *git_runner.calls.lock().unwrap(),
        vec!["/repo/api".to_string()]
    );

    now.store(16_000, Ordering::SeqCst);
    let _ = source.snapshot_json();

    assert_eq!(
        *git_runner.calls.lock().unwrap(),
        vec!["/repo/api".to_string(), "/repo/api".to_string()]
    );
}

#[test]
fn state_source_keeps_current_session_visible_when_hidden() {
    let source = ReadOnlyMuxStateSource::new(vec![Arc::new(ServerMux {
        current: Some("api".to_string()),
        sessions: vec![
            MuxSessionInfo {
                name: "api".to_string(),
                created_at: 1,
                dir: "/repo/api".to_string(),
                windows: 1,
            },
            MuxSessionInfo {
                name: "web".to_string(),
                created_at: 2,
                dir: "/repo/web".to_string(),
                windows: 1,
            },
        ],
        panes: 1,
        create_calls: Mutex::new(0),
        switch_calls: Mutex::new(Vec::new()),
        kill_calls: Mutex::new(Vec::new()),
    })]);

    let _ = source.snapshot_json();
    let state = source
        .handle_client_command(&serde_json::json!({"type":"hide-session","name":"api"}))
        .expect("hide-session should broadcast state");

    assert_eq!(
        session_names(&state),
        vec!["api".to_string(), "web".to_string()]
    );
}

#[test]
fn state_source_reports_warmup_after_ensure_sidebar_spawns() {
    let mux = Arc::new(HookMux {
        sidebar_panes: Vec::new(),
        active_windows: vec![ActiveWindow {
            id: "@2".to_string(),
            session_name: "worker".to_string(),
            active: true,
        }],
        spawn_calls: Mutex::new(Vec::new()),
        hide_calls: Mutex::new(Vec::new()),
        orphan_cleanup_calls: Mutex::new(0),
    });
    let source = ReadOnlyMuxStateSource::new(vec![mux]);

    source.handle_http_hook("/ensure-sidebar", "/dev/ttys-test|worker|@2");

    let state = source.snapshot_json();
    let parsed = serde_json::from_str::<serde_json::Value>(&state).unwrap();
    assert_eq!(parsed["initializing"], true);
    assert_eq!(parsed["initLabel"], "warming up…");
}

#[test]
fn state_source_streams_agent_panes_for_all_sessions() {
    let source = ReadOnlyMuxStateSource::new(vec![Arc::new(AgentPaneListMux)]);

    let state = source.snapshot_json();
    let parsed = serde_json::from_str::<serde_json::Value>(&state).unwrap();
    let api = parsed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["name"] == "api")
        .expect("api session should exist");
    let worker = parsed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["name"] == "worker")
        .expect("worker session should exist");

    assert_eq!(api["agentState"]["agent"], "amp");
    assert_eq!(api["agentState"]["status"], "running");
    assert_eq!(api["agents"].as_array().unwrap().len(), 1);
    assert_eq!(api["agents"][0]["paneId"], "%api-agent");
    assert_eq!(api["agents"][0]["liveness"], "alive");
    assert_eq!(worker["agentState"]["agent"], "codex");
    assert_eq!(worker["agents"][0]["paneId"], "%worker-agent");
}

#[derive(Debug)]
struct ServerMux {
    current: Option<String>,
    sessions: Vec<MuxSessionInfo>,
    panes: u32,
    create_calls: Mutex<usize>,
    switch_calls: Mutex<Vec<(String, Option<String>)>>,
    kill_calls: Mutex<Vec<String>>,
}

#[derive(Debug)]
struct AgentPaneListMux;

impl MuxProvider for AgentPaneListMux {
    fn name(&self) -> &str {
        "agent-pane-list-mux"
    }

    fn list_sessions(&self) -> Vec<MuxSessionInfo> {
        vec![
            MuxSessionInfo {
                name: "api".to_string(),
                created_at: 1,
                dir: "/repo/api".to_string(),
                windows: 1,
            },
            MuxSessionInfo {
                name: "worker".to_string(),
                created_at: 2,
                dir: "/repo/worker".to_string(),
                windows: 1,
            },
        ]
    }

    fn switch_session(&self, _name: &str, _client_tty: Option<&str>) {}

    fn get_current_session(&self) -> Option<String> {
        Some("api".to_string())
    }

    fn get_session_dir(&self, name: &str) -> String {
        format!("/repo/{name}")
    }

    fn get_pane_count(&self, _name: &str) -> u32 {
        1
    }

    fn get_client_tty(&self) -> String {
        "/dev/ttys-test".to_string()
    }

    fn create_session(&self, _name: Option<&str>, _dir: Option<&str>) {}

    fn kill_session(&self, _name: &str) {}

    fn setup_hooks(&self, _server_host: &str, _server_port: u16) {}

    fn cleanup_hooks(&self) {}

    fn list_agent_panes(&self, session_name: &str) -> Vec<AgentPane> {
        match session_name {
            "api" => vec![AgentPane {
                agent: "amp".to_string(),
                pane_id: "%api-agent".to_string(),
                thread_id: Some("T-api".to_string()),
                thread_name: Some("Implement API".to_string()),
            }],
            "worker" => vec![AgentPane {
                agent: "codex".to_string(),
                pane_id: "%worker-agent".to_string(),
                thread_id: None,
                thread_name: None,
            }],
            _ => Vec::new(),
        }
    }
}

impl MuxProvider for ServerMux {
    fn name(&self) -> &str {
        "server-mux"
    }

    fn list_sessions(&self) -> Vec<MuxSessionInfo> {
        self.sessions.clone()
    }

    fn switch_session(&self, name: &str, client_tty: Option<&str>) {
        self.switch_calls
            .lock()
            .unwrap()
            .push((name.to_string(), client_tty.map(ToString::to_string)));
    }

    fn get_current_session(&self) -> Option<String> {
        self.current.clone()
    }

    fn get_session_dir(&self, _name: &str) -> String {
        String::new()
    }

    fn get_pane_count(&self, _name: &str) -> u32 {
        self.panes
    }

    fn get_client_tty(&self) -> String {
        "/dev/ttys-test".to_string()
    }

    fn create_session(&self, _name: Option<&str>, _dir: Option<&str>) {
        *self.create_calls.lock().unwrap() += 1;
    }

    fn kill_session(&self, name: &str) {
        self.kill_calls.lock().unwrap().push(name.to_string());
    }

    fn setup_hooks(&self, _server_host: &str, _server_port: u16) {}

    fn cleanup_hooks(&self) {}
}

#[derive(Debug)]
struct PortMux;

impl MuxProvider for PortMux {
    fn name(&self) -> &str {
        "port-mux"
    }

    fn list_sessions(&self) -> Vec<MuxSessionInfo> {
        vec![MuxSessionInfo {
            name: "api".to_string(),
            created_at: 1,
            dir: "/repo/api".to_string(),
            windows: 1,
        }]
    }

    fn switch_session(&self, _name: &str, _client_tty: Option<&str>) {}

    fn get_current_session(&self) -> Option<String> {
        Some("api".to_string())
    }

    fn get_session_dir(&self, _name: &str) -> String {
        "/repo/api".to_string()
    }

    fn get_session_pane_pids(&self, _name: &str) -> Vec<u32> {
        vec![100]
    }

    fn get_pane_count(&self, _name: &str) -> u32 {
        1
    }

    fn get_client_tty(&self) -> String {
        "/dev/ttys-test".to_string()
    }

    fn create_session(&self, _name: Option<&str>, _dir: Option<&str>) {}

    fn kill_session(&self, _name: &str) {}

    fn setup_hooks(&self, _server_host: &str, _server_port: u16) {}

    fn cleanup_hooks(&self) {}
}

struct StaticPortRunner {
    process_rows: Vec<(u32, u32)>,
    lsof_fields: String,
}

impl PortCommandRunner for StaticPortRunner {
    fn process_rows(&self) -> Vec<(u32, u32)> {
        self.process_rows.clone()
    }

    fn lsof_fields(&self) -> String {
        self.lsof_fields.clone()
    }
}

struct CountingPortRunner {
    process_calls: AtomicUsize,
    lsof_calls: AtomicUsize,
}

impl PortCommandRunner for CountingPortRunner {
    fn process_rows(&self) -> Vec<(u32, u32)> {
        self.process_calls.fetch_add(1, Ordering::SeqCst);
        vec![(101, 100)]
    }

    fn lsof_fields(&self) -> String {
        self.lsof_calls.fetch_add(1, Ordering::SeqCst);
        "p101\nn127.0.0.1:3000\n".to_string()
    }
}

struct StaticGitRunner {
    output: String,
    calls: Mutex<Vec<String>>,
}

impl GitCommandRunner for StaticGitRunner {
    fn git_info_output(&self, dir: &str) -> String {
        self.calls.lock().unwrap().push(dir.to_string());
        self.output.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentPaneResolveCall {
    session: String,
    agent: String,
    thread_id: Option<String>,
    thread_name: Option<String>,
}

#[derive(Debug)]
struct AgentPaneMux {
    focus_calls: Mutex<Vec<String>>,
    kill_pane_calls: Mutex<Vec<String>>,
    resolve_calls: Mutex<Vec<AgentPaneResolveCall>>,
}

impl MuxProvider for AgentPaneMux {
    fn name(&self) -> &str {
        "agent-pane-mux"
    }

    fn list_sessions(&self) -> Vec<MuxSessionInfo> {
        vec![MuxSessionInfo {
            name: "api".to_string(),
            created_at: 1,
            dir: "/repo/api".to_string(),
            windows: 1,
        }]
    }

    fn switch_session(&self, _name: &str, _client_tty: Option<&str>) {}

    fn get_current_session(&self) -> Option<String> {
        Some("api".to_string())
    }

    fn get_session_dir(&self, _name: &str) -> String {
        "/repo/api".to_string()
    }

    fn get_pane_count(&self, _name: &str) -> u32 {
        1
    }

    fn get_client_tty(&self) -> String {
        "/dev/ttys-test".to_string()
    }

    fn create_session(&self, _name: Option<&str>, _dir: Option<&str>) {}

    fn kill_session(&self, _name: &str) {}

    fn setup_hooks(&self, _server_host: &str, _server_port: u16) {}

    fn cleanup_hooks(&self) {}

    fn resolve_agent_pane_id(
        &self,
        session: &str,
        agent: &str,
        thread_id: Option<&str>,
        thread_name: Option<&str>,
    ) -> Option<String> {
        self.resolve_calls
            .lock()
            .unwrap()
            .push(AgentPaneResolveCall {
                session: session.to_string(),
                agent: agent.to_string(),
                thread_id: thread_id.map(ToString::to_string),
                thread_name: thread_name.map(ToString::to_string),
            });
        Some("%agent".to_string())
    }

    fn focus_pane(&self, pane_id: &str) {
        self.focus_calls.lock().unwrap().push(pane_id.to_string());
    }

    fn kill_pane(&self, pane_id: &str) {
        self.kill_pane_calls
            .lock()
            .unwrap()
            .push(pane_id.to_string());
    }
}

#[derive(Debug)]
struct HookMux {
    sidebar_panes: Vec<SidebarPane>,
    active_windows: Vec<ActiveWindow>,
    spawn_calls: Mutex<Vec<EnsureSpawnCall>>,
    hide_calls: Mutex<Vec<String>>,
    orphan_cleanup_calls: Mutex<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EnsureSpawnCall {
    session_name: String,
    window_id: String,
    width: u16,
    position: SidebarPosition,
}

impl MuxProvider for HookMux {
    fn name(&self) -> &str {
        "hook-mux"
    }

    fn list_sessions(&self) -> Vec<MuxSessionInfo> {
        Vec::new()
    }

    fn switch_session(&self, _name: &str, _client_tty: Option<&str>) {}

    fn get_current_session(&self) -> Option<String> {
        self.active_windows
            .iter()
            .find(|window| window.active)
            .map(|window| window.session_name.clone())
    }

    fn get_session_dir(&self, _name: &str) -> String {
        String::new()
    }

    fn get_pane_count(&self, _name: &str) -> u32 {
        0
    }

    fn get_client_tty(&self) -> String {
        String::new()
    }

    fn get_current_window_id(&self) -> Option<String> {
        self.active_windows
            .iter()
            .find(|window| window.active)
            .map(|window| window.id.clone())
    }

    fn create_session(&self, _name: Option<&str>, _dir: Option<&str>) {}

    fn kill_session(&self, _name: &str) {}

    fn setup_hooks(&self, _server_host: &str, _server_port: u16) {}

    fn cleanup_hooks(&self) {}

    fn is_window_capable(&self) -> bool {
        true
    }

    fn is_sidebar_capable(&self) -> bool {
        true
    }

    fn list_active_windows(&self) -> Vec<ActiveWindow> {
        self.active_windows.clone()
    }

    fn list_sidebar_panes(&self, _session_name: Option<&str>) -> Vec<SidebarPane> {
        self.sidebar_panes.clone()
    }

    fn spawn_sidebar(
        &self,
        session_name: &str,
        window_id: &str,
        width: u16,
        position: SidebarPosition,
        _scripts_dir: &str,
    ) -> Option<String> {
        self.spawn_calls.lock().unwrap().push(EnsureSpawnCall {
            session_name: session_name.to_string(),
            window_id: window_id.to_string(),
            width,
            position,
        });
        Some("%sidebar".to_string())
    }

    fn hide_sidebar(&self, pane_id: &str) {
        self.hide_calls.lock().unwrap().push(pane_id.to_string());
    }

    fn kill_orphaned_sidebar_panes(&self) {
        *self.orphan_cleanup_calls.lock().unwrap() += 1;
    }
}

fn session_names(state_json: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(state_json)
        .unwrap()
        .get("sessions")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|session| session.get("name").unwrap().as_str().unwrap().to_string())
        .collect()
}

async fn post_json(addr: std::net::SocketAddr, path: &str, body: &str) -> String {
    post_with_content_type(addr, path, "application/json", body).await
}

async fn post_text(addr: std::net::SocketAddr, path: &str, body: &str) -> String {
    post_with_content_type(addr, path, "text/plain", body).await
}

async fn post_with_content_type(
    addr: std::net::SocketAddr,
    path: &str,
    content_type: &str,
    body: &str,
) -> String {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("server should accept http clients");
    stream
        .write_all(
            format!(
                "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .expect("json request should write");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("json response should read");
    String::from_utf8(response).expect("response should be utf-8")
}
