use opensessions_sidebar::cli::{Args, resolve_endpoint_from_env};

#[test]
fn cli_defaults_match_typescript_runtime() {
    let args = Args::try_parse_from(["opensessions-sidebar"]).unwrap();
    assert_eq!(args.server_host, "127.0.0.1");
    assert_eq!(args.server_port, 7_391);
}

#[test]
fn cli_accepts_explicit_server_endpoint() {
    let args = Args::try_parse_from([
        "opensessions-sidebar",
        "--server-host",
        "0.0.0.0",
        "--server-port",
        "8123",
    ])
    .unwrap();
    assert_eq!(args.server_host, "0.0.0.0");
    assert_eq!(args.server_port, 8_123);
}

#[test]
fn resolves_tmux_derived_endpoint_from_environment_like_typescript_client() {
    let endpoint = resolve_endpoint_from_env(|key| match key {
        "TMUX" => Some("/private/tmp/tmux-501/default,13614,3".to_string()),
        _ => None,
    });

    assert_eq!(endpoint.server_host, "127.0.0.1");
    assert_eq!(endpoint.server_port, 36_916);
}

#[test]
fn explicit_env_endpoint_overrides_tmux_derived_endpoint() {
    let endpoint = resolve_endpoint_from_env(|key| match key {
        "TMUX" => Some("/private/tmp/tmux-501/default,13614,3".to_string()),
        "OPENSESSIONS_HOST" => Some("0.0.0.0".to_string()),
        "OPENSESSIONS_PORT" => Some("8123".to_string()),
        _ => None,
    });

    assert_eq!(endpoint.server_host, "0.0.0.0");
    assert_eq!(endpoint.server_port, 8_123);
}
