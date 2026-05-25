use opensessions_sidebar::app::App;
use opensessions_sidebar::snapshot::{buffer_symbol_at, render_to_buffer};

fn row_text(buffer: &opensessions_sidebar::snapshot::RenderedBuffer, width: u16, y: u16) -> String {
    let mut row = String::new();
    for x in 0..width {
        row.push_str(&buffer_symbol_at(buffer, x, y));
    }
    row
}

#[test]
fn footer_top_truncates_to_fit_narrow_width() {
    // The footer is built from short hint pairs ("⇥ cycle", "⏎ go", etc.).
    // At narrow widths the renderer must drop trailing pairs cleanly so the
    // line stops on a complete word — never mid-token like "ag" or "fil".
    let mut app = App::reference_fixture("pane-attached-session-list");
    let buffer = render_to_buffer(&mut app, 22, 56);

    let footer_top = row_text(&buffer, 22, 53);

    let bad_fragments = [" ag", " fil", "ent", "ter"];
    for frag in bad_fragments {
        assert!(
            !footer_top.contains(frag),
            "footer_top at width=22 must drop hints that don't fit cleanly; \
             contained partial fragment {frag:?}: {footer_top:?}",
        );
    }
    // The first hint "⇥ cycle" must survive at any non-trivial width.
    assert!(
        footer_top.contains("⇥") && footer_top.contains("cycle"),
        "footer_top must always show the first hint; got: {footer_top:?}",
    );
}

#[test]
fn footer_bottom_truncates_to_fit_narrow_width() {
    let mut app = App::reference_fixture("pane-attached-session-list");
    let buffer = render_to_buffer(&mut app, 10, 56);

    let footer_bottom = row_text(&buffer, 10, 54);

    // Width=10 cannot fit "  d hide  x kill" (~16 cells). Bottom row must
    // either show a complete "d hide" hint or drop it; never partial "kil".
    assert!(
        !footer_bottom.contains("kil"),
        "footer_bottom must not show partial 'kil'; got: {footer_bottom:?}",
    );
    assert!(
        !footer_bottom.contains("hid") || footer_bottom.contains("hide"),
        "if 'hid' appears it must be the complete 'hide' word; got: \
         {footer_bottom:?}",
    );
}
