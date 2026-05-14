/**
 * Kilo agent watcher
 *
 * Polls the Kilo SQLite database (~/.local/share/kilo/kilo.db)
 * to determine agent status and emits events mapped to mux sessions
 * via the `directory` field on each Kilo session row.
 *
 * All queries use bun:sqlite in readonly mode.
 *
 * ## Kilo SQLite Schema (observed v0.1, May 2026)
 *
 * ### Tables used
 *   - `session` — one row per Kilo session
 *       - `id`, `project_id`, `parent_id`, `slug`, `directory`, `title`, `version`,
 *         `share_url`, `summary_additions`, `summary_deletions`, `summary_files`,
 *         `summary_diffs`, `revert`, `permission`, `time_created`, `time_updated`,
 *         `time_compacting`, `time_archived`, `workspace_id`, `path`
 *   - `message` — one row per message (user prompt or assistant response)
 *       - `id`, `session_id`, `time_created`, `time_updated`, `data` (JSON)
 *   - `part` — one row per content block within a message
 *       - `id`, `message_id`, `session_id`, `data` (JSON)
 *
 * ### Key differences from OpenCode schema
 *   - Session table has many additional fields (`project_id`, `parent_id`, `slug`, etc.)
 *   - Message `data` JSON includes Kilo-specific fields:
 *     - `agent`: agent mode name (e.g., "code", "architect")
 *     - `mode`: operation mode (e.g., "code")
 *     - `parentID`: parent message reference
 *     - `path`: { cwd, root } working directory info
 *     - `cost`, `tokens`: usage tracking
 *   - User messages have `summary` and `model` fields instead of plain text
 *   - No `error` object observed in message data (errors tracked differently)
 *
 * ### Message data JSON structure (assistant)
 *   ```
 *   {
 *     role: "assistant",
 *     time: { created: number, completed?: number },
 *     finish?: "stop" | "tool-calls" | null,
 *     parentID?: string,
 *     modelID?: string,
 *     providerID?: string,
 *     mode?: string,
 *     agent?: string,
 *     path?: { cwd: string, root: string },
 *     cost?: number,
 *     tokens?: { input: number, output: number, reasoning: number, cache: {...} }
 *   }
 *   ```
 *
 * ### Message data JSON structure (user)
 *   ```
 *   {
 *     role: "user",
 *     time: { created: number },
 *     agent?: string,
 *     model?: { providerID: string, modelID: string },
 *     summary?: { diffs: [...] }
 *   }
 *   ```
 *
 * ### Part data JSON structure (selected types)
 *   - `{ type: "text", text: "..." }`
 *   - `{ type: "reasoning", text: "..." }`
 *   - `{ type: "tool", tool: "bash", callID: "...", state: { status, input, output } }`
 *   - `{ type: "step-start" }`
 *   - `{ type: "step-finish", reason: "stop" | "tool-calls" | "unknown" | "error" }`
 *   - `{ type: "patch" }`, `{ type: "file" }`, `{ type: "agent" }`, `{ type: "compaction" }`
 *
 * ### Tool part state.status values
 *   - `"pending"`    — tool queued but not yet started (very rare)
 *   - `"running"`    — tool actively executing
 *   - `"completed"`  — tool finished successfully
 *   - `"error"`      — tool failed
 *
 * ## Message Lifecycle (observed)
 *
 * ### Normal turn completion (text only)
 *   1. User prompt:    role=user, finish=null, time.completed=null
 *   2. Streaming:      role=assistant, finish=null, time.completed=null
 *      Parts appear: step-start → reasoning → text → step-finish(reason=stop)
 *   3. Complete:       role=assistant, finish=stop, time.completed=<ts>
 *
 * ### Tool use cycle
 *   1. User prompt:    role=user
 *   2. Streaming:      role=assistant, finish=null, time.completed=null
 *      Parts: step-start → reasoning → tool(state.status=pending→running→completed)
 *             → step-finish(reason=tool-calls)
 *   3. Tool complete:  role=assistant, finish=tool-calls, time.completed=<ts>
 *   4. Next step:      NEW assistant message (same session), finish=null (streaming)
 *      Kilo creates a new message row for each step, NOT a user
 *      message with tool_result like Amp/Claude Code.
 *   5. Final:          role=assistant, finish=stop, time.completed=<ts>
 *
 * ### Finish values
 *   - `null`          — streaming / in-progress (no time.completed)
 *   - `"stop"`        — normal turn completion
 *   - `"tool-calls"`  — tool calls pending; next assistant message coming
 *
 * ### Interrupt scenarios
 *   - SIGINT: process killed, DB stops updating. Last message stuck as
 *     finish=null, time.completed=null. Indistinguishable from active streaming
 *     except session.time_updated stops advancing.
 *   - SIGKILL: identical to SIGINT — no cleanup, DB frozen mid-state.
 *
 * ### Process death detection
 *   When status is "running" but session.time_updated hasn't advanced for
 *   STUCK_MS (15s), we assume the process died and emit "stale".
 *
 * ### Multi-step assistant turns
 *   Unlike Amp/Claude Code, Kilo creates a NEW assistant message row
 *   for each step in a tool-use chain. The first step has finish=tool-calls,
 *   the next step starts as a new streaming assistant message. There is NO
 *   user message with tool_result between steps. Status detection only needs
 *   to look at the LAST message in the session.
 *
 * ### Permission prompts
 *   Kilo does NOT have a visible permission prompt mechanism like Claude
 *   Code. Tool execution is either auto-approved or denied via session-level
 *   permission config. There is no "waiting for user approval" state.
 *   The TOOL_USE_WAIT_MS heuristic from the previous implementation is removed.
 */

import { existsSync } from "fs";
import { homedir } from "os";
import { join } from "path";
import type { AgentStatus } from "../../contracts/agent";
import type { AgentWatcher, AgentWatcherContext } from "../../contracts/agent-watcher";

// --- Types ---

interface SessionRow {
  id: string;
  title: string | null;
  directory: string;
  time_updated: number;
}

interface MessageRow {
  id: string;
  data: string;
}

interface MessageData {
  role?: string;
  finish?: string | null;
  time?: { created?: number; completed?: number };
  error?: { name?: string; data?: { message?: string } };
}

const POLL_MS = 3000;
const STALE_MS = 5 * 60 * 1000;
/** How long a "running" session can go without DB updates before we assume the process died */
const STUCK_MS = 15_000;

// --- Status detection ---

/**
 * Determine the agent status from the last message in a session.
 *
 * The logic checks error state first (since errors can have any finish value),
 * then the finish field, then falls back to streaming detection.
 */
export function determineStatus(msg: MessageData | null): AgentStatus {
  if (!msg) return "idle";

  // Handle Kilo schema - try to parse data string if present
  if (typeof msg?.data === "string") {
    try {
      const parsed = JSON.parse(msg.data);
      msg = parsed;
    } catch {
      return "idle";
    }
  }

  if (!msg?.role) {
    // Kilo schema might omit role field - fallback to finish field inference
    if (msg?.finish === "stop") return "done";
    if (msg?.finish === "tool-calls") return "running";
    if (msg?.finish === "error") return "error";
    return "idle";
  }

  // Error states take precedence — the error object is the definitive signal
  if (msg.error?.name) {
    if (msg.error.name === "MessageAbortedError") return "interrupted";
    // APIError, UnknownError, etc. → error
    return "error";
  }

  if (msg.role === "assistant") {
    // Check finish field (set when message is complete)
    if (msg.finish === "tool-calls") return "running";
    if (msg.finish === "stop") return "done";
    if (msg.finish === "error") return "error";
    if (msg.finish === "unknown") return "done";

    // No finish value: either actively streaming or has an error without finish
    // If time.completed is set but no finish → unusual edge case, treat as done
    if (msg.time?.completed && !msg.finish) return "done";

    // Actively streaming (no finish, no time.completed)
    return "running";
  }

  if (msg.role === "user") return "running";

  return "idle";
}

// --- Session snapshot ---

interface SessionSnapshot {
  status: AgentStatus;
  title: string | null;
  directory: string;
  lastTimestamp: number;
  /** When we last observed the session's time_updated to advance. For stuck detection. */
  lastGrowthAt: number;
}

// --- Watcher implementation ---

export class KiloAgentWatcher implements AgentWatcher {
  readonly name = "kilo";

  private sessions = new Map<string, SessionSnapshot>();
  private pollTimer: ReturnType<typeof setInterval> | null = null;
  private ctx: AgentWatcherContext | null = null;
  private db: any = null;
  private dbPath: string;
  private polling = false;
  private seeded = false;

  constructor() {
    this.dbPath = process.env.KILO_DB_PATH
      ?? join(homedir(), ".local", "share", "kilo", "kilo.db");
  }

  start(ctx: AgentWatcherContext): void {
    this.ctx = ctx;
    setTimeout(() => this.poll(), 50);
    this.pollTimer = setInterval(() => this.poll(), POLL_MS);
  }

  stop(): void {
    if (this.pollTimer) { clearInterval(this.pollTimer); this.pollTimer = null; }
    try { this.db?.close(); } catch {}
    this.db = null;
    this.ctx = null;
  }

  /** Emit a status change event if we have a valid session mapping */
  private emitStatus(sessionId: string, snapshot: SessionSnapshot): boolean {
    if (!this.ctx || !snapshot.directory || snapshot.status === "idle") return false;

    const session = this.ctx.resolveThreadOwner?.("kilo", sessionId, snapshot.title)?.session
      ?? this.ctx.resolveSession(snapshot.directory);
    if (!session) return false;

    this.ctx.emit({
      agent: "kilo",
      session,
      status: snapshot.status,
      ts: Date.now(),
      threadId: sessionId,
      ...(snapshot.title && { threadName: snapshot.title }),
    });
    return true;
  }

  private openDb(): boolean {
    if (this.db) return true;
    if (!existsSync(this.dbPath)) return false;
    try {
      const { Database } = require("bun:sqlite");
      this.db = new Database(this.dbPath, { readonly: true });
      return true;
    } catch {
      return false;
    }
  }

  /** Read the last message for a session and determine status */
  private readSessionStatus(sessionId: string): AgentStatus {
    let lastMsg: MessageRow | null = null;
    try {
      lastMsg = this.db.query(
        `SELECT id, data FROM message WHERE session_id = ? ORDER BY time_created DESC LIMIT 1`,
      ).get(sessionId);
    } catch {
      return "idle";
    }

    if (!lastMsg) return "idle";

    let msgData: MessageData | null = null;
    try { msgData = JSON.parse(lastMsg.data); } catch {}

    return determineStatus(msgData);
  }

  private poll(): void {
    if (!this.ctx || this.polling) return;
    this.polling = true;

    try {
      if (!this.openDb()) return;

       let rows: SessionRow[];
       const staleThreshold = Date.now() - STALE_MS;
       try {
         rows = this.db.query(
           `SELECT id, title, directory, time_updated FROM session WHERE time_updated > ? ORDER BY time_updated DESC`,
         ).all(staleThreshold);
       } catch {
         // Try alternative query for Kilo schema if the above fails
         try {
           rows = this.db.query(
             `SELECT id, title, directory, time_updated FROM session WHERE time_updated > ? ORDER BY time_updated DESC`,
           ).all(staleThreshold);
         } catch {
           try { this.db.close(); } catch {}
           this.db = null;
           return;
         }
       }

      const now = Date.now();

      // --- Seed: record current state, then emit non-idle sessions ---
      if (!this.seeded) {
        for (const row of rows) {
          const status = this.readSessionStatus(row.id);
          const snapshot: SessionSnapshot = {
            status,
            title: row.title,
            directory: row.directory,
            lastTimestamp: row.time_updated,
            lastGrowthAt: now,
          };
          this.sessions.set(row.id, snapshot);
        }
        this.seeded = true;

        for (const [sessionId, snapshot] of this.sessions) {
          this.emitStatus(sessionId, snapshot);
        }
        return;
      }

      // --- Incremental: detect changes via time_updated ---
      for (const row of rows) {
        const prev = this.sessions.get(row.id);

        if (prev && prev.lastTimestamp === row.time_updated) {
          // Session unchanged — check for stuck detection
          if (prev.status === "running" && now - prev.lastGrowthAt >= STUCK_MS) {
            prev.status = "stale";
            this.emitStatus(row.id, prev);
          }
          continue;
        }

        // Session changed — read current status
        const status = this.readSessionStatus(row.id);
        const prevStatus = prev?.status;

        const snapshot: SessionSnapshot = {
          status,
          title: row.title,
          directory: row.directory,
          lastTimestamp: row.time_updated,
          lastGrowthAt: now,
        };
        this.sessions.set(row.id, snapshot);

        // Only emit if status or title changed AND we had a previous state
        // (new sessions appearing for the first time after seed don't emit)
        if (prev && (status !== prevStatus || prev.title !== row.title)) {
          this.emitStatus(row.id, snapshot);
        }
      }
    } finally {
      this.polling = false;
    }
  }
}
