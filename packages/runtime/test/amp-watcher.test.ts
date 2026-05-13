import { describe, test, expect, beforeEach, afterEach } from "bun:test";
import { AmpAgentWatcher, determineStatus, mapAmpState } from "../src/agents/watchers/amp";
import type { AgentEvent } from "../src/contracts/agent";
import type { AgentWatcherContext } from "../src/contracts/agent-watcher";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

// --- determineStatus (exported for independent testing) ---

describe("Amp determineStatus", () => {
  test("returns idle for null message", () => {
    expect(determineStatus(null)).toBe("idle");
  });

  test("returns idle for message with no role", () => {
    expect(determineStatus({})).toBe("idle");
  });

  test("returns idle for empty messages array (no last message)", () => {
    expect(determineStatus(null)).toBe("idle");
  });

  test("returns running for user message (new prompt)", () => {
    expect(determineStatus({ role: "user" })).toBe("running");
  });

  test("returns running for user message with text content", () => {
    expect(determineStatus({ role: "user", content: [{ type: "text" }] })).toBe("running");
  });

  test("returns running for user message with tool_result", () => {
    expect(determineStatus({ role: "user", content: [{ type: "tool_result" }] })).toBe("running");
  });

  test("returns tool-running for user message with in-progress tool_result", () => {
    expect(determineStatus({ role: "user", content: [{ type: "tool_result", run: { status: "in-progress" } }] })).toBe("tool-running");
  });

  test("returns running for user message with interrupted=true", () => {
    expect(determineStatus({ role: "user", interrupted: true, content: [{ type: "text" }] })).toBe("running");
  });

  test("returns running for assistant with no state (pre-streaming)", () => {
    expect(determineStatus({ role: "assistant" })).toBe("running");
  });

  test("returns running for assistant with empty state", () => {
    expect(determineStatus({ role: "assistant", state: {} })).toBe("running");
  });

  test("returns running for streaming assistant (thinking)", () => {
    expect(determineStatus({ role: "assistant", state: { type: "streaming" } })).toBe("running");
  });

  test("returns running for streaming assistant with tool_use content", () => {
    expect(determineStatus({
      role: "assistant",
      state: { type: "streaming" },
      content: [{ type: "thinking" }, { type: "tool_use" }],
    })).toBe("running");
  });

  test("returns running for complete with tool_use stopReason", () => {
    expect(determineStatus({ role: "assistant", state: { type: "complete", stopReason: "tool_use" } })).toBe("running");
  });

  test("returns done for complete with end_turn stopReason", () => {
    expect(determineStatus({ role: "assistant", state: { type: "complete", stopReason: "end_turn" } })).toBe("done");
  });

  test("returns error for complete with max_tokens stopReason", () => {
    expect(determineStatus({ role: "assistant", state: { type: "complete", stopReason: "max_tokens" } })).toBe("error");
  });

  test("returns error for complete with unknown stopReason", () => {
    expect(determineStatus({ role: "assistant", state: { type: "complete", stopReason: "unknown_reason" } })).toBe("error");
  });

  test("returns interrupted for cancelled state", () => {
    expect(determineStatus({ role: "assistant", state: { type: "cancelled" } })).toBe("interrupted");
  });

  test("returns interrupted for cancelled with thinking content", () => {
    expect(determineStatus({
      role: "assistant",
      state: { type: "cancelled" },
      content: [{ type: "thinking" }],
    })).toBe("interrupted");
  });

  test("returns interrupted for cancelled with tool_use content", () => {
    expect(determineStatus({
      role: "assistant",
      state: { type: "cancelled" },
      content: [{ type: "tool_use" }],
    })).toBe("interrupted");
  });

  test("returns interrupted for cancelled with text content", () => {
    expect(determineStatus({
      role: "assistant",
      state: { type: "cancelled" },
      content: [{ type: "text" }],
    })).toBe("interrupted");
  });

  test("returns interrupted for cancelled with empty content", () => {
    expect(determineStatus({
      role: "assistant",
      state: { type: "cancelled" },
      content: [],
    })).toBe("interrupted");
  });

  test("returns running for unknown assistant state type", () => {
    expect(determineStatus({ role: "assistant", state: { type: "some_future_state" } })).toBe("running");
  });

  test("returns idle for unknown role", () => {
    expect(determineStatus({ role: "system" })).toBe("idle");
  });
});

// --- mapAmpState ---

describe("mapAmpState", () => {
  test("maps working to running", () => expect(mapAmpState("working")).toBe("running"));
  test("maps streaming to running", () => expect(mapAmpState("streaming")).toBe("running"));
  test("maps running_tools to running", () => expect(mapAmpState("running_tools")).toBe("running"));
  test("maps tool_use to tool-running", () => expect(mapAmpState("tool_use")).toBe("tool-running"));
  test("maps awaiting_approval to waiting", () => expect(mapAmpState("awaiting_approval")).toBe("waiting"));
  test("maps idle to done", () => expect(mapAmpState("idle")).toBe("done"));
  test("maps error to error", () => expect(mapAmpState("error")).toBe("error"));
  test("returns null for unknown state", () => expect(mapAmpState("some_future_state")).toBe(null));
});

// --- AmpAgentWatcher integration ---

/**
 * Mock fetch serving thread list and DTW tokens.
 */
function createMockFetch(state: {
  threads: Map<string, { title?: string; env: any; updatedAt?: string; messages?: any[] }>;
  dtwTokens: Map<string, string>;
}) {
  return async (input: string | URL | Request, init?: RequestInit): Promise<Response> => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;

    // POST /api/durable-thread-workers
    if (url.includes("/api/durable-thread-workers") && init?.method === "POST") {
      const body = JSON.parse(init.body as string);
      const token = state.dtwTokens.get(body.threadId);
      if (!token) return new Response("Not Found", { status: 404 });
      return new Response(JSON.stringify({ wsToken: token }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }

    // GET /api/threads/:id — detail fetch for non-DTW threads
    const detailMatch = url.match(/\/api\/threads\/(T-[^?/]+)/);
    if (detailMatch) {
      const threadId = detailMatch[1];
      const thread = state.threads.get(threadId!);
      if (!thread) return new Response("Not Found", { status: 404 });
      return new Response(JSON.stringify({
        id: threadId,
        v: 1,
        title: thread.title,
        messages: thread.messages ?? [],
        env: thread.env,
      }), { status: 200, headers: { "Content-Type": "application/json" } });
    }

    // GET /api/threads?limit=N — discovery
    if (url.includes("/api/threads")) {
      const list = Array.from(state.threads.entries()).map(([id, t]) => ({
        id,
        v: 1,
        title: t.title,
        updatedAt: t.updatedAt ?? new Date().toISOString(),
        env: t.env,
      }));
      return new Response(JSON.stringify(list), { status: 200, headers: { "Content-Type": "application/json" } });
    }

    return new Response("Not Found", { status: 404 });
  };
}

/**
 * Mock WebSocket that supports subprotocol auth (matches Amp's ["amp", token] pattern).
 */
class MockWebSocket {
  static instances: MockWebSocket[] = [];
  url: string;
  protocols: string | string[] | undefined;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  closed = false;

  constructor(url: string, protocols?: string | string[]) {
    this.url = url;
    this.protocols = protocols;
    MockWebSocket.instances.push(this);
  }

  close() {
    this.closed = true;
    if (this.onclose) this.onclose();
  }

  /** Simulate a cf_agent_state message (the format Amp actually sends) */
  simulateAgentState(status: string, threadId = "unknown") {
    if (this.onmessage) {
      this.onmessage({ data: JSON.stringify({ type: "cf_agent_state", state: { status, threadId } }) });
    }
  }

  /** Simulate any raw message */
  simulateMessage(data: unknown) {
    if (this.onmessage) this.onmessage({ data: typeof data === "string" ? data : JSON.stringify(data) });
  }

  simulateError() {
    if (this.onerror) this.onerror();
  }
}

describe("AmpAgentWatcher", () => {
  let watcher: AmpAgentWatcher;
  let events: AgentEvent[];
  let ctx: AgentWatcherContext;
  let mockState: {
    threads: Map<string, { title?: string; env: any; updatedAt?: string; messages?: any[] }>;
    dtwTokens: Map<string, string>;
  };

  function setThread(id: string, data: { title?: string; env: any; updatedAt?: string; messages?: any[] }) {
    mockState.threads.set(id, data);
  }

  function mkEnv(dir: string) {
    return { initial: { trees: [{ uri: `file://${dir}` }] } };
  }

  beforeEach(() => {
    events = [];
    MockWebSocket.instances = [];
    mockState = { threads: new Map(), dtwTokens: new Map() };
    ctx = {
      resolveSession: (dir) => dir === "/projects/myapp" ? "myapp-session" : null,
      emit: (event) => events.push(event),
    };
    watcher = new AmpAgentWatcher();
    (watcher as any).ampUrl = "https://test.ampcode.com";
    (watcher as any).apiKey = "sgamp_test_key";
    (watcher as any).workerUrl = "https://test.ampworkers.com";
    watcher._fetch = createMockFetch(mockState);
    watcher._WebSocket = MockWebSocket as any;
  });

  afterEach(() => {
    watcher.stop();
  });

  async function startWatcher() {
    (watcher as any).ctx = ctx;
    await (watcher as any).poll();
    await new Promise((r) => setTimeout(r, 50));
    (watcher as any).pollTimer = setInterval(() => (watcher as any).poll(), 1000);
  }

  // --- Discovery (polling) ---

  test("seed scan discovers threads and connects WebSockets", async () => {
    mockState.dtwTokens.set("T-test-001", "token-001");
    mockState.dtwTokens.set("T-test-002", "token-002");
    setThread("T-test-001", { title: "Thread one", env: mkEnv("/projects/myapp") });
    setThread("T-test-002", { title: "Thread two", env: mkEnv("/projects/myapp") });

    await startWatcher();

    expect(MockWebSocket.instances.length).toBe(2);
    // No events yet — status comes from WebSocket
    expect(events).toHaveLength(0);
  });

  test("WebSocket connects with subprotocol auth", async () => {
    mockState.dtwTokens.set("T-proto", "my-secret-token");
    setThread("T-proto", { env: mkEnv("/projects/myapp") });

    await startWatcher();

    const ws = MockWebSocket.instances[0]!;
    expect(ws.url).toBe("wss://test.ampworkers.com/threads");
    expect(ws.protocols).toEqual(["amp", "my-secret-token"]);
  });

  test("uses live thread ownership when project-dir matching is ambiguous", async () => {
    mockState.dtwTokens.set("T-live-001", "token-live-001");
    setThread("T-live-001", { title: "Live thread", env: mkEnv("/projects/myapp") });
    ctx.resolveSession = () => null;
    ctx.resolveThreadOwner = (agent, threadId, threadName) =>
      agent === "amp" && threadId === "T-live-001" && threadName === "Live thread"
        ? { session: "live-session", paneId: "%9" }
        : null;

    await startWatcher();

    const ws = MockWebSocket.instances[0]!;
    ws.simulateAgentState("working", "T-live-001");

    expect(events).toHaveLength(1);
    expect(events[0]!.session).toBe("live-session");
    expect(events[0]!.threadId).toBe("T-live-001");
    expect(events[0]!.status).toBe("running");
  });

  test("does not connect WebSocket for threads outside local sessions", async () => {
    mockState.dtwTokens.set("T-other", "token-other");
    setThread("T-other", { env: mkEnv("/projects/other") });

    await startWatcher();

    expect(MockWebSocket.instances.length).toBe(0);
    expect(events).toHaveLength(0);
  });

  test("skips threads older than RECENT_MS", async () => {
    mockState.dtwTokens.set("T-old", "token-old");
    setThread("T-old", {
      env: mkEnv("/projects/myapp"),
      updatedAt: new Date(Date.now() - 10 * 60 * 1000).toISOString(),
    });

    await startWatcher();

    expect(MockWebSocket.instances.length).toBe(0);
  });

  test("emits title update when poll discovers title change", async () => {
    mockState.dtwTokens.set("T-title", "token-title");
    setThread("T-title", { env: mkEnv("/projects/myapp") });

    await startWatcher();

    const ws = MockWebSocket.instances[0]!;
    ws.simulateAgentState("working", "T-title");
    events = [];

    setThread("T-title", { title: "Named thread", env: mkEnv("/projects/myapp") });
    await (watcher as any).poll();

    expect(events).toHaveLength(1);
    expect(events[0]!.threadName).toBe("Named thread");
    expect(events[0]!.status).toBe("running");
  });

  test("handles API failure gracefully", async () => {
    watcher._fetch = async () => new Response("Internal Server Error", { status: 500 });
    await startWatcher();
    expect(events).toHaveLength(0);
  });

  test("request timeout clears scanning so polling can recover", async () => {
    watcher._fetchTimeoutMs = 10;
    let calls = 0;
    watcher._fetch = async (_input, init) => {
      calls++;
      if (calls === 1) {
        return await new Promise<Response>((_resolve, reject) => {
          const signal = init?.signal as AbortSignal | undefined;
          signal?.addEventListener("abort", () => reject(new Error("aborted")), { once: true });
        });
      }
      return new Response(JSON.stringify([]), { status: 200, headers: { "Content-Type": "application/json" } });
    };

    (watcher as any).ctx = ctx;
    await (watcher as any).poll();
    expect((watcher as any).scanning).toBe(false);

    await (watcher as any).poll();
    expect(calls).toBe(2);
  });

  // --- WebSocket status (cf_agent_state messages) ---

  test("cf_agent_state working emits running", async () => {
    mockState.dtwTokens.set("T-ws-001", "token-001");
    setThread("T-ws-001", { title: "Active thread", env: mkEnv("/projects/myapp") });

    await startWatcher();

    MockWebSocket.instances[0]!.simulateAgentState("working", "T-ws-001");

    expect(events).toHaveLength(1);
    expect(events[0]!.status).toBe("running");
    expect(events[0]!.threadId).toBe("T-ws-001");
    expect(events[0]!.threadName).toBe("Active thread");
  });

  test("cf_agent_state streaming emits running", async () => {
    mockState.dtwTokens.set("T-ws-stream", "token-stream");
    setThread("T-ws-stream", { env: mkEnv("/projects/myapp") });

    await startWatcher();
    MockWebSocket.instances[0]!.simulateAgentState("streaming", "T-ws-stream");

    expect(events).toHaveLength(1);
    expect(events[0]!.status).toBe("running");
  });

  test("cf_agent_state tool_use emits tool-running", async () => {
    mockState.dtwTokens.set("T-ws-tool", "token-tool");
    setThread("T-ws-tool", { env: mkEnv("/projects/myapp") });

    await startWatcher();
    MockWebSocket.instances[0]!.simulateAgentState("tool_use", "T-ws-tool");

    expect(events).toHaveLength(1);
    expect(events[0]!.status).toBe("tool-running");
  });

  test("cf_agent_state awaiting_approval emits waiting", async () => {
    mockState.dtwTokens.set("T-ws-wait", "token-wait");
    setThread("T-ws-wait", { env: mkEnv("/projects/myapp") });

    await startWatcher();
    MockWebSocket.instances[0]!.simulateAgentState("awaiting_approval", "T-ws-wait");

    expect(events).toHaveLength(1);
    expect(events[0]!.status).toBe("waiting");
  });

  test("cf_agent_state idle emits done and disconnects", async () => {
    mockState.dtwTokens.set("T-ws-idle", "token-idle");
    setThread("T-ws-idle", { env: mkEnv("/projects/myapp") });

    await startWatcher();

    const ws = MockWebSocket.instances[0]!;
    ws.simulateAgentState("working", "T-ws-idle");
    events = [];

    ws.simulateAgentState("idle", "T-ws-idle");

    expect(events).toHaveLength(1);
    expect(events[0]!.status).toBe("done");
    expect(ws.closed).toBe(true);
    expect((watcher as any).wsConnections.size).toBe(0);
  });

  test("cf_agent_state error emits error and disconnects", async () => {
    mockState.dtwTokens.set("T-ws-err", "token-err");
    setThread("T-ws-err", { env: mkEnv("/projects/myapp") });

    await startWatcher();

    const ws = MockWebSocket.instances[0]!;
    ws.simulateAgentState("working", "T-ws-err");
    events = [];

    ws.simulateAgentState("error", "T-ws-err");

    expect(events).toHaveLength(1);
    expect(events[0]!.status).toBe("error");
    expect(ws.closed).toBe(true);
  });

  test("full lifecycle: working→tool_use→running_tools→idle", async () => {
    mockState.dtwTokens.set("T-ws-lifecycle", "token-lc");
    setThread("T-ws-lifecycle", { title: "Lifecycle thread", env: mkEnv("/projects/myapp") });

    await startWatcher();
    events = [];

    const ws = MockWebSocket.instances[0]!;

    ws.simulateAgentState("working", "T-ws-lifecycle");
    expect(events).toHaveLength(1);
    expect(events[0]!.status).toBe("running");

    ws.simulateAgentState("tool_use", "T-ws-lifecycle");
    expect(events).toHaveLength(2);
    expect(events[1]!.status).toBe("tool-running");

    ws.simulateAgentState("running_tools", "T-ws-lifecycle");
    expect(events).toHaveLength(3);
    expect(events[2]!.status).toBe("running");

    ws.simulateAgentState("idle", "T-ws-lifecycle");
    expect(events).toHaveLength(4);
    expect(events[3]!.status).toBe("done");
    expect(ws.closed).toBe(true);
  });

  test("duplicate status does not emit", async () => {
    mockState.dtwTokens.set("T-ws-dup", "token-dup");
    setThread("T-ws-dup", { env: mkEnv("/projects/myapp") });

    await startWatcher();

    const ws = MockWebSocket.instances[0]!;
    ws.simulateAgentState("working", "T-ws-dup");
    events = [];

    ws.simulateAgentState("working", "T-ws-dup");
    expect(events).toHaveLength(0);
  });

  test("ignores non-cf_agent_state messages", async () => {
    mockState.dtwTokens.set("T-ws-ignore", "token-ignore");
    setThread("T-ws-ignore", { env: mkEnv("/projects/myapp") });

    await startWatcher();
    events = [];

    const ws = MockWebSocket.instances[0]!;
    ws.simulateMessage({ type: "cf_agent_identity" });
    ws.simulateMessage({ type: "cf_agent_mcp_servers" });
    ws.simulateMessage({ type: "heartbeat" });
    expect(events).toHaveLength(0);
  });

  test("ignores malformed messages", async () => {
    mockState.dtwTokens.set("T-ws-bad", "token-bad");
    setThread("T-ws-bad", { env: mkEnv("/projects/myapp") });

    await startWatcher();
    events = [];

    const ws = MockWebSocket.instances[0]!;
    ws.simulateMessage("not json {{{");
    ws.simulateMessage({ noTypeField: true });
    ws.simulateMessage("");

    expect(events).toHaveLength(0);
  });

  test("WebSocket error falls back gracefully", async () => {
    mockState.dtwTokens.set("T-ws-error", "token-error");
    setThread("T-ws-error", { env: mkEnv("/projects/myapp") });

    await startWatcher();

    MockWebSocket.instances[0]!.simulateError();

    expect((watcher as any).wsConnections.size).toBe(0);
  });

  test("stop() closes all WebSocket connections", async () => {
    mockState.dtwTokens.set("T-ws-stop", "token-stop");
    setThread("T-ws-stop", { env: mkEnv("/projects/myapp") });

    await startWatcher();

    expect(MockWebSocket.instances.length).toBe(1);
    watcher.stop();

    expect(MockWebSocket.instances[0]!.closed).toBe(true);
    expect((watcher as any).wsConnections.size).toBe(0);
  });

  test("treats DTW 404 as non-DTW and falls back to detail polling", async () => {
    setThread("T-ws-notoken", {
      env: mkEnv("/projects/myapp"),
      messages: [{ role: "user" }],
    });

    await startWatcher();

    expect(MockWebSocket.instances.length).toBe(0);
    expect((watcher as any).threads.has("T-ws-notoken")).toBe(true);
    expect((watcher as any).nonDtwThreads.has("T-ws-notoken")).toBe(true);
    expect(events.some((e) => e.status === "running" && e.threadId === "T-ws-notoken")).toBe(true);
  });

  test("non-DTW thread falls back to detail fetch for status", async () => {
    const baseFetch = watcher._fetch;
    watcher._fetch = async (input, init) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
      if (url.includes("/api/durable-thread-workers") && init?.method === "POST") {
        return new Response(JSON.stringify({ wsToken: "token-nodtw", usesDtw: false }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }
      return baseFetch(input, init);
    };

    setThread("T-ws-nodtw", {
      env: mkEnv("/projects/myapp"),
      messages: [{ role: "user" }],
    });

    await startWatcher();

    expect(MockWebSocket.instances.length).toBe(0);
    expect((watcher as any).nonDtwThreads.has("T-ws-nodtw")).toBe(true);
    expect(events.some((e) => e.status === "running" && e.threadId === "T-ws-nodtw")).toBe(true);
  });

  test("reconnects on unexpected close", async () => {
    watcher._wsRetryMs = 0;
    mockState.dtwTokens.set("T-ws-reconnect", "token-reconnect");
    setThread("T-ws-reconnect", { title: "Reconnect me", env: mkEnv("/projects/myapp") });

    await startWatcher();

    MockWebSocket.instances[0]!.close();

    await (watcher as any).poll();
    await new Promise((r) => setTimeout(r, 50));

    expect(MockWebSocket.instances.length).toBe(2);
    expect((watcher as any).wsConnections.size).toBe(1);
  });

  test("duplicate connect attempts do not create orphan sockets", async () => {
    const tokenReply = deferred<Response>();
    watcher._fetch = async (input, init) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
      if (url.includes("/api/durable-thread-workers") && init?.method === "POST") {
        return await tokenReply.promise;
      }
      return new Response("Not Found", { status: 404 });
    };

    (watcher as any).ctx = ctx;
    (watcher as any).threads.set("T-ws-race", {
      status: "idle",
      version: 1,
      title: "Race thread",
      projectDir: "/projects/myapp",
      lastListedAt: Date.now(),
    });

    const p1 = (watcher as any).connectWebSocket("T-ws-race");
    const p2 = (watcher as any).connectWebSocket("T-ws-race");
    tokenReply.resolve(new Response(JSON.stringify({ wsToken: "race-token" }), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    }));

    await Promise.all([p1, p2]);
    await new Promise((r) => setTimeout(r, 0));

    expect(MockWebSocket.instances.length).toBe(1);
    expect((watcher as any).wsConnections.size).toBe(1);
  });

  test("stop prevents delayed token fetch from reviving a socket", async () => {
    const tokenReply = deferred<Response>();
    watcher._fetch = async (input, init) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
      if (url.includes("/api/durable-thread-workers") && init?.method === "POST") {
        return await tokenReply.promise;
      }
      return new Response("Not Found", { status: 404 });
    };

    (watcher as any).ctx = ctx;
    (watcher as any).threads.set("T-ws-stoprace", {
      status: "idle",
      version: 1,
      title: "Stop thread",
      projectDir: "/projects/myapp",
      lastListedAt: Date.now(),
    });

    const connectPromise = (watcher as any).connectWebSocket("T-ws-stoprace");
    watcher.stop();
    tokenReply.resolve(new Response(JSON.stringify({ wsToken: "stop-token" }), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    }));

    await connectPromise;
    await new Promise((r) => setTimeout(r, 0));

    expect(MockWebSocket.instances.length).toBe(0);
    expect((watcher as any).wsConnections.size).toBe(0);
  });

  test("old socket close does not delete the replacement socket", async () => {
    mockState.dtwTokens.set("T-ws-oldsock", "token-oldsock");
    setThread("T-ws-oldsock", { title: "Identity-safe close", env: mkEnv("/projects/myapp") });

    await startWatcher();

    const oldSocket = MockWebSocket.instances[0]!;
    const replacement = new MockWebSocket("wss://replacement");
    (watcher as any).wsConnections.set("T-ws-oldsock", {
      gen: 999,
      phase: "open",
      ws: replacement,
    });

    oldSocket.close();

    expect((watcher as any).wsConnections.get("T-ws-oldsock")?.ws).toBe(replacement);
  });

  test("initial cf_agent_state on connect seeds status instantly", async () => {
    mockState.dtwTokens.set("T-ws-seed", "token-seed");
    setThread("T-ws-seed", { title: "Instant status", env: mkEnv("/projects/myapp") });

    await startWatcher();

    // Simulate the immediate cf_agent_state that Amp sends on connect
    MockWebSocket.instances[0]!.simulateAgentState("working", "T-ws-seed");

    expect(events).toHaveLength(1);
    expect(events[0]!.status).toBe("running");
    expect(events[0]!.threadName).toBe("Instant status");
  });

  test("never fetches individual thread details", async () => {
    let detailFetched = false;
    const baseFetch = watcher._fetch;
    watcher._fetch = async (input, init) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
      if (url.match(/\/api\/threads\/T-/)) {
        detailFetched = true;
      }
      return baseFetch(input, init);
    };

    mockState.dtwTokens.set("T-nofetch", "token-nf");
    setThread("T-nofetch", { env: mkEnv("/projects/myapp") });

    await startWatcher();

    const ws = MockWebSocket.instances[0]!;
    ws.simulateAgentState("working", "T-nofetch");
    ws.simulateAgentState("idle", "T-nofetch");

    await (watcher as any).poll();

    expect(detailFetched).toBe(false);
  });

  // --- Plugin ownership ---

  test("markPluginOwned skips WebSocket connect for that thread", async () => {
    mockState.dtwTokens.set("T-plugin", "token-plugin");
    setThread("T-plugin", { env: mkEnv("/projects/myapp") });

    watcher.markPluginOwned("T-plugin");
    await startWatcher();

    expect(MockWebSocket.instances.length).toBe(0);
  });

  test("markPluginOwned tears down an existing WebSocket", async () => {
    mockState.dtwTokens.set("T-plugin-late", "token-plugin-late");
    setThread("T-plugin-late", { env: mkEnv("/projects/myapp") });

    await startWatcher();
    expect(MockWebSocket.instances.length).toBe(1);
    const ws = MockWebSocket.instances[0]!;
    expect(ws.closed).toBe(false);

    watcher.markPluginOwned("T-plugin-late");
    expect(ws.closed).toBe(true);
  });

  test("markPluginOwned does not block other threads", async () => {
    mockState.dtwTokens.set("T-plugin", "token-a");
    mockState.dtwTokens.set("T-other", "token-b");
    setThread("T-plugin", { env: mkEnv("/projects/myapp") });
    setThread("T-other", { env: mkEnv("/projects/myapp") });

    watcher.markPluginOwned("T-plugin");
    await startWatcher();

    expect(MockWebSocket.instances.length).toBe(1);
    expect(MockWebSocket.instances[0]!.protocols).toEqual(["amp", "token-b"]);
  });

  test("ownership expires after TTL and watcher resumes WS connect", async () => {
    mockState.dtwTokens.set("T-expire", "token-expire");
    setThread("T-expire", { env: mkEnv("/projects/myapp") });

    watcher.markPluginOwned("T-expire", 1); // 1ms TTL
    await new Promise((r) => setTimeout(r, 10));
    await startWatcher();

    expect(MockWebSocket.instances.length).toBe(1);
  });
});
