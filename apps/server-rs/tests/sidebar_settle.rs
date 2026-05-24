//! TDD wiring tests that verify apps/server-rs/src/lib.rs hooks the
//! [`SidebarCoordinator::tick_user_drag_settle`] helper into `snapshot_json`.
//!
//! Without this wiring the sidebar gets stuck in "adjusting…" forever after a
//! width report is accepted (see thread T-019dd34a-a0c7-77ab-8fa9-4cf23a739edc
//! and TS `startTransientSidebarResize` / FINISH_USER_DRAG `setTimeout`).

#[test]
fn lib_rs_invokes_tick_user_drag_settle_in_snapshot_json() {
    let lib_rs = include_str!("../src/lib.rs");

    assert!(
        lib_rs.contains("USER_DRAG_SETTLE_MS"),
        "apps/server-rs/src/lib.rs must declare a USER_DRAG_SETTLE_MS \
         constant matching the TS USER_DRAG_SETTLE_MS = 600"
    );
    assert!(
        lib_rs.contains("tick_user_drag_settle"),
        "apps/server-rs/src/lib.rs must call tick_user_drag_settle so the \
         coordinator clears the UserDrag authority once the settle window \
         passes; otherwise the sidebar shows 'adjusting…' forever"
    );
    assert!(
        lib_rs.contains("run_drag_settle_loop"),
        "apps/server-rs/src/lib.rs must run a background ticker that \
         broadcasts a fresh state once the user-drag settle window expires; \
         without it the websocket only emits a stuck 'adjusting…' state and \
         never streams the cleared snapshot to connected clients"
    );
}
