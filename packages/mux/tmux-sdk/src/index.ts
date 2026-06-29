// --- Types ---

export interface TmuxClientOptions {
  /** Path to tmux binary (default: "tmux") */
  bin?: string;
  /** tmux socket name (-L flag) */
  socketName?: string;
  /** tmux socket path (-S flag) */
  socketPath?: string;
  /** Whether run() throws on non-zero exit (default: false) */
  throwOnError?: boolean;
}

export interface TmuxRunResult {
  args: readonly string[];
  exitCode: number;
  stdout: string;
  stderr: string;
  ok: boolean;
}

export class TmuxError extends Error {
  readonly args: readonly string[];
  readonly exitCode: number;
  readonly stdout: string;
  readonly stderr: string;

  constructor(result: TmuxRunResult) {
    super(result.stderr || `tmux exited with code ${result.exitCode}`);
    this.name = "TmuxError";
    this.args = result.args;
    this.exitCode = result.exitCode;
    this.stdout = result.stdout;
    this.stderr = result.stderr;
  }
}

// --- Typed result interfaces ---

export interface SessionInfo {
  id: string;
  name: string;
  createdAt: number;
  attachedClients: number;
  windowCount: number;
  dir: string;
}

export interface WindowInfo {
  id: string;
  sessionId: string;
  sessionName: string;
  index: number;
  name: string;
  active: boolean;
  paneCount: number;
}

export interface PaneInfo {
  id: string;
  sessionName: string;
  windowId: string;
  windowIndex: number;
  index: number;
  active: boolean;
  tty: string;
  pid: number;
  cwd: string;
  command: string;
  title: string;
  width: number;
  height: number;
  left: number;
  right: number;
}

export interface ClientInfo {
  name: string;
  tty: string;
  pid: number;
  sessionName: string;
  width: number;
  height: number;
}

// --- Hook types ---

export const HOOK_NAMES = [
  "client-session-changed",
  "client-resized",
  "client-attached",
  "client-detached",
  "client-focus-in",
  "client-focus-out",
  "session-created",
  "session-closed",
  "session-renamed",
  "session-window-changed",
  "window-linked",
  "window-unlinked",
  "window-renamed",
  "window-layout-changed",
  "after-select-window",
  "after-new-window",
  "after-resize-pane",
  "pane-died",
  "pane-exited",
  "pane-focus-in",
  "pane-focus-out",
] as const;

export type HookName = (typeof HOOK_NAMES)[number] | (string & {});

// --- Pane list scoping ---

export type PaneScope =
  | { scope?: "all" }
  | { scope: "session"; target: string }
  | { scope: "window"; target: string };

// --- Split-window options ---

export interface SplitWindowOptions {
  target: string;
  direction?: "horizontal" | "vertical";
  /** "before" = -b flag (split left/above), default is right/below */
  before?: boolean;
  /** Split against the entire window instead of only the target pane. */
  fullWindow?: boolean;
  size?: number;
  command?: string;
}

// --- Internal parser helpers ---

/** Field delimiter — tab character, universally supported by tmux */
const SEP = "\t";

type Parser<T> = (raw: string) => T;

const str: Parser<string> = (s) => s;
const int: Parser<number> = (s) => {
  const n = parseInt(s, 10);
  return isNaN(n) ? 0 : n;
};
const bool: Parser<boolean> = (s) => s === "1";

type FieldSpec<T> = { [K in keyof T]: readonly [field: string, parse: Parser<T[K]>] };

function buildFormat<T>(spec: FieldSpec<T>): string {
  const keys = Object.keys(spec) as (keyof T)[];
  return keys.map((k) => `#{${spec[k][0]}}`).join(SEP);
}

function parseRows<T>(spec: FieldSpec<T>, raw: string): T[] {
  if (!raw) return [];
  const keys = Object.keys(spec) as (keyof T)[];
  return raw.split("\n").reduce<T[]>((acc, line) => {
    if (!line) return acc;
    const parts = line.split(SEP);
    const obj = {} as T;
    for (let i = 0; i < keys.length; i++) {
      const key = keys[i]!;
      const [, parse] = spec[key];
      obj[key] = parse(parts[i] ?? "");
    }
    acc.push(obj);
    return acc;
  }, []);
}

// --- Specs ---

const SESSION_SPEC: FieldSpec<SessionInfo> = {
  id: ["session_id", str],
  name: ["session_name", str],
  createdAt: ["session_created", int],
  attachedClients: ["session_attached", int],
  windowCount: ["session_windows", int],
  dir: ["pane_current_path", str],
};

const WINDOW_SPEC: FieldSpec<WindowInfo> = {
  id: ["window_id", str],
  sessionId: ["session_id", str],
  sessionName: ["session_name", str],
  index: ["window_index", int],
  name: ["window_name", str],
  active: ["window_active", bool],
  paneCount: ["window_panes", int],
};

const PANE_SPEC: FieldSpec<PaneInfo> = {
  id: ["pane_id", str],
  sessionName: ["session_name", str],
  windowId: ["window_id", str],
  windowIndex: ["window_index", int],
  index: ["pane_index", int],
  active: ["pane_active", bool],
  tty: ["pane_tty", str],
  pid: ["pane_pid", int],
  cwd: ["pane_current_path", str],
  command: ["pane_current_command", str],
  title: ["pane_title", str],
  width: ["pane_width", int],
  height: ["pane_height", int],
  left: ["pane_left", int],
  right: ["pane_right", int],
};

const CLIENT_SPEC: FieldSpec<ClientInfo> = {
  name: ["client_name", str],
  tty: ["client_tty", str],
  pid: ["client_pid", int],
  sessionName: ["session_name", str],
  width: ["client_width", int],
  height: ["client_height", int],
};

// --- Format string constants (pre-built for performance) ---

const SESSION_FORMAT = buildFormat(SESSION_SPEC);
const WINDOW_FORMAT = buildFormat(WINDOW_SPEC);
const PANE_FORMAT = buildFormat(PANE_SPEC);
const CLIENT_FORMAT = buildFormat(CLIENT_SPEC);

// --- TmuxClient ---

export class TmuxClient {
  private bin: string;
  private globalArgs: string[];
  private throwOnError: boolean;

  constructor(options: TmuxClientOptions = {}) {
    this.bin = options.bin ?? "tmux";
    this.globalArgs = [];
    if (options.socketName) this.globalArgs.push("-L", options.socketName);
    if (options.socketPath) this.globalArgs.push("-S", options.socketPath);
    this.throwOnError = options.throwOnError ?? false;
  }

  /**
   * Low-level escape hatch: run any tmux subcommand and get typed result.
   * All other methods use this internally.
   */
  run(args: readonly string[], options?: { throwOnError?: boolean }): TmuxRunResult {
    const fullArgs = [this.bin, ...this.globalArgs, ...args];
    const shouldThrow = options?.throwOnError ?? this.throwOnError;

    try {
      const result = Bun.spawnSync(fullArgs, { stdout: "pipe", stderr: "pipe" });
      const out: TmuxRunResult = {
        args: fullArgs,
        exitCode: result.exitCode,
        stdout: result.stdout.toString().trim(),
        stderr: result.stderr.toString().trim(),
        ok: result.exitCode === 0,
      };
      if (!out.ok && shouldThrow) throw new TmuxError(out);
      return out;
    } catch (e) {
      if (e instanceof TmuxError) throw e;
      return {
        args: fullArgs,
        exitCode: -1,
        stdout: "",
        stderr: e instanceof Error ? e.message : String(e),
        ok: false,
      };
    }
  }

  // ─── Sessions ──────────────────────────────────────

  listSessions(): SessionInfo[] {
    const { stdout } = this.run(["list-sessions", "-F", SESSION_FORMAT]);
    return parseRows(SESSION_SPEC, stdout);
  }

  newSession(options: { name?: string; cwd?: string; detached?: boolean } = {}): string {
    const args = ["new-session"];
    if (options.detached !== false) args.push("-d");
    if (options.name) args.push("-s", options.name);
    if (options.cwd) args.push("-c", options.cwd);
    args.push("-P", "-F", "#{session_name}");
    const { stdout } = this.run(args);
    return stdout;
  }

  killSession(target: string): void {
    this.run(["kill-session", "-t", target]);
  }

  // ─── Windows ───────────────────────────────────────

  listWindows(options?: { scope?: "all" } | { scope: "session"; target: string }): WindowInfo[] {
    const args = ["list-windows"];
    if (!options || options.scope === "all" || !options.scope) {
      args.push("-a");
    } else if (options.scope === "session") {
      args.push("-t", options.target);
    }
    args.push("-F", WINDOW_FORMAT);
    const { stdout } = this.run(args);
    return parseRows(WINDOW_SPEC, stdout);
  }

  killWindow(target: string): void {
    this.run(["kill-window", "-t", target]);
  }

  // ─── Panes ─────────────────────────────────────────

  listPanes(options?: PaneScope): PaneInfo[] {
    const args = ["list-panes"];
    if (!options || !options.scope || options.scope === "all") {
      args.push("-a");
    } else if (options.scope === "session") {
      args.push("-s", "-t", options.target);
    } else if (options.scope === "window") {
      args.push("-t", options.target);
    }
    args.push("-F", PANE_FORMAT);
    const { stdout } = this.run(args);
    return parseRows(PANE_SPEC, stdout);
  }

  splitWindow(options: SplitWindowOptions): PaneInfo | null {
    const args = ["split-window"];
    if (options.direction === "horizontal" || !options.direction) {
      args.push(options.before ? "-hb" : "-h");
    } else {
      args.push(options.before ? "-vb" : "-v");
    }
    if (options.fullWindow) args.push("-f");
    if (options.size != null) args.push("-l", String(options.size));
    args.push("-t", options.target);
    args.push("-P", "-F", PANE_FORMAT);
    if (options.command) args.push(options.command);
    const { stdout, ok } = this.run(args);
    if (!ok || !stdout) return null;
    const rows = parseRows(PANE_SPEC, stdout);
    return rows[0] ?? null;
  }

  selectPane(target: string): void {
    this.run(["select-pane", "-t", target]);
  }

  setPaneStyle(target: string, style: string): void {
    this.run(["select-pane", "-t", target, "-P", style]);
  }

  setPaneTitle(target: string, title: string): void {
    this.run(["select-pane", "-t", target, "-T", title]);
  }

  killPane(target: string): void {
    this.run(["kill-pane", "-t", target]);
  }

  resetPane(target: string): void {
    this.run(["send-keys", "-R", "-t", target]);
  }

  clearPaneHistory(target: string): void {
    this.run(["clear-history", "-t", target]);
  }

  resizePane(target: string, options: { width?: number; height?: number }): void {
    const args = ["resize-pane", "-t", target];
    if (options.width != null) args.push("-x", String(options.width));
    if (options.height != null) args.push("-y", String(options.height));
    this.run(args);
  }

  breakPane(options: { source: string; name?: string; detached?: boolean }): void {
    const args = ["break-pane"];
    if (options.detached !== false) args.push("-d");
    args.push("-s", options.source);
    if (options.name) args.push("-n", options.name);
    this.run(args);
  }

  // ─── Clients ───────────────────────────────────────

  listClients(): ClientInfo[] {
    const { stdout } = this.run(["list-clients", "-F", CLIENT_FORMAT]);
    return parseRows(CLIENT_SPEC, stdout);
  }

  private getInteractiveClients(): ClientInfo[] {
    return this.listClients().filter((client) => client.tty.length > 0);
  }

  switchClient(target: string, options?: { clientTty?: string }): void {
    const args = ["switch-client"];
    if (options?.clientTty) args.push("-c", options.clientTty);
    args.push("-t", target);
    this.run(args);
  }

  // ─── Display / query ───────────────────────────────

  /**
   * Run `display-message -p` to query a tmux format string.
   * Returns the raw stdout string, trimmed.
   */
  display(format: string, options?: { target?: string }): string {
    const args = ["display-message"];
    if (options?.target) args.push("-t", options.target);
    args.push("-p", format);
    return this.run(args).stdout;
  }

  /**
   * Get the current window ID (display-message -p '#{window_id}')
   */
  getCurrentWindowId(target?: string): string {
    return this.display("#{window_id}", target ? { target } : undefined);
  }

  /**
   * Get the current session name from the first attached client
   */
  getCurrentSession(): string | null {
    const clients = this.getInteractiveClients();
    if (clients.length === 0) return null;
    return clients[0]!.sessionName || null;
  }

  /**
   * Get the client TTY for the current client
   */
  getClientTty(): string {
    return this.display("#{client_tty}");
  }

  /**
   * Get session directory (pane_current_path of the active pane)
   */
  getSessionDir(target: string): string {
    return this.display("#{pane_current_path}", { target });
  }

  /**
   * Get pane count for a session by listing panes with -s flag
   */
  getPaneCount(target: string): number {
    const panes = this.listPanes({ scope: "session", target });
    return panes.length;
  }

  /**
   * Batch pane count retrieval for all sessions.
   * Returns a Map of session name → pane count.
   */
  getAllPaneCounts(): Map<string, number> {
    const counts = new Map<string, number>();
    const panes = this.listPanes({ scope: "all" });
    for (const p of panes) {
      counts.set(p.sessionName, (counts.get(p.sessionName) ?? 0) + 1);
    }
    return counts;
  }

  // ─── Hooks ─────────────────────────────────────────

  setGlobalHook(name: HookName, command: string): void {
    this.run(["set-hook", "-g", name, command]);
  }

  unsetGlobalHook(name: HookName): void {
    this.run(["set-hook", "-gu", name]);
  }

  // ─── Environment ───────────────────────────────────

  setGlobalEnv(name: string, value: string): void {
    this.run(["set-environment", "-g", name, value]);
  }

  getGlobalEnv(name: string): string | null {
    const { stdout, ok } = this.run(["show-environment", "-g", name]);
    if (!ok || !stdout) return null;
    const eqIdx = stdout.indexOf("=");
    return eqIdx >= 0 ? stdout.slice(eqIdx + 1) : null;
  }
}

/**
 * Factory function for creating a TmuxClient with default options.
 */
export function tmux(options?: TmuxClientOptions): TmuxClient {
  return new TmuxClient(options);
}
