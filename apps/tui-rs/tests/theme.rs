use opensessions_sidebar::app::App;
use opensessions_sidebar::generated::protocol::{
    ClientCommand, ServerMessage, ServerState, SessionFilterMode,
};
use opensessions_sidebar::renderer::palette_for_theme;
use opensessions_sidebar::snapshot::{buffer_to_ansi, render_to_buffer};

fn empty_state(theme: Option<&str>) -> ServerState {
    ServerState {
        sessions: Vec::new(),
        focused_session: None,
        current_session: None,
        theme: theme.map(str::to_string),
        session_filter: Some(SessionFilterMode::All),
        sidebar_width: 35,
        initializing: false,
        init_label: None,
        ts: 0,
    }
}

#[test]
fn from_state_records_initial_theme_name() {
    let app = App::from_state(empty_state(Some("tokyo-night")));
    assert_eq!(app.theme.as_deref(), Some("tokyo-night"));
}

#[test]
fn applying_state_with_new_theme_updates_app_theme() {
    let mut app = App::from_state(empty_state(Some("catppuccin-mocha")));
    app.apply_server_message(ServerMessage::State(empty_state(Some("catppuccin-latte"))));
    assert_eq!(app.theme.as_deref(), Some("catppuccin-latte"));
}

#[test]
fn applying_state_with_no_theme_clears_theme_field() {
    let mut app = App::from_state(empty_state(Some("tokyo-night")));
    app.apply_server_message(ServerMessage::State(empty_state(None)));
    assert_eq!(app.theme, None);
}

#[test]
fn set_theme_helper_queues_set_theme_command() {
    let mut app = App::reference_fixture("pane-attached-session-list");
    app.set_theme_request("rose-pine".into());

    assert_eq!(
        app.drain_commands(),
        vec![ClientCommand::SetTheme {
            theme: "rose-pine".into(),
        }]
    );
}

#[test]
fn palette_for_theme_returns_distinct_palettes_for_known_themes() {
    let mocha = palette_for_theme(Some("catppuccin-mocha"));
    let latte = palette_for_theme(Some("catppuccin-latte"));

    assert_ne!(
        mocha.text, latte.text,
        "catppuccin-mocha and -latte must differ in their text foreground"
    );
    assert_ne!(
        mocha.surface1, latte.surface1,
        "catppuccin-mocha and -latte must differ in their selection background"
    );
}

#[test]
fn palette_for_theme_falls_back_to_default_for_unknown_or_missing() {
    let default_palette = palette_for_theme(None);
    let unknown = palette_for_theme(Some("not-a-real-theme"));

    assert_eq!(
        default_palette, unknown,
        "an unknown theme name must fall back to the default palette"
    );

    // Default palette must keep snapshot fidelity → catppuccin-mocha values.
    assert_eq!(
        default_palette,
        palette_for_theme(Some("catppuccin-mocha")),
        "the default palette must be catppuccin-mocha so existing reference snapshots stay valid"
    );
}

#[test]
fn rendered_output_uses_active_theme_palette_for_focused_session_text() {
    let mut mocha_app = App::reference_fixture("pane-attached-session-list");
    mocha_app.theme = Some("catppuccin-mocha".into());
    let mocha_ansi = buffer_to_ansi(&render_to_buffer(&mut mocha_app, 35, 56));

    let mut latte_app = App::reference_fixture("pane-attached-session-list");
    latte_app.theme = Some("catppuccin-latte".into());
    let latte_ansi = buffer_to_ansi(&render_to_buffer(&mut latte_app, 35, 56));

    assert_ne!(
        mocha_ansi, latte_ansi,
        "switching app.theme between mocha and latte must produce different rendered output"
    );

    let latte = palette_for_theme(Some("catppuccin-latte"));
    let latte_text_sgr = latte.text.fg_sgr();
    assert!(
        latte_ansi.contains(&latte_text_sgr),
        "latte rendering must emit the latte text color SGR escape; got latte text fg: {latte_text_sgr:?}"
    );
}
