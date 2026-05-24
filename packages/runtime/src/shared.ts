import type { AgentStatus, AgentEvent } from "./contracts/agent";
import type { MuxSessionInfo } from "./contracts/mux";
import type { SessionFilterMode } from "./config";

const DEFAULT_SERVER_PORT = 7391;
const DEFAULT_SERVER_HOST = "127.0.0.1";

function getTmuxSocketPath(): string | null {
  const tmux = process.env.TMUX?.trim();
  if (!tmux) return null;
  const [socketPath] = tmux.split(",", 1);
  return socketPath || null;
}

function hashServerKey(input: string): number {
  let hash = 0;
  for (let i = 0; i < input.length; i += 1) {
    hash = (hash + input.charCodeAt(i) * (i + 1)) % 20000;
  }
  return hash;
}

function resolveServerKey(): string | null {
  const explicit = process.env.OPENSESSIONS_SERVER_KEY?.trim();
  if (explicit) return explicit;

  const socketPath = getTmuxSocketPath();
  if (!socketPath) return null;

  return String(hashServerKey(socketPath));
}

function resolveServerPort(serverKey: string | null): number {
  const explicit = Number.parseInt(process.env.OPENSESSIONS_PORT ?? "", 10);
  if (Number.isFinite(explicit) && explicit > 0) return explicit;
  if (!serverKey) return DEFAULT_SERVER_PORT;
  return 17000 + Number.parseInt(serverKey, 10);
}

function resolvePidFile(serverKey: string | null): string {
  const explicit = process.env.OPENSESSIONS_PID_FILE?.trim();
  if (explicit) return explicit;
  if (!serverKey) return "/tmp/opensessions.pid";
  return `/tmp/opensessions.${serverKey}.pid`;
}

export const SERVER_KEY = resolveServerKey();
export const SERVER_PORT = resolveServerPort(SERVER_KEY);
export const SERVER_HOST = process.env.OPENSESSIONS_HOST?.trim() || DEFAULT_SERVER_HOST;
// Address that local in-process hooks use to reach the server. Always
// loopback — remote callers should build their own URL pointing at
// whichever address SERVER_HOST is bound to.
export const LOCAL_CLIENT_HOST = "127.0.0.1";
export const PID_FILE = resolvePidFile(SERVER_KEY);
export const SERVER_IDLE_TIMEOUT_MS = 30_000;
export const STUCK_RUNNING_TIMEOUT_MS = 3 * 60 * 1000;
export const PROTOCOL_VERSION = 1;

export interface ProtocolHello {
  type: "hello";
  protocol: typeof PROTOCOL_VERSION;
  serverVersion: string;
}

export interface LocalLink {
  kind: "direct" | "portless";
  port: number;
  url: string;
  label: string;
}

export interface SessionData {
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

export interface ServerState {
  type: "state";
  sessions: SessionData[];
  focusedSession: string | null;
  currentSession: string | null;
  theme: string | undefined;
  sessionFilter: SessionFilterMode | undefined;
  sidebarWidth: number;
  initializing: boolean;
  initLabel?: string;
  ts: number;
}

export interface FocusUpdate {
  type: "focus";
  focusedSession: string | null;
  currentSession: string | null;
}

export interface ResizeNotify {
  type: "resize";
  width: number;
}

export interface QuitNotify {
  type: "quit";
}

export interface YourSession {
  type: "your-session";
  name: string;
  clientTty: string | null;
}

export interface ReIdentify {
  type: "re-identify";
}

export type ServerMessage = ProtocolHello | ServerState | FocusUpdate | ResizeNotify | QuitNotify | YourSession | ReIdentify;

// --- Programmatic metadata (agent/script-pushed) ---

export type MetadataTone = "neutral" | "info" | "success" | "warn" | "error";

export interface MetadataStatus {
  text: string;
  tone?: MetadataTone;
  ts: number;
}

export interface MetadataProgress {
  current?: number;
  total?: number;
  percent?: number;
  label?: string;
  ts: number;
}

export interface MetadataLogEntry {
  message: string;
  tone?: MetadataTone;
  source?: string;
  ts: number;
}

export interface SessionMetadata {
  status: MetadataStatus | null;
  progress: MetadataProgress | null;
  logs: MetadataLogEntry[];
}

export type ClientCommand =
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

// Catppuccin Mocha palette
export const C = {
  blue: "#89b4fa",
  lavender: "#b4befe",
  pink: "#cba6f7",
  mauve: "#cba6f7",
  yellow: "#f9e2af",
  green: "#a6e3a1",
  red: "#f38ba8",
  peach: "#fab387",
  teal: "#94e2d5",
  sky: "#89dceb",
  text: "#cdd6f4",
  subtext0: "#a6adc8",
  subtext1: "#bac2de",
  overlay0: "#6c7086",
  overlay1: "#7f849c",
  surface0: "#313244",
  surface1: "#45475a",
  surface2: "#585b70",
  base: "#1e1e2e",
  mantle: "#181825",
  crust: "#11111b",
} as const;

export const STATUS_COLORS: Record<AgentStatus, string> = {
  idle: C.surface2,
  running: C.yellow,
  "tool-running": C.sky,
  done: C.green,
  error: C.red,
  waiting: C.blue,
  interrupted: C.peach,
  stale: C.yellow,
};

export const STATUS_ICONS: Record<AgentStatus, string> = {
  idle: "○",
  running: "●",
  "tool-running": "⚙",
  done: "✓",
  error: "✗",
  waiting: "◉",
  interrupted: "⚠",
  stale: "⚠",
};
