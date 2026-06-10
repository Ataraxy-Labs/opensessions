use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarLifecycle {
    Idle,
    Warming,
    Ready,
    Closing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarCoordinatorState {
    pub mode: String,
    pub visible: bool,
    pub initializing: bool,
    pub init_label: String,
    pub width: u32,
    pub lifecycle: SidebarLifecycle,
}

#[derive(Debug, Clone)]
pub struct SidebarCoordinator {
    width: u32,
    visible: bool,
    lifecycle: SidebarLifecycle,
    warmup_until: Option<u64>,
    pending_warmup_windows: HashSet<String>,
    hidden_by_user: bool,
}

impl SidebarCoordinator {
    pub fn new(width: u32) -> Self {
        Self {
            width,
            visible: false,
            lifecycle: SidebarLifecycle::Idle,
            warmup_until: None,
            pending_warmup_windows: HashSet::new(),
            hidden_by_user: false,
        }
    }

    pub fn state(&self) -> SidebarCoordinatorState {
        let mode = match (self.visible, self.lifecycle) {
            (true, SidebarLifecycle::Closing) => "closing",
            (false, _) => "hidden",
            (true, SidebarLifecycle::Warming) => "warming",
            (true, SidebarLifecycle::Ready | SidebarLifecycle::Idle) => "ready",
        };
        let init_label = match mode {
            "warming" => "warming up…",
            "closing" => "closing…",
            _ => "",
        };

        SidebarCoordinatorState {
            mode: mode.to_string(),
            visible: self.visible,
            initializing: !init_label.is_empty(),
            init_label: init_label.to_string(),
            width: self.width,
            lifecycle: self.lifecycle,
        }
    }

    pub fn set_width(&mut self, width: u32) {
        self.width = width;
    }

    pub fn begin_warmup(&mut self) {
        if self.is_closing() {
            return;
        }
        self.visible = true;
        self.lifecycle = SidebarLifecycle::Warming;
        self.warmup_until = None;
        self.pending_warmup_windows.clear();
        self.hidden_by_user = false;
    }

    pub fn begin_warmup_until(&mut self, until: u64) {
        self.begin_warmup();
        self.warmup_until = Some(until);
    }

    pub fn begin_warmup_for_windows<I>(&mut self, windows: I, until: u64)
    where
        I: IntoIterator<Item = String>,
    {
        self.begin_warmup_until(until);
        self.pending_warmup_windows = windows.into_iter().collect();
        if self.pending_warmup_windows.is_empty() {
            self.warmup_done();
        }
    }

    pub fn warmup_done(&mut self) {
        if self.is_closing() {
            return;
        }
        self.visible = true;
        self.lifecycle = SidebarLifecycle::Ready;
        self.warmup_until = None;
        self.pending_warmup_windows.clear();
        self.hidden_by_user = false;
    }

    pub fn mark_ready(&mut self) {
        self.warmup_done();
    }

    pub fn acknowledge_sidebar_connected(&mut self) -> bool {
        self.acknowledge_sidebar_window_connected(None)
    }

    pub fn acknowledge_sidebar_window_connected(&mut self, window_id: Option<&str>) -> bool {
        let before = self.state();
        if self.is_closing() {
            return false;
        }
        if self.hidden_by_user && !self.visible && self.lifecycle == SidebarLifecycle::Idle {
            return false;
        }
        self.visible = true;
        if let Some(window_id) = window_id {
            self.pending_warmup_windows.remove(window_id);
        }
        if self.lifecycle == SidebarLifecycle::Warming && self.pending_warmup_windows.is_empty() {
            self.warmup_done();
        } else if self.lifecycle != SidebarLifecycle::Warming {
            self.lifecycle = SidebarLifecycle::Ready;
        }
        before != self.state()
    }

    pub fn hide(&mut self) {
        if self.is_closing() {
            return;
        }
        self.visible = false;
        self.lifecycle = SidebarLifecycle::Idle;
        self.warmup_until = None;
        self.pending_warmup_windows.clear();
        self.hidden_by_user = true;
    }

    pub fn begin_closing(&mut self) {
        self.visible = true;
        self.lifecycle = SidebarLifecycle::Closing;
        self.warmup_until = None;
        self.pending_warmup_windows.clear();
        self.hidden_by_user = false;
    }

    pub fn tick_timers(&mut self, now: u64) -> bool {
        let before = self.state();
        if self.lifecycle == SidebarLifecycle::Warming
            && self.warmup_until.is_some_and(|until| now >= until)
        {
            self.lifecycle = SidebarLifecycle::Ready;
            self.warmup_until = None;
            self.pending_warmup_windows.clear();
            self.hidden_by_user = false;
        }
        before != self.state()
    }

    fn is_closing(&self) -> bool {
        self.lifecycle == SidebarLifecycle::Closing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acknowledging_last_pending_window_reports_ready_transition() {
        let mut coordinator = SidebarCoordinator::new(30);
        coordinator.begin_warmup_for_windows(["@1".to_string()], 10_000);

        let changed = coordinator.acknowledge_sidebar_window_connected(Some("@1"));

        assert!(changed);
        let state = coordinator.state();
        assert_eq!(state.lifecycle, SidebarLifecycle::Ready);
        assert!(!state.initializing);
    }

    #[test]
    fn acknowledging_already_ready_window_reports_no_transition() {
        let mut coordinator = SidebarCoordinator::new(30);
        coordinator.mark_ready();

        assert!(!coordinator.acknowledge_sidebar_window_connected(Some("@1")));
    }
}
