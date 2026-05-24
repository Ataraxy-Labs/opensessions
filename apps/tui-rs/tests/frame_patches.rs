use opensessions_sidebar::app::App;
use opensessions_sidebar::frame::{FrameDiff, apply_patch_rows, diff_rows, render_rows};

#[test]
fn first_render_produces_full_frame_rows() {
    let mut app = App::reference_fixture("pane-attached-session-list");

    let rows = render_rows(&mut app, 35, 56);

    assert_eq!(rows.width, 35);
    assert_eq!(rows.height, 56);
    assert_eq!(rows.rows.len(), 56);
    assert!(
        rows.rows.iter().all(|row| row.starts_with(b"\x1b[0m")),
        "every transport row must reset style before writing or clear-to-EOL leaks backgrounds"
    );
    assert!(
        rows.rows[1]
            .windows(b"Sessions".len())
            .any(|w| w == b"Sessions")
    );
}

#[test]
fn row_diff_only_includes_changed_lines_and_can_be_applied() {
    let mut before_app = App::reference_fixture("pane-attached-session-list");
    before_app.focused_session = Some("opensessions".into());
    let before = render_rows(&mut before_app, 35, 56);

    let mut after_app = App::reference_fixture("pane-attached-session-list");
    after_app.focused_session = Some("plane-pdf-word-formatting".into());
    let after = render_rows(&mut after_app, 35, 56);

    let diff = diff_rows(&before, &after);

    let FrameDiff::Patch { changed_rows, .. } = &diff else {
        panic!("same-sized render should produce a patch")
    };
    assert!(!changed_rows.is_empty());
    assert!(changed_rows.len() < after.rows.len());
    assert!(
        changed_rows
            .iter()
            .all(|(_, row)| row.starts_with(b"\x1b[0m")),
        "patch rows must reset style before writing or one focused row leaks into later clears"
    );

    let patched = apply_patch_rows(&before, &diff);
    assert_eq!(patched, after);
}

#[test]
fn row_diff_uses_full_frame_when_dimensions_change() {
    let mut app = App::reference_fixture("pane-attached-session-list");
    let before = render_rows(&mut app, 35, 56);
    let after = render_rows(&mut app, 35, 40);

    assert!(matches!(diff_rows(&before, &after), FrameDiff::Full(_)));
}
