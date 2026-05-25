use anyhow::Result;
use http::Uri;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_websockets::{ClientBuilder, MaybeTlsStream, WebSocketStream};

use crate::generated::protocol::{ClientCommand, ServerMessage};

pub const EXPECTED_PROTOCOL_VERSION: u16 = 1;

pub fn validate_hello(msg: &ServerMessage) -> std::result::Result<(), String> {
    let ServerMessage::Hello(hello) = msg else {
        return Err("expected hello as first server message".to_string());
    };

    if hello.protocol != EXPECTED_PROTOCOL_VERSION {
        return Err(format!(
            "unsupported protocol {}, expected {}",
            hello.protocol, EXPECTED_PROTOCOL_VERSION
        ));
    }

    Ok(())
}

pub fn decode_server_message(payload: &[u8]) -> serde_json::Result<ServerMessage> {
    serde_json::from_slice(payload)
}

pub fn encode_client_command(command: &ClientCommand) -> serde_json::Result<String> {
    serde_json::to_string(command)
}

/// Build the raw HTTP/1.1 request the sidebar fires at `http://host:port/quit`
/// when the user presses 'q'. Mirrors the TypeScript fallback in
/// `apps/tui/src/index.tsx`:
///   `fetch(`http://${SERVER_HOST}:${SERVER_PORT}/quit`, { method: "POST" })`
/// This is fire-and-forget — the server replies, then closes the WS, which
/// tears down the renderer.
pub fn build_quit_http_request(host: &str, port: u16) -> String {
    format!(
        "POST /quit HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
}

/// Fire-and-forget HTTP POST to `/quit`. Errors are intentionally swallowed:
/// this is a fallback for when the WS Quit frame might be lost while the TUI
/// is tearing down.
pub async fn fire_quit_http(host: &str, port: u16) {
    let Ok(mut stream) = TcpStream::connect((host, port)).await else {
        return;
    };
    let _ = stream
        .write_all(build_quit_http_request(host, port).as_bytes())
        .await;
    let _ = stream.shutdown().await;
}

pub async fn connect_ws(
    host: &str,
    port: u16,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
    connect_ws_path(host, port, "/").await
}

pub async fn connect_ws_path(
    host: &str,
    port: u16,
    path_and_query: &str,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
    let uri: Uri = format!("ws://{host}:{port}{path_and_query}").parse()?;
    let (ws, _) = ClientBuilder::from_uri(uri).connect().await?;
    Ok(ws)
}
