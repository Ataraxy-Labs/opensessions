import type { ExtensionAPI, ExtensionContext } from "@mariozechner/pi-coding-agent";
import { readFileSync } from "fs";

interface PiRuntimePayload {
  pid: number;
  ppid: number;
  sessionId: string;
  sessionFile?: string;
  cwd: string;
  sessionName?: string;
  ts: number;
}

const DEFAULT_SERVER_PORT = 7391;
const HEARTBEAT_MS = 5_000;
const AUTH_TOKEN_HEADER = "x-opensessions-token";

/**
 * Mirror opensessions `packages/runtime/src/shared.ts` port resolution. The
 * server port is derived from a hash of the tmux socket path so concurrent
 * tmux servers on the same machine get independent opensessions servers.
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
const TOKEN_FILE = resolveTokenFile(SERVER_KEY);

function getServerUrl(): string {
  const explicit = process.env.OPENSESSIONS_URL;
  if (explicit) return explicit.replace(/\/+$/, "");
  return `http://127.0.0.1:${resolveServerPort(SERVER_KEY)}`;
}

/**
 * Reads the server's per-instance auth token from disk on every call so a
 * server restart that rotates the token is picked up without restarting pi.
 * Returns null if the file is missing — `post()` then skips the request,
 * matching the previous "opensessions may not be running yet" semantics.
 */
function readAuthToken(): string | null {
  try {
    const raw = readFileSync(TOKEN_FILE, "utf-8").trim();
    return raw.length > 0 ? raw : null;
  } catch {
    return null;
  }
}

export default function opensessionsRuntime(pi: ExtensionAPI) {
  let heartbeat: ReturnType<typeof setInterval> | null = null;
  let current: Omit<PiRuntimePayload, "ts" | "sessionName"> | null = null;

  function buildPayload(ctx: ExtensionContext): PiRuntimePayload {
    return {
      pid: process.pid,
      ppid: process.ppid,
      sessionId: ctx.sessionManager.getSessionId(),
      sessionFile: ctx.sessionManager.getSessionFile(),
      cwd: ctx.sessionManager.getCwd(),
      sessionName: pi.getSessionName(),
      ts: Date.now(),
    };
  }

  async function post(path: string, body: unknown): Promise<void> {
    const token = readAuthToken();
    if (!token) {
      // opensessions may not be running yet (or this is a pre-token build);
      // skip silently rather than 401-spam the server log. Heartbeat will
      // retry on the next tick.
      return;
    }
    try {
      await fetch(`${getServerUrl()}${path}`, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          [AUTH_TOKEN_HEADER]: token,
        },
        body: JSON.stringify(body),
      });
    } catch {
      // network blip or server bouncing; next heartbeat will retry
    }
  }

  function clearHeartbeat(): void {
    if (!heartbeat) return;
    clearInterval(heartbeat);
    heartbeat = null;
  }

  function startHeartbeat(ctx: ExtensionContext): void {
    clearHeartbeat();
    heartbeat = setInterval(() => {
      if (!current) {
        current = {
          pid: process.pid,
          ppid: process.ppid,
          sessionId: ctx.sessionManager.getSessionId(),
          sessionFile: ctx.sessionManager.getSessionFile(),
          cwd: ctx.sessionManager.getCwd(),
        };
      }
      void post("/api/runtime/pi/upsert", {
        ...current,
        sessionName: pi.getSessionName(),
        ts: Date.now(),
      } satisfies PiRuntimePayload);
    }, HEARTBEAT_MS);
  }

  pi.on("session_start", async (_event, ctx) => {
    const payload = buildPayload(ctx);
    current = {
      pid: payload.pid,
      ppid: payload.ppid,
      sessionId: payload.sessionId,
      sessionFile: payload.sessionFile,
      cwd: payload.cwd,
    };
    void post("/api/runtime/pi/upsert", payload);
    startHeartbeat(ctx);
  });

  pi.on("session_shutdown", async () => {
    clearHeartbeat();
    current = null;
    void post("/api/runtime/pi/delete", { pid: process.pid });
  });
}
