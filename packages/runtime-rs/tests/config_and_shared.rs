use std::fs;
use std::path::PathBuf;

use opensessions_runtime::config::{
    OpensessionsConfig, SidebarPosition, load_config_from_home, save_config_to_home,
};
use opensessions_runtime::protocol::SessionFilterMode;
use opensessions_runtime::shared::{
    DEFAULT_SERVER_HOST, DEFAULT_SERVER_PORT, hash_server_key, resolve_pid_file,
    resolve_server_host, resolve_server_key, resolve_server_port, resolve_server_settings,
};

#[test]
fn server_key_hash_and_port_resolution_match_typescript() {
    assert_eq!(hash_server_key("/private/tmp/tmux-501/default"), 19_916);
    assert_eq!(resolve_server_port(None, None), DEFAULT_SERVER_PORT);
    assert_eq!(resolve_server_port(Some("19916"), None), 41_916);
    assert_eq!(resolve_server_port(Some("19916"), Some("8123")), 8_123);
    // Overflow fallback: keys that push base+key above u16::MAX (and
    // even values that overflow the u32 checked_add) must fall back to
    // DEFAULT_SERVER_PORT rather than silently wrap.
    assert_eq!(resolve_server_port(Some("50000"), None), DEFAULT_SERVER_PORT);
    assert_eq!(
        resolve_server_port(Some("4294967295"), None),
        DEFAULT_SERVER_PORT
    );
}

#[test]
fn resolves_server_settings_from_tmux_socket_and_env_overrides() {
    let settings = resolve_server_settings(|key| match key {
        "TMUX" => Some("/private/tmp/tmux-501/default,123,0".to_string()),
        "OPENSESSIONS_HOST" => Some("0.0.0.0".to_string()),
        _ => None,
    });

    assert_eq!(settings.server_key.as_deref(), Some("19916"));
    assert_eq!(settings.host, "0.0.0.0");
    assert_eq!(settings.port, 41_916);
    assert_eq!(settings.pid_file, "/tmp/opensessions.19916.pid");
}

#[test]
fn resolve_server_settings_ignores_opensessions_rust_env_var() {
    // OPENSESSIONS_RUST=1 was an opt-in toggle while the TS bun server
    // (17000+server_key) coexisted with the Rust stack. The TS server is
    // gone (commit 89168a3) and the Rust stack now always binds
    // 22000+server_key (`SERVER_PORT_BASE`) regardless of this env var.
    let settings = resolve_server_settings(|key| match key {
        "TMUX" => Some("/private/tmp/tmux-501/os-rs-test,123,0".to_string()),
        "OPENSESSIONS_RUST" => Some("1".to_string()),
        _ => None,
    });

    assert_eq!(settings.server_key.as_deref(), Some("8011"));
    assert_eq!(
        settings.port, 30_011,
        "Rust stack must bind base 22000 (got {})",
        settings.port
    );
}

#[test]
fn resolve_server_settings_uses_default_port_base_without_opensessions_rust() {
    let settings = resolve_server_settings(|key| match key {
        "TMUX" => Some("/private/tmp/tmux-501/os-rs-test,123,0".to_string()),
        _ => None,
    });

    assert_eq!(settings.port, 30_011);
}

#[test]
fn resolve_server_settings_explicit_port_overrides_port_base() {
    let settings = resolve_server_settings(|key| match key {
        "TMUX" => Some("/private/tmp/tmux-501/os-rs-test,123,0".to_string()),
        "OPENSESSIONS_RUST" => Some("1".to_string()),
        "OPENSESSIONS_PORT" => Some("42424".to_string()),
        _ => None,
    });

    assert_eq!(settings.port, 42_424);
}

#[test]
fn resolves_defaults_and_explicit_pid_file_like_typescript() {
    assert_eq!(resolve_server_key(|_| None), None);
    assert_eq!(
        resolve_server_key(|key| match key {
            "OPENSESSIONS_SERVER_KEY" => Some(" 123 ".to_string()),
            "TMUX" => Some("/private/tmp/tmux-501/default,123,0".to_string()),
            _ => None,
        })
        .as_deref(),
        Some("123")
    );
    assert_eq!(resolve_server_host(None), DEFAULT_SERVER_HOST);
    assert_eq!(resolve_pid_file(None, None), "/tmp/opensessions.pid");
    assert_eq!(
        resolve_pid_file(Some("abc"), Some("/tmp/custom.pid")),
        "/tmp/custom.pid"
    );
}

#[test]
fn load_config_returns_defaults_when_missing_or_invalid() {
    let home = temp_home("missing-config");

    let config = load_config_from_home(&home);
    assert_eq!(config.plugins, Vec::<String>::new());
    assert_eq!(config.mux, None);

    fs::create_dir_all(home.join(".config/opensessions")).unwrap();
    fs::write(home.join(".config/opensessions/config.json"), "not json").unwrap();
    let invalid = load_config_from_home(&home);
    assert_eq!(invalid.plugins, Vec::<String>::new());

    fs::remove_dir_all(home).unwrap();
}

#[test]
fn load_config_reads_sidebar_settings_and_theme_values() {
    let home = temp_home("load-config");
    fs::create_dir_all(home.join(".config/opensessions")).unwrap();
    fs::write(
        home.join(".config/opensessions/config.json"),
        r##"{"mux":"zellij","plugins":["opensessions-mux-zellij"],"theme":{"palette":{"base":"#000000"}},"sidebarWidth":30,"sidebarPosition":"right","keybinding":"b","sessionFilter":"running"}"##,
    )
    .unwrap();

    let config = load_config_from_home(&home);

    assert_eq!(config.mux.as_deref(), Some("zellij"));
    assert_eq!(config.plugins, vec!["opensessions-mux-zellij"]);
    assert_eq!(config.sidebar_width, Some(30));
    assert_eq!(config.sidebar_position, Some(SidebarPosition::Right));
    assert_eq!(config.keybinding.as_deref(), Some("b"));
    assert_eq!(config.session_filter, Some(SessionFilterMode::Running));
    assert_eq!(config.theme.unwrap()["palette"]["base"], "#000000");

    fs::remove_dir_all(home).unwrap();
}

#[test]
fn save_config_merges_with_existing_file_and_preserves_detail_heights() {
    let home = temp_home("save-config");
    fs::create_dir_all(home.join(".config/opensessions")).unwrap();
    fs::write(
        home.join(".config/opensessions/config.json"),
        r#"{"mux":"tmux","plugins":["some-plugin"],"detailPanelHeights":{"alpha":12}}"#,
    )
    .unwrap();

    save_config_to_home(
        &home,
        OpensessionsConfig {
            theme: Some(serde_json::json!("nord")),
            ..Default::default()
        },
    )
    .unwrap();

    let config = load_config_from_home(&home);
    assert_eq!(config.theme.unwrap(), "nord");
    assert_eq!(config.mux.as_deref(), Some("tmux"));
    assert_eq!(config.plugins, vec!["some-plugin"]);
    assert_eq!(config.detail_panel_heights.get("alpha"), Some(&12));

    fs::remove_dir_all(home).unwrap();
}

fn temp_home(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "opensessions-runtime-rs-{name}-{}",
        std::process::id()
    ))
}
