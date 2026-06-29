import { describe, test, expect, beforeEach } from "bun:test";
import { AgentTracker } from "../src/agents/tracker";
import type { AgentEvent } from "../src/contracts/agent";

function event(overrides: Partial<AgentEvent> = {}): AgentEvent {
  return {
    agent: "amp",
    session: "sess-1",
    status: "running",
    ts: Date.now(),
    ...overrides,
  };
}

describe("AgentTracker", () => {
  let tracker: AgentTracker;

  beforeEach(() => {
    tracker = new AgentTracker();
  });

  // --- applyEvent ---

  test("applyEvent stores agent state by session", () => {
    tracker.applyEvent(event({ session: "sess-1", status: "running" }));

    const state = tracker.getState("sess-1");
    expect(state).not.toBeNull();
    expect(state!.status).toBe("running");
    expect(state!.agent).toBe("amp");
  });

  test("applyEvent overwrites previous state for same session", () => {
    tracker.applyEvent(event({ session: "sess-1", status: "running" }));
    tracker.applyEvent(event({ session: "sess-1", status: "done" }));

    expect(tracker.getState("sess-1")!.status).toBe("done");
  });

  test("applyEvent marks terminal status as unseen when session not active", () => {
    tracker.applyEvent(event({ session: "sess-1", status: "done" }));

    expect(tracker.getUnseen()).toContain("sess-1");
  });

  test("applyEvent treats stale as a terminal unseen status", () => {
    tracker.applyEvent(event({ session: "sess-1", status: "stale" }));

    expect(tracker.getUnseen()).toContain("sess-1");
    expect(tracker.getState("sess-1")!.status).toBe("stale");
  });

  test("pruneStuck removes tool-running states older than timeout", () => {
    const oldTs = Date.now() - 4 * 60 * 1000;
    tracker.applyEvent(event({ session: "sess-1", status: "tool-running", ts: oldTs }));

    tracker.pruneStuck(3 * 60 * 1000);

    expect(tracker.getState("sess-1")).toBeNull();
  });

  test("applyEvent does NOT mark terminal status as unseen when session is active", () => {
    tracker.setActiveSessions(["sess-1"]);
    tracker.applyEvent(event({ session: "sess-1", status: "done" }));

    expect(tracker.getUnseen()).not.toContain("sess-1");
  });

  test("applyEvent clears unseen when same instance transitions to non-terminal", () => {
    tracker.applyEvent(event({ session: "sess-1", status: "done", threadId: "t1" }));
    expect(tracker.getUnseen()).toContain("sess-1");

    tracker.applyEvent(event({ session: "sess-1", status: "running", threadId: "t1" }));
    expect(tracker.getUnseen()).not.toContain("sess-1");
  });

  test("applyEvent: resuming thread A does NOT clear thread B unseen", () => {
    tracker.applyEvent(event({ session: "sess-1", status: "done", threadId: "t1" }));
    tracker.applyEvent(event({ session: "sess-1", status: "done", threadId: "t2" }));
    expect(tracker.isUnseen("sess-1")).toBe(true);

    // Thread A resumes (user interacted) — but thread B is still unseen
    tracker.applyEvent(event({ session: "sess-1", status: "running", threadId: "t1" }));
    expect(tracker.isUnseen("sess-1")).toBe(true); // thread B still unseen
  });

  // --- getState ---

  test("getState returns null for unknown session", () => {
    expect(tracker.getState("unknown")).toBeNull();
  });

  // --- markSeen ---

  test("markSeen clears unseen flag but keeps terminal instances", () => {
    tracker.applyEvent(event({ session: "sess-1", status: "done" }));
    expect(tracker.getUnseen()).toContain("sess-1");

    const cleared = tracker.markSeen("sess-1");
    expect(cleared).toBe(true);
    expect(tracker.getUnseen()).not.toContain("sess-1");
    // Instance still exists (seen terminal), pruneTerminal will clean it up
    expect(tracker.getState("sess-1")).not.toBeNull();
    expect(tracker.getState("sess-1")!.status).toBe("done");
  });

  test("markSeen returns false when session has no unseen", () => {
    expect(tracker.markSeen("nonexistent")).toBe(false);
  });

  test("markSeen does NOT remove state when status is not terminal", () => {
    tracker.applyEvent(event({ session: "sess-1", status: "running" }));
    // Manually add to unseen to test edge case
    const cleared = tracker.markSeen("sess-1");
    expect(cleared).toBe(false);
    expect(tracker.getState("sess-1")).not.toBeNull();
  });

  test("dismiss removes only the targeted agent instance", () => {
    tracker.applyEvent(event({ session: "sess-1", status: "done", agent: "amp", threadId: "t1" }));
    tracker.applyEvent(event({ session: "sess-1", status: "running", agent: "codex", threadId: "t2" }));

    const dismissed = tracker.dismiss("sess-1", "amp", "t1");

    expect(dismissed).toBe(true);
    expect(tracker.getAgents("sess-1").map((agent) => `${agent.agent}:${agent.threadId}`)).toEqual(["codex:t2"]);
    expect(tracker.getUnseen()).not.toContain("sess-1");
  });

  test("dismiss removes synthetic pane-backed Pi entries", () => {
    // Pi synthetic entries are keyed `pi:<threadId>:pane:<paneId>`, which the
    // old dismiss missed entirely.
    tracker.applyPanePresence("sess-1", [
      { agent: "pi", paneId: "%1", threadId: "dead" },
      { agent: "pi", paneId: "%1", threadId: "live" },
    ]);
    expect(tracker.getAgents("sess-1")).toHaveLength(2);

    const dismissed = tracker.dismiss("sess-1", "pi", "dead");

    expect(dismissed).toBe(true);
    const remaining = tracker.getAgents("sess-1");
    expect(remaining).toHaveLength(1);
    expect(remaining[0]!.threadId).toBe("live");
  });

  test("dedupeInstanceToSession removes duplicate watcher instance from other sessions", () => {
    tracker.applyEvent(event({ session: "sess-1", agent: "pi", threadId: "shared", status: "running" }));
    tracker.applyEvent(event({ session: "sess-2", agent: "pi", threadId: "shared", status: "running" }));

    const changed = tracker.dedupeInstanceToSession("sess-2", "pi", "shared");

    expect(changed).toBe(true);
    expect(tracker.getAgents("sess-1")).toHaveLength(0);
    expect(tracker.getAgents("sess-2")).toHaveLength(1);
    expect(tracker.getAgents("sess-2")[0]!.threadId).toBe("shared");
  });

  // --- pruneStuck ---

  test("pruneStuck removes running states older than timeout", () => {
    const oldTs = Date.now() - 4 * 60 * 1000; // 4 minutes ago
    tracker.applyEvent(event({ session: "sess-1", status: "running", ts: oldTs }));

    tracker.pruneStuck(3 * 60 * 1000);

    expect(tracker.getState("sess-1")).toBeNull();
    expect(tracker.getUnseen()).not.toContain("sess-1");
  });

  test("pruneStuck does NOT remove recent running states", () => {
    tracker.applyEvent(event({ session: "sess-1", status: "running", ts: Date.now() }));

    tracker.pruneStuck(3 * 60 * 1000);

    expect(tracker.getState("sess-1")).not.toBeNull();
  });

  test("pruneStuck does NOT remove non-running states regardless of age", () => {
    const oldTs = Date.now() - 10 * 60 * 1000;
    tracker.applyEvent(event({ session: "sess-1", status: "stale", ts: oldTs }));

    tracker.pruneStuck(3 * 60 * 1000);

    expect(tracker.getState("sess-1")).not.toBeNull();
  });

  // --- hibernation ---

  test("findHibernationCandidates returns old alive idle agents in inactive sessions", () => {
    const oldTs = Date.now() - 7 * 60 * 60 * 1000;
    tracker.applyEvent(event({ session: "sess-1", agent: "amp", threadId: "t1", threadName: "Old work", status: "done", ts: oldTs }));
    tracker.applyPanePresence("sess-1", [{ agent: "amp", paneId: "%1" }]);

    const candidates = tracker.findHibernationCandidates(Date.now(), 6 * 60 * 60 * 1000);

    expect(candidates).toEqual([{
      session: "sess-1",
      agent: "amp",
      threadId: "t1",
      threadName: "Old work",
      paneId: "%1",
    }]);
  });

  test("findHibernationCandidates skips active sessions and running agents", () => {
    const oldTs = Date.now() - 7 * 60 * 60 * 1000;
    tracker.applyEvent(event({ session: "active", agent: "amp", threadId: "t1", status: "done", ts: oldTs }));
    tracker.applyPanePresence("active", [{ agent: "amp", paneId: "%1" }]);
    tracker.applyEvent(event({ session: "inactive", agent: "codex", threadId: "t2", status: "running", ts: oldTs }));
    tracker.applyPanePresence("inactive", [{ agent: "codex", paneId: "%2" }]);
    tracker.setActiveSessions(["active"]);

    const candidates = tracker.findHibernationCandidates(Date.now(), 6 * 60 * 60 * 1000);

    expect(candidates).toEqual([]);
  });

  test("markHibernated keeps the agent entry but clears live pane state", () => {
    const oldTs = Date.now() - 7 * 60 * 60 * 1000;
    tracker.applyEvent(event({ session: "sess-1", agent: "amp", threadId: "t1", status: "done", ts: oldTs }));
    tracker.applyPanePresence("sess-1", [{ agent: "amp", paneId: "%1" }]);
    const [candidate] = tracker.findHibernationCandidates(Date.now(), 6 * 60 * 60 * 1000);

    expect(tracker.markHibernated(candidate!, 1234)).toBe(true);

    const [agent] = tracker.getAgents("sess-1");
    expect(agent!.status).toBe("hibernated");
    expect(agent!.ts).toBe(1234);
    expect(agent!.liveness).toBe("exited");
    expect(agent!.paneId).toBeUndefined();
    expect(agent!.unseen).toBeUndefined();
  });

  // --- isUnseen ---

  test("isUnseen returns correct value", () => {
    expect(tracker.isUnseen("sess-1")).toBe(false);

    tracker.applyEvent(event({ session: "sess-1", status: "error" }));
    expect(tracker.isUnseen("sess-1")).toBe(true);

    tracker.markSeen("sess-1");
    expect(tracker.isUnseen("sess-1")).toBe(false);
  });

  // --- handleFocus ---

  test("handleFocus clears unseen for focused session", () => {
    tracker.applyEvent(event({ session: "sess-1", status: "done" }));
    expect(tracker.isUnseen("sess-1")).toBe(true);

    const hadUnseen = tracker.handleFocus("sess-1");
    expect(hadUnseen).toBe(true);
    expect(tracker.isUnseen("sess-1")).toBe(false);
  });

  test("handleFocus updates active sessions", () => {
    tracker.handleFocus("sess-2");

    // Now sess-2 is active; a terminal event shouldn't mark it unseen
    tracker.applyEvent(event({ session: "sess-2", status: "done" }));
    expect(tracker.isUnseen("sess-2")).toBe(false);
  });

  // --- getAgents unseen flag ---

  test("getAgents stamps unseen flag on terminal instances", () => {
    tracker.applyEvent(event({ session: "sess-1", status: "done", threadId: "t1" }));
    const agents = tracker.getAgents("sess-1");
    expect(agents.length).toBe(1);
    expect(agents[0]!.unseen).toBe(true);
  });

  test("getAgents does not stamp unseen on seen terminal instances", () => {
    tracker.applyEvent(event({ session: "sess-1", status: "done", threadId: "t1" }));
    tracker.markSeen("sess-1");
    const agents = tracker.getAgents("sess-1");
    expect(agents.length).toBe(1);
    expect(agents[0]!.unseen).toBeUndefined();
  });

  // --- getAgents ordering ---

  test("getAgents returns newest items first", () => {
    tracker.applyEvent(event({ session: "sess-1", status: "done", threadId: "t1", ts: 100 }));
    tracker.applyEvent(event({ session: "sess-1", status: "running", threadId: "t2", ts: 200 }));

    const agents = tracker.getAgents("sess-1");

    expect(agents.map((agent) => agent.threadId)).toEqual(["t2", "t1"]);
  });

  // --- pruneTerminal ---

  test("pruneTerminal removes seen terminal instances after timeout", () => {
    const oldTs = Date.now() - 6 * 60 * 1000; // 6 min ago, past TERMINAL_PRUNE_MS
    tracker.applyEvent(event({ session: "sess-1", status: "done", ts: oldTs }));
    tracker.markSeen("sess-1"); // Mark seen so pruneTerminal can remove it

    tracker.pruneTerminal();

    expect(tracker.getState("sess-1")).toBeNull();
  });

  test("pruneTerminal does NOT remove unseen terminal instances", () => {
    const oldTs = Date.now() - 6 * 60 * 1000;
    tracker.applyEvent(event({ session: "sess-1", status: "done", ts: oldTs }));
    // NOT marked seen

    tracker.pruneTerminal();

    expect(tracker.getState("sess-1")).not.toBeNull();
  });

  // --- applyPanePresence ---

  describe("applyPanePresence", () => {
    test("enriches existing watcher entry with paneId and liveness", () => {
      tracker.applyEvent(event({ session: "sess-1", agent: "claude-code", threadId: "abc", status: "running" }));

      const changed = tracker.applyPanePresence("sess-1", [
        { agent: "claude-code", paneId: "%1" },
      ]);

      expect(changed).toBe(true);
      const agents = tracker.getAgents("sess-1");
      expect(agents.length).toBe(1);
      expect(agents[0]!.paneId).toBe("%1");
      expect(agents[0]!.liveness).toBe("alive");
      // Watcher status preserved — scanner doesn't touch it
      expect(agents[0]!.status).toBe("running");
    });

    test("creates minimal synthetic entry for unmatched pane agent", () => {
      const changed = tracker.applyPanePresence("sess-1", [
        { agent: "claude-code", paneId: "%5" },
      ]);

      expect(changed).toBe(true);
      const agents = tracker.getAgents("sess-1");
      expect(agents.length).toBe(1);
      expect(agents[0]!.agent).toBe("claude-code");
      expect(agents[0]!.paneId).toBe("%5");
      expect(agents[0]!.liveness).toBe("alive");
      expect(agents[0]!.status).toBe("idle"); // default for synthetics
      expect(agents[0]!.threadId).toBeUndefined(); // scanner doesn't resolve threadId
    });

    test("transitions previously-alive watcher entry to exited when missing from scan", () => {
      // Watcher entry first, then enrich via pane presence
      tracker.applyEvent(event({ session: "sess-1", agent: "claude-code", threadId: "abc", status: "running" }));
      tracker.applyPanePresence("sess-1", [
        { agent: "claude-code", paneId: "%1" },
      ]);

      // Verify it's alive
      let agents = tracker.getAgents("sess-1");
      expect(agents[0]!.liveness).toBe("alive");

      // Empty scan — agent disappeared
      const changed = tracker.applyPanePresence("sess-1", []);

      expect(changed).toBe(true);
      agents = tracker.getAgents("sess-1");
      expect(agents.length).toBe(1);
      expect(agents[0]!.liveness).toBe("exited");
      expect(agents[0]!.paneId).toBeUndefined();
    });

    test("drops synthetic pane entry (no watcher) when it disappears from scan", () => {
      // Synthetic-only entry (no prior applyEvent)
      tracker.applyPanePresence("sess-1", [
        { agent: "claude-code", paneId: "%1" },
      ]);
      expect(tracker.getAgents("sess-1").length).toBe(1);

      // Empty scan — synthetic presence markers have nothing left to represent
      const changed = tracker.applyPanePresence("sess-1", []);

      expect(changed).toBe(true);
      expect(tracker.getAgents("sess-1")).toHaveLength(0);
    });

    test("thread-aware liveness: live sibling in same pane does not keep dead Pi thread alive", () => {
      // Two Pi threads started in the same pane over time
      tracker.applyPanePresence("sess-1", [
        { agent: "pi", paneId: "%1", threadId: "old-dead" },
      ]);
      tracker.applyPanePresence("sess-1", [
        { agent: "pi", paneId: "%1", threadId: "old-dead" },
        { agent: "pi", paneId: "%1", threadId: "live" },
      ]);
      expect(tracker.getAgents("sess-1")).toHaveLength(2);

      // Next scan: only the live thread remains in the pane
      const changed = tracker.applyPanePresence("sess-1", [
        { agent: "pi", paneId: "%1", threadId: "live" },
      ]);

      expect(changed).toBe(true);
      const agents = tracker.getAgents("sess-1");
      expect(agents).toHaveLength(1);
      expect(agents[0]!.threadId).toBe("live");
      expect(agents[0]!.liveness).toBe("alive");
    });

    test("does not transition unknown-liveness agents to exited", () => {
      // Watcher-only entry (no pane info)
      tracker.applyEvent(event({ session: "sess-1", agent: "claude-code", threadId: "abc", status: "running" }));

      // Empty pane scan — should NOT affect watcher-only entries
      const changed = tracker.applyPanePresence("sess-1", []);

      expect(changed).toBe(false);
      const agents = tracker.getAgents("sess-1");
      expect(agents[0]!.liveness).toBeUndefined(); // still unknown
    });

    test("pruneStuck marks alive agents stale instead of preserving running animation", () => {
      const oldTs = Date.now() - 10 * 60 * 1000;
      tracker.applyEvent(event({ session: "sess-1", agent: "claude-code", threadId: "abc", status: "running", ts: oldTs }));

      // Make it alive via pane presence
      tracker.applyPanePresence("sess-1", [
        { agent: "claude-code", paneId: "%1" },
      ]);

      // Prune with a timeout that would normally remove it
      const changed = tracker.pruneStuck(3 * 60 * 1000);

      // It should survive because the pane is alive, but should no longer
      // look actively running forever if the watcher/plugin went silent.
      expect(changed).toBe(true);
      const agents = tracker.getAgents("sess-1");
      expect(agents.length).toBe(1);
      expect(agents[0]!.status).toBe("stale");
      expect(agents[0]!.liveness).toBe("alive");
    });

    test("pruneStuck removes exited agents", () => {
      const oldTs = Date.now() - 10 * 60 * 1000;
      tracker.applyEvent(event({ session: "sess-1", agent: "claude-code", threadId: "abc", status: "running", ts: oldTs }));

      // Make alive then exited
      tracker.applyPanePresence("sess-1", [{ agent: "claude-code", paneId: "%1" }]);
      tracker.applyPanePresence("sess-1", []); // exits

      tracker.pruneStuck(3 * 60 * 1000);

      expect(tracker.getState("sess-1")).toBeNull();
    });

    test("pruneTerminal skips alive agents even with terminal status", () => {
      const oldTs = Date.now() - 6 * 60 * 1000;
      tracker.applyEvent(event({ session: "sess-1", agent: "claude-code", threadId: "abc", status: "done", ts: oldTs }));
      tracker.markSeen("sess-1"); // Mark seen so prune would normally remove

      // Make it alive
      tracker.applyPanePresence("sess-1", [{ agent: "claude-code", paneId: "%1" }]);

      tracker.pruneTerminal();

      // Should survive because alive
      expect(tracker.getAgents("sess-1").length).toBe(1);
    });

    test("returns false when nothing changed", () => {
      tracker.applyPanePresence("sess-1", [
        { agent: "claude-code", paneId: "%1" },
      ]);

      // Apply same presence again
      const changed = tracker.applyPanePresence("sess-1", [
        { agent: "claude-code", paneId: "%1" },
      ]);

      expect(changed).toBe(false);
    });

    test("enriches watcher entry matched by agent name", () => {
      tracker.applyEvent(event({ session: "sess-1", agent: "amp", status: "running" }));

      const changed = tracker.applyPanePresence("sess-1", [
        { agent: "amp", paneId: "%3" },
      ]);

      // Should not create a duplicate — enriches the existing entry
      expect(changed).toBe(true);
      expect(tracker.getAgents("sess-1").length).toBe(1);
      expect(tracker.getAgents("sess-1")[0]!.paneId).toBe("%3");
      expect(tracker.getAgents("sess-1")[0]!.liveness).toBe("alive");
    });

    test("prefers watcher entry over synthetic when enriching", () => {
      // Watcher tracks the current conversation
      tracker.applyEvent(event({ session: "sess-1", agent: "claude-code", threadId: "current-convo", status: "running" }));

      // Scanner just reports agent + paneId (no threadId)
      const changed = tracker.applyPanePresence("sess-1", [
        { agent: "claude-code", paneId: "%21" },
      ]);

      expect(changed).toBe(true);
      const agents = tracker.getAgents("sess-1");
      expect(agents.length).toBe(1);
      expect(agents[0]!.threadId).toBe("current-convo"); // watcher's threadId preserved
      expect(agents[0]!.paneId).toBe("%21");
      expect(agents[0]!.liveness).toBe("alive");
    });

    test("enriches exact watcher entry when pane presence includes threadId", () => {
      tracker.applyEvent(event({ session: "sess-1", agent: "pi", threadId: "thread-a", status: "running" }));
      tracker.applyEvent(event({ session: "sess-1", agent: "pi", threadId: "thread-b", status: "running" }));

      const changed = tracker.applyPanePresence("sess-1", [
        { agent: "pi", paneId: "%31", threadId: "thread-b" },
      ]);

      expect(changed).toBe(true);
      const agents = tracker.getAgents("sess-1");
      expect(agents.find((agent) => agent.threadId === "thread-a")?.paneId).toBeUndefined();
      expect(agents.find((agent) => agent.threadId === "thread-b")?.paneId).toBe("%31");
    });

    test("keeps ambiguous same-agent watcher entries separate when pane presence has no threadId", () => {
      tracker.applyEvent(event({ session: "sess-1", agent: "pi", threadId: "thread-a", status: "running" }));
      tracker.applyEvent(event({ session: "sess-1", agent: "pi", threadId: "thread-b", status: "running" }));

      const changed = tracker.applyPanePresence("sess-1", [
        { agent: "pi", paneId: "%21" },
      ]);

      expect(changed).toBe(true);
      const agents = tracker.getAgents("sess-1");
      expect(agents).toHaveLength(3);
      expect(agents.filter((agent) => agent.agent === "pi" && agent.threadId)).toHaveLength(2);
      expect(agents.find((agent) => agent.threadId === undefined)?.paneId).toBe("%21");
    });

    test("cleans up synthetic entry when watcher creates entry for same exact thread", () => {
      tracker.applyPanePresence("sess-1", [
        { agent: "pi", paneId: "%21", threadId: "abc" },
      ]);
      expect(tracker.getAgents("sess-1").length).toBe(1);
      expect(tracker.getAgents("sess-1")[0]!.paneId).toBe("%21");
      expect(tracker.getAgents("sess-1")[0]!.threadId).toBe("abc");

      // Watcher catches up → creates entry with same threadId, auto-cleans synthetic
      tracker.applyEvent(event({ session: "sess-1", agent: "pi", threadId: "abc", status: "running" }));
      const agents = tracker.getAgents("sess-1");
      expect(agents.length).toBe(1);
      expect(agents[0]!.threadId).toBe("abc");
      expect(agents[0]!.paneId).toBe("%21");
      expect(agents[0]!.liveness).toBe("alive");
    });
  });
});
