use opensessions_runtime::sidebar_coordinator::{
    SidebarCoordinator, SidebarResizeAuthority, SidebarWidthReportInput,
};

#[test]
fn sidebar_coordinator_starts_hidden_and_idle() {
    let coordinator = SidebarCoordinator::new(26);
    let state = coordinator.state();

    assert_eq!(state.mode, "hidden");
    assert!(!state.visible);
    assert!(!state.initializing);
    assert_eq!(state.init_label, "");
    assert_eq!(state.width, 26);
    assert_eq!(state.resize_authority, SidebarResizeAuthority::None);
}

#[test]
fn sidebar_coordinator_tracks_warmup_and_ready_lifecycle() {
    let mut coordinator = SidebarCoordinator::new(26);

    coordinator.begin_warmup();
    let warming = coordinator.state();
    assert_eq!(warming.mode, "warming");
    assert!(warming.visible);
    assert!(warming.initializing);
    assert_eq!(warming.init_label, "warming up…");

    coordinator.warmup_done();
    let ready = coordinator.state();
    assert_eq!(ready.mode, "ready");
    assert!(ready.visible);
    assert!(!ready.initializing);
    assert_eq!(ready.init_label, "");
}

#[test]
fn sidebar_coordinator_prioritizes_adjusting_over_warmup() {
    let mut coordinator = SidebarCoordinator::new(26);

    coordinator.begin_warmup();
    coordinator.begin_client_resize_sync(500, 700);
    let resizing = coordinator.state();
    assert_eq!(resizing.mode, "resizing");
    assert_eq!(resizing.init_label, "adjusting…");
    assert_eq!(
        resizing.resize_authority,
        SidebarResizeAuthority::ClientResizeSync
    );

    coordinator.finish_client_resize_sync();
    let warming = coordinator.state();
    assert_eq!(warming.mode, "warming");
    assert_eq!(warming.init_label, "warming up…");
}

#[test]
fn sidebar_coordinator_accepts_active_foreground_width_report() {
    let mut coordinator = SidebarCoordinator::new(26);
    coordinator.mark_ready();

    let decision = coordinator.apply_width_report(SidebarWidthReportInput {
        width: 30,
        session: Some("alpha".to_string()),
        window_id: Some("@1".to_string()),
        is_active_session: true,
        is_foreground_client: true,
        is_current_window: true,
        now: 100,
        suppress_ms: 500,
    });

    let state = coordinator.state();
    assert!(decision.accepted);
    assert_eq!(decision.reason, "accepted");
    assert_eq!(decision.previous_width, 26);
    assert_eq!(decision.next_width, 30);
    assert_eq!(state.width, 30);
    assert_eq!(state.mode, "resizing");
    assert_eq!(state.resize_authority, SidebarResizeAuthority::UserDrag);
}

#[test]
fn sidebar_coordinator_suppressed_reports_only_continue_current_drag_owner() {
    let mut coordinator = SidebarCoordinator::new(26);
    coordinator.mark_ready();

    let first = coordinator.apply_width_report(SidebarWidthReportInput {
        width: 30,
        session: Some("alpha".to_string()),
        window_id: Some("@1".to_string()),
        is_active_session: true,
        is_foreground_client: true,
        is_current_window: true,
        now: 100,
        suppress_ms: 500,
    });
    let continued = coordinator.apply_width_report(SidebarWidthReportInput {
        width: 32,
        session: Some("alpha".to_string()),
        window_id: Some("@1".to_string()),
        is_active_session: true,
        is_foreground_client: true,
        is_current_window: true,
        now: 200,
        suppress_ms: 500,
    });
    let rejected = coordinator.apply_width_report(SidebarWidthReportInput {
        width: 34,
        session: Some("alpha".to_string()),
        window_id: Some("@2".to_string()),
        is_active_session: true,
        is_foreground_client: true,
        is_current_window: true,
        now: 250,
        suppress_ms: 500,
    });

    assert!(first.accepted);
    assert!(continued.accepted);
    assert!(continued.continued_drag);
    assert!(!rejected.accepted);
    assert_eq!(rejected.reason, "suppressed");
    assert_eq!(coordinator.state().width, 32);
}

#[test]
fn sidebar_coordinator_rejects_warmup_and_client_resize_guard_reports() {
    let mut coordinator = SidebarCoordinator::new(26);

    coordinator.begin_warmup();
    let warmup = coordinator.apply_width_report(width_report(30, 100));
    coordinator.mark_ready();
    coordinator.note_client_resize_guard(400);
    let guarded = coordinator.apply_width_report(width_report(31, 300));

    assert!(!warmup.accepted);
    assert_eq!(warmup.reason, "warming");
    assert!(!guarded.accepted);
    assert_eq!(guarded.reason, "client-resize-guard");
}

#[test]
fn sidebar_coordinator_rejects_inactive_background_and_same_width_reports() {
    let mut coordinator = SidebarCoordinator::new(26);
    coordinator.mark_ready();

    let inactive = coordinator.apply_width_report(SidebarWidthReportInput {
        is_active_session: false,
        ..width_report(30, 100)
    });
    let background = coordinator.apply_width_report(SidebarWidthReportInput {
        is_foreground_client: false,
        ..width_report(30, 100)
    });
    let same_width = coordinator.apply_width_report(width_report(26, 100));

    assert_eq!(inactive.reason, "inactive-session");
    assert_eq!(background.reason, "background-sidebar");
    assert_eq!(same_width.reason, "same-width");
    assert_eq!(coordinator.state().width, 26);
}

#[test]
fn sidebar_coordinator_hide_resets_lifecycle_and_authority() {
    let mut coordinator = SidebarCoordinator::new(26);

    coordinator.begin_warmup();
    coordinator.hide();

    let state = coordinator.state();
    assert_eq!(state.mode, "hidden");
    assert!(!state.visible);
    assert!(!state.initializing);
    assert_eq!(state.resize_authority, SidebarResizeAuthority::None);
}

#[test]
fn sidebar_coordinator_suppression_windows_only_extend() {
    let mut coordinator = SidebarCoordinator::new(26);

    coordinator.suppress_width_reports(500);
    coordinator.suppress_width_reports(300);
    assert_eq!(coordinator.state().suppress_width_reports_until, 500);

    coordinator.suppress_width_reports(900);
    assert_eq!(coordinator.state().suppress_width_reports_until, 900);
}

#[test]
fn sidebar_coordinator_focus_context_change_preserves_drag_tail() {
    let mut coordinator = SidebarCoordinator::new(26);
    coordinator.mark_ready();
    coordinator.apply_width_report(width_report(30, 100));

    coordinator.focus_context_changed();
    coordinator.suppress_width_reports(400);
    let continued = coordinator.apply_width_report(SidebarWidthReportInput {
        width: 32,
        session: Some("alpha".to_string()),
        window_id: Some("@1".to_string()),
        is_active_session: false,
        is_foreground_client: false,
        is_current_window: false,
        now: 200,
        suppress_ms: 500,
    });
    let foreign = coordinator.apply_width_report(SidebarWidthReportInput {
        width: 34,
        session: Some("alpha".to_string()),
        window_id: Some("@2".to_string()),
        is_active_session: true,
        is_foreground_client: true,
        is_current_window: true,
        now: 250,
        suppress_ms: 500,
    });

    assert!(continued.accepted);
    assert!(continued.continued_drag);
    assert!(!foreign.accepted);
    assert_eq!(foreign.reason, "suppressed");
    assert_eq!(coordinator.state().width, 32);
}

#[test]
fn sidebar_coordinator_user_drag_settles_after_grace_period() {
    // Mirrors apps/server-rs's setTimeout(USER_DRAG_SETTLE_MS) behavior in TS:
    // once a width report is accepted, authority becomes UserDrag and the
    // sidebar shows "adjusting…". After the settle window passes with no
    // further reports, the next tick must clear the drag so callers see
    // mode="ready" again. Without this the sidebar is permanently stuck in
    // "adjusting…" because nothing else fires FINISH_USER_DRAG.
    let mut coordinator = SidebarCoordinator::new(26);
    coordinator.mark_ready();
    let accept_at = 1_000;
    let settle_ms = 600;

    let decision = coordinator.apply_width_report(SidebarWidthReportInput {
        width: 30,
        session: Some("alpha".to_string()),
        window_id: Some("@1".to_string()),
        is_active_session: true,
        is_foreground_client: true,
        is_current_window: true,
        now: accept_at,
        suppress_ms: 500,
    });
    assert!(decision.accepted);
    assert_eq!(
        coordinator.state().resize_authority,
        SidebarResizeAuthority::UserDrag
    );

    // Within the settle window, drag must remain.
    coordinator.tick_user_drag_settle(accept_at + settle_ms - 1, settle_ms);
    assert_eq!(
        coordinator.state().resize_authority,
        SidebarResizeAuthority::UserDrag,
        "drag must persist while still within USER_DRAG_SETTLE_MS",
    );

    // After the settle window the next tick must finish the drag so callers
    // see mode=ready / initializing=false again.
    coordinator.tick_user_drag_settle(accept_at + settle_ms, settle_ms);
    let settled = coordinator.state();
    assert_eq!(settled.resize_authority, SidebarResizeAuthority::None);
    assert_eq!(settled.mode, "ready");
    assert!(!settled.initializing);
    assert_eq!(settled.init_label, "");
}

fn width_report(width: u32, now: u64) -> SidebarWidthReportInput {
    SidebarWidthReportInput {
        width,
        session: Some("alpha".to_string()),
        window_id: Some("@1".to_string()),
        is_active_session: true,
        is_foreground_client: true,
        is_current_window: true,
        now,
        suppress_ms: 500,
    }
}
