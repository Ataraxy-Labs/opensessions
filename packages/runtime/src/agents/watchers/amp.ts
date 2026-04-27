/**
 * Amp agent watcher — Cloud API + DTW WebSocket
 *
 * 1. **Polling** (discovery only): GET /api/threads?limit=20&after=<ts>
 *    every 10s to discover threads (id, title, projectDir). No detail fetches.
 *
 * 2. **WebSocket** (status source): For each discovered thread with a local
 *    mux session mapping, connects to the DTW WebSocket for real-time status.
 *    The WebSocket sends the current state immediately on connect plus
 *    real-time updates on every state transition.
 *
 * ## Credentials
 *
 * - ~/.local/share/amp/secrets.json — API key
 * - ~/.config/amp/settings.json — Amp URL (default: https://ampcode.com)
 *
 * ## WebSocket protocol
 *
 * 1. POST <ampUrl>/api/durable-thread-workers {threadId} → {wsToken}
 * 2. Connect to <workerUrl>/threads with subprotocol ["amp", wsToken]
 *    Worker URL: derived from ampUrl (production.ampworkers.com by default)
 * 3. Receive cf_agent_state messages:
 *    { type: "cf_agent_state", state: { status: "<state>", threadId, ... } }
 *
 * ## State mapping (Amp → our AgentStatus)
 *
 * - working, streaming, running_tools → "running"
 * - tool_use → "tool-running"
 * - awaiting_approval → "waiting"
 * - idle → "done" (if previously active), disconnect WS
 * - error → "error" (terminal), disconnect WS
 *
 * All network I/O is async to avoid blocking the server event loop.
 */

import { join } from "path";
import { homedir } from "os";
import type { AgentStatus } from "../../contracts/agent";
import { TERMINAL_STATUSES } from "../../contracts/agent";
import type { AgentWatcher, AgentWatcherContext } from "../../contracts/agent-watcher";

interface MessageState {
  type?: string;
  stopReason?: string;
}

interface Message {
  role?: string;
  state?: MessageState;
  interrupted?: boolean;
  content?: ContentItem[] | string;
}

interface ContentItem {
  type?: string;
  run?: {
    status?: string;
  };
}

interface ThreadSnapshot {
  status: AgentStatus;
  version: number;
  title?: string;
  projectDir?: string;
  lastListedAt: number;
}

interface WebSocketConnection {
  gen: number;
  phase: "connecting" | "open";
  ws: WebSocket | null;
}

/** API thread list item */
interface ApiThreadSummary {
  id: string;
  v: number;
  title?: string;
  updatedAt?: string;
  env?: {
    initial?: {
      trees?: Array<{ uri?: string }>;
    };
  };
}

/** API thread detail — used for non-DTW threads */
interface ApiThreadDetail {
  id: string;
  v: number;
  title?: string;
  messages?: Message[];
  env?: {
    initial?: {
      trees?: Array<{ uri?: string }>;
    };
  };
}

/** POST /api/durable-thread-workers response */
interface DtwTokenResponse {
  wsToken: string;
  threadVersion?: number;
  usesDtw?: boolean;
}

/** cf_agent_state WebSocket message */
interface CfAgentStateMessage {
  type?: string;
  state?: {
    status?: string;
    threadId?: string;
  };
}

const DTW_WS_BASE = "wss://production.ampworkers.com";
/** How long to wait before promoting quiet tool boundaries from running → waiting */
const TOOL_WAIT_MS = 3_000;
/** How long Amp can stay quiet before we consider the thread stale */
const STUCK_RUNNING_MS = 2 * 60 * 1000;
/** Only consider threads updated in the last 5 minutes */
const RECENT_MS = 5 * 60 * 1000;
const FETCH_TIMEOUT_MS = 10_000;
const WS_RETRY_MS = 5_000;
const USER_AGENT = "opensessions/0.1.0";

/**
 * Determine the agent status from the last message in a thread.
 *
 * Exported for independent testing. The watcher itself uses WebSocket
 * agent_state messages, not message parsing.
 */
export function determineStatus(lastMsg: { role?: string; state?: MessageState; interrupted?: boolean; content?: ContentItem[] | string } | null): AgentStatus {
  if (!lastMsg?.role) return "idle";

  if (lastMsg.role === "user") {
    if (hasToolResultRunStatus(lastMsg.content, "in-progress")) return "tool-running";
    return "running";
  }

  if (lastMsg.role === "assistant") {
    const state = lastMsg.state;
    if (!state || !state.type) return "running";

    if (state.type === "streaming") return "running";
    if (state.type === "cancelled") return "interrupted";

    if (state.type === "complete") {
      if (state.stopReason === "tool_use") return "running";
      if (state.stopReason === "end_turn") return "done";
      return "error";
    }

    return "running";
  }

  return "idle";
}

function hasToolResultRunStatus(content: Message["content"], status: string): boolean {
  return Array.isArray(content) && content.some((item) => item?.type === "tool_result" && item.run?.status === status);
}

/**
 * Map Amp's DTW agent state to our AgentStatus.
 *
 * Amp states: idle, working, streaming, tool_use, running_tools, awaiting_approval, error
 */
export function mapAmpState(ampState: string): AgentStatus | null {
  switch (ampState) {
    case "working":
    case "streaming":
    case "running_tools":
      return "running";
    case "tool_use":
      return "tool-running";
    case "awaiting_approval":
      return "waiting";
    case "idle":
      return "done";
    case "error":
      return "error";
    default:
      return null;
  }
}

/** Derive the DTW worker URL from the Amp server URL (matches Amp client logic). */
function deriveWorkerUrl(ampUrl: string): string {
  if (ampUrl.includes("staging.ampcodedev.org")) return "https://staging.ampworkers.com";
  return "https://production.ampworkers.com";
}

async function loadAmpUrl(): Promise<string> {
  try {
    const settingsPath = join(homedir(), ".config", "amp", "settings.json");
    const raw = await Bun.file(settingsPath).text();
    const settings = JSON.parse(raw);
    if (settings.url && typeof settings.url === "string") {
      const url = settings.url.replace(/\/$/, "");
      if (!url.startsWith("https://")) {
        console.warn(`[amp-watcher] Refusing non-HTTPS Amp URL: ${url}`);
        return "https://ampcode.com";
      }
      return url;
    }
  } catch {
  }
  return "https://ampcode.com";
}

async function loadApiKey(ampUrl: string): Promise<string | null> {
  try {
    const secretsPath = join(homedir(), ".local", "share", "amp", "secrets.json");
    const raw = await Bun.file(secretsPath).text();
    const secrets = JSON.parse(raw);

    const urlWithSlash = ampUrl.endsWith("/") ? ampUrl : `${ampUrl}/`;
    const urlWithoutSlash = ampUrl.replace(/\/$/, "");

    const key =
      secrets[`apiKey@${urlWithSlash}`] ??
      secrets[`apiKey@${urlWithoutSlash}`] ??
      secrets.apiKey;

    return typeof key === "string" && key.length > 0 ? key : null;
  } catch {
    return null;
  }
}

function extractProjectDir(thread: { env?: { initial?: { trees?: Array<{ uri?: string }> } } }): string | undefined {
  const uri = thread.env?.initial?.trees?.[0]?.uri ?? "";
  return uri.startsWith("file://") ? uri.slice(7) : undefined;
}

export class AmpAgentWatcher implements AgentWatcher {
  readonly name = "amp";

  private threads = new Map<string, ThreadSnapshot>();
  private wsConnections = new Map<string, WebSocketConnection>();
  private nonDtwThreads = new Set<string>();
  private wsRetryAfter = new Map<string, number>();
  private requestControllers = new Set<AbortController>();
  private lastPollAt: string | null = null;
  private pollTimer: ReturnType<typeof setInterval> | null = null;
  private ctx: AgentWatcherContext | null = null;
  private scanning = false;
  private seeded = false;
  private stopped = false;
  private lifecycle = 0;
  private wsGeneration = 0;

  private ampUrl: string | null = null;
  private apiKey: string | null = null;
  private workerUrl: string | null = null;

  _fetchTimeoutMs = FETCH_TIMEOUT_MS;
  _wsRetryMs = WS_RETRY_MS;

  _fetch: typeof fetch = globalThis.fetch.bind(globalThis);

  _WebSocket: { new(url: string, protocols?: string | string[]): WebSocket } = globalThis.WebSocket;

  start(ctx: AgentWatcherContext): void {
    this.stopped = false;
    this.ctx = ctx;
    const lifecycle = ++this.lifecycle;
    void this.initAndPoll(lifecycle);
  }

  stop(): void {
    this.stopped = true;
    this.lifecycle++;
    if (this.pollTimer) { clearInterval(this.pollTimer); this.pollTimer = null; }
    for (const controller of this.requestControllers) {
      try { controller.abort(); } catch {}
    }
    this.requestControllers.clear();
    for (const [, connection] of this.wsConnections) {
      if (!connection.ws) continue;
      try { connection.ws.close(); } catch {}
    }
    this.wsConnections.clear();
    this.wsRetryAfter.clear();
    this.nonDtwThreads.clear();
    this.threads.clear();
    this.lastPollAt = null;
    this.seeded = false;
    this.scanning = false;
    this.ampUrl = null;
    this.apiKey = null;
    this.workerUrl = null;
    this.ctx = null;
  }

  /** Trigger an immediate poll cycle (e.g., when sidebar is focused). */
  refresh(): void {
    if (!this.stopped && this.ctx) {
      void this.poll(this.lifecycle);
    }
  }

  private isActive(lifecycle = this.lifecycle): boolean {
    return !this.stopped && this.ctx !== null && this.lifecycle === lifecycle;
  }

  private resolveMuxSession(projectDir?: string): string | null {
    if (!this.ctx || !projectDir) return null;
    const session = this.ctx.resolveSession(projectDir);
    return session && session !== "unknown" ? session : null;
  }

  private scheduleRetry(threadId: string, now = Date.now()): void {
    this.wsRetryAfter.set(threadId, now + this._wsRetryMs);
  }

  private clearRetry(threadId: string): void {
    this.wsRetryAfter.delete(threadId);
  }

  private shouldRetry(threadId: string, now = Date.now()): boolean {
    return (this.wsRetryAfter.get(threadId) ?? 0) <= now;
  }

  private disconnectWebSocket(threadId: string): void {
    const connection = this.wsConnections.get(threadId);
    this.wsConnections.delete(threadId);
    this.clearRetry(threadId);
    if (connection?.ws) {
      try { connection.ws.close(); } catch {}
    }
  }

  private pruneDormantThreads(now: number): void {
    for (const [threadId, snapshot] of this.threads) {
      if (now - snapshot.lastListedAt <= RECENT_MS) continue;
      this.disconnectWebSocket(threadId);
      this.threads.delete(threadId);
    }
  }

  private async initAndPoll(lifecycle: number): Promise<void> {
    const ampUrl = await loadAmpUrl();
    if (!this.isActive(lifecycle)) return;
    this.ampUrl = ampUrl;
    this.workerUrl = deriveWorkerUrl(ampUrl);

    const apiKey = await loadApiKey(ampUrl);
    if (!this.isActive(lifecycle)) return;
    this.apiKey = apiKey;
    if (!apiKey) return;

    await this.poll(lifecycle);
    // No periodic polling — rely on on-demand refresh() calls (e.g., sidebar focus).
    // WebSocket handles real-time status for active threads.
  }

  private emitStatus(threadId: string, snapshot: ThreadSnapshot): boolean {
    if (!this.ctx || snapshot.status === "idle") return false;

    const session = this.resolveMuxSession(snapshot.projectDir);
    if (!session) return false;

    this.ctx.emit({
      agent: "amp",
      session,
      status: snapshot.status,
      ts: Date.now(),
      threadId,
      threadName: snapshot.title,
    });
    return true;
  }

  private async poll(lifecycle = this.lifecycle): Promise<void> {
    if (this.scanning || !this.isActive(lifecycle) || !this.ampUrl || !this.apiKey) return;
    this.scanning = true;
    const initialSeed = !this.seeded;
    const pollStartedAt = new Date().toISOString();

    try {
      const threads = await this.fetchThreadList(lifecycle);
      if (!threads) return;
      if (!this.isActive(lifecycle)) return;

      this.lastPollAt = pollStartedAt;
      const now = Date.now();

      for (const thread of threads) {
        const updatedAt = thread.updatedAt ? new Date(thread.updatedAt).getTime() : 0;
        if (now - updatedAt > RECENT_MS) continue;

        const projectDir = extractProjectDir(thread);
        const session = this.resolveMuxSession(projectDir);
        if (!session) continue;

        const title = thread.title || undefined;
        const current = this.threads.get(thread.id);

        if (current) {
          current.lastListedAt = now;
          const titleChanged = current.title !== title;
          const projectDirChanged = current.projectDir !== projectDir;
          if (titleChanged) current.title = title;
          if (projectDirChanged) current.projectDir = projectDir;

          // For non-DTW threads, fetch detail on version bump
          if (this.nonDtwThreads.has(thread.id)) {
            if (thread.v !== current.version) {
              await this.processNonDtwThread(thread.id, thread, now, lifecycle);
              if (!this.isActive(lifecycle)) return;
            } else if ((titleChanged || projectDirChanged) && this.seeded) {
              this.emitStatus(thread.id, current);
            }
            continue;
          }

          if ((titleChanged || projectDirChanged) && this.seeded) {
            this.emitStatus(thread.id, current);
          }
        } else {
          const snapshot: ThreadSnapshot = {
            status: "idle",
            version: thread.v,
            title,
            projectDir,
            lastListedAt: now,
          };
          this.threads.set(thread.id, snapshot);
        }

        if (!this.wsConnections.has(thread.id) && !this.nonDtwThreads.has(thread.id) && this.shouldRetry(thread.id, now)) {
          void this.connectWebSocket(thread.id, lifecycle);
        }
      }

      this.pruneDormantThreads(now);

      // Re-fetch detail for active non-DTW threads not in this poll's list
      // (the after= filter may have excluded them)
      if (this.seeded) {
        for (const [threadId, snapshot] of this.threads) {
          if (!this.nonDtwThreads.has(threadId)) continue;
          if (TERMINAL_STATUSES.has(snapshot.status) || snapshot.status === "idle") continue;
          if (snapshot.lastListedAt === now) continue;
          await this.processNonDtwThread(threadId, null, now, lifecycle);
          if (!this.isActive(lifecycle)) return;
        }
      }
    } finally {
      if (initialSeed && this.isActive(lifecycle)) {
        this.seeded = true;
        for (const [threadId, snapshot] of this.threads) {
          if (snapshot.status !== "idle") {
            this.emitStatus(threadId, snapshot);
          }
        }
      }
      this.scanning = false;
    }
  }

  private async connectWebSocket(threadId: string, lifecycle = this.lifecycle): Promise<void> {
    if (!this.isActive(lifecycle) || !this.ampUrl || !this.apiKey || !this.workerUrl) return;
    if (this.wsConnections.has(threadId)) return;
    if (!this.shouldRetry(threadId)) return;

    const snapshot = this.threads.get(threadId);
    if (!snapshot || !this.resolveMuxSession(snapshot.projectDir)) return;

    const gen = ++this.wsGeneration;
    this.wsConnections.set(threadId, { gen, phase: "connecting", ws: null });

    const dtwResult = await this.fetchDtwToken(threadId, lifecycle);
    const connection = this.wsConnections.get(threadId);
    if (!this.isActive(lifecycle) || !connection || connection.gen !== gen) return;

    if (!dtwResult || !dtwResult.wsToken) {
      this.wsConnections.delete(threadId);
      this.scheduleRetry(threadId);
      return;
    }

    if (dtwResult.usesDtw === false) {
      this.wsConnections.delete(threadId);
      this.nonDtwThreads.add(threadId);
      this.clearRetry(threadId);
      void this.processNonDtwThread(threadId, null, Date.now(), lifecycle);
      return;
    }

    const latestSnapshot = this.threads.get(threadId);
    if (!latestSnapshot || !this.resolveMuxSession(latestSnapshot.projectDir)) {
      this.wsConnections.delete(threadId);
      this.clearRetry(threadId);
      return;
    }

    try {
      const wsBase = this.workerUrl.replace(/^http:/, "ws:").replace(/^https:/, "wss:");
      const wsUrl = `${wsBase}/threads`;
      const ws = new this._WebSocket(wsUrl, ["amp", dtwResult.wsToken]);
      const current = this.wsConnections.get(threadId);
      if (!current || current.gen !== gen) {
        try { ws.close(); } catch {}
        return;
      }

      current.phase = "open";
      current.ws = ws;
      this.clearRetry(threadId);

      ws.onmessage = (event) => {
        this.handleWsMessage(threadId, gen, event.data);
      };

      ws.onclose = (event) => {
        const active = this.wsConnections.get(threadId);
        if (!active || active.gen !== gen || active.ws !== ws) return;
        this.wsConnections.delete(threadId);
        const code = (event as CloseEvent)?.code;
        if (code) console.warn(`[amp-watcher] WebSocket closed for ${threadId} (code ${code})`);
        if (this.isActive(lifecycle)) {
          this.scheduleRetry(threadId);
        } else {
          this.clearRetry(threadId);
        }
      };

      ws.onerror = () => {
        const active = this.wsConnections.get(threadId);
        if (!active || active.gen !== gen || active.ws !== ws) return;
        this.wsConnections.delete(threadId);
        console.warn(`[amp-watcher] WebSocket error for ${threadId}`);
        if (this.isActive(lifecycle)) {
          this.scheduleRetry(threadId);
        } else {
          this.clearRetry(threadId);
        }
        try { ws.close(); } catch {}
      };
    } catch {
      const active = this.wsConnections.get(threadId);
      if (active?.gen === gen) {
        this.wsConnections.delete(threadId);
        this.scheduleRetry(threadId);
      }
    }
  }

  private handleWsMessage(threadId: string, gen: number, data: unknown): void {
    if (!this.ctx) return;
    const connection = this.wsConnections.get(threadId);
    if (!connection || connection.gen !== gen) return;

    try {
      const raw = typeof data === "string" ? data : (data instanceof ArrayBuffer ? new TextDecoder().decode(data) : String(data));
      const msg: CfAgentStateMessage = JSON.parse(raw);

      if (msg.type !== "cf_agent_state" || !msg.state?.status) return;

      const status = mapAmpState(msg.state.status);
      if (!status) return;

      const snapshot = this.threads.get(threadId);
      if (!snapshot) {
        this.disconnectWebSocket(threadId);
        return;
      }

      if (snapshot.status === status) return;

      snapshot.status = status;
      this.emitStatus(threadId, snapshot);

      if (TERMINAL_STATUSES.has(status)) {
        this.disconnectWebSocket(threadId);
      }
    } catch {
    }
  }

  private async processNonDtwThread(
    threadId: string,
    summary: ApiThreadSummary | null,
    now: number,
    lifecycle = this.lifecycle,
  ): Promise<void> {
    const detail = await this.fetchThreadDetail(threadId, lifecycle);
    if (!detail) return;
    if (!this.isActive(lifecycle)) return;

    const messages = detail.messages ?? [];
    const lastMsg = messages.length > 0 ? messages[messages.length - 1] : null;
    const projectDir = extractProjectDir(detail);
    const title = detail.title || undefined;
    const version = detail.v ?? summary?.v ?? 0;
    const current = this.threads.get(threadId);

    if (current && current.version > version) return;

    const status = determineStatus(lastMsg ? { role: lastMsg.role, state: lastMsg.state, interrupted: lastMsg.interrupted, content: lastMsg.content } : null);
    const statusChanged = current?.status !== status;
    const titleChanged = current?.title !== title;
    const projectDirChanged = current?.projectDir !== projectDir;

    if (current && version === current.version && !statusChanged && !titleChanged && !projectDirChanged) {
      current.lastListedAt = now;
      return;
    }

    const snapshot: ThreadSnapshot = {
      status,
      version,
      title,
      projectDir,
      lastListedAt: now,
    };
    this.threads.set(threadId, snapshot);

    if (!this.seeded) return;

    if (statusChanged || titleChanged || projectDirChanged) this.emitStatus(threadId, snapshot);
  }

  private async fetchJson<T>(url: string, init: RequestInit = {}, lifecycle = this.lifecycle): Promise<T | null> {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), this._fetchTimeoutMs);
    this.requestControllers.add(controller);

    const headers = new Headers(init.headers);
    if (!headers.has("User-Agent")) headers.set("User-Agent", USER_AGENT);

    try {
      const res = await this._fetch(url, { ...init, headers, signal: controller.signal });
      if (!this.isActive(lifecycle) || !res.ok) return null;
      return await res.json() as T;
    } catch {
      return null;
    } finally {
      clearTimeout(timeout);
      this.requestControllers.delete(controller);
    }
  }

  private async fetchDtwToken(threadId: string, lifecycle = this.lifecycle): Promise<DtwTokenResponse | null> {
    // Try configured ampUrl first; fall back to ampcode.com if it doesn't support this endpoint
    const result = await this.fetchJson<DtwTokenResponse>(`${this.ampUrl}/api/durable-thread-workers`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${this.apiKey}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ threadId }),
    }, lifecycle);
    if (result) return result;

    // Fall back to ampcode.com if configured URL doesn't have this endpoint
    const fallbackUrl = "https://ampcode.com";
    if (this.ampUrl === fallbackUrl) return null;

    const fallbackKey = await loadApiKey(fallbackUrl);
    if (!fallbackKey) return null;

    return this.fetchJson<DtwTokenResponse>(`${fallbackUrl}/api/durable-thread-workers`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${fallbackKey}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ threadId }),
    }, lifecycle);
  }

  private async fetchThreadList(lifecycle = this.lifecycle): Promise<ApiThreadSummary[] | null> {
    let url = `${this.ampUrl}/api/threads?limit=20`;
    if (this.lastPollAt) url += `&after=${encodeURIComponent(this.lastPollAt)}`;
    return this.fetchJson<ApiThreadSummary[]>(url, {
      headers: { Authorization: `Bearer ${this.apiKey}` },
    }, lifecycle);
  }

  private async fetchThreadDetail(threadId: string, lifecycle = this.lifecycle): Promise<ApiThreadDetail | null> {
    return this.fetchJson<ApiThreadDetail>(`${this.ampUrl}/api/threads/${threadId}`, {
      headers: { Authorization: `Bearer ${this.apiKey}` },
    }, lifecycle);
  }
}
