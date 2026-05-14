import { describe, test, expect, beforeEach, afterEach } from "bun:test";
import { mkdirSync, writeFileSync, rmSync } from "fs";
import { join } from "path";
import { tmpdir } from "os";
import { Database } from "bun:sqlite";
import { KiloAgentWatcher, determineStatus } from "../src/agents/watchers/kilo";
import type { AgentEvent } from "../src/contracts/agent";
import type { AgentWatcherContext } from "../src/contracts/agent-watcher";

// --- determineStatus ---

describe("Kilo determineStatus", () => {
  // Null / empty cases
  test("returns idle for null message", () => {
    expect(determineStatus(null)).toBe("idle");
  });

  test("returns idle for message with no role", () => {
    expect(determineStatus({})).toBe("idle");
  });

  // User messages — always running (new prompt)
  test("returns running for user message", () => {
    expect(determineStatus({ role: "user" })).toBe("running");
  });

  // Assistant streaming — no finish, no time.completed
  test("returns running for assistant streaming (no finish)", () => {
    expect(determineStatus({ role: "assistant" })).toBe("running");
  });

  test("returns running for assistant streaming with time.created only", () => {
    expect(determineStatus({ role: "assistant", time: { created: 1234 } })).toBe("running");
  });

  // Assistant with tool-calls finish
  test("returns running for assistant with finish=tool-calls", () => {
    expect(determineStatus({
      role: "assistant",
      finish: "tool-calls",
      time: { created: 1234, completed: 5678 },
    })).toBe("running");
  });

  // Assistant with stop finish — normal completion
  test("returns done for assistant with finish=stop", () => {
    expect(determineStatus({
      role: "assistant",
      finish: "stop",
      time: { created: 1234, completed: 5678 },
    })).toBe("done");
  });

  // Assistant with error finish
  test("returns error for assistant with finish=error", () => {
    expect(determineStatus({
      role: "assistant",
      finish: "error",
      time: { created: 1234, completed: 5678 },
    })).toBe("error");
  });

  // Assistant with unknown finish (provider-specific)
  test("returns done for assistant with finish=unknown", () => {
    expect(determineStatus({
      role: "assistant",
      finish: "unknown",
      time: { created: 1234, completed: 5678 },
    })).toBe("done");
  });

  // MessageAbortedError — user interrupt (Escape in TUI)
  test("returns interrupted for MessageAbortedError", () => {
    expect(determineStatus({
      role: "assistant",
      time: { created: 1234, completed: 5678 },
      error: { name: "MessageAbortedError", data: { message: "The operation was aborted." } },
    })).toBe("interrupted");
  });

  test("returns interrupted for MessageAbortedError even with tool-calls finish", () => {
    expect(determineStatus({
      role: "assistant",
      finish: "tool-calls",
      time: { created: 1234, completed: 5678 },
      error: { name: "MessageAbortedError", data: { message: "The operation was aborted." } },
    })).toBe("interrupted");
  });

  // APIError — provider failure
  test("returns error for APIError", () => {
    expect(determineStatus({
      role: "assistant",
      time: { created: 1234, completed: 5678 },
      error: { name: "APIError", data: { message: "No payment method" } },
    })).toBe("error");
  });

  // UnknownError
  test("returns error for UnknownError", () => {
    expect(determineStatus({
      role: "assistant",
      time: { created: 1234, completed: 5678 },
      error: { name: "UnknownError", data: { message: "Something went wrong" } },
    })).toBe("error");
  });

  // Edge case: completed assistant with no finish (unusual)
  test("returns done for completed assistant with no finish and no error", () => {
    expect(determineStatus({
      role: "assistant",
      time: { created: 1234, completed: 5678 },
    })).toBe("done");
  });

  // Unknown role
  test("returns idle for unknown role", () => {
    expect(determineStatus({ role: "system" })).toBe("idle");
  });
});

// --- KiloAgentWatcher integration ---

describe("KiloAgentWatcher", () => {
  let tmpDir: string;
  let dbPath: string;
  let db: InstanceType<typeof Database>;
  let watcher: KiloAgentWatcher;
  let events: AgentEvent[];
  let ctx: AgentWatcherContext;

  function createDb() {
    db = new Database(dbPath);
    db.run(`CREATE TABLE IF NOT EXISTS session (
      id TEXT PRIMARY KEY,
      project_id TEXT NOT NULL DEFAULT '',
      slug TEXT NOT NULL DEFAULT '',
      directory TEXT NOT NULL,
      title TEXT NOT NULL DEFAULT '',
      version TEXT NOT NULL DEFAULT '',
      time_created INTEGER NOT NULL,
      time_updated INTEGER NOT NULL
    )`);
    db.run(`CREATE TABLE IF NOT EXISTS message (
      id TEXT PRIMARY KEY,
      session_id TEXT NOT NULL,
      time_created INTEGER NOT NULL,
      time_updated INTEGER NOT NULL,
      data TEXT NOT NULL
    )`);
    db.run(`CREATE TABLE IF NOT EXISTS part (
      id TEXT PRIMARY KEY,
      message_id TEXT NOT NULL,
      session_id TEXT NOT NULL,
      time_created INTEGER NOT NULL,
      time_updated INTEGER NOT NULL,
      data TEXT NOT NULL
    )`);
  }

  function insertSession(id: string, dir: string, title = "", timeUpdated = Date.now()) {
    db.run(
      `INSERT OR REPLACE INTO session (id, directory, title, time_created, time_updated) VALUES (?, ?, ?, ?, ?)`,
      [id, dir, title, timeUpdated - 1000, timeUpdated],
    );
  }

  function insertMessage(id: string, sessionId: string, data: object, timeCreated = Date.now()) {
    db.run(
      `INSERT OR REPLACE INTO message (id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?)`,
      [id, sessionId, timeCreated, timeCreated, JSON.stringify(data)],
    );
  }

  function updateSessionTimestamp(sessionId: string, timeUpdated = Date.now()) {
    db.run(`UPDATE session SET time_updated = ? WHERE id = ?`, [timeUpdated, sessionId]);
  }

  beforeEach(() => {
    tmpDir = join(tmpdir(), `kilo-watcher-test-${Date.now()}`);
    mkdirSync(tmpDir, { recursive: true });
    dbPath = join(tmpDir, "kilo.db");
    createDb();
    events = [];
    ctx = {
      resolveSession: (dir) => dir === "/projects/myapp" ? "myapp-session" : null,
      emit: (event) => events.push(event),
    };
    watcher = new KiloAgentWatcher();
    (watcher as any).dbPath = dbPath;
  });

  afterEach(() => {
    watcher.stop();
    try { db.close(); } catch {}
    rmSync(tmpDir, { recursive: true, force: true });
  });

  test("seed scan emits current non-idle sessions", async () => {
    const now = Date.now();
    insertSession("ses_001", "/projects/myapp", "Test session", now);
    insertMessage("msg_001", "ses_001", {
      role: "user",
      time: { created: now },
    }, now);

    watcher.start(ctx);
    await new Promise((r) => setTimeout(r, 200));

    expect(events.length).toBe(1);
    expect(events[0]!.agent).toBe("kilo");
    expect(events[0]!.session).toBe("myapp-session");
    expect(events[0]!.status).toBe("running");
    expect(events[0]!.threadName).toBe("Test session");
  });

  test("seed scan skips idle sessions (no messages)", async () => {
    insertSession("ses_002", "/projects/myapp", "Empty session");
    // No messages → idle → not emitted

    watcher.start(ctx);
    await new Promise((r) => setTimeout(r, 200));

    expect(events.length).toBe(0);
  });

  test("emits status change after seed", async () => {
    const now = Date.now();
    insertSession("ses_003", "/projects/myapp", "Active session", now);
    insertMessage("msg_003a", "ses_003", {
      role: "user",
      time: { created: now },
    }, now);

    watcher.start(ctx);
    await new Promise((r) => setTimeout(r, 200));
    const seedCount = events.length;

    // Simulate assistant completing
    const later = Date.now() + 100;
    insertMessage("msg_003b", "ses_003", {
      role: "assistant",
      finish: "stop",
      time: { created: later, completed: later + 500 },
    }, later);
    updateSessionTimestamp("ses_003", later + 500);

    await new Promise((r) => setTimeout(r, 3500));

    const postSeed = events.slice(seedCount);
    expect(postSeed.length).toBeGreaterThanOrEqual(1);
    expect(postSeed[0]!.status).toBe("done");
    expect(postSeed[0]!.session).toBe("myapp-session");
  });

  test("skips when session cannot be resolved", async () => {
    const now = Date.now();
    insertSession("ses_004", "/unknown/dir", "Unknown session", now);
    insertMessage("msg_004", "ses_004", {
      role: "user",
      time: { created: now },
    }, now);

    watcher.start(ctx);
    await new Promise((r) => setTimeout(r, 200));

    expect(events.length).toBe(0);
  });

  test("detects tool-calls as running", async () => {
    const now = Date.now();
    insertSession("ses_005", "/projects/myapp", "Tool session", now);
    insertMessage("msg_005a", "ses_005", {
      role: "user",
      time: { created: now },
    }, now);

    watcher.start(ctx);
    await new Promise((r) => setTimeout(r, 200));
    const seedCount = events.length;

    // Assistant with tool-calls → still running
    const later = Date.now() + 100;
    insertMessage("msg_005b", "ses_005", {
      role: "assistant",
      finish: "tool-calls",
      time: { created: later, completed: later + 500 },
    }, later);
    updateSessionTimestamp("ses_005", later + 500);

    await new Promise((r) => setTimeout(r, 3500));

    // Status should stay running (tool-calls means more work to do)
    const postSeed = events.slice(seedCount);
    // No event emitted because prev status was running and new status is still running
    const doneEvents = postSeed.filter((e) => e.status === "done");
    expect(doneEvents.length).toBe(0);
  });

  test("detects MessageAbortedError as interrupted", async () => {
    const now = Date.now();
    insertSession("ses_006", "/projects/myapp", "Interrupted session", now);
    insertMessage("msg_006a", "ses_006", {
      role: "user",
      time: { created: now },
    }, now);

    watcher.start(ctx);
    await new Promise((r) => setTimeout(r, 200));
    const seedCount = events.length;

    // User interrupts with Escape → MessageAbortedError
    const later = Date.now() + 100;
    insertMessage("msg_006b", "ses_006", {
      role: "assistant",
      time: { created: later, completed: later + 500 },
      error: { name: "MessageAbortedError", data: { message: "The operation was aborted." } },
    }, later);
    updateSessionTimestamp("ses_006", later + 500);

    await new Promise((r) => setTimeout(r, 3500));

    const postSeed = events.slice(seedCount);
    expect(postSeed.length).toBeGreaterThanOrEqual(1);
    expect(postSeed[0]!.status).toBe("interrupted");
  });

  test("detects APIError as error", async () => {
    const now = Date.now();
    insertSession("ses_007", "/projects/myapp", "Error session", now);
    insertMessage("msg_007a", "ses_007", {
      role: "user",
      time: { created: now },
    }, now);

    watcher.start(ctx);
    await new Promise((r) => setTimeout(r, 200));
    const seedCount = events.length;

    // API error
    const later = Date.now() + 100;
    insertMessage("msg_007b", "ses_007", {
      role: "assistant",
      time: { created: later, completed: later + 500 },
      error: { name: "APIError", data: { message: "No payment method" } },
    }, later);
    updateSessionTimestamp("ses_007", later + 500);

    await new Promise((r) => setTimeout(r, 3500));

    const postSeed = events.slice(seedCount);
    expect(postSeed.length).toBeGreaterThanOrEqual(1);
    expect(postSeed[0]!.status).toBe("error");
  });

  test("detects stuck running and promotes to stale (process killed)", async () => {
    const now = Date.now();
    insertSession("ses_008", "/projects/myapp", "Stuck session", now);
    insertMessage("msg_008a", "ses_008", {
      role: "assistant",
      time: { created: now },
      // No finish, no time.completed → streaming
    }, now);

    watcher.start(ctx);
    await new Promise((r) => setTimeout(r, 200));
    const seedCount = events.length;

    // Backdate lastGrowthAt to simulate process killed 16s ago
    const snapshot = (watcher as any).sessions.get("ses_008");
    snapshot.lastGrowthAt = Date.now() - 16_000;

    // Wait for next poll cycle
    await new Promise((r) => setTimeout(r, 3500));

    const staleEvents = events.slice(seedCount).filter((e) => e.status === "stale");
    expect(staleEvents.length).toBeGreaterThanOrEqual(1);
  }, 10_000);

  test("stays running through multi-step tool use cycle", async () => {
    const now = Date.now();
    insertSession("ses_009", "/projects/myapp", "Multi-step", now);
    insertMessage("msg_009a", "ses_009", {
      role: "user",
      time: { created: now },
    }, now);

    watcher.start(ctx);
    await new Promise((r) => setTimeout(r, 200));
    const seedCount = events.length;

    // Step 1: assistant with tool-calls (running → still running)
    const step1 = Date.now() + 100;
    insertMessage("msg_009b", "ses_009", {
      role: "assistant",
      finish: "tool-calls",
      time: { created: step1, completed: step1 + 500 },
    }, step1);
    updateSessionTimestamp("ses_009", step1 + 500);
    await new Promise((r) => setTimeout(r, 500));

    // Step 2: new assistant message streaming (still running)
    const step2 = Date.now() + 200;
    insertMessage("msg_009c", "ses_009", {
      role: "assistant",
      time: { created: step2 },
    }, step2);
    updateSessionTimestamp("ses_009", step2);
    await new Promise((r) => setTimeout(r, 3500));

    // Should never have been "done" during the cycle
    const postSeed = events.slice(seedCount);
    const doneEvents = postSeed.filter((e) => e.status === "done");
    expect(doneEvents.length).toBe(0);
  });

  test("emits title update even when status stays the same", async () => {
    const now = Date.now();
    insertSession("ses_010", "/projects/myapp", "", now);
    insertMessage("msg_010a", "ses_010", {
      role: "user",
      time: { created: now },
    }, now);

    watcher.start(ctx);
    await new Promise((r) => setTimeout(r, 200));
    events = [];

    // Session gets a title update while still running
    const later = Date.now() + 100;
    db.run(`UPDATE session SET title = ?, time_updated = ? WHERE id = ?`, ["Named session", later, "ses_010"]);

    await new Promise((r) => setTimeout(r, 3500));

    expect(events.length).toBe(1);
    expect(events[0]!.status).toBe("running");
    expect(events[0]!.threadName).toBe("Named session");
  });

  test("does not emit for status same + title same", async () => {
    const now = Date.now();
    insertSession("ses_011", "/projects/myapp", "Same title", now);
    insertMessage("msg_011a", "ses_011", {
      role: "user",
      time: { created: now },
    }, now);

    watcher.start(ctx);
    await new Promise((r) => setTimeout(r, 200));
    events = [];

    // Update session timestamp but nothing else changed
    const later = Date.now() + 100;
    insertMessage("msg_011b", "ses_011", {
      role: "user",
      time: { created: later },
    }, later);
    updateSessionTimestamp("ses_011", later);

    await new Promise((r) => setTimeout(r, 3500));

    // Status is still running, title is still "Same title" — no emit
    expect(events.length).toBe(0);
  });

  test("detects finish=error as error", async () => {
    const now = Date.now();
    insertSession("ses_012", "/projects/myapp", "Error finish", now);
    insertMessage("msg_012a", "ses_012", {
      role: "user",
      time: { created: now },
    }, now);

    watcher.start(ctx);
    await new Promise((r) => setTimeout(r, 200));
    const seedCount = events.length;

    // Assistant with finish=error (no error object)
    const later = Date.now() + 100;
    insertMessage("msg_012b", "ses_012", {
      role: "assistant",
      finish: "error",
      time: { created: later, completed: later + 500 },
    }, later);
    updateSessionTimestamp("ses_012", later + 500);

    await new Promise((r) => setTimeout(r, 3500));

    const postSeed = events.slice(seedCount);
    expect(postSeed.length).toBeGreaterThanOrEqual(1);
    expect(postSeed[0]!.status).toBe("error");
  });

  test("seed emits done for sessions with finish=stop", async () => {
    const now = Date.now();
    insertSession("ses_013", "/projects/myapp", "Done session", now);
    insertMessage("msg_013a", "ses_013", {
      role: "assistant",
      finish: "stop",
      time: { created: now, completed: now + 500 },
    }, now);

    watcher.start(ctx);
    await new Promise((r) => setTimeout(r, 200));

    expect(events.length).toBe(1);
    expect(events[0]!.status).toBe("done");
    expect(events[0]!.threadName).toBe("Done session");
  });

  test("seed emits interrupted for sessions with MessageAbortedError", async () => {
    const now = Date.now();
    insertSession("ses_014", "/projects/myapp", "Aborted session", now);
    insertMessage("msg_014a", "ses_014", {
      role: "assistant",
      time: { created: now, completed: now + 500 },
      error: { name: "MessageAbortedError", data: { message: "The operation was aborted." } },
    }, now);

    watcher.start(ctx);
    await new Promise((r) => setTimeout(r, 200));

    expect(events.length).toBe(1);
    expect(events[0]!.status).toBe("interrupted");
  });

  test("recovers from DB errors by reopening", async () => {
    const now = Date.now();
    insertSession("ses_015", "/projects/myapp", "Recovery", now);
    insertMessage("msg_015a", "ses_015", {
      role: "user",
      time: { created: now },
    }, now);

    watcher.start(ctx);
    await new Promise((r) => setTimeout(r, 200));

    // Close and recreate the DB (simulates WAL checkpoint or corruption recovery)
    db.close();
    createDb();
    insertSession("ses_015", "/projects/myapp", "Recovery", now + 1000);
    insertMessage("msg_015b", "ses_015", {
      role: "assistant",
      finish: "stop",
      time: { created: now + 1000, completed: now + 1500 },
    }, now + 1000);

    // Force watcher to reopen DB
    (watcher as any).db = null;

    await new Promise((r) => setTimeout(r, 3500));

    // Should have recovered and emitted
    const doneEvents = events.filter((e) => e.status === "done");
    expect(doneEvents.length).toBeGreaterThanOrEqual(1);
  });
});
