# 05 — State Management

Solid's reactive primitives map cleanly to plain Rust struct fields in an
immediate-mode renderer. Below is the complete state surface from
`apps/tui/src/index.tsx::App()` and its Rust equivalent.

## TS state surface (App component)

| TS primitive | Field |
|---|---|
| `createSignal<Theme>(...)` | `theme` |
| `createStore<SessionData[]>([])` | `sessions` |
| `createSignal<string \| null>(...)` | `focusedSession`, `currentSession`, `mySession` |
| `createSignal<boolean>` | `connected` |
| `createSignal<number>` | `spinIdx`, `terminalWidth`, `detailPanelHeight` |
| `createSignal<boolean>` | `isDetailResizeHover`, `isDetailResizing` |
| `createMemo` | `detailPanelSessionName`, `filteredSessions`, `focusedData` |
| `createSignal<SessionFilterMode>` | `sessionFilter` |
| `createSignal<boolean>` | `initializing` |
| `createSignal<string>` | `initLabel`, `flashMessage` |
| `createSignal<PanelFocus>` | `panelFocus` ("sessions" \| "agents") |
| `createSignal<number>` | `focusedAgentIdx` |
| `createSignal<Modal>` | `modal` ("none" \| "theme-picker" \| "confirm-kill") |
| `createSignal<string \| null>` | `killTarget` |
| `createSignal<string>` | `clientTty` |
| Plain mutable | `ws`, `startupFocusSynced`, `lastIdentifiedSessionName`, `lastIdentifiedWindowId`, `detailResizeStartY`, `detailResizeStartHeight`, `themeBeforePreview`, `flashTimer` |

## Rust `App` struct (proposed)

```rust
use std::collections::HashMap;
use std::time::Instant;

pub struct App {
    // --- Data from server ---
    pub sessions: Vec<SessionData>,
    pub focused_session: Option<String>,
    pub current_session: Option<String>,
    pub my_session: Option<String>,
    pub session_filter: SessionFilterMode,
    pub theme: Theme,
    pub initializing: bool,
    pub init_label: String,
    pub sidebar_width: u32,

    // --- Connection / WS ---
    pub connected: bool,
    pub ws_tx: mpsc::Sender<ClientCommand>,   // outbound queue
    pub last_identified_session: Option<String>,
    pub last_identified_window: Option<String>,

    // --- Local UI state ---
    pub panel_focus: PanelFocus,              // Sessions | Agents
    pub focused_agent_idx: usize,
    pub modal: Modal,                          // None | ThemePicker(ThemePickerState) | ConfirmKill(String)
    pub flash: Option<Flash>,                  // text + expiry Instant
    pub spin_idx: usize,
    pub detail_panel_height: u16,
    pub detail_panel_heights: HashMap<String, u16>,  // persisted per-session
    pub detail_resize: Option<DetailResizeState>,    // active drag
    pub theme_before_preview: Option<Theme>,
    pub client_tty: Option<String>,

    // --- Mux + system ---
    pub mux: MuxContext,                       // Tmux { pane_id, sdk_via_server } | Zellij { ... } | None
    pub startup_focus_synced: bool,
    pub terminal_width: u16,

    // --- Rendering hit-test cache ---
    pub click_zones: Vec<ClickZone>,           // populated each draw, consumed on click
    pub link_zones: Vec<(Rect, String)>,       // url click targets

    // --- Quit ---
    pub should_quit: bool,
}

pub enum PanelFocus { Sessions, Agents }

pub enum Modal {
    None,
    ThemePicker(ThemePickerState),
    ConfirmKill(String),  // session name
}

pub struct Flash { pub text: String, pub expires_at: Instant }

pub struct DetailResizeState {
    pub start_y: u16,
    pub start_height: u16,
}
```

## Memos → derived methods

Solid `createMemo` recomputes on dependency change. In Rust we just compute on
read (cheap, called once per frame). No memoization needed.

```rust
impl App {
    pub fn detail_panel_session_name(&self) -> Option<&str> {
        self.focused_session.as_deref()
            .or(self.my_session.as_deref())
    }

    pub fn filtered_sessions(&self) -> impl Iterator<Item = &SessionData> {
        let mode = self.session_filter;
        self.sessions.iter().filter(move |s| match mode {
            SessionFilterMode::All => true,
            SessionFilterMode::Active => !s.agents.is_empty() || s.agent_state.is_some(),
            SessionFilterMode::Running => matches!(
                s.agent_state.as_ref().map(|a| a.status),
                Some(AgentStatus::Running)
                    | Some(AgentStatus::ToolRunning)
                    | Some(AgentStatus::Waiting)
            ),
        })
    }

    pub fn focused_data(&self) -> Option<&SessionData> {
        let name = self.focused_session.as_deref()?;
        self.sessions.iter().find(|s| s.name == name)
    }

    pub fn has_running(&self) -> bool {
        self.sessions.iter().any(|s|
            matches!(s.agent_state.as_ref().map(|a| a.status), Some(AgentStatus::Running)))
    }
}
```

## Optimistic update pattern

The TS `switchToSession` updates local state **before** the server confirms
(critical for instant Tab-tab repeat feel). Mirror in Rust:

```rust
impl App {
    pub fn switch_to_session(&mut self, name: String) {
        self.my_session = Some(name.clone());
        self.current_session = Some(name.clone());
        self.focused_session = Some(name.clone());
        self.panel_focus = PanelFocus::Sessions;
        self.focused_agent_idx = 0;
        self.send(ClientCommand::SwitchSession { name, client_tty: None });
    }

    pub fn send(&self, cmd: ClientCommand) {
        // Non-blocking try_send; drop if full (server will reconcile via state broadcast)
        let _ = self.ws_tx.try_send(cmd);
    }
}
```

## Reconciliation

On `ServerMessage::State`, replace `App.sessions` wholesale (cheap; ~10–100 KB).
Don't try to diff — server is source of truth, just trust it.

```rust
impl App {
    pub fn handle_server_message(&mut self, msg: ServerMessage) {
        match msg {
            ServerMessage::State(s) => {
                self.sessions = s.sessions;
                // Don't blindly overwrite focused_session if user just optimistically moved
                if self.focused_session.is_none() {
                    self.focused_session = s.focused_session;
                }
                self.current_session = s.current_session.or(self.current_session.take());
                if let Some(t) = s.theme { self.set_theme_by_name(&t); }
                if let Some(f) = s.session_filter { self.session_filter = f; }
                self.sidebar_width = s.sidebar_width;
                self.initializing = s.initializing;
                self.init_label = s.init_label.unwrap_or_default();
                self.connected = true;
            }
            ServerMessage::Focus(f) => {
                self.focused_session = f.focused_session;
                self.current_session = f.current_session;
            }
            ServerMessage::Resize { width } => self.sidebar_width = width,
            ServerMessage::Quit => self.should_quit = true,
            ServerMessage::YourSession { name, client_tty } => {
                self.my_session = Some(name);
                self.client_tty = client_tty;
                self.startup_focus_synced = true;
            }
            ServerMessage::ReIdentify => self.send_identify_pane(),
        }
    }
}
```

## Why no Arc<Mutex<>>

- Tokio current-thread runtime → no parallelism.
- WS reader task sends `Event::Server(msg)` over `mpsc::channel`, doesn't touch
  `App` directly.
- Input task sends `Event::Input(crossterm_event)` over the same channel.
- Main loop owns `App` and pulls events.

→ Single owner, no locks, no `Arc`.
