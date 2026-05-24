# 04 — Protocol and Types

The TS protocol lives in `packages/runtime/src/shared.ts` and `contracts/`.
Below is the **complete, frozen** wire format and its 1:1 mapping to Rust
serde structs. All field names use camelCase on the wire (matching the TS
source); use `#[serde(rename_all = "camelCase")]` on every struct.

## Connection

- URL: `ws://{SERVER_HOST}:{SERVER_PORT}/`  (no path used today)
- Default `SERVER_HOST = 127.0.0.1`, port resolved from `OPENSESSIONS_PORT`
  env or hashed from `$TMUX` socket path → `17000 + (hash % 20000)`.
- Single connection per pane. Auto-reconnect on close.
- Text frames only (JSON). No binary.

## Server → Client (`ServerMessage`)

Tagged union on `type` field.

### `state`

The full state broadcast. Sent on connect and after every change.

```ts
interface ServerState {
  type: "state";
  sessions: SessionData[];
  focusedSession: string | null;
  currentSession: string | null;
  theme: string | undefined;          // theme name
  sessionFilter: SessionFilterMode | undefined;
  sidebarWidth: number;
  initializing: boolean;
  initLabel?: string;
  ts: number;                         // ms epoch
}
```

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerState {
    pub sessions: Vec<SessionData>,
    pub focused_session: Option<String>,
    pub current_session: Option<String>,
    pub theme: Option<String>,
    pub session_filter: Option<SessionFilterMode>,
    pub sidebar_width: u32,
    pub initializing: bool,
    #[serde(default)]
    pub init_label: Option<String>,
    pub ts: u64,
}
```

### `focus`

```ts
{ type: "focus"; focusedSession: string | null; currentSession: string | null }
```

```rust
pub struct FocusUpdate {
    pub focused_session: Option<String>,
    pub current_session: Option<String>,
}
```

### `resize`

```ts
{ type: "resize"; width: number }
```

### `quit`

```ts
{ type: "quit" }
```
Server is shutting down; the client should `terminal.show_cursor()` + exit cleanly.

### `your-session`

```ts
{ type: "your-session"; name: string; clientTty: string | null }
```
Tells this client which mux session it currently belongs to. Used to identify
"which row is mine?" in the list.

### `re-identify`

```ts
{ type: "re-identify" }
```
Server lost track of who this client is (e.g., after a tmux session move).
Client should re-send an `identify-pane` command.

### Master enum

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ServerMessage {
    State(ServerState),
    Focus(FocusUpdate),
    Resize { width: u32 },
    Quit,
    YourSession { name: String, client_tty: Option<String> },
    ReIdentify,
}
```

> ⚠ Note: TS uses `"your-session"` and `"re-identify"` (kebab-case). Confirm
> that `#[serde(rename_all = "kebab-case")]` on the **enum tag** matches.
> `State` → `state`, `Focus` → `focus`, `Resize` → `resize`, `Quit` → `quit`,
> `YourSession` → `your-session`, `ReIdentify` → `re-identify`. ✅

## Nested types

### `SessionData`

```ts
interface SessionData {
  name: string;
  createdAt: number;
  dir: string;
  branch: string;
  dirty: boolean;
  isWorktree: boolean;
  unseen: boolean;
  panes: number;
  ports: number[];
  localLinks: LocalLink[];
  windows: number;
  uptime: string;
  agentState: AgentEvent | null;
  agents: AgentEvent[];
  eventTimestamps: number[];
  metadata?: SessionMetadata | null;
}
```

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionData {
    pub name: String,
    pub created_at: u64,
    pub dir: String,
    pub branch: String,
    pub dirty: bool,
    pub is_worktree: bool,
    pub unseen: bool,
    pub panes: u32,
    pub ports: Vec<u32>,
    pub local_links: Vec<LocalLink>,
    pub windows: u32,
    pub uptime: String,
    pub agent_state: Option<AgentEvent>,
    pub agents: Vec<AgentEvent>,
    pub event_timestamps: Vec<u64>,
    #[serde(default)]
    pub metadata: Option<SessionMetadata>,
}
```

### `LocalLink`

```ts
interface LocalLink {
  kind: "direct" | "portless";
  port: number;
  url: string;
  label: string;
}
```

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct LocalLink {
    pub kind: LocalLinkKind,
    pub port: u32,
    pub url: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocalLinkKind { Direct, Portless }
```

### `AgentEvent` & `AgentStatus`

```ts
type AgentStatus =
  "idle" | "running" | "tool-running" | "done" | "error"
  | "waiting" | "interrupted" | "stale";

type AgentLiveness = "alive" | "exited" | "unknown";

interface AgentEvent {
  agent: string;
  session: string;
  status: AgentStatus;
  ts: number;
  threadId?: string;
  threadName?: string;
  unseen?: boolean;
  paneId?: string;
  liveness?: AgentLiveness;
}
```

```rust
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum AgentStatus {
    Idle, Running, ToolRunning, Done, Error,
    Waiting, Interrupted, Stale,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentLiveness { Alive, Exited, Unknown }

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    pub agent: String,
    pub session: String,
    pub status: AgentStatus,
    pub ts: u64,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub thread_name: Option<String>,
    #[serde(default)]
    pub unseen: Option<bool>,
    #[serde(default)]
    pub pane_id: Option<String>,
    #[serde(default)]
    pub liveness: Option<AgentLiveness>,
}
```

### `SessionMetadata`, `MetadataStatus`, `MetadataProgress`, `MetadataLogEntry`

```rust
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MetadataTone { Neutral, Info, Success, Warn, Error }

#[derive(Debug, Clone, Deserialize)]
pub struct MetadataStatus {
    pub text: String,
    #[serde(default)]
    pub tone: Option<MetadataTone>,
    pub ts: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MetadataProgress {
    #[serde(default)]
    pub current: Option<u64>,
    #[serde(default)]
    pub total: Option<u64>,
    #[serde(default)]
    pub percent: Option<f64>,
    #[serde(default)]
    pub label: Option<String>,
    pub ts: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MetadataLogEntry {
    pub message: String,
    #[serde(default)]
    pub tone: Option<MetadataTone>,
    #[serde(default)]
    pub source: Option<String>,
    pub ts: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SessionMetadata {
    #[serde(default)]
    pub status: Option<MetadataStatus>,
    #[serde(default)]
    pub progress: Option<MetadataProgress>,
    #[serde(default)]
    pub logs: Vec<MetadataLogEntry>,
}
```

### `SessionFilterMode`

```rust
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionFilterMode { All, Active, Running }
```

## Client → Server (`ClientCommand`)

All 18 variants:

```ts
type ClientCommand =
  | { type: "switch-session"; name: string; clientTty?: string }
  | { type: "switch-index"; index: number }
  | { type: "new-session" }
  | { type: "hide-session"; name: string }
  | { type: "show-all-sessions" }
  | { type: "kill-session"; name: string }
  | { type: "reorder-session"; name: string; delta: -1 | 1 }
  | { type: "refresh" }
  | { type: "move-focus"; delta: -1 | 1 }
  | { type: "focus-session"; name: string }
  | { type: "mark-seen"; name: string }
  | { type: "dismiss-agent"; session: string; agent: string; threadId?: string }
  | { type: "set-theme"; theme: string }
  | { type: "set-filter"; filter: SessionFilterMode }
  | { type: "identify"; clientTty: string }
  | { type: "quit" }
  | { type: "identify-pane"; paneId: string; sessionName: string; windowId?: string }
  | { type: "focus-agent-pane"; session: string; agent: string; threadId?: string; threadName?: string }
  | { type: "kill-agent-pane"; session: string; agent: string; threadId?: string; threadName?: string }
  | { type: "report-width"; width: number };
```

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ClientCommand {
    SwitchSession {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        client_tty: Option<String>,
    },
    SwitchIndex { index: u32 },
    NewSession,
    HideSession { name: String },
    ShowAllSessions,
    KillSession { name: String },
    ReorderSession { name: String, delta: i8 }, // -1 | 1
    Refresh,
    MoveFocus { delta: i8 },
    FocusSession { name: String },
    MarkSeen { name: String },
    DismissAgent {
        session: String,
        agent: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
    },
    SetTheme { theme: String },
    SetFilter { filter: SessionFilterMode },
    Identify { client_tty: String },
    Quit,
    IdentifyPane {
        pane_id: String,
        session_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        window_id: Option<String>,
    },
    FocusAgentPane {
        session: String,
        agent: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_name: Option<String>,
    },
    KillAgentPane {
        session: String,
        agent: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_name: Option<String>,
    },
    ReportWidth { width: u32 },
}
```

## Codegen path (Phase 0)

To prevent type drift between TS and Rust, the recommended Phase 0 work:

1. Annotate every TS interface in `packages/runtime/src/shared.ts` and
   `contracts/agent.ts` with `ts-rs` (or `specta`).
2. Add a `pnpm run gen-types` script that emits Rust code into
   `apps/tui-rs/src/generated/protocol.rs`.
3. Wire it into the Cargo build via `build.rs` so a stale TS schema fails the
   Rust build immediately.

If we don't want runtime TS deps, alternatively use `typeshare` or hand-maintain
this file with a snapshot test in TS that asserts the wire format hasn't changed.

## Schema versioning (Phase 0 add)

Add **once** to the protocol, then never break it:

```ts
interface ProtocolHello {
  type: "hello";
  protocol: 1;
  serverVersion: string;
}
```

Server emits `hello` as the **first** message after the WS upgrade. Client
checks `protocol === EXPECTED_VERSION` and exits with a clear error if not.
This single hook lets us evolve the protocol later without silent breakage.

## Quit / shutdown semantics

Two paths from current TS client (preserve both in Rust):

1. **Primary**: `ws.send_text(json!({ "type": "quit" }))` — server processes,
   broadcasts `{type:"quit"}` to all clients, then `process.exit(0)`.
2. **Fallback HTTP**: `POST http://{HOST}:{PORT}/quit` on a fresh TCP
   connection. Used because Bun's WS buffer can lose the last message if the
   process tears down before the TCP send completes. **Keep this in Rust** —
   it's a real corner case the user would hit.
3. **Final timeout**: 500 ms after sending quit, force-exit if still alive.
