use opensessions_sidebar_core::app::App;
use opensessions_sidebar_core::snapshot::{buffer_symbol_at, render_to_buffer};

#[test]
fn renderer_uses_app_terminal_width_for_separator_column() {
    let mut app = App::reference_fixture("pane-attached-session-list");
    // Tell the App that the terminal canvas is narrower than the test backend.
    app.set_terminal_width(40);
    let buffer = render_to_buffer(&mut app, 60, 56);

    // Separator on row 43 should reflect terminal_width=40 (39 dashes, cols 1..=39).
    assert_eq!(buffer_symbol_at(&buffer, 1, 43), "─");
    assert_eq!(buffer_symbol_at(&buffer, 39, 43), "─");
    // Col 40 onward must be blank because the App width caps the separator.
    assert_eq!(buffer_symbol_at(&buffer, 40, 43), " ");
    assert_eq!(buffer_symbol_at(&buffer, 50, 43), " ");
}

#[test]
fn app_default_terminal_width_is_unset() {
    let app = App::reference_fixture("pane-attached-session-list");
    assert_eq!(app.terminal_width(), None);
}
