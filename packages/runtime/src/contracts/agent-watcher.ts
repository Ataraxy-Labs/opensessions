import type { AgentEvent } from "./agent";

/**
 * Callback context provided by the server to each watcher.
 * Lets watchers resolve project directories to mux session names
 * and emit events without knowing about server internals.
 */
export interface AgentThreadOwner {
  session: string;
  paneId?: string;
}

export interface AgentWatcherContext {
  /** Resolve a project directory path to a mux session name, or null if unmatched */
  resolveSession(projectDir: string): string | null;
  /** Resolve the live owner for a specific agent thread when pane-backed identity is available. */
  resolveThreadOwner?(agent: string, threadId?: string, threadName?: string): AgentThreadOwner | null;
  /** Emit an agent event (applied to tracker + broadcast automatically) */
  emit(event: AgentEvent): void;
}

/**
 * Interface for agent watchers that detect agent status by watching
 * external data sources (thread files, databases, etc).
 *
 * Implementations:
 *   - amp: watches ~/.local/share/amp/threads/*.json
 *   - claude-code: watches ~/.claude/projects/ JSONL files
 *   - codex: watches ~/.codex/sessions/ JSONL transcripts
 *   - opencode: polls OpenCode SQLite database
 *   - pi: watches ~/.pi/agent/sessions/ JSONL transcripts
 *
 * To add a new watcher:
 *   1. Create a file implementing AgentWatcher
 *   2. Register it via PluginAPI.registerWatcher() or in start.ts
 */
export interface AgentWatcher {
  /** Unique name for this watcher (e.g. "amp", "claude-code") */
  readonly name: string;

  /** Start watching. Called once by the server with the watcher context. */
  start(ctx: AgentWatcherContext): void;

  /** Stop watching and clean up resources. */
  stop(): void;
}
