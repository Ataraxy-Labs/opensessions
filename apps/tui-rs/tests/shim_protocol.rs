use opensessions_sidebar_protocol::{
    KeyCode, KeyMessage, KeyModifiers, MouseButton, MouseEventKind, MouseMessage, ServerToShim,
    ShimHello, ShimToServer, decode_server_message, decode_shim_message, encode_server_message,
    encode_shim_message,
};

#[test]
fn encodes_and_decodes_shim_hello_with_pane_context() {
    let message = ShimToServer::Hello(ShimHello {
        protocol: 1,
        pane_id: "%42".into(),
        session_name: "opensessions".into(),
        window_id: Some("@7".into()),
        client_tty: Some("/dev/ttys001".into()),
        width: 35,
        height: 56,
    });

    let encoded = encode_shim_message(&message);

    assert_eq!(decode_shim_message(&encoded).unwrap(), message);
}

#[test]
fn encodes_key_resize_mouse_and_close_messages() {
    let key = ShimToServer::Key(KeyMessage {
        code: KeyCode::Char('j'),
        modifiers: KeyModifiers::CONTROL,
    });
    let resize = ShimToServer::Resize {
        width: 42,
        height: 20,
    };
    let mouse = ShimToServer::Mouse(MouseMessage {
        kind: MouseEventKind::Drag,
        button: MouseButton::Left,
        column: 4,
        row: 38,
        modifiers: KeyModifiers::empty(),
    });
    let close = ShimToServer::Close;

    for message in [key, resize, mouse, close] {
        assert_eq!(
            decode_shim_message(&encode_shim_message(&message)).unwrap(),
            message
        );
    }
}

#[test]
fn encodes_full_and_patch_frames_as_server_messages() {
    let full = ServerToShim::FullFrame {
        seq: 1,
        width: 35,
        height: 2,
        rows: vec![b"first".to_vec(), b"second".to_vec()],
    };
    let patch = ServerToShim::PatchFrame {
        seq: 2,
        width: 35,
        height: 2,
        changed_rows: vec![(1, b"changed".to_vec())],
        clear_from_row: None,
    };

    assert_eq!(
        decode_server_message(&encode_server_message(&full)).unwrap(),
        full
    );
    assert_eq!(
        decode_server_message(&encode_server_message(&patch)).unwrap(),
        patch
    );
}
