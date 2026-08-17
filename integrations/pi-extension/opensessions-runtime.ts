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

type AgentStatus = "running" | "done" | "error" | "interrupted";

interface AgentEventPayload {
  agent: "pi";
  status: AgentStatus;
  threadId: string;
  threadName?: string;
  lastUserPrompt?: string;
  projectDir: string;
  ts: number;
}

const DEFAULT_SERVER_PORT = 7391;
const RUST_SERVER_PORT_BASE = 22000;
const HEARTBEAT_MS = 5_000;
const REQUEST_TIMEOUT_MS = 750;

/**
 * Mirror opensessions Rust runtime port resolution. The
 * server port is derived from a hash of the tmux socket path so concurrent
 * tmux servers on the same machine get independent opensessions servers.
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

  add(process.env.OPENSESSIONS_URL?.replace(/\/+$/, ""));

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

  add(`http://127.0.0.1:${DEFAULT_SERVER_PORT}`);
  return urls;
}

export default function opensessionsRuntime(pi: ExtensionAPI) {
  let heartbeat: ReturnType<typeof setInterval> | null = null;
  let heartbeatRequest: Promise<void> | null = null;
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
    for (const serverUrl of resolveServerUrls()) {
      try {
        const response = await fetch(`${serverUrl}${path}`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(body),
          signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
        });
        if (response.status >= 200 && response.status < 300) return;
      } catch {
        // opensessions may not be running yet; retry on next heartbeat
      }
    }
  }

  function agentPayload(
    status: AgentStatus,
    ctx: ExtensionContext,
    lastUserPrompt?: string,
  ): AgentEventPayload {
    return {
      agent: "pi",
      status,
      threadId: ctx.sessionManager.getSessionId(),
      threadName: pi.getSessionName(),
      lastUserPrompt,
      projectDir: ctx.sessionManager.getCwd(),
      ts: Date.now(),
    };
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
      if (!heartbeatRequest) {
        heartbeatRequest = post("/api/runtime/pi/upsert", {
          ...current,
          sessionName: pi.getSessionName(),
          ts: Date.now(),
        } satisfies PiRuntimePayload).finally(() => {
          heartbeatRequest = null;
        });
      }
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

  pi.on("before_agent_start", async (event, ctx) => {
    void post(
      "/api/agent-event",
      agentPayload("running", ctx, typeof event.prompt === "string" ? event.prompt : undefined),
    );
  });

  pi.on("agent_end", async (_event, ctx) => {
    void post("/api/agent-event", agentPayload("done", ctx));
  });

  pi.on("session_shutdown", async () => {
    clearHeartbeat();
    current = null;
    void post("/api/runtime/pi/delete", { pid: process.pid });
  });
}
