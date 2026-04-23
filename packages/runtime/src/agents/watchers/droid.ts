/**
 * Droid (Factory) agent watcher
 *
 * Watches ~/.factory/sessions/ for JSONL file changes,
 * determines agent status from journal entries, and emits events
 * mapped to mux sessions via the project directory encoded in folder names.
 *
 * Directory structure: ~/.factory/sessions/<encoded-path>/<session-id>.jsonl
 * Encoded path: /Users/foo/myproject → -Users-foo-myproject
 *   (Droid replaces `/` with `-` but preserves `.` and `_`)
 *
 * ## Droid JSONL Lifecycle
 *
 * Each JSONL file represents one Droid session. Entries are appended
 * as the session progresses. The top-level `type` field determines the
 * entry category:
 *
 * ### Entry types:
 *   - `session_start` — session metadata: id, sessionTitle, cwd, version
 *   - `message`       — user or assistant message with content blocks
 *   - `todo_state`    — task tracking (no status change)
 *   - `session_end`   — terminal: durationMs, toolCount, finalText
 *
 * ### Message structure:
 *   Droid writes one complete message entry per turn (not streamed).
 *   Assistant messages contain all content blocks for the turn:
 *     - `thinking` blocks (model reasoning)
 *     - `text` blocks (response text)
 *     - `tool_use` blocks (tool calls)
 *   User messages contain:
 *     - `text` blocks (user prompt)
 *     - `tool_result` blocks (tool execution results)
 *
 * ### Status mapping:
 *   - session_end entry                    → "done"
 *   - assistant + content has tool_use     → "running" (tool calls in progress)
 *   - assistant + content has only text    → "done"    (final answer, no more tools)
 *   - assistant + content has thinking     → "running" (model is reasoning)
 *   - user + content is tool_result        → "running" (tool executed, next turn coming)
 *   - user + text                          → "running" (new prompt submitted)
 *   - todo_state / session_start           → null      (no status change)
 *
 * ### Permission prompt detection:
 *   When Droid awaits permission, the last entry is assistant with tool_use
 *   and the file stops growing. After TOOL_USE_WAIT_MS with no growth,
 *   we promote "running" → "waiting".
 *
 * ### Stuck process detection:
 *   If status is "running" or "waiting" and the file hasn't grown for
 *   STUCK_RUNNING_MS, we assume the process died and emit "stale".
 */

import { watch, type FSWatcher } from "fs";
import { readdir, stat } from "fs/promises";
import { join, basename } from "path";
import { homedir } from "os";
import type { AgentStatus } from "../../contracts/agent";
import type { AgentWatcher, AgentWatcherContext } from "../../contracts/agent-watcher";

// --- Types ---

interface ContentItem {
  type?: string;
  text?: string;
}

interface DroidEntry {
  type?: string;
  /** session_start fields */
  id?: string;
  sessionTitle?: string;
  cwd?: string;
  /** message fields */
  message?: {
    role?: string;
    content?: ContentItem[] | string;
  };
}

interface SessionState {
  status: AgentStatus;
  fileSize: number;
  threadName?: string;
  projectDir?: string;
  /** Timestamp when status first became "running" from a tool_use entry */
  toolUseSeenAt?: number;
  /** Timestamp when the file was last observed to have grown (for stuck detection) */
  lastGrowthAt?: number;
  /** File mtime at last observation — used for seed emission ts instead of Date.now() */
  lastMtimeMs?: number;
}

const POLL_MS = 2000;
const STALE_MS = 5 * 60 * 1000;
/** How long to wait before promoting tool_use "running" → "waiting" (permission prompt heuristic) */
const TOOL_USE_WAIT_MS = 3000;
/** How long a "running" session can go without file growth before we assume the process died */
const STUCK_RUNNING_MS = 15_000;

// --- Status detection ---

/**
 * Returns the status implied by a journal entry, or `null` if the entry
 * is a control/metadata record that should not change the current status.
 */
export function determineStatus(entry: DroidEntry): AgentStatus | null {
  // session_end is terminal
  if (entry.type === "session_end") return "done";

  // Skip non-message entries (session_start, todo_state, etc.)
  if (entry.type !== "message") return null;

  const msg = entry.message;
  if (!msg?.role) return null;

  const content = msg.content;
  const items: ContentItem[] = Array.isArray(content)
    ? content
    : typeof content === "string"
      ? [{ type: "text", text: content }]
      : [];

  if (msg.role === "assistant") {
    // tool_use → running (tool calls in progress)
    if (items.some((c) => c.type === "tool_use")) return "running";
    // thinking only → running (model is reasoning, more entries will follow)
    if (items.some((c) => c.type === "thinking") && !items.some((c) => c.type === "text")) return "running";
    // text only (no tool_use) → done (final answer)
    if (items.some((c) => c.type === "text")) return "done";
    return "running";
  }

  if (msg.role === "user") {
    // tool_result → running (tool just executed, next turn coming)
    if (items.some((c) => c.type === "tool_result")) return "running";
    // Normal user message → running (new prompt)
    return "running";
  }

  return null;
}

/** Returns true if the entry is an assistant message containing a tool_use block */
export function isToolUseEntry(entry: DroidEntry): boolean {
  const msg = entry.message;
  if (msg?.role !== "assistant") return false;
  const content = msg.content;
  if (!Array.isArray(content)) return false;
  return content.some((c) => c.type === "tool_use");
}

function extractThreadName(entry: DroidEntry): string | undefined {
  // Prefer sessionTitle from session_start
  if (entry.type === "session_start" && entry.sessionTitle) {
    return entry.sessionTitle.slice(0, 80);
  }

  // Fall back to first user message text
  const msg = entry.message;
  if (msg?.role !== "user") return undefined;

  const content = msg.content;
  let text: string | undefined;

  if (typeof content === "string") {
    text = content;
  } else if (Array.isArray(content)) {
    text = content.find((c) => c.type === "text" && c.text)?.text;
  }

  if (!text) return undefined;
  // Skip system/internal messages
  if (text.startsWith("<") || text.startsWith("{") || text.startsWith("[")) return undefined;
  return text.slice(0, 80);
}

function extractProjectDir(entry: DroidEntry): string | undefined {
  if (entry.type === "session_start" && entry.cwd) return entry.cwd;
  return undefined;
}

/**
 * Decode Droid's encoded project dir name back to a path.
 *
 * Droid encodes by replacing `/` with `-` but preserves `.` and `_`.
 * The encoding is still ambiguous for paths containing literal hyphens.
 * We try the naive decode first, then check if the directory exists.
 */
function decodeProjectDir(encoded: string): string {
  const naive = encoded.replace(/-/g, "/");
  try { if (require("fs").statSync(naive).isDirectory()) return naive; } catch {}
  return `__encoded__:${encoded}`;
}

// --- Watcher implementation ---

export class DroidAgentWatcher implements AgentWatcher {
  readonly name = "droid";

  private sessions = new Map<string, SessionState>();
  private fsWatchers: FSWatcher[] = [];
  private pollTimer: ReturnType<typeof setInterval> | null = null;
  private ctx: AgentWatcherContext | null = null;
  private sessionsDir: string;
  private scanning = false;
  private seeded = false;
  private scanPromise: Promise<void> | null = null;

  constructor() {
    this.sessionsDir = join(homedir(), ".factory", "sessions");
  }

  start(ctx: AgentWatcherContext): void {
    this.ctx = ctx;
    this.setupWatchers();
    setTimeout(() => this.scan(), 50);
    this.pollTimer = setInterval(() => this.scan(), POLL_MS);
  }

  stop(): void {
    for (const w of this.fsWatchers) { try { w.close(); } catch {} }
    this.fsWatchers = [];
    if (this.pollTimer) { clearInterval(this.pollTimer); this.pollTimer = null; }
    this.ctx = null;
  }

  /** Trigger an immediate scan and return when complete.
   *  If a scan is already in flight, waits for it then runs another. */
  async flush(): Promise<void> {
    if (this.scanPromise) await this.scanPromise;
    await this.scan();
  }

  /** Emit a status change event if we have a valid session mapping */
  private emitStatus(threadId: string, state: SessionState): void {
    if (!this.ctx || !this.seeded || !state.projectDir) return;
    const session = this.ctx.resolveSession(state.projectDir);
    if (!session) return;
    this.ctx.emit({
      agent: "droid",
      session,
      status: state.status,
      ts: Date.now(),
      threadId,
      threadName: state.threadName,
    });
  }

  private async processFile(filePath: string, fallbackProjectDir: string): Promise<void> {
    if (!this.ctx) return;

    let size: number;
    let mtimeMs: number;
    try { const s = await stat(filePath); size = s.size; mtimeMs = s.mtimeMs; } catch { return; }

    const threadId = basename(filePath, ".jsonl");
    const prev = this.sessions.get(threadId);

    // --- File unchanged ---
    if (prev && size === prev.fileSize) {
      const now = Date.now();

      // Promote tool_use "running" → "waiting" (permission prompt heuristic)
      if (prev.status === "running" && prev.toolUseSeenAt && now - prev.toolUseSeenAt >= TOOL_USE_WAIT_MS) {
        prev.status = "waiting";
        prev.toolUseSeenAt = undefined;
        this.emitStatus(threadId, prev);
      }

      // Stuck detection: no file growth while running/waiting → assume process died
      if ((prev.status === "running" || prev.status === "waiting") && prev.lastGrowthAt && now - prev.lastGrowthAt >= STUCK_RUNNING_MS) {
        prev.status = "stale";
        prev.toolUseSeenAt = undefined;
        prev.lastGrowthAt = undefined;
        this.emitStatus(threadId, prev);
      }

      return;
    }

    // --- Seed mode: read full file to capture current status ---
    if (!this.seeded) {
      let text: string;
      try {
        text = await Bun.file(filePath).text();
      } catch { return; }

      const lines = text.split("\n").filter(Boolean);
      let latestStatus: AgentStatus = "idle";
      let threadName: string | undefined;
      let projectDir: string | undefined;
      let lastEntryIsToolUse = false;

      for (const line of lines) {
        let entry: DroidEntry;
        try { entry = JSON.parse(line); } catch { continue; }

        const dir = extractProjectDir(entry);
        if (dir) projectDir = dir;

        const name = extractThreadName(entry);
        if (name && !threadName) threadName = name;

        const s = determineStatus(entry);
        if (s !== null) latestStatus = s;
        lastEntryIsToolUse = isToolUseEntry(entry);
      }

      this.sessions.set(threadId, {
        status: latestStatus, fileSize: size, threadName,
        projectDir: projectDir ?? fallbackProjectDir,
        toolUseSeenAt: lastEntryIsToolUse && latestStatus === "running" ? mtimeMs : undefined,
        lastGrowthAt: (latestStatus === "running" || latestStatus === "waiting") ? mtimeMs : undefined,
        lastMtimeMs: mtimeMs,
      });
      return;
    }

    // --- Incremental read: only new bytes ---
    const offset = prev?.fileSize ?? 0;
    if (size <= offset) return;

    let text: string;
    try {
      const buf = await Bun.file(filePath).arrayBuffer();
      text = new TextDecoder().decode(new Uint8Array(buf).subarray(offset, size));
    } catch {
      return;
    }

    const lines = text.split("\n").filter(Boolean);
    let latestStatus: AgentStatus = prev?.status ?? "idle";
    let threadName = prev?.threadName;
    let projectDir = prev?.projectDir;
    let lastEntryIsToolUse = false;

    for (const line of lines) {
      let entry: DroidEntry;
      try { entry = JSON.parse(line); } catch { continue; }

      const dir = extractProjectDir(entry);
      if (dir) projectDir = dir;

      if (!threadName) {
        const name = extractThreadName(entry);
        if (name) threadName = name;
      }

      const s = determineStatus(entry);
      if (s !== null) latestStatus = s;
      lastEntryIsToolUse = isToolUseEntry(entry);
    }

    const prevStatus = prev?.status;
    const prevThreadName = prev?.threadName;
    const now = Date.now();
    const toolUseSeenAt = lastEntryIsToolUse && latestStatus === "running" ? now : undefined;
    this.sessions.set(threadId, {
      status: latestStatus, fileSize: size, threadName,
      projectDir: projectDir ?? fallbackProjectDir,
      toolUseSeenAt, lastGrowthAt: now,
    });

    if (latestStatus !== prevStatus || threadName !== prevThreadName) {
      this.emitStatus(threadId, this.sessions.get(threadId)!);
    }
  }

  private async scan(): Promise<void> {
    if (this.scanning || !this.ctx) return;
    this.scanning = true;

    const p = this.scanInternal();
    this.scanPromise = p;
    await p;
    this.scanPromise = null;
  }

  private async scanInternal(): Promise<void> {
    try {
      let dirs: string[];
      try { dirs = await readdir(this.sessionsDir); } catch { return; }
      const now = Date.now();

      for (const dir of dirs) {
        const dirPath = join(this.sessionsDir, dir);
        try { if (!(await stat(dirPath)).isDirectory()) continue; } catch { continue; }

        const fallbackProjectDir = decodeProjectDir(dir);

        let files: string[];
        try { files = await readdir(dirPath); } catch { continue; }

        for (const file of files) {
          if (!file.endsWith(".jsonl")) continue;
          const filePath = join(dirPath, file);
          let fileStat;
          try { fileStat = await stat(filePath); } catch { continue; }
          if (now - fileStat.mtimeMs > STALE_MS) continue;
          await this.processFile(filePath, fallbackProjectDir);
        }
      }
    } finally {
      if (!this.seeded) {
        this.seeded = true;
        for (const [threadId, state] of this.sessions) {
          if (state.status === "idle" || !state.projectDir) continue;
          const session = this.ctx?.resolveSession(state.projectDir);
          if (!session) continue;
          this.ctx?.emit({
            agent: "droid",
            session,
            status: state.status,
            ts: state.lastMtimeMs ?? Date.now(),
            threadId,
            threadName: state.threadName,
          });
        }
      }
      this.scanning = false;
    }
  }

  private setupWatchers(): void {
    let dirs: string[];
    try { dirs = require("fs").readdirSync(this.sessionsDir); } catch { return; }

    for (const dir of dirs) {
      const dirPath = join(this.sessionsDir, dir);
      try { if (!require("fs").statSync(dirPath).isDirectory()) continue; } catch { continue; }

      const fallbackProjectDir = decodeProjectDir(dir);
      try {
        const w = watch(dirPath, (_eventType, filename) => {
          if (!filename?.endsWith(".jsonl")) return;
          this.processFile(join(dirPath, filename), fallbackProjectDir);
        });
        this.fsWatchers.push(w);
      } catch {}
    }

    // Watch sessions dir for new project directories
    try {
      const w = watch(this.sessionsDir, (eventType, filename) => {
        if (eventType !== "rename" || !filename) return;
        const dirPath = join(this.sessionsDir, filename);
        try { if (!require("fs").statSync(dirPath).isDirectory()) return; } catch { return; }

        const fallbackProjectDir = decodeProjectDir(filename);
        try {
          const sub = watch(dirPath, (_et, fn) => {
            if (!fn?.endsWith(".jsonl")) return;
            this.processFile(join(dirPath, fn), fallbackProjectDir);
          });
          this.fsWatchers.push(sub);
        } catch {}
      });
      this.fsWatchers.push(w);
    } catch {}
  }
}
