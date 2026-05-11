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
import { appendFileSync, readFileSync } from "fs";
import { join } from "path";
import { homedir } from "os";

const PLUGIN_LOG_PATH = "/tmp/opensessions-plugin.log";
function plog(msg: string): void {
  try { appendFileSync(PLUGIN_LOG_PATH, `[${new Date().toISOString()}] ${msg}\n`); } catch {}
}

const DEFAULT_SERVER_PORT = 7391;
const POST_TIMEOUT_MS = 3_000;

type Status = "idle" | "running" | "tool-running" | "done" | "error" | "interrupted";

interface EventPayload {
  agent: "amp";
  status: Status;
  threadId?: string;
  threadName?: string;
  tmuxSession?: string;
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
 * Port resolution — matches opensessions `packages/runtime/src/shared.ts`.
 * The server port is derived from the tmux socket path so that two users on
 * the same machine (or the same user with multiple tmux servers) don't fight
 * over the default port.
 */
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
  const tmux = process.env.TMUX?.trim();
  if (!tmux) return null;
  const socketPath = tmux.split(",", 1)[0];
  if (!socketPath) return null;
  return String(hashServerKey(socketPath));
}

function resolveServerPort(serverKey: string | null): number {
  const explicit = Number.parseInt(process.env.OPENSESSIONS_PORT ?? "", 10);
  if (Number.isFinite(explicit) && explicit > 0) return explicit;
  if (serverKey) return 17000 + Number.parseInt(serverKey, 10);
  return DEFAULT_SERVER_PORT;
}

function resolveTokenFile(serverKey: string | null): string {
  const explicit = process.env.OPENSESSIONS_TOKEN_FILE?.trim();
  if (explicit) return explicit;
  if (serverKey) return `/tmp/opensessions.${serverKey}.token`;
  return "/tmp/opensessions.token";
}

const SERVER_KEY = resolveServerKey();
const SERVER_URL = process.env.OPENSESSIONS_URL ?? `http://127.0.0.1:${resolveServerPort(SERVER_KEY)}`;
const ENDPOINT = `${SERVER_URL}/api/agent-event`;
const TOKEN_FILE = resolveTokenFile(SERVER_KEY);
const AUTH_TOKEN_HEADER = "x-opensessions-token";

/**
 * Reads the server's per-instance auth token from disk on every call so a
 * server restart that rotates the token doesn't require restarting Amp.
 * Returns null if the file is missing — `post()` then skips the request and
 * logs the skip rather than emitting an unauthenticated POST that would
 * 401-spam the server log.
 */
function readAuthToken(): string | null {
  try {
    const raw = readFileSync(TOKEN_FILE, "utf-8").trim();
    return raw.length > 0 ? raw : null;
  } catch {
    return null;
  }
}

plog(`plugin loaded endpoint=${ENDPOINT} ampUrl=${AMP_URL} apiKey=${API_KEY ? "set" : "missing"} tmux=${process.env.TMUX ?? "none"} cwd=${process.cwd()} pid=${process.pid}`);

async function resolveTmuxSession($: PluginAPI["$"]): Promise<string | null> {
  try {
    const result = await $`tmux display-message -p '#S'`;
    const name = result.stdout.trim();
    return name.length > 0 ? name : null;
  } catch {
    return null;
  }
}

async function post(payload: EventPayload): Promise<void> {
  const token = readAuthToken();
  if (!token) {
    // Server isn't running, or it's a pre-token build — drop the event
    // silently rather than 401-spam the server log. The next event will
    // retry; that's fine because agent-status events are inherently lossy
    // (they describe a state, not a transaction).
    plog(`POST status=${payload.status} thread=${payload.threadId?.slice(0, 8)} SKIP no-token (file=${TOKEN_FILE})`);
    return;
  }
  try {
    const res = await fetch(ENDPOINT, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        [AUTH_TOKEN_HEADER]: token,
      },
      body: JSON.stringify(payload),
      signal: AbortSignal.timeout(POST_TIMEOUT_MS),
    });
    plog(`POST status=${payload.status} thread=${payload.threadId?.slice(0, 8)} name=${payload.threadName ?? "-"} -> ${res.status}`);
  } catch (err) {
    plog(`POST status=${payload.status} thread=${payload.threadId?.slice(0, 8)} ERROR ${String(err)}`);
  }
}

export default function (amp: PluginAPI) {
  const projectDir = process.cwd();
  let tmuxSession: string | null = null;

  // Resolve tmux session eagerly so we have it ready for the first event.
  resolveTmuxSession(amp.$).then((name) => {
    tmuxSession = name;
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

  const send = async (status: Status, threadId: string | undefined): Promise<void> => {
    rememberThreadId(threadId);
    const tid = threadId ?? lastKnownThreadId;
    await post({
      agent: "amp",
      status,
      threadId: tid,
      threadName: resolveTitle(tid),
      tmuxSession: tmuxSession ?? undefined,
      projectDir,
      ts: Date.now(),
    });
  };

  amp.on("session.start", async (event, ctx) => {
    if (!tmuxSession) tmuxSession = await resolveTmuxSession(ctx.$);
    await send("idle", event.thread?.id ?? ctx.thread?.id);
  });

  amp.on("agent.start", async (_event, ctx) => {
    await send("running", ctx.thread?.id);
    return {};
  });

  amp.on("agent.end", async (event, ctx) => {
    const status: Status =
      event.status === "done" ? "done" :
      event.status === "error" ? "error" :
      "interrupted";
    await send(status, ctx.thread?.id);
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
