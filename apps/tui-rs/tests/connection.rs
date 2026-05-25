use opensessions_sidebar::client::{
    EXPECTED_PROTOCOL_VERSION, build_quit_http_request, decode_server_message, validate_hello,
};
use opensessions_sidebar::generated::protocol::{ProtocolHello, ServerMessage};

#[test]
fn accepts_matching_protocol_hello() {
    let hello = ServerMessage::Hello(ProtocolHello {
        protocol: EXPECTED_PROTOCOL_VERSION,
        server_version: "0.2.0-alpha.5".into(),
    });
    assert!(validate_hello(&hello).is_ok());
}

#[test]
fn rejects_mismatched_protocol_hello() {
    let hello = ServerMessage::Hello(ProtocolHello {
        protocol: EXPECTED_PROTOCOL_VERSION + 1,
        server_version: "future".into(),
    });
    let err = validate_hello(&hello).unwrap_err();
    assert!(err.contains("unsupported protocol"));
}

#[test]
fn rejects_non_hello_first_message() {
    let err = validate_hello(&ServerMessage::Quit).unwrap_err();
    assert!(err.contains("expected hello"));
}

#[test]
fn build_quit_http_request_matches_typescript_fallback() {
    // Mirrors the TypeScript fallback in apps/tui/src/index.tsx:
    //   fetch(`http://${SERVER_HOST}:${SERVER_PORT}/quit`, { method: "POST" })
    // The Rust client has no fetch, so it sends a minimal HTTP/1.1 POST over a
    // raw TCP connection. The wire format must be a complete request with
    // Host, zero-length body, and Connection: close so the server side can
    // drop the socket immediately after replying.
    let request = build_quit_http_request("127.0.0.1", 7391);
    assert!(
        request.starts_with("POST /quit HTTP/1.1\r\n"),
        "request line must POST /quit; got: {request:?}"
    );
    assert!(
        request.contains("Host: 127.0.0.1:7391\r\n"),
        "Host header must include host:port; got: {request:?}"
    );
    assert!(
        request.contains("Content-Length: 0\r\n"),
        "Content-Length must be 0; got: {request:?}"
    );
    assert!(
        request.contains("Connection: close\r\n"),
        "Connection: close lets the server tear down promptly; got: {request:?}"
    );
    assert!(
        request.ends_with("\r\n\r\n"),
        "request must end with the empty-line terminator; got: {request:?}"
    );
}

#[test]
fn decodes_text_server_message_payload() {
    let msg =
        decode_server_message(br#"{"type":"hello","protocol":1,"serverVersion":"0.2.0-alpha.5"}"#)
            .unwrap();
    assert!(matches!(msg, ServerMessage::Hello(_)));
}
