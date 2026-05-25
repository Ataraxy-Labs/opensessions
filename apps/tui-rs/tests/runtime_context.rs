use opensessions_sidebar::generated::protocol::ClientCommand;
use opensessions_sidebar::runtime_context::{
    pane_identity_from_env, pane_identity_resolve, refocus_plan, report_width_command,
    should_report_width,
};

#[test]
fn pane_identity_prefers_explicit_spawn_context() {
    let identity = pane_identity_from_env(|key| match key {
        "TMUX_PANE" => Some("%99".to_string()),
        "OPENSESSIONS_SESSION_NAME" => Some("opensessions".to_string()),
        "OPENSESSIONS_WINDOW_ID" => Some("@7".to_string()),
        _ => None,
    })
    .expect("identity should resolve from env");

    assert_eq!(identity.pane_id, "%99");
    assert_eq!(identity.session_name, "opensessions");
    assert_eq!(identity.window_id.as_deref(), Some("@7"));
}

#[test]
fn foreground_sidebar_reports_width_like_opentui_client() {
    assert!(should_report_width(Some("opensessions"), Some("opensessions")));
    assert!(!should_report_width(Some("opensessions"), Some("other")));
    assert!(!should_report_width(None, Some("opensessions")));
}

#[test]
fn pane_identity_falls_back_to_tmux_display_message_when_env_missing() {
    let identity = pane_identity_resolve(
        |key| match key {
            "TMUX_PANE" => Some("%99".to_string()),
            _ => None,
        },
        |format, target| {
            assert_eq!(target, "%99");
            match format {
                "#{session_name}" => Some("opensessions".to_string()),
                "#{window_id}" => Some("@7".to_string()),
                _ => None,
            }
        },
    )
    .expect("identity should resolve via tmux fallback");

    assert_eq!(identity.pane_id, "%99");
    assert_eq!(identity.session_name, "opensessions");
    assert_eq!(identity.window_id.as_deref(), Some("@7"));
}

#[test]
fn pane_identity_prefers_env_over_tmux_fallback() {
    let identity = pane_identity_resolve(
        |key| match key {
            "TMUX_PANE" => Some("%99".to_string()),
            "OPENSESSIONS_SESSION_NAME" => Some("explicit".to_string()),
            "OPENSESSIONS_WINDOW_ID" => Some("@1".to_string()),
            _ => None,
        },
        |_, _| panic!("tmux fallback must not be queried when env is present"),
    )
    .expect("identity should resolve from env without consulting tmux");

    assert_eq!(identity.session_name, "explicit");
    assert_eq!(identity.window_id.as_deref(), Some("@1"));
}

#[test]
fn pane_identity_returns_none_when_pane_id_missing() {
    let identity = pane_identity_resolve(|_| None, |_, _| None);
    assert!(identity.is_none());
}

#[test]
fn refocus_plan_uses_refocus_window_env_when_present() {
    let plan = refocus_plan("%99", Some("@7"), |args| match args {
        ["list-panes", "-t", "@7", "-F", "#{pane_id} #{pane_title}"] => {
            Some("%99 opensessions-sidebar\n%100 main".to_string())
        }
        _ => panic!("unexpected tmux invocation: {args:?}"),
    })
    .expect("plan should resolve when REFOCUS_WINDOW points at a window with a main pane");
    assert_eq!(plan.select_pane, "%100");
}

#[test]
fn refocus_plan_falls_back_to_tmux_display_message() {
    let plan = refocus_plan("%99", None, |args| match args {
        ["display-message", "-t", "%99", "-p", "#{window_id}"] => Some("@7".to_string()),
        ["list-panes", "-t", "@7", "-F", "#{pane_id} #{pane_title}"] => {
            Some("%99 opensessions-sidebar\n%100 main".to_string())
        }
        _ => None,
    })
    .expect("plan should resolve via tmux display-message fallback");
    assert_eq!(plan.select_pane, "%100");
}

#[test]
fn refocus_plan_returns_none_when_no_main_pane_found() {
    let plan = refocus_plan("%99", Some("@7"), |args| match args {
        ["list-panes", "-t", "@7", "-F", "#{pane_id} #{pane_title}"] => {
            Some("%99 opensessions-sidebar".to_string())
        }
        _ => None,
    });
    assert!(plan.is_none());
}

#[test]
fn report_width_command_uses_current_terminal_width() {
    assert_eq!(
        report_width_command(31, Some("opensessions"), Some("opensessions")),
        Some(ClientCommand::ReportWidth { width: 31 })
    );
    assert_eq!(
        report_width_command(31, Some("opensessions"), Some("other")),
        None
    );
}
