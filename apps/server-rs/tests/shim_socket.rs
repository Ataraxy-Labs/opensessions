use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use opensessions_server::{ServerConfig, start_server};
use opensessions_sidebar_protocol::{
    ServerToShim, ShimHello, ShimToServer, decode_server_message, encode_shim_message,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::time::{Duration, timeout};

fn test_path(name: &str, extension: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "os-{name}-{}-{stamp}.{extension}",
        std::process::id()
    ))
}

#[tokio::test(flavor = "current_thread")]
async fn shim_socket_accepts_hello_and_returns_rendered_full_frame() {
    let pid_file = test_path("shim", "pid");
    let socket_path = test_path("shim", "sock");
    let state = r#"{
        "type":"state",
        "sessions":[{
            "name":"opensessions","createdAt":1,"dir":"/repo","branch":"main",
            "dirty":false,"isWorktree":false,"unseen":false,"panes":1,
            "ports":[],"localLinks":[],"windows":1,"uptime":"1m",
            "agentState":null,"agents":[],"eventTimestamps":[],"metadata":null
        }],
        "focusedSession":"opensessions","currentSession":"opensessions",
        "theme":"catppuccin-mocha","sessionFilter":"all","sidebarWidth":35,
        "initializing":false,"ts":3
    }"#;
    let server = start_server(
        ServerConfig::new("127.0.0.1", 0, &pid_file)
            .with_shim_socket_path(&socket_path)
            .with_state_source(move || state.to_string()),
    )
    .await
    .expect("server should start");

    let mut stream = UnixStream::connect(server.shim_socket_path().unwrap())
        .await
        .expect("shim socket should accept local clients");
    stream
        .write_all(&encode_shim_message(&ShimToServer::Hello(ShimHello {
            protocol: 1,
            pane_id: "%42".into(),
            session_name: "opensessions".into(),
            window_id: Some("@1".into()),
            client_tty: Some("/dev/ttys-test".into()),
            width: 35,
            height: 10,
        })))
        .await
        .expect("shim hello should write");

    let hello = timeout(Duration::from_secs(1), read_server_message(&mut stream))
        .await
        .expect("server hello should arrive")
        .expect("server hello should decode");
    assert_eq!(hello, ServerToShim::Hello { protocol: 1 });

    let frame = timeout(Duration::from_secs(1), read_server_message(&mut stream))
        .await
        .expect("initial frame should arrive")
        .expect("initial frame should decode");
    let ServerToShim::FullFrame {
        width,
        height,
        rows,
        ..
    } = frame
    else {
        panic!("initial shim render should be a full frame")
    };
    assert_eq!((width, height), (35, 10));
    assert!(
        rows.iter()
            .any(|row| row.windows(8).any(|w| w == b"Sessions"))
    );

    server.shutdown().await.expect("server should shut down");
    assert!(!socket_path.exists(), "shutdown should remove shim socket");
    let _ = fs::remove_file(pid_file);
}

async fn read_server_message(stream: &mut UnixStream) -> std::io::Result<ServerToShim> {
    let mut len = [0_u8; 4];
    stream.read_exact(&mut len).await?;
    let len = u32::from_le_bytes(len) as usize;
    let mut payload = vec![0; len];
    stream.read_exact(&mut payload).await?;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    decode_server_message(&frame).map_err(std::io::Error::other)
}
