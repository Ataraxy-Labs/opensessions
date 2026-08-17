/**
 * opensessions plugin for Amp
 *
 * Reports agent status to the opensessions server via HTTP. When this plugin
 * is installed and the server is running, opensessions will skip its cloud
 * API / DTW WebSocket polling for the active thread and rely entirely on the
 * events emitted here.
 *
 * Install:
 *   Copy (or symlink) this file to ~/.config/amp/plugins/opensessions.ts
 *
 * Requires a recent Amp build that exposes `ctx.thread.id` on session.start.
 *
 * Event mapping:
 *   session.start → idle      (registers the thread + its project/session)
 *   agent.start   → running
 *   agent.end     → done | error | interrupted  (from event.status)
 *   tool.call     → tool-running
 *   tool.result   → error on failure, interrupted on cancel (no-op on success)
 *
 * Session identity:
 *   1. `tmux display-message -p '#S'` — works when Amp is launched inside a
 *      tmux pane managed by opensessions.
 *   2. `process.cwd()` — server resolves to a session via its dir→session map.
 *   Both are sent in every payload; the server picks whichever resolves.
 */

// @i-know-the-amp-plugin-api-is-wip-and-very-experimental-right-now
import type { PluginAPI } from "@ampcode/plugin";
import { appendFileSync, readFileSync, readdirSync } from "fs";
import { join } from "path";
import { homedir } from "os";

const PLUGIN_LOG_PATH = "/tmp/opensessions-plugin.log";
function plog(msg: string): void {
  try { appendFileSync(PLUGIN_LOG_PATH, `[${new Date().toISOString()}] ${msg}\n`); } catch {}
}

const DEFAULT_SERVER_PORT = 7391;
const RUST_SERVER_PORT_BASE = 22000;
const POST_TIMEOUT_MS = 750;
const RETRY_INITIAL_MS = 250;
const RETRY_MAX_MS = 2_000;
const RETRY_FOR_MS = 30_000;

type Status = "idle" | "running" | "tool-running" | "done" | "error" | "interrupted";

interface EventPayload {
  agent: "amp";
  status: Status;
  threadId?: string;
  threadName?: string;
  lastUserPrompt?: string;
  tmuxSession?: string;
  paneId?: string;
  projectDir: string;
  ts: number;
}

/**
 * Resolve a thread's title via the Amp cloud API. Recent Amp builds no longer
 * persist every thread to ~/.local/share/amp/threads/<id>.json, so we hit
 * GET <ampUrl>/api/threads/:id with the local apiKey instead.
 *
 * This is a one-shot fetch per thread — the plugin caches the result in
 * memory and includes it in every subsequent event POST.
 */
const SETTINGS_PATH = join(homedir(), ".config", "amp", "settings.json");
const SECRETS_PATH = join(homedir(), ".local", "share", "amp", "secrets.json");
const DEFAULT_AMP_URL = "https://ampcode.com";
const TITLE_FETCH_TIMEOUT_MS = 5_000;

function loadAmpUrl(): string {
  try {
    const raw = readFileSync(SETTINGS_PATH, "utf8");
    const settings = JSON.parse(raw) as { url?: unknown };
    if (typeof settings.url === "string" && settings.url.length > 0) {
      return settings.url.replace(/\/$/, "");
    }
  } catch {}
  return DEFAULT_AMP_URL;
}

function loadApiKey(ampUrl: string): string | null {
  try {
    const raw = readFileSync(SECRETS_PATH, "utf8");
    const secrets = JSON.parse(raw) as Record<string, unknown>;
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

const AMP_URL = loadAmpUrl();
const API_KEY = loadApiKey(AMP_URL);

async function fetchThreadTitle(threadId: string): Promise<string | null> {
  if (!API_KEY) return null;
  try {
    const res = await fetch(`${AMP_URL}/api/threads/${threadId}`, {
      headers: { Authorization: `Bearer ${API_KEY}` },
      signal: AbortSignal.timeout(TITLE_FETCH_TIMEOUT_MS),
    });
    if (!res.ok) return null;
    const body = (await res.json()) as { title?: unknown };
    return typeof body.title === "string" && body.title.length > 0 ? body.title : null;
  } catch {
    return null;
  }
}

/**
 * Port resolution — matches the tmux-scoped opensessions server namespace.
 * Rust servers use 22000+server_key.
 */
function hashServerKey(input: string): number {
  let hash = 0;
  const bytes = new TextEncoder().encode(input);
  for (let i = 0; i < bytes.length; i += 1) {
    hash = (hash + bytes[i] * (i + 1)) % 20000;
  }
  return hash;
}

function resolveServerUrls(): string[] {
  const urls: string[] = [];
  const add = (url: string | undefined): void => {
    if (url && !urls.includes(url)) urls.push(url);
  };

  add(process.env.OPENSESSIONS_URL);

  const explicit = Number.parseInt(process.env.OPENSESSIONS_PORT ?? "", 10);
  if (Number.isFinite(explicit) && explicit > 0) add(`http://127.0.0.1:${explicit}`);

  const explicitKey = process.env.OPENSESSIONS_SERVER_KEY?.trim();
  if (explicitKey) {
    const key = Number.parseInt(explicitKey, 10);
    if (Number.isFinite(key)) {
      add(`http://127.0.0.1:${RUST_SERVER_PORT_BASE + key}`);
    }
  }

  const tmux = process.env.TMUX?.trim();
  if (tmux) {
    const socketPath = tmux.split(",", 1)[0];
    if (socketPath) {
      const key = hashServerKey(socketPath);
      add(`http://127.0.0.1:${RUST_SERVER_PORT_BASE + key}`);
    }
  }

  // Broadcast agent telemetry to every opensessions server currently known on
  // this machine. Each server maps projectDir/tmuxSession against its own tmux
  // sessions and no-ops events for folders it does not own.
  try {
    for (const entry of readdirSync("/tmp")) {
      const match = /^opensessions\.(\d+)\.pid$/.exec(entry);
      if (!match) continue;
      if (!pidFileIsAlive(join("/tmp", entry))) continue;
      const key = Number.parseInt(match[1], 10);
      if (!Number.isFinite(key)) continue;
      add(`http://127.0.0.1:${RUST_SERVER_PORT_BASE + key}`);
    }
  } catch {}

  if (urls.length === 0) add(`http://127.0.0.1:${DEFAULT_SERVER_PORT}`);
  return urls;
}

function pidFileIsAlive(path: string): boolean {
  try {
    const pid = Number.parseInt(readFileSync(path, "utf8").trim(), 10);
    if (!Number.isFinite(pid) || pid <= 0) return false;
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

function serverUrls(): string[] {
  // Resolve at send time, not plugin-load time. Amp plugin processes can live
  // across opensessions restarts and tmux env changes; stale endpoints during
  // a restart should not strand events until Amp itself is restarted.
  return resolveServerUrls();
}

let preferredServerUrl: string | undefined;

plog(`plugin loaded endpoints=${serverUrls().join(",")} ampUrl=${AMP_URL} apiKey=${API_KEY ? "set" : "missing"} tmux=${process.env.TMUX ?? "none"} cwd=${process.cwd()} pid=${process.pid}`);

async function resolveTmuxSession($: PluginAPI["$"]): Promise<string | null> {
  try {
    const result = await $`tmux display-message -p '#S'`;
    const name = result.stdout.trim();
    return name.length > 0 ? name : null;
  } catch {
    return null;
  }
}

async function resolveTmuxPane($: PluginAPI["$"]): Promise<string | null> {
  try {
    const result = await $`tmux display-message -p '#{pane_id}'`;
    const paneId = result.stdout.trim();
    return paneId.length > 0 ? paneId : null;
  } catch {
    return null;
  }
}

type PendingPayload = EventPayload & { firstAttemptTs: number; retryDelayMs: number };

const pendingByThread = new Map<string, PendingPayload>();
let retryTimer: ReturnType<typeof setTimeout> | undefined;

function pendingKey(payload: EventPayload): string {
  return payload.threadId ?? `${payload.projectDir}:${payload.tmuxSession ?? ""}`;
}

function scheduleRetry(): void {
  if (retryTimer || pendingByThread.size === 0) return;
  const nextDelay = Math.min(
    ...Array.from(pendingByThread.values()).map((payload) => payload.retryDelayMs),
  );
  retryTimer = setTimeout(() => {
    retryTimer = undefined;
    void flushPending();
  }, nextDelay);
}

async function flushPending(): Promise<void> {
  const now = Date.now();
  for (const [key, payload] of Array.from(pendingByThread.entries())) {
    if (now - payload.firstAttemptTs > RETRY_FOR_MS) {
      pendingByThread.delete(key);
      plog(`retry drop status=${payload.status} thread=${payload.threadId?.slice(0, 8)} ageMs=${now - payload.firstAttemptTs}`);
      continue;
    }
    const { firstAttemptTs, retryDelayMs, ...eventPayload } = payload;
    if (await postOnce(eventPayload)) {
      pendingByThread.delete(key);
    } else {
      pendingByThread.set(key, {
        ...payload,
        retryDelayMs: Math.min(retryDelayMs * 2, RETRY_MAX_MS),
      });
    }
  }
  scheduleRetry();
}

async function postOnce(payload: EventPayload): Promise<boolean> {
  const urls = serverUrls();
  const candidates = preferredServerUrl
    ? [preferredServerUrl, ...urls.filter((url) => url !== preferredServerUrl)]
    : urls;
  let lastError: unknown;
  for (const serverUrl of candidates) {
    const endpoint = `${serverUrl}/api/agent-event`;
    try {
      const res = await fetch(endpoint, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
        signal: AbortSignal.timeout(POST_TIMEOUT_MS),
      });
      if (res.status === 204) {
        preferredServerUrl = serverUrl;
        plog(`POST endpoint=${endpoint} status=${payload.status} thread=${payload.threadId?.slice(0, 8)} name=${payload.threadName ?? "-"} -> ${res.status}`);
        return true;
      }
      plog(`POST endpoint=${endpoint} status=${payload.status} thread=${payload.threadId?.slice(0, 8)} name=${payload.threadName ?? "-"} -> ${res.status}`);
    } catch (err) {
      lastError = err;
      plog(`POST endpoint=${endpoint} status=${payload.status} thread=${payload.threadId?.slice(0, 8)} ERROR ${String(err)}`);
    }
  }
  plog(`POST status=${payload.status} thread=${payload.threadId?.slice(0, 8)} failed all endpoints last=${String(lastError)}`);
  return false;
}

async function post(payload: EventPayload): Promise<void> {
  if (await postOnce(payload)) return;

  const key = pendingKey(payload);
  const existing = pendingByThread.get(key);
  pendingByThread.set(key, {
    ...payload,
    firstAttemptTs: existing?.firstAttemptTs ?? Date.now(),
    retryDelayMs: RETRY_INITIAL_MS,
  });
  plog(`retry queued status=${payload.status} thread=${payload.threadId?.slice(0, 8)} key=${key} pending=${pendingByThread.size}`);
  scheduleRetry();
}

export default function (amp: PluginAPI) {
  const projectDir = process.cwd();
  let tmuxSession: string | null = null;
  let paneId: string | null = null;

  // Resolve tmux session eagerly so we have it ready for the first event.
  resolveTmuxSession(amp.$).then((name) => {
    tmuxSession = name;
  });
  resolveTmuxPane(amp.$).then((id) => {
    paneId = id;
  });

  /**
   * ctx.thread.id is documented as "available when in the current invocation
   * context" — in practice it's present for session.start and tool.call but
   * often missing for agent.start, agent.end, and tool.result. The plugin
   * process is shared across concurrent threads, so we can't just stash a
   * single "current threadId" globally. Instead we correlate tool.result
   * with the preceding tool.call via toolUseID, and fall back to whatever
   * ctx.thread.id gives us otherwise.
   */
  const threadByToolUseID = new Map<string, string>();
  const lastPromptByThread = new Map<string, string>();
  let lastKnownThreadId: string | undefined;

  // Title cache. On first encounter of a thread we fire an async cloud fetch
  // so subsequent events carry the title. Titles are typed shortly after the
  // thread starts, so one retry on null is worth it — the first event is
  // usually session.start, before Amp has generated a title.
  const titleCache = new Map<string, string>();
  const titleInFlight = new Set<string>();

  const kickTitleFetch = (threadId: string): void => {
    if (titleCache.has(threadId) || titleInFlight.has(threadId)) return;
    titleInFlight.add(threadId);
    void fetchThreadTitle(threadId).then((title) => {
      titleInFlight.delete(threadId);
      if (title) {
        titleCache.set(threadId, title);
        plog(`title cached thread=${threadId.slice(0, 8)} title=${JSON.stringify(title)}`);
      }
    });
  };

  const resolveTitle = (threadId: string | undefined): string | undefined => {
    if (!threadId) return undefined;
    const cached = titleCache.get(threadId);
    if (cached) return cached;
    kickTitleFetch(threadId);
    return undefined;
  };

  const rememberThreadId = (threadId: string | undefined): void => {
    if (threadId) lastKnownThreadId = threadId;
  };

  const send = async (status: Status, threadId: string | undefined, lastUserPrompt?: string): Promise<void> => {
    rememberThreadId(threadId);
    const tid = threadId ?? lastKnownThreadId;
    if (tid && lastUserPrompt) lastPromptByThread.set(tid, lastUserPrompt);
    await post({
      agent: "amp",
      status,
      threadId: tid,
      threadName: resolveTitle(tid),
      lastUserPrompt: lastUserPrompt ?? (tid ? lastPromptByThread.get(tid) : undefined),
      tmuxSession: tmuxSession ?? undefined,
      paneId: paneId ?? undefined,
      projectDir,
      ts: Date.now(),
    });
  };

  amp.on("session.start", async (event, ctx) => {
    if (!tmuxSession) tmuxSession = await resolveTmuxSession(ctx.$);
    if (!paneId) paneId = await resolveTmuxPane(ctx.$);
    await send("idle", event.thread?.id ?? ctx.thread?.id);
  });

  amp.on("agent.start", async (event, ctx) => {
    await send("running", ctx.thread?.id, event.message);
    return {};
  });

  amp.on("agent.end", async (event, ctx) => {
    const status: Status =
      event.status === "done" ? "done" :
      event.status === "error" ? "error" :
      "interrupted";
    await send(status, ctx.thread?.id, event.message);
  });

  amp.on("tool.call", async (event, ctx) => {
    const threadId = event.thread?.id ?? ctx.thread?.id;
    if (threadId && event.toolUseID) threadByToolUseID.set(event.toolUseID, threadId);
    await send("tool-running", threadId);
    return { action: "allow" };
  });

  amp.on("tool.result", async (event, ctx) => {
    // Recover threadId from the matching tool.call since tool.result doesn't
    // carry one on the event payload.
    const threadId = ctx.thread?.id ?? (event.toolUseID ? threadByToolUseID.get(event.toolUseID) : undefined);
    if (event.toolUseID) threadByToolUseID.delete(event.toolUseID);

    if (event.status === "error") {
      await send("error", threadId);
    } else if (event.status === "cancelled") {
      await send("interrupted", threadId);
    } else {
      // Tool finished successfully. The agent is now streaming the reply, so
      // flip back to "running" — otherwise the UI stays pinned at
      // "tool-running" until the next agent.end.
      await send("running", threadId);
    }
  });
}
