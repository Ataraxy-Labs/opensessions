use opensessions_sidebar::app::App;
use opensessions_sidebar::snapshot::{buffer_symbol_at, buffer_to_ansi, render_to_buffer};

#[test]
fn header_renders_init_label_with_spinner_when_initializing() {
    let mut app = App::reference_fixture("pane-attached-session-list");
    app.initializing = true;
    app.init_label = Some("loading sessions".into());
    app.ts = 0;

    let buffer = render_to_buffer(&mut app, 60, 56);
    let ansi = buffer_to_ansi(&buffer);

    // Spinner glyph at ts=0 is "◐"; the label text must follow.
    assert!(
        ansi.contains("◐"),
        "header must include a spinner glyph while initializing; got:\n{ansi}"
    );
    assert!(
        ansi.contains("loading sessions"),
        "header must include the init_label string; got:\n{ansi}"
    );
}

#[test]
fn header_falls_back_to_warming_up_when_init_label_missing() {
    let mut app = App::reference_fixture("pane-attached-session-list");
    app.initializing = true;
    app.init_label = None;
    app.ts = 250; // selects spinner index 1 = "◓"

    let buffer = render_to_buffer(&mut app, 60, 56);
    let ansi = buffer_to_ansi(&buffer);

    assert!(
        ansi.contains("◓"),
        "spinner must advance frame with ts; got:\n{ansi}"
    );
    assert!(
        ansi.contains("warming up"),
        "missing init_label must fall back to 'warming up…'; got:\n{ansi}"
    );
}

#[test]
fn header_at_narrow_width_omits_init_label_when_it_does_not_fit() {
    // When the pane width is too narrow to fit "  Sessions N ◐ <label>",
    // the renderer must omit the spinner+label section entirely rather than
    // letting ratatui's Paragraph truncate it mid-word (e.g. "◐ adj").
    let mut app = App::reference_fixture("pane-attached-session-list");
    app.initializing = true;
    app.init_label = Some("adjusting widths to fit".into());
    app.ts = 0;

    let buffer = render_to_buffer(&mut app, 20, 10);

    let mut header = String::new();
    for x in 0..20 {
        header.push_str(&buffer_symbol_at(&buffer, x, 1));
    }

    assert!(
        !header.contains("adj"),
        "header at width=20 must drop the init_label section instead of \
         showing a partial truncation; got: {header:?}",
    );
    assert!(
        !header.contains('◐'),
        "header at width=20 must drop the spinner section together with \
         the label; got: {header:?}",
    );
    assert!(
        header.contains("Sessions"),
        "header must still show the Sessions label even at narrow widths; \
         got: {header:?}",
    );
}

#[test]
fn header_omits_spinner_when_not_initializing() {
    let mut app = App::reference_fixture("pane-attached-session-list");
    app.initializing = false;
    app.init_label = Some("ignored".into());
    app.ts = 0;

    let buffer = render_to_buffer(&mut app, 60, 56);

    // The header line is row 1 (after marker line).
    let mut header = String::new();
    for x in 0..60 {
        header.push_str(&buffer_symbol_at(&buffer, x, 1));
    }
    assert!(
        !header.contains('◐'),
        "spinner must not appear when initializing=false; got header: {header:?}"
    );
    assert!(
        !header.contains("ignored"),
        "init_label must be hidden when initializing=false; got header: {header:?}"
    );
}
