import type {
  MuxProviderV1,
  MuxSessionInfo,
  ActiveWindow,
  SidebarPane,
  SidebarPosition,
  WindowCapable,
  SidebarCapable,
  BatchCapable,
} from "@opensessions/mux";
import { appendFileSync } from "fs";

/** Settings for creating a zellij provider (ai-sdk style) */
export interface ZellijProviderSettings {
  /** Override the provider name */
  name?: string;
}

const SIDEBAR_PANE_NAME = "opensessions-sidebar";

function plog(msg: string, data?: Record<string, unknown>) {
  const ts = new Date().toISOString().slice(11, 23);
  const extra = data ? " " + JSON.stringify(data) : "";
  try { appendFileSync("/tmp/opensessions-debug.log", `[${ts}] [zellij] ${msg}${extra}\n`); } catch {}
}

function run(cmd: string[]): string {
  try {
    const result = Bun.spawnSync(cmd, { stdout: "pipe", stderr: "pipe" });
    return result.stdout.toString().trim();
  } catch {
    return "";
  }
}

function runInSession(session: string, args: string[]): string {
  return run(["zellij", "--session", session, "action", ...args]);
}

function runInSessionJSON<T>(session: string, args: string[]): T | null {
  const raw = runInSession(session, args);
  if (!raw) return null;
  try { return JSON.parse(raw) as T; } catch { return null; }
}

function runJSON<T>(cmd: string[]): T | null {
  const raw = run(cmd);
  if (!raw) return null;
  try {
    return JSON.parse(raw) as T;
  } catch {
    return null;
  }
}

interface ZellijPaneInfo {
  id: number;
  is_plugin: boolean;
  is_focused: boolean;
  title: string;
  is_floating: boolean;
  tab_id: number;
  tab_name: string;
  pane_cwd?: string;
  pane_columns?: number;
  pane_x?: number;
}

interface ZellijTabInfo {
  position: number;
  name: string;
  active: boolean;
  selectable_tiled_panes_count: number;
  selectable_floating_panes_count: number;
  tab_id: number;
  viewport_columns?: number;
}

/**
 * ZellijProvider — MuxProvider implementation for Zellij.
 *
 * Key differences from tmux:
 * - Zellij "sessions" are top-level, "tabs" are like tmux "windows"
 * - No session creation timestamps from CLI — we approximate with Date.now()
 * - No native CLI hooks — we use polling (the server already does periodic broadcasts)
 * - $ZELLIJ_SESSION_NAME gives the current session name
 * - `zellij action list-panes --json` gives pane info for the current session
 * - `zellij ls` lists all running session names
 */
export class ZellijProvider implements MuxProviderV1, WindowCapable, SidebarCapable, BatchCapable {
  readonly specificationVersion = "v1" as const;
  readonly name: string;

  private sessionCreatedAt = new Map<string, number>();

  constructor(settings?: ZellijProviderSettings) {
    this.name = settings?.name ?? "zellij";
  }

  listSessions(): MuxSessionInfo[] {
    // Use long-form `zellij ls` so we can filter out EXITED sessions.
    // Short form (`-s`) omits status, causing dead sessions to appear forever.
    const raw = run(["zellij", "ls"]);
    if (!raw) return [];

    const names = raw
      .split("\n")
      .map((l) => l.trim())
      .filter((l) => l && !l.includes("(EXITED"))
      .map((l) => l.split(/\s/)[0]!)
      .filter(Boolean);

    const sessions: MuxSessionInfo[] = [];
    for (const name of names) {
      if (!this.sessionCreatedAt.has(name)) {
        this.sessionCreatedAt.set(name, Math.floor(Date.now() / 1000));
      }

      // Get tab count for this session by querying it
      // We can only get detailed info for the current session via `zellij action`
      // For other sessions, we default to 1 tab
      let tabs = 1;
      let dir = "";
      const currentSession = this.getCurrentSession();
      if (name === currentSession) {
        const tabInfo = runJSON<ZellijTabInfo[]>(["zellij", "action", "list-tabs", "--json"]);
        if (tabInfo) tabs = tabInfo.length;

        const panes = runJSON<ZellijPaneInfo[]>(["zellij", "action", "list-panes", "--json"]);
        if (panes) {
          const focused = panes.find((p) => p.is_focused && !p.is_plugin);
          if (focused?.pane_cwd) dir = focused.pane_cwd;
        }
      }

      sessions.push({
        name,
        createdAt: this.sessionCreatedAt.get(name)!,
        dir,
        windows: tabs,
      });
    }

    return sessions;
  }

  switchSession(name: string, _clientTty?: string): void {
    if (process.env.ZELLIJ_SESSION_NAME) {
      // Inside zellij — use action to switch
      Bun.spawnSync(["zellij", "action", "switch-session", name], {
        stdout: "pipe",
        stderr: "pipe",
      });
    } else if (process.env.TMUX) {
      // Inside tmux — detach the client and replace it with zellij attach.
      // Wrap in bash so that when zellij detaches, the user auto-reattaches to tmux
      // (using the session name written to /tmp/opensessions-reattach by cross-mux switch).
      const zellijBin = "/opt/homebrew/bin/zellij";
      const tmuxBin = "/opt/homebrew/bin/tmux";
      const tty = _clientTty;
      const args = ["detach-client"];
      if (tty) args.push("-t", tty);
      const reattachCmd = `bash -c 'export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH"; ${zellijBin} attach ${name}; ${tmuxBin} attach -t "$(cat /tmp/opensessions-reattach 2>/dev/null || echo :)"'`;
      args.push("-E", reattachCmd);
      Bun.spawnSync(["tmux", ...args], {
        stdout: "pipe",
        stderr: "pipe",
        env: { ...process.env, PATH: `/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:${process.env.PATH ?? ""}` },
      });
    } else {
      // Outside any mux — just attach directly
      Bun.spawnSync(["zellij", "attach", name], {
        stdout: "pipe",
        stderr: "pipe",
      });
    }
  }

  getCurrentSession(): string | null {
    return process.env.ZELLIJ_SESSION_NAME ?? null;
  }

  getSessionDir(name: string): string {
    const currentSession = this.getCurrentSession();
    if (name !== currentSession) return "";

    const panes = runJSON<ZellijPaneInfo[]>(["zellij", "action", "list-panes", "--json"]);
    if (!panes) return "";
    const focused = panes.find((p) => p.is_focused && !p.is_plugin);
    return focused?.pane_cwd ?? "";
  }

  getPaneCount(name: string): number {
    const currentSession = this.getCurrentSession();
    if (name !== currentSession) return 1;

    const panes = runJSON<ZellijPaneInfo[]>(["zellij", "action", "list-panes", "--json"]);
    if (!panes) return 1;
    return panes.filter((p) => !p.is_plugin && p.is_focused !== undefined).length;
  }

  getClientTty(): string {
    // Zellij doesn't expose client TTY — not needed for switch-session
    return "";
  }

  createSession(name?: string, _dir?: string): void {
    const sessionName = name ?? `session-${Date.now()}`;
    // Create a background session using attach --create-background
    Bun.spawnSync(["zellij", "attach", "--create-background", sessionName], {
      stdout: "pipe",
      stderr: "pipe",
    });
  }

  killSession(name: string): void {
    // kill-session stops a running session; delete-session removes an exited one.
    // We try both so the caller doesn't need to know the session state.
    Bun.spawnSync(["zellij", "kill-session", name], {
      stdout: "pipe",
      stderr: "pipe",
    });
    Bun.spawnSync(["zellij", "delete-session", name, "--force"], {
      stdout: "pipe",
      stderr: "pipe",
    });
  }

  setupHooks(_serverHost: string, _serverPort: number): void {
    // Zellij doesn't have CLI hooks like tmux's set-hook.
    // The server relies on polling (broadcastState on interval/websocket reconnect)
    // and the /event endpoint for agent status updates.
    // Future: could use a Zellij WASM plugin to emit events.
  }

  cleanupHooks(): void {
    // No-op — no hooks to clean up
  }

  // --- Window operations ---

  listActiveWindows(): ActiveWindow[] {
    // Query all zellij sessions' tabs.
    // Note: when no client is attached, zellij marks ALL tabs as active:false.
    // So we fall back to the first tab if none are active.
    const result: ActiveWindow[] = [];
    const sessions = this.listSessions();
    for (const sess of sessions) {
      const tabs = runInSessionJSON<ZellijTabInfo[]>(sess.name, ["list-tabs", "--json"]);
      if (!tabs || tabs.length === 0) continue;
      const active = tabs.find((t) => t.active) ?? tabs[0]!;
      result.push({ id: String(active.tab_id), sessionName: sess.name, active: true });
    }
    return result;
  }

  getCurrentWindowId(): string | null {
    const session = this.getCurrentSession();
    if (!session) return null;
    const tabs = runInSessionJSON<ZellijTabInfo[]>(session, ["list-tabs", "--json"]);
    if (!tabs || tabs.length === 0) return null;
    const active = tabs.find((t) => t.active) ?? tabs[0]!;
    return String(active.tab_id);
  }

  // --- Sidebar operations ---

  listSidebarPanes(sessionName?: string): SidebarPane[] {
    const session = sessionName ?? this.getCurrentSession();
    if (!session) return [];
    const panes = runInSessionJSON<ZellijPaneInfo[]>(session, ["list-panes", "--json"]);
    const tabs = runInSessionJSON<ZellijTabInfo[]>(session, ["list-tabs", "--json"]);
    if (!panes) return [];
    const tabWidths = new Map((tabs ?? []).map((tab) => [String(tab.tab_id), tab.viewport_columns]));
    return panes
      .filter((p) => !p.is_plugin && p.title === "opensessions-sidebar")
      .map((p) => ({
        paneId: `terminal_${p.id}`,
        sessionName: session,
        windowId: String(p.tab_id),
        width: p.pane_columns,
        windowWidth: tabWidths.get(String(p.tab_id)),
      }));
  }

  spawnSidebar(
    sessionName: string,
    _windowId: string,
    _width: number,
    position: SidebarPosition,
    scriptsDir: string,
  ): string | null {
    // Resolve paths — scriptsDir = .../apps/tui/scripts
    const repoDir = scriptsDir.replace(/\/apps\/tui\/scripts$/, "");
    const startScript = `${scriptsDir}/start.sh`;

    // Zellij only supports -d right|down for new-pane.
    // To get a sidebar on the left: create pane to the right, then move it left.
    const paneId = run(["zellij", "--session", sessionName, "action", "new-pane",
      "-d", "right",
      "-n", SIDEBAR_PANE_NAME,
      "--close-on-exit",
      "--cwd", `${repoDir}/apps/tui`,
      "--", "bash", startScript,
    ]);

    plog("spawnSidebar", { sessionName, position, paneId, startScript });

    if (!paneId) {
      plog("spawnSidebar FAILED — no pane ID returned");
      return null;
    }

    if (position === "left") {
      // Move the new pane from right to left
      run(["zellij", "--session", sessionName, "action", "move-pane", "left"]);
    }

    // Resize using --pane-id (no focus switching needed).
    // Each resize step is ~5 cols. We iterate, checking actual width each time.
    const direction = position === "left" ? "right" : "left";
    const MAX_ITERATIONS = 20;
    for (let i = 0; i < MAX_ITERATIONS; i++) {
      const panes = runInSessionJSON<ZellijPaneInfo[]>(sessionName, ["list-panes", "--json"]);
      const sidebar = panes?.find((p) => `terminal_${p.id}` === paneId);
      const cols = sidebar?.pane_columns;
      if (!cols || cols <= _width + 2) break;
      runInSession(sessionName, ["resize", "decrease", direction, "--pane-id", paneId]);
      plog("spawnSidebar resize", { i, cols, target: _width });
    }

    // Refocus back to the main pane (in case new-pane stole focus)
    run(["zellij", "--session", sessionName, "action", "move-focus", position === "left" ? "right" : "left"]);

    return paneId;
  }

  hideSidebar(paneId: string): void {
    // Zellij has no stash equivalent — just close the pane
    this.killSidebarPane(paneId);
  }

  killSidebarPane(paneId: string): void {
    // Use --pane-id to close directly (no need to find session or focus)
    // Try all sessions since we don't know which one owns this pane
    const sessions = this.listSessions();
    for (const sess of sessions) {
      const panes = runInSessionJSON<ZellijPaneInfo[]>(sess.name, ["list-panes", "--json"]);
      if (!panes) continue;
      const target = panes.find((p) => `terminal_${p.id}` === paneId && p.title === SIDEBAR_PANE_NAME);
      if (target) {
        run(["zellij", "--session", sess.name, "action", "close-pane", "--pane-id", paneId]);
        plog("killSidebarPane", { paneId, session: sess.name });
        return;
      }
    }
  }

  resizeSidebarPane(paneId: string, width: number): void {
    // Use --pane-id for targeted resize without focus switching
    const sessions = this.listSessions();
    for (const sess of sessions) {
      const panes = runInSessionJSON<ZellijPaneInfo[]>(sess.name, ["list-panes", "--json"]);
      if (!panes) continue;
      const sidebar = panes.find((p) => `terminal_${p.id}` === paneId && p.title === SIDEBAR_PANE_NAME);
      if (!sidebar) continue;
      const cols = sidebar.pane_columns;
      if (!cols || cols <= width + 2) return;
      // Determine direction based on pane position (x=0 means left sidebar)
      const direction = sidebar.pane_x === 0 ? "right" : "left";
      const MAX_ITERATIONS = 20;
      for (let i = 0; i < MAX_ITERATIONS; i++) {
        const updated = runInSessionJSON<ZellijPaneInfo[]>(sess.name, ["list-panes", "--json"]);
        const current = updated?.find((p) => `terminal_${p.id}` === paneId);
        if (!current?.pane_columns || current.pane_columns <= width + 2) break;
        runInSession(sess.name, ["resize", "decrease", direction, "--pane-id", paneId]);
      }
      return;
    }
  }

  cleanupSidebar(): void {
    // No stash session to clean up in Zellij
  }

  killOrphanedSidebarPanes(): void {
    // Zellij handles pane lifecycle automatically
  }

  killStaleSidebarPanes(): void {
    // Zellij does not persist pane state across restarts the way
    // tmux-resurrect/continuum does, so there is nothing to clean up.
  }

  /**
   * Batch pane count retrieval for all sessions.
   * Only the current session can be queried via `zellij action`.
   */
  getAllPaneCounts(): Map<string, number> {
    const counts = new Map<string, number>();
    const currentSession = this.getCurrentSession();
    if (!currentSession) return counts;

    const panes = runJSON<ZellijPaneInfo[]>(["zellij", "action", "list-panes", "--json"]);
    if (panes) {
      counts.set(currentSession, panes.filter((p) => !p.is_plugin).length);
    }

    return counts;
  }
}
