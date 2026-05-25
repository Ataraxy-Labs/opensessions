use std::time::{Duration, Instant};

use opensessions_sidebar::app::App;
use opensessions_sidebar::input::{UiMouse, apply_ui_mouse};
use opensessions_sidebar::renderer::{HitTarget, compute_hit_map};
use opensessions_sidebar::snapshot::{buffer_bg_at, render_to_buffer};

const W: u16 = 35;
const H: u16 = 56;
const FLASH_BG: (u8, u8, u8) = (69, 71, 90); // SURFACE1 in renderer.rs

#[test]
fn clicking_a_session_arms_a_flash_for_150ms() {
    let mut app = App::reference_fixture("pane-attached-session-list");
    let hits = compute_hit_map(&app, W, H);
    let row = hits
        .iter()
        .position(|hit| matches!(hit, Some(HitTarget::Session(name)) if name == "learning"))
        .expect("learning session must have a clickable row");

    let before = Instant::now();
    apply_ui_mouse(
        &mut app,
        UiMouse::Click {
            x: 5,
            y: row as u16,
            width: W,
            height: H,
        },
    );

    let deadline = app
        .flash_deadline
        .expect("clicking a session must arm a flash deadline");

    let upper = before + Duration::from_millis(160);
    let lower = before + Duration::from_millis(140);
    assert!(
        deadline <= upper && deadline >= lower,
        "flash deadline must be ~150ms in the future, mirroring TS triggerFlash setTimeout"
    );

    assert_eq!(
        app.flash_target,
        Some(HitTarget::Session("learning".into())),
        "flash target must be the clicked session"
    );
}

#[test]
fn flashed_session_row_renders_with_highlight_background() {
    let mut app = App::reference_fixture("pane-attached-session-list");
    // Pick a non-focused, non-current session so its baseline bg is transparent.
    app.focused_session = Some("plane-pdf-word-formatting".into());
    app.current_session = Some("plane-pdf-word-formatting".into());

    app.flash_target = Some(HitTarget::Session("learning".into()));
    app.flash_deadline = Some(Instant::now() + Duration::from_millis(150));

    let buffer = render_to_buffer(&mut app, W, H);

    let hits = {
        let mut app2 = App::reference_fixture("pane-attached-session-list");
        app2.focused_session = Some("plane-pdf-word-formatting".into());
        app2.current_session = Some("plane-pdf-word-formatting".into());
        compute_hit_map(&app2, W, H)
    };
    let row = hits
        .iter()
        .position(|hit| matches!(hit, Some(HitTarget::Session(name)) if name == "learning"))
        .expect("learning session must have a clickable row");

    assert_eq!(
        buffer_bg_at(&buffer, 5, row as u16),
        Some(FLASH_BG),
        "flashed session row must render with SURFACE1 background"
    );
}

#[test]
fn expired_flash_does_not_paint_highlight_background() {
    let mut app = App::reference_fixture("pane-attached-session-list");
    app.focused_session = Some("plane-pdf-word-formatting".into());
    app.current_session = Some("plane-pdf-word-formatting".into());

    app.flash_target = Some(HitTarget::Session("learning".into()));
    app.flash_deadline = Some(Instant::now() - Duration::from_millis(1));

    let hits = compute_hit_map(&app, W, H);
    let row = hits
        .iter()
        .position(|hit| matches!(hit, Some(HitTarget::Session(name)) if name == "learning"))
        .expect("learning session must have a clickable row");

    let buffer = render_to_buffer(&mut app, W, H);
    let bg = buffer_bg_at(&buffer, 5, row as u16);

    assert_ne!(
        bg,
        Some(FLASH_BG),
        "expired flash must not paint SURFACE1 background"
    );
}

#[test]
fn live_tui_drives_flash_expiry_timer() {
    let main_rs = include_str!("../src/main.rs");

    assert!(
        main_rs.contains("flash_deadline"),
        "main.rs must observe app.flash_deadline so the flash highlight clears within ~150ms even without other events"
    );
}
