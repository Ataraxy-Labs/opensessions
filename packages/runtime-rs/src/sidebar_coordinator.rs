#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarLifecycle {
    Idle,
    Warming,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarResizeAuthority {
    None,
    UserDrag,
    ClientResizeSync,
    ProgrammaticAdjust,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarCoordinatorState {
    pub mode: String,
    pub visible: bool,
    pub initializing: bool,
    pub init_label: String,
    pub width: u32,
    pub lifecycle: SidebarLifecycle,
    pub resize_authority: SidebarResizeAuthority,
    pub suppress_width_reports_until: u64,
    pub client_resize_report_guard_until: u64,
    pub last_width_report_decision: Option<SidebarWidthReportDecision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarWidthReportDecision {
    pub accepted: bool,
    pub reason: String,
    pub previous_width: u32,
    pub next_width: u32,
    pub continued_drag: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarWidthReportInput {
    pub width: u32,
    pub session: Option<String>,
    pub window_id: Option<String>,
    pub is_active_session: bool,
    pub is_foreground_client: bool,
    pub is_current_window: bool,
    pub now: u64,
    pub suppress_ms: u64,
}

#[derive(Debug, Clone)]
pub struct SidebarCoordinator {
    width: u32,
    visible: bool,
    lifecycle: SidebarLifecycle,
    authority: SidebarResizeAuthority,
    suppress_width_reports_until: u64,
    client_resize_report_guard_until: u64,
    drag_owner_session: Option<String>,
    drag_owner_window_id: Option<String>,
    last_width_report_decision: Option<SidebarWidthReportDecision>,
    last_user_drag_at: Option<u64>,
}

impl SidebarCoordinator {
    pub fn new(width: u32) -> Self {
        Self {
            width,
            visible: false,
            lifecycle: SidebarLifecycle::Idle,
            authority: SidebarResizeAuthority::None,
            suppress_width_reports_until: 0,
            client_resize_report_guard_until: 0,
            drag_owner_session: None,
            drag_owner_window_id: None,
            last_width_report_decision: None,
            last_user_drag_at: None,
        }
    }

    pub fn state(&self) -> SidebarCoordinatorState {
        let mode = match (self.visible, self.lifecycle, self.authority) {
            (_, _, SidebarResizeAuthority::ClientResizeSync)
            | (_, _, SidebarResizeAuthority::ProgrammaticAdjust)
            | (_, _, SidebarResizeAuthority::UserDrag) => "resizing",
            (false, _, _) => "hidden",
            (true, SidebarLifecycle::Warming, _) => "warming",
            (true, SidebarLifecycle::Ready, _) => "ready",
            (true, SidebarLifecycle::Idle, _) => "ready",
        };
        let init_label = match mode {
            "resizing" => "adjusting…",
            "warming" => "warming up…",
            _ => "",
        };

        SidebarCoordinatorState {
            mode: mode.to_string(),
            visible: self.visible,
            initializing: !init_label.is_empty(),
            init_label: init_label.to_string(),
            width: self.width,
            lifecycle: self.lifecycle,
            resize_authority: self.authority,
            suppress_width_reports_until: self.suppress_width_reports_until,
            client_resize_report_guard_until: self.client_resize_report_guard_until,
            last_width_report_decision: self.last_width_report_decision.clone(),
        }
    }

    pub fn begin_warmup(&mut self) {
        self.visible = true;
        self.lifecycle = SidebarLifecycle::Warming;
        self.authority = SidebarResizeAuthority::None;
        self.clear_drag_owner();
    }

    pub fn warmup_done(&mut self) {
        self.visible = true;
        self.lifecycle = SidebarLifecycle::Ready;
        if self.authority == SidebarResizeAuthority::None {
            self.clear_drag_owner();
        }
    }

    pub fn mark_ready(&mut self) {
        self.visible = true;
        self.lifecycle = SidebarLifecycle::Ready;
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.lifecycle = SidebarLifecycle::Idle;
        self.authority = SidebarResizeAuthority::None;
        self.clear_drag_owner();
    }

    pub fn begin_client_resize_sync(&mut self, suppress_until: u64, guard_until: u64) {
        self.visible = true;
        self.authority = SidebarResizeAuthority::ClientResizeSync;
        self.suppress_width_reports_until = self.suppress_width_reports_until.max(suppress_until);
        self.client_resize_report_guard_until =
            self.client_resize_report_guard_until.max(guard_until);
        self.clear_drag_owner();
    }

    pub fn finish_client_resize_sync(&mut self) {
        if self.authority == SidebarResizeAuthority::ClientResizeSync {
            self.authority = SidebarResizeAuthority::None;
        }
    }

    pub fn begin_programmatic_adjustment(&mut self) {
        self.visible = true;
        self.authority = SidebarResizeAuthority::ProgrammaticAdjust;
        self.clear_drag_owner();
    }

    pub fn finish_programmatic_adjustment(&mut self) {
        if self.authority == SidebarResizeAuthority::ProgrammaticAdjust {
            self.authority = SidebarResizeAuthority::None;
        }
    }

    pub fn finish_user_drag(&mut self) {
        if self.authority == SidebarResizeAuthority::UserDrag {
            self.authority = SidebarResizeAuthority::None;
        }
        self.last_user_drag_at = None;
        self.clear_drag_owner();
    }

    pub fn suppress_width_reports(&mut self, until: u64) {
        self.suppress_width_reports_until = self.suppress_width_reports_until.max(until);
    }

    pub fn note_client_resize_guard(&mut self, until: u64) {
        self.client_resize_report_guard_until = self.client_resize_report_guard_until.max(until);
    }

    pub fn focus_context_changed(&mut self) {
        if self.authority != SidebarResizeAuthority::UserDrag {
            self.clear_drag_owner();
        }
    }

    pub fn apply_width_report(
        &mut self,
        report: SidebarWidthReportInput,
    ) -> SidebarWidthReportDecision {
        let decision = self.decide_width_report(&report);
        if decision.accepted {
            self.width = decision.next_width;
            self.authority = SidebarResizeAuthority::UserDrag;
            self.suppress_width_reports_until = self
                .suppress_width_reports_until
                .max(report.now.saturating_add(report.suppress_ms));
            self.drag_owner_session = report.session;
            self.drag_owner_window_id = report.window_id;
            self.last_user_drag_at = Some(report.now);
        }
        self.last_width_report_decision = Some(decision.clone());
        decision
    }

    /// Clear the UserDrag authority when no further width reports have arrived
    /// for `settle_ms` after the most recent accepted drag report. Mirrors
    /// `startTransientSidebarResize` + the FINISH_USER_DRAG `setTimeout` in
    /// `packages/runtime/src/server/index.ts` so the sidebar does not get
    /// stuck showing "adjusting…" forever once the user stops resizing.
    pub fn tick_user_drag_settle(&mut self, now: u64, settle_ms: u64) {
        if self.authority != SidebarResizeAuthority::UserDrag {
            return;
        }
        let Some(last) = self.last_user_drag_at else {
            return;
        };
        if now < last.saturating_add(settle_ms) {
            return;
        }
        self.authority = SidebarResizeAuthority::None;
        self.last_user_drag_at = None;
        self.clear_drag_owner();
    }

    fn decide_width_report(&self, report: &SidebarWidthReportInput) -> SidebarWidthReportDecision {
        let continued_drag = self.is_continuing_drag(report);
        if !self.visible {
            return self.reject("hidden");
        }
        if self.lifecycle != SidebarLifecycle::Ready {
            return self.reject("warming");
        }
        if !continued_drag && !report.is_active_session {
            return self.reject("inactive-session");
        }
        if !continued_drag && !report.is_foreground_client {
            return self.reject("background-sidebar");
        }
        if self.client_resize_report_guard_until > report.now {
            return self.reject("client-resize-guard");
        }
        if self.authority == SidebarResizeAuthority::ClientResizeSync {
            return self.reject("client-resize-sync");
        }
        if self.suppress_width_reports_until > report.now && !continued_drag {
            return self.reject("suppressed");
        }
        if report.width == self.width {
            return self.reject("same-width");
        }

        SidebarWidthReportDecision {
            accepted: true,
            reason: "accepted".to_string(),
            previous_width: self.width,
            next_width: report.width,
            continued_drag,
        }
    }

    fn is_continuing_drag(&self, report: &SidebarWidthReportInput) -> bool {
        self.authority == SidebarResizeAuthority::UserDrag
            && self.drag_owner_session.as_ref() == report.session.as_ref()
            && self.drag_owner_window_id.as_ref() == report.window_id.as_ref()
            && self.drag_owner_session.is_some()
            && self.drag_owner_window_id.is_some()
    }

    fn reject(&self, reason: &str) -> SidebarWidthReportDecision {
        SidebarWidthReportDecision {
            accepted: false,
            reason: reason.to_string(),
            previous_width: self.width,
            next_width: self.width,
            continued_drag: false,
        }
    }

    fn clear_drag_owner(&mut self) {
        self.drag_owner_session = None;
        self.drag_owner_window_id = None;
    }
}
