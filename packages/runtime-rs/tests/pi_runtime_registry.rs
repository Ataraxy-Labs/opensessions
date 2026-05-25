use opensessions_runtime::pi_runtime_registry::{PiRuntimeRegistry, parse_pi_runtime_info};

#[test]
fn parses_valid_pi_runtime_info_and_defaults_timestamp() {
    let info = parse_pi_runtime_info(
        &serde_json::json!({
            "pid": 123,
            "ppid": 1,
            "sessionId": "thread-1",
            "sessionFile": "/tmp/session.json",
            "cwd": "/repo",
            "sessionName": "api"
        }),
        42,
    )
    .expect("valid runtime info should parse");

    assert_eq!(info.pid, 123);
    assert_eq!(info.ppid, Some(1));
    assert_eq!(info.session_id, "thread-1");
    assert_eq!(info.session_file.as_deref(), Some("/tmp/session.json"));
    assert_eq!(info.cwd, "/repo");
    assert_eq!(info.session_name.as_deref(), Some("api"));
    assert_eq!(info.ts, 42);
}

#[test]
fn rejects_invalid_pi_runtime_info() {
    for value in [
        serde_json::json!(null),
        serde_json::json!({ "pid": 0, "sessionId": "s", "cwd": "/repo" }),
        serde_json::json!({ "pid": 1, "ppid": -1, "sessionId": "s", "cwd": "/repo" }),
        serde_json::json!({ "pid": 1, "sessionId": "", "cwd": "/repo" }),
        serde_json::json!({ "pid": 1, "sessionId": "s", "cwd": "" }),
        serde_json::json!({ "pid": 1, "sessionId": "s", "cwd": "/repo", "sessionName": 3 }),
    ] {
        assert_eq!(parse_pi_runtime_info(&value, 1), None);
    }
}

#[test]
fn registry_upsert_delete_and_ttl_match_typescript_behavior() {
    let mut registry = PiRuntimeRegistry::new(20_000);
    let info = parse_pi_runtime_info(
        &serde_json::json!({ "pid": 123, "sessionId": "thread-1", "cwd": "/repo", "ts": 1_000 }),
        0,
    )
    .unwrap();

    registry.upsert(info);
    assert_eq!(registry.get(123, 2_000).unwrap().session_id, "thread-1");
    assert!(registry.get(123, 22_000).is_none());

    registry.upsert(
        parse_pi_runtime_info(
            &serde_json::json!({ "pid": 456, "sessionId": "thread-2", "cwd": "/repo", "ts": 3_000 }),
            0,
        )
        .unwrap(),
    );
    assert!(registry.delete(456));
    assert!(!registry.delete(456));
}
