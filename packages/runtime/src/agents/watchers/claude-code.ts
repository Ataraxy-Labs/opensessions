/**
 * Claude Code agent watcher
 *
 * Watches ~/.claude/projects/ for JSONL file changes,
 * determines agent status from journal entries, and emits events
 * mapped to mux sessions via the project directory encoded in folder names.
 *
 * Directory structure: ~/.claude/projects/<encoded-path>/<session-id>.jsonl
 * Encoded path: /Users/foo/myproject → -Users-foo-myproject
 *
 * All file I/O is async to avoid blocking the server event loop.
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

interface JournalEntry {
  type?: string;
  customTitle?: string;
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
}

const POLL_MS = 2000;
const STALE_MS = 5 * 60 * 1000;

// --- Status detection ---

export function determineStatus(entry: JournalEntry): AgentStatus {
  const msg = entry.message;
  if (!msg?.role) return "idle";

  const content = msg.content;
  const items: ContentItem[] = Array.isArray(content)
    ? content
    : typeof content === "string"
      ? [{ type: "text", text: content }]
      : [];

  if (msg.role === "assistant") {
    const hasToolUse = items.some((c) => c.type === "tool_use");
    return hasToolUse ? "running" : "done";
  }

  if (msg.role === "user") return "running";

  return "idle";
}

function extractCustomTitle(entry: JournalEntry): string | undefined {
  if (entry.type === "custom-title" && entry.customTitle) return entry.customTitle;
  return undefined;
}


function extractThreadName(entry: JournalEntry): string | undefined {
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
  if (text.startsWith("<") || text.startsWith("{")) return undefined;
  return text.slice(0, 80);
}

/** Decode Claude's encoded project dir name back to a path */
function decodeProjectDir(encoded: string): string {
  return encoded.replace(/-/g, "/");
}

// --- Watcher implementation ---

export class ClaudeCodeAgentWatcher implements AgentWatcher {
  readonly name = "claude-code";

  private sessions = new Map<string, SessionState>();
  private fsWatchers: FSWatcher[] = [];
  private pollTimer: ReturnType<typeof setInterval> | null = null;
  private ctx: AgentWatcherContext | null = null;
  private projectsDir: string;
  private scanning = false;
  private seeded = false;

  constructor() {
    this.projectsDir = join(homedir(), ".claude", "projects");
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

  private async processFile(filePath: string, projectDir: string): Promise<void> {
    if (!this.ctx) return;

    let size: number;
    try { size = (await stat(filePath)).size; } catch { return; }

    const threadId = basename(filePath, ".jsonl");
    const prev = this.sessions.get(threadId);

    if (prev && size === prev.fileSize) return;

    // Seed mode: read last entry to capture real status for post-seed emit
    if (!this.seeded) {
      let text: string;
      try {
        text = await Bun.file(filePath).text();
      } catch { return; }

      const lines = text.split("\n").filter(Boolean);
      let latestStatus: AgentStatus = "idle";
      let threadName: string | undefined;

      for (const line of lines) {
        let entry: JournalEntry;
        try { entry = JSON.parse(line); } catch { continue; }
        const customTitle = extractCustomTitle(entry);
        if (customTitle) threadName = customTitle;
        else if (!threadName) {
          const name = extractThreadName(entry);
          if (name) threadName = name;
        }
        latestStatus = determineStatus(entry);
      }

      this.sessions.set(threadId, { status: latestStatus, fileSize: size, threadName, projectDir });
      return;
    }

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

    for (const line of lines) {
      let entry: JournalEntry;
      try { entry = JSON.parse(line); } catch { continue; }

      const customTitle = extractCustomTitle(entry);
      if (customTitle) threadName = customTitle;
      else if (!threadName) {
        const name = extractThreadName(entry);
        if (name) threadName = name;
      }

      latestStatus = determineStatus(entry);
    }

    const prevStatus = prev?.status;
    const prevThreadName = prev?.threadName;
    this.sessions.set(threadId, { status: latestStatus, fileSize: size, threadName, projectDir });

    if (latestStatus !== prevStatus || threadName !== prevThreadName) {
      const session = this.ctx.resolveSession(projectDir);
      if (session) {
        this.ctx.emit({
          agent: "claude-code",
          session,
          status: latestStatus,
          ts: Date.now(),
          threadId,
          threadName,
        });
      }
    }
  }

  private async scan(): Promise<void> {
    if (this.scanning || !this.ctx) return;
    this.scanning = true;

    try {
      let dirs: string[];
      try { dirs = await readdir(this.projectsDir); } catch { return; }
      const now = Date.now();

      for (const dir of dirs) {
        const dirPath = join(this.projectsDir, dir);
        try { if (!(await stat(dirPath)).isDirectory()) continue; } catch { continue; }

        const projectDir = decodeProjectDir(dir);

        let files: string[];
        try { files = await readdir(dirPath); } catch { continue; }

        for (const file of files) {
          if (!file.endsWith(".jsonl")) continue;
          const filePath = join(dirPath, file);
          let fileStat;
          try { fileStat = await stat(filePath); } catch { continue; }
          if (now - fileStat.mtimeMs > STALE_MS) continue;
          await this.processFile(filePath, projectDir);
        }
      }
    } finally {
      if (!this.seeded) {
        this.seeded = true;
        // Emit seeded sessions with non-idle status (like amp watcher does)
        for (const [threadId, state] of this.sessions) {
          if (state.status === "idle" || !state.projectDir) continue;
          const session = this.ctx?.resolveSession(state.projectDir);
          if (!session) continue;
          this.ctx?.emit({
            agent: "claude-code",
            session,
            status: state.status,
            ts: Date.now(),
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
    try { dirs = require("fs").readdirSync(this.projectsDir); } catch { return; }

    for (const dir of dirs) {
      const dirPath = join(this.projectsDir, dir);
      try { if (!require("fs").statSync(dirPath).isDirectory()) continue; } catch { continue; }

      const projectDir = decodeProjectDir(dir);
      try {
        const w = watch(dirPath, (_eventType, filename) => {
          if (!filename?.endsWith(".jsonl")) return;
          this.processFile(join(dirPath, filename), projectDir);
        });
        this.fsWatchers.push(w);
      } catch {}
    }

    // Watch projects dir for new project directories
    try {
      const w = watch(this.projectsDir, (eventType, filename) => {
        if (eventType !== "rename" || !filename) return;
        const dirPath = join(this.projectsDir, filename);
        try { if (!require("fs").statSync(dirPath).isDirectory()) return; } catch { return; }

        const projectDir = decodeProjectDir(filename);
        try {
          const sub = watch(dirPath, (_et, fn) => {
            if (!fn?.endsWith(".jsonl")) return;
            this.processFile(join(dirPath, fn), projectDir);
          });
          this.fsWatchers.push(sub);
        } catch {}
      });
      this.fsWatchers.push(w);
    } catch {}
  }
}
