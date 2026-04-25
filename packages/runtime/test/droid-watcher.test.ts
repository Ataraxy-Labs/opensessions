import { describe, test, expect, beforeEach, afterEach } from "bun:test";
import { appendFileSync, mkdirSync, rmSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";
import { DroidAgentWatcher, determineStatus } from "../src/agents/watchers/droid";
import type { AgentEvent } from "../src/contracts/agent";
import type { AgentWatcherContext } from "../src/contracts/agent-watcher";

describe("Droid determineStatus", () => {
  test("returns running for user text messages", () => {
    expect(determineStatus({
      type: "message",
      message: { role: "user", content: [{ type: "text", text: "Fix the bug" }] },
    })).toBe("running");
  });

  test("returns running for user tool_result messages", () => {
    expect(determineStatus({
      type: "message",
      message: { role: "user", content: [{ type: "tool_result" }] },
    })).toBe("running");
  });

  test("returns running for assistant tool_use messages", () => {
    expect(determineStatus({
      type: "message",
      message: { role: "assistant", content: [{ type: "text" }, { type: "tool_use" }] },
    })).toBe("running");
  });

  test("returns running for assistant thinking-only messages", () => {
    expect(determineStatus({
      type: "message",
      message: { role: "assistant", content: [{ type: "thinking" }] },
    })).toBe("running");
  });

  test("returns done for assistant text-only messages", () => {
    expect(determineStatus({
      type: "message",
      message: { role: "assistant", content: [{ type: "text", text: "Here is the fix." }] },
    })).toBe("done");
  });

  test("returns done for session_end entries", () => {
    expect(determineStatus({ type: "session_end" })).toBe("done");
  });

  test("returns null for session_start entries", () => {
    expect(determineStatus({ type: "session_start", sessionTitle: "test" })).toBeNull();
  });

  test("returns null for todo_state entries", () => {
    expect(determineStatus({ type: "todo_state" })).toBeNull();
  });

  test("returns null for messages without role", () => {
    expect(determineStatus({ type: "message", message: {} })).toBeNull();
  });
});

describe("DroidAgentWatcher", () => {
  let tmpDir: string;
  let watcher: DroidAgentWatcher;
  let events: AgentEvent[];
  let ctx: AgentWatcherContext;
  let sessionFile: string;

  beforeEach(() => {
    tmpDir = join(tmpdir(), `droid-watcher-test-${Date.now()}`);
    const projectDir = join(tmpDir, "sessions", "-projects-myapp");
    mkdirSync(projectDir, { recursive: true });

    sessionFile = join(projectDir, "abc12345-1234-1234-1234-123456789abc.jsonl");
    writeFileSync(sessionFile,
      JSON.stringify({
        type: "session_start",
        id: "abc12345-1234-1234-1234-123456789abc",
        sessionTitle: "Fix the watcher",
        cwd: "/projects/myapp",
        version: 2,
      }) + "\n" +
      JSON.stringify({
        type: "message",
        id: "msg-user-1",
        timestamp: "2026-04-15T12:00:01.000Z",
        message: {
          role: "user",
          content: [{ type: "text", text: "Fix the watcher status mapping" }],
        },
      }) + "\n",
    );

    events = [];
    ctx = {
      resolveSession: (dir) => dir === "/projects/myapp" ? "myapp-session" : null,
      emit: (event) => events.push(event),
    };

    watcher = new DroidAgentWatcher();
    (watcher as any).sessionsDir = join(tmpDir, "sessions");
  });

  afterEach(() => {
    watcher.stop();
    rmSync(tmpDir, { recursive: true, force: true });
  });

  test("seed scan emits events for non-idle sessions", async () => {
    watcher.start(ctx);
    await new Promise((resolve) => setTimeout(resolve, 200));

    expect(events).toHaveLength(1);
    expect(events[0]!.agent).toBe("droid");
    expect(events[0]!.session).toBe("myapp-session");
    expect(events[0]!.status).toBe("running");
    expect(events[0]!.threadId).toBe("abc12345-1234-1234-1234-123456789abc");
    expect(events[0]!.threadName).toBe("Fix the watcher");
  });

  test("emits running when assistant uses tools", async () => {
    watcher.start(ctx);
    await new Promise((resolve) => setTimeout(resolve, 200));
    const seedCount = events.length;

    appendFileSync(sessionFile,
      JSON.stringify({
        type: "message",
        id: "msg-assistant-1",
        timestamp: "2026-04-15T12:00:05.000Z",
        message: {
          role: "assistant",
          content: [{ type: "thinking" }, { type: "tool_use" }],
        },
      }) + "\n",
    );

    await new Promise((resolve) => setTimeout(resolve, 2500));

    // Status stays running (tool_use), so no new event emitted (was already running)
    const postSeed = events.slice(seedCount);
    // Running → running doesn't emit, which is correct
    expect(postSeed).toHaveLength(0);
  });

  test("emits done when session_end is written", async () => {
    watcher.start(ctx);
    await new Promise((resolve) => setTimeout(resolve, 200));
    const seedCount = events.length;

    appendFileSync(sessionFile,
      JSON.stringify({
        type: "message",
        id: "msg-assistant-1",
        timestamp: "2026-04-15T12:00:05.000Z",
        message: {
          role: "assistant",
          content: [{ type: "text", text: "All done." }],
        },
      }) + "\n" +
      JSON.stringify({
        type: "session_end",
        timestamp: "2026-04-15T12:00:06.000Z",
        durationMs: 5000,
        toolCount: 3,
        finalText: "All done.",
      }) + "\n",
    );

    await new Promise((resolve) => setTimeout(resolve, 2500));

    const postSeed = events.slice(seedCount);
    expect(postSeed.length).toBeGreaterThanOrEqual(1);
    const last = postSeed[postSeed.length - 1]!;
    expect(last.agent).toBe("droid");
    expect(last.session).toBe("myapp-session");
    expect(last.status).toBe("done");
  });

  test("emits done for assistant text-only response", async () => {
    watcher.start(ctx);
    await new Promise((resolve) => setTimeout(resolve, 200));
    const seedCount = events.length;

    appendFileSync(sessionFile,
      JSON.stringify({
        type: "message",
        id: "msg-assistant-1",
        timestamp: "2026-04-15T12:00:05.000Z",
        message: {
          role: "assistant",
          content: [{ type: "text", text: "Here is the result." }],
        },
      }) + "\n",
    );

    await new Promise((resolve) => setTimeout(resolve, 2500));

    const postSeed = events.slice(seedCount);
    expect(postSeed.length).toBeGreaterThanOrEqual(1);
    const last = postSeed[postSeed.length - 1]!;
    expect(last.status).toBe("done");
    expect(last.threadName).toBe("Fix the watcher");
  });

  test("extracts sessionTitle as threadName", async () => {
    watcher.start(ctx);
    await new Promise((resolve) => setTimeout(resolve, 200));

    expect(events[0]!.threadName).toBe("Fix the watcher");
  });

  test("uses cwd from session_start for project dir resolution", async () => {
    watcher.start(ctx);
    await new Promise((resolve) => setTimeout(resolve, 200));

    // resolveSession was called with "/projects/myapp" (from cwd in session_start)
    expect(events[0]!.session).toBe("myapp-session");
  });
});
