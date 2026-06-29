import type { AgentEvent, PanePresenceInput } from "../contracts/agent";
import { TERMINAL_STATUSES } from "../contracts/agent";

const MAX_EVENT_TIMESTAMPS = 30;
const TERMINAL_PRUNE_MS = 5 * 60 * 1000;
const SYNTHETIC_PANE_MARKER = ":pane:";

const STATUS_PRIORITY: Record<string, number> = {
  "tool-running": 7,
  running: 6,
  error: 5,
  stale: 4,
  interrupted: 3,
  waiting: 2,
  hibernated: 1,
  done: 1,
  idle: 0,
};

const HIBERNATABLE_STATUSES = new Set(["idle", "done", "error", "interrupted", "stale"]);

export interface HibernationCandidate {
  session: string;
  agent: string;
  threadId?: string;
  threadName?: string;
  paneId: string;
}

export function instanceKey(agent: string, threadId?: string): string {
  return threadId ? `${agent}:${threadId}` : agent;
}

function syntheticPaneKey(agent: string, paneId: string, threadId?: string): string {
  return threadId
    ? `${agent}:${threadId}${SYNTHETIC_PANE_MARKER}${paneId}`
    : `${agent}${SYNTHETIC_PANE_MARKER}${paneId}`;
}

function isSyntheticPaneKey(key: string): boolean {
  return key.includes(SYNTHETIC_PANE_MARKER);
}

export class AgentTracker {
  // Outer key: session name, inner key: instance key (agent or agent:threadId)
  private instances = new Map<string, Map<string, AgentEvent>>();
  private eventTimestamps = new Map<string, number[]>();
  // Per-instance unseen tracking: "session\0instanceKey"
  private unseenInstances = new Set<string>();
  private active = new Set<string>();

  private unseenKey(session: string, key: string): string {
    return `${session}\0${key}`;
  }

  applyEvent(event: AgentEvent, options?: { seed?: boolean }): void {
    const key = instanceKey(event.agent, event.threadId);

    // Store instance
    let sessionInstances = this.instances.get(event.session);
    if (!sessionInstances) {
      sessionInstances = new Map();
      this.instances.set(event.session, sessionInstances);
    }
    // Preserve pane info from prior enrichment by applyPanePresence, and
    // preserve a previously-known threadName if the incoming event omits it
    // (e.g. plugin-driven events that don't carry a title on every POST).
    const prev = sessionInstances.get(key);
    if (prev?.paneId) {
      event.paneId = event.paneId ?? prev.paneId;
      event.liveness = event.liveness ?? prev.liveness;
    }
    if (prev?.threadName && !event.threadName) {
      event.threadName = prev.threadName;
    }
    sessionInstances.set(key, event);

    // Clean up a matching synthetic pane-keyed entry for this agent.
    // Prefer exact thread matches. Fall back to a single generic synthetic
    // only when there is no ambiguity.
    const exactSyntheticMatches: Array<{ key: string; event: AgentEvent }> = [];
    const genericSyntheticMatches: Array<{ key: string; event: AgentEvent }> = [];
    for (const [k, ev] of sessionInstances) {
      if (k === key || ev.agent !== event.agent || !isSyntheticPaneKey(k)) continue;
      if (ev.threadId && event.threadId && ev.threadId === event.threadId) {
        exactSyntheticMatches.push({ key: k, event: ev });
      } else if (!ev.threadId) {
        genericSyntheticMatches.push({ key: k, event: ev });
      }
    }

    const matchToMerge = exactSyntheticMatches.length === 1
      ? exactSyntheticMatches[0]
      : exactSyntheticMatches.length === 0 && genericSyntheticMatches.length === 1
        ? genericSyntheticMatches[0]
        : undefined;

    if (matchToMerge) {
      if (matchToMerge.event.paneId && !event.paneId) {
        event.paneId = matchToMerge.event.paneId;
        event.liveness = matchToMerge.event.liveness;
      }
      sessionInstances.delete(matchToMerge.key);
      this.unseenInstances.delete(this.unseenKey(event.session, matchToMerge.key));
    }

    // Track event timestamps
    let timestamps = this.eventTimestamps.get(event.session);
    if (!timestamps) {
      timestamps = [];
      this.eventTimestamps.set(event.session, timestamps);
    }
    timestamps.push(event.ts);
    if (timestamps.length > MAX_EVENT_TIMESTAMPS) {
      timestamps.splice(0, timestamps.length - MAX_EVENT_TIMESTAMPS);
    }

    // Per-instance unseen tracking
    // Seeded events always mark as unseen (they represent state from before the user connected)
    const ukey = this.unseenKey(event.session, key);
    if (TERMINAL_STATUSES.has(event.status)) {
      if (options?.seed || !this.active.has(event.session)) {
        this.unseenInstances.add(ukey);
      }
    } else {
      // Non-terminal status for this instance = user is interacting, mark seen
      this.unseenInstances.delete(ukey);
    }
  }

  /** Returns the most important agent state for backward compat */
  getState(session: string): AgentEvent | null {
    const sessionInstances = this.instances.get(session);
    if (!sessionInstances || sessionInstances.size === 0) return null;

    let best: AgentEvent | null = null;
    let bestPriority = -1;
    for (const event of sessionInstances.values()) {
      const p = STATUS_PRIORITY[event.status] ?? 0;
      if (p > bestPriority) {
        bestPriority = p;
        best = event;
      }
    }
    return best;
  }

  /** Returns all agent instances for a session, with unseen flag stamped */
  getAgents(session: string): AgentEvent[] {
    const sessionInstances = this.instances.get(session);
    if (!sessionInstances) return [];
    return [...sessionInstances.values()]
      .map((event) => {
        const key = instanceKey(event.agent, event.threadId);
        const isUnseen = this.unseenInstances.has(this.unseenKey(session, key));
        return isUnseen ? { ...event, unseen: true } : event;
      })
      .sort((a, b) => b.ts - a.ts);
  }

  /** Returns recent event timestamps for sparkline rendering */
  getEventTimestamps(session: string): number[] {
    return this.eventTimestamps.get(session) ?? [];
  }

  markSeen(session: string): boolean {
    const hadUnseen = this.isUnseen(session);
    if (!hadUnseen) return false;

    // Clear unseen flags for all instances — keep the instances themselves
    // (pruneTerminal will remove seen terminal instances after timeout)
    const sessionInstances = this.instances.get(session);
    if (sessionInstances) {
      for (const key of sessionInstances.keys()) {
        this.unseenInstances.delete(this.unseenKey(session, key));
      }
    }
    return true;
  }

  dismiss(session: string, agent: string, threadId?: string): boolean {
    const sessionInstances = this.instances.get(session);
    if (!sessionInstances) return false;

    const exactKey = instanceKey(agent, threadId);
    let removed = sessionInstances.delete(exactKey);
    if (removed) this.unseenInstances.delete(this.unseenKey(session, exactKey));

    // Also remove any synthetic pane-backed entries matching (agent, threadId).
    // These have keys like `agent:threadId:pane:paneId` (or `agent:pane:paneId`
    // when no threadId) and are invisible to instanceKey-based lookup.
    for (const [k, event] of [...sessionInstances.entries()]) {
      if (!isSyntheticPaneKey(k)) continue;
      if (event.agent !== agent) continue;
      if (event.threadId !== threadId) continue;
      sessionInstances.delete(k);
      this.unseenInstances.delete(this.unseenKey(session, k));
      removed = true;
    }

    if (!removed) return false;
    if (sessionInstances.size === 0) {
      this.instances.delete(session);
    }
    return true;
  }

  /** Ensure a specific watcher instance only exists in one session.
   *  Useful when cwd-based watcher resolution and live pane ownership disagree.
   *  Returns true if any duplicate instances were removed from other sessions. */
  dedupeInstanceToSession(session: string, agent: string, threadId?: string): boolean {
    if (!threadId) return false;
    const key = instanceKey(agent, threadId);
    let changed = false;

    for (const [otherSession, sessionInstances] of this.instances) {
      if (otherSession === session) continue;
      if (!sessionInstances.delete(key)) continue;
      this.unseenInstances.delete(this.unseenKey(otherSession, key));
      if (sessionInstances.size === 0) {
        this.instances.delete(otherSession);
      }
      changed = true;
    }

    return changed;
  }

  pruneStuck(timeoutMs: number): boolean {
    const now = Date.now();
    let changed = false;
    for (const [session, sessionInstances] of this.instances) {
      for (const [key, event] of sessionInstances) {
        if ((event.status === "running" || event.status === "tool-running") && now - event.ts > timeoutMs) {
          if (event.liveness === "alive") {
            event.status = "stale";
            changed = true;
          } else {
            sessionInstances.delete(key);
            this.unseenInstances.delete(this.unseenKey(session, key));
            changed = true;
          }
        }
      }
      if (sessionInstances.size === 0) {
        this.instances.delete(session);
        changed = true;
      }
    }
    return changed;
  }

  findHibernationCandidates(now: number, idleAfterMs: number): HibernationCandidate[] {
    const candidates: HibernationCandidate[] = [];
    for (const [session, sessionInstances] of this.instances) {
      if (this.active.has(session)) continue;
      for (const event of sessionInstances.values()) {
        if (event.liveness !== "alive" || !event.paneId) continue;
        if (!HIBERNATABLE_STATUSES.has(event.status)) continue;
        if (now - event.ts <= idleAfterMs) continue;
        candidates.push({
          session,
          agent: event.agent,
          threadId: event.threadId,
          threadName: event.threadName,
          paneId: event.paneId,
        });
      }
    }
    return candidates;
  }

  markHibernated(candidate: HibernationCandidate, ts = Date.now()): boolean {
    const sessionInstances = this.instances.get(candidate.session);
    if (!sessionInstances) return false;

    let changed = false;
    for (const [key, event] of sessionInstances) {
      if (event.agent !== candidate.agent) continue;
      if (event.paneId !== candidate.paneId) continue;
      if (event.threadId !== candidate.threadId) continue;

      event.status = "hibernated";
      event.ts = ts;
      event.liveness = "exited";
      event.paneId = undefined;
      this.unseenInstances.delete(this.unseenKey(candidate.session, key));
      changed = true;
    }
    return changed;
  }

  /** Auto-prune terminal instances older than timeout, but only if instance is not unseen or alive */
  pruneTerminal(): void {
    const now = Date.now();
    for (const [session, sessionInstances] of this.instances) {
      for (const [key, event] of sessionInstances) {
        if (!TERMINAL_STATUSES.has(event.status)) continue;
        const ukey = this.unseenKey(session, key);
        if (this.unseenInstances.has(ukey)) continue; // Don't prune unseen — user hasn't looked yet
        if (event.liveness === "alive") continue; // Don't prune agents backed by live panes
        if (now - event.ts > TERMINAL_PRUNE_MS) {
          sessionInstances.delete(key);
        }
      }
      if (sessionInstances.size === 0) {
        this.instances.delete(session);
      }
    }
  }

  isUnseen(session: string): boolean {
    // Session is unseen if any instance within it is unseen
    const sessionInstances = this.instances.get(session);
    if (!sessionInstances) return false;
    for (const key of sessionInstances.keys()) {
      if (this.unseenInstances.has(this.unseenKey(session, key))) return true;
    }
    return false;
  }

  getUnseen(): string[] {
    // Derive session-level unseen from per-instance tracking
    const sessions = new Set<string>();
    for (const ukey of this.unseenInstances) {
      sessions.add(ukey.split("\0")[0]!);
    }
    return [...sessions];
  }

  handleFocus(session: string): boolean {
    this.active.clear();
    this.active.add(session);

    const hadUnseen = this.isUnseen(session);
    if (hadUnseen) {
      // Clear unseen flags — keep terminal instances visible (as "seen")
      // pruneTerminal will clean them up after timeout
      const sessionInstances = this.instances.get(session);
      if (sessionInstances) {
        for (const key of sessionInstances.keys()) {
          this.unseenInstances.delete(this.unseenKey(session, key));
        }
      }
    }
    return hadUnseen;
  }

  setActiveSessions(sessions: string[]): void {
    this.active.clear();
    for (const s of sessions) this.active.add(s);
  }

  /** Fold pane scanner results into the tracker.
   *  The scanner reports {agent, paneId} and may include threadId when it can
   *  resolve a live process to a specific watcher instance.
   *
   *  1. Entries with liveness "alive" whose paneId is missing from the scan → "exited"
   *  2. Exact threadId matches enrich the corresponding watcher entry.
   *  3. If no exact match exists, unambiguous single-instance matches fall back by agent.
   *  4. Otherwise create or update a synthetic pane-backed idle entry.
   *  Returns true if anything changed (caller uses this for broadcast decisions). */
  applyPanePresence(session: string, paneAgents: PanePresenceInput[]): boolean {
    let changed = false;
    let sessionInstances = this.instances.get(session);

    // Index incoming pane IDs and (agent, threadId) tuples for fast lookup.
    // When the scanner can resolve threadIds for an agent (currently only Pi),
    // we use exact (agent, threadId) matching so that a pane hosting one live
    // instance doesn't keep stale sibling instances marked alive.
    const activePaneIds = new Set<string>();
    const agentsWithThreadIds = new Set<string>();
    const aliveAgentThreads = new Set<string>(); // `${agent}\0${threadId}`
    for (const pa of paneAgents) {
      activePaneIds.add(pa.paneId);
      if (pa.threadId) {
        agentsWithThreadIds.add(pa.agent);
        aliveAgentThreads.add(`${pa.agent}\0${pa.threadId}`);
      }
    }

    // 1. Transition previously-alive entries whose pane (or thread) disappeared → "exited".
    //    Synthetic pane-backed entries (no watcher event) are removed outright on exit,
    //    since they exist only as a presence marker for a live process.
    if (sessionInstances) {
      for (const [k, event] of [...sessionInstances.entries()]) {
        if (event.liveness !== "alive" || !event.paneId) continue;

        const isAlive = event.threadId && agentsWithThreadIds.has(event.agent)
          ? aliveAgentThreads.has(`${event.agent}\0${event.threadId}`)
          : activePaneIds.has(event.paneId);
        if (isAlive) continue;

        if (isSyntheticPaneKey(k)) {
          sessionInstances.delete(k);
          this.unseenInstances.delete(this.unseenKey(session, k));
        } else {
          event.liveness = "exited";
          event.paneId = undefined;
        }
        changed = true;
      }
    }

    // 2. Stamp pane info onto existing entries, or create minimal synthetics
    for (const pa of paneAgents) {
      if (!sessionInstances) {
        sessionInstances = new Map();
        this.instances.set(session, sessionInstances);
      }

      const stampAlive = (target: AgentEvent) => {
        const wasDifferent = target.paneId !== pa.paneId || target.liveness !== "alive";
        target.paneId = pa.paneId;
        target.liveness = "alive";
        if (wasDifferent) changed = true;
      };

      if (pa.threadId) {
        const exactKey = instanceKey(pa.agent, pa.threadId);
        const exactEvent = sessionInstances.get(exactKey);
        if (exactEvent) {
          stampAlive(exactEvent);

          // Drop any synthetic for the same pane now that we have an exact watcher entry.
          const genericSyntheticKey = syntheticPaneKey(pa.agent, pa.paneId);
          const exactSyntheticKey = syntheticPaneKey(pa.agent, pa.paneId, pa.threadId);
          if (sessionInstances.delete(genericSyntheticKey)) {
            this.unseenInstances.delete(this.unseenKey(session, genericSyntheticKey));
          }
          if (sessionInstances.delete(exactSyntheticKey)) {
            this.unseenInstances.delete(this.unseenKey(session, exactSyntheticKey));
          }
          continue;
        }

        const exactSyntheticKey = syntheticPaneKey(pa.agent, pa.paneId, pa.threadId);
        const genericSyntheticKey = syntheticPaneKey(pa.agent, pa.paneId);
        const existing = sessionInstances.get(exactSyntheticKey);
        if (existing) {
          stampAlive(existing);
          continue;
        }

        if (sessionInstances.delete(genericSyntheticKey)) {
          this.unseenInstances.delete(this.unseenKey(session, genericSyntheticKey));
        }

        sessionInstances.set(exactSyntheticKey, {
          agent: pa.agent,
          session,
          status: "idle",
          ts: Date.now(),
          threadId: pa.threadId,
          paneId: pa.paneId,
          liveness: "alive",
        });
        changed = true;
        continue;
      }

      const watcherEntries = [...sessionInstances.entries()]
        .filter(([k, ev]) => ev.agent === pa.agent && !isSyntheticPaneKey(k));

      if (watcherEntries.length === 1) {
        stampAlive(watcherEntries[0]![1]);
        continue;
      }

      const syntheticKey = syntheticPaneKey(pa.agent, pa.paneId);
      const existing = sessionInstances.get(syntheticKey);
      if (!existing) {
        sessionInstances.set(syntheticKey, {
          agent: pa.agent,
          session,
          status: "idle",
          ts: Date.now(),
          paneId: pa.paneId,
          liveness: "alive",
        });
        changed = true;
      } else {
        stampAlive(existing);
      }
    }

    return changed;
  }
}
