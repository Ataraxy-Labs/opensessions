import type { ExtensionAPI, ExtensionContext } from "@mariozechner/pi-coding-agent";

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

// Mirror Rust's strict integer parse (`parse::<u32>()` / `parse::<u16>()` in
// packages/runtime-rs/src/shared.rs). JavaScript's `Number.parseInt` and
// `Number()` both accept formats Rust rejects ("123abc", "0.5", "1e3",
// "0x10"); the heartbeat client must Err on the same inputs or it will
// post to a different port than the server binds.
const DECIMAL_INTEGER_RE = /^[0-9]+$/;

function resolveServerPort(): number {
  const explicitPort = process.env.OPENSESSIONS_PORT?.trim();
  if (explicitPort && DECIMAL_INTEGER_RE.test(explicitPort)) {
    const port = Number(explicitPort);
    if (Number.isInteger(port) && port > 0 && port <= 65535) return port;
  }

  const explicitKey = process.env.OPENSESSIONS_SERVER_KEY?.trim();
  if (explicitKey) {
    if (DECIMAL_INTEGER_RE.test(explicitKey)) {
      const key = Number(explicitKey);
      if (Number.isInteger(key) && key >= 0) {
        const port = 22000 + key;
        if (port > 0 && port <= 65535) return port;
      }
    }
    // Any explicit-but-invalid OPENSESSIONS_SERVER_KEY falls back to the
    // default port (matching the Rust server) so heartbeat client and
    // server agree.
    return DEFAULT_SERVER_PORT;
  }

  const tmux = process.env.TMUX?.trim();
  if (tmux) {
    const socketPath = tmux.split(",", 1)[0];
    if (socketPath) return 22000 + hashServerKey(socketPath);
  }
  return DEFAULT_SERVER_PORT;
}

function getServerUrl(): string {
  const explicit = process.env.OPENSESSIONS_URL;
  if (explicit) return explicit.replace(/\/+$/, "");
  return `http://127.0.0.1:${resolveServerPort()}`;
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
    try {
      await fetch(`${getServerUrl()}${path}`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
    } catch {
      // opensessions may not be running yet; retry on next heartbeat
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
