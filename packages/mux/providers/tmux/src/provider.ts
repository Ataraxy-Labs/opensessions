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
import { TmuxClient } from "./client";
import { appendFileSync } from "fs";

/** Settings for creating a tmux provider (ai-sdk style) */
export interface TmuxProviderSettings {
  /** Override the provider name */
  name?: string;
}

const tmux = new TmuxClient();

function plog(msg: string, data?: Record<string, unknown>) {
  const ts = new Date().toISOString().slice(11, 23);
  const extra = data ? " " + JSON.stringify(data) : "";
  try { appendFileSync("/tmp/opensessions-debug.log", `[${ts}] [provider] ${msg}${extra}\n`); } catch {}
}

/** Direct tmux call bypassing SDK (SDK has \x1f parsing issues) */
function rawTmux(args: string[]): string {
  try {
    const r = Bun.spawnSync(["tmux", ...args], { stdout: "pipe", stderr: "pipe" });
    return r.stdout.toString().trim();
  } catch { return ""; }
}

const STASH_SESSION = "_os_stash";
const SIDEBAR_PANE_TITLE = "opensessions-sidebar";
// Pane is stale if it has the sidebar title but its current command is a plain
// shell — e.g. tmux-resurrect/continuum restored the pane title after a system
// restart but spawned a default shell instead of the TUI process.
const SHELL_COMMANDS = new Set(["zsh", "bash", "fish", "sh", "ksh", "dash"]);

export class TmuxProvider implements MuxProviderV1, WindowCapable, SidebarCapable, BatchCapable {
  readonly specificationVersion = "v1" as const;
  readonly name: string;

  constructor(settings?: TmuxProviderSettings) {
    this.name = settings?.name ?? "tmux";
  }

  listSessions(): MuxSessionInfo[] {
    const sessions = tmux.listSessions()
      .filter((s) => s.name !== STASH_SESSION);
    const activeDirs = tmux.getActiveSessionDirs();
    return sessions.map((s) => ({
      name: s.name,
      createdAt: s.createdAt,
      dir: activeDirs.get(s.name) ?? s.dir,
      windows: s.windowCount,
    }));
  }

  switchSession(name: string, clientTty?: string): void {
    tmux.switchClient(name, clientTty ? { clientTty } : undefined);
    this.selectMainPane(name);
  }

  private selectMainPane(sessionName: string): void {
    const activeWindow = tmux.listWindows({ scope: "session", target: sessionName })
      .find((w) => w.active);
    if (!activeWindow) return;

    const panes = tmux.listPanes({ scope: "window", target: activeWindow.id });
    const activePane = panes.find((p) => p.active);
    if (activePane && activePane.title !== SIDEBAR_PANE_TITLE) return;

    const mainPane = panes.find((p) => p.title !== SIDEBAR_PANE_TITLE);
    if (mainPane) tmux.selectPane(mainPane.id);
  }

  getCurrentSession(): string | null {
    return tmux.getCurrentSession();
  }

  getSessionDir(name: string): string {
    return tmux.getSessionDir(name);
  }

  getPaneCount(name: string): number {
    return tmux.getPaneCount(name);
  }

  getClientTty(): string {
    return tmux.getClientTty();
  }

  createSession(name?: string, dir?: string): void {
    tmux.newSession({ name, cwd: dir });
  }

  killSession(name: string): void {
    tmux.killSession(name);
  }

  setupHooks(serverHost: string, serverPort: number, tokenFile?: string): void {
    // Enable terminal focus reporting forwarding so the sidebar TUI can
    // distinguish real keystrokes (delivered with focus-in escape sequences)
    // from `tmux send-keys` injection (which delivers raw bytes without
    // focus events). The TUI uses this signal to gate destructive shortcuts
    // and avoid accidental UI-action injection from other tmux clients.
    try { rawTmux(["set-option", "-g", "focus-events", "on"]); } catch {}

    const base = `http://${serverHost}:${serverPort}`;
    // tmux passes the run-shell body to /bin/sh at hook fire time; we resolve
    // the token via `cat` inside that shell so a server restart that rotates
    // the token doesn't require re-installing every hook. The single-quoted
    // header prefix is concatenated (shell-style) with the `$(cat ...)`
    // expansion. If the token file is missing the header value is empty and
    // the server returns 401 — the curl no-ops via `|| true`, which is the
    // right behaviour while the token is being written during startup.
    const authHeader = tokenFile
      ? ` -H 'x-opensessions-token: '$(cat ${tokenFile} 2>/dev/null)`
      : "";
    const hookPost = (path: string, data?: string) => {
      const body = data ? ` -d '${data}'` : "";
      return `run-shell -b "curl -s -o /dev/null -m 0.2 --connect-timeout 0.1 -X POST${authHeader} ${base}${path}${body} >/dev/null 2>&1 || true"`;
    };
    // tmux expands #{} formats at hook-fire time — no need for $(tmux display-message)
    // Use | as field separator (safe for session names, window IDs, TTYs)
    const focusCmd = hookPost("/focus", "#{client_tty}|#{session_name}|#{window_id}");
    const refreshCmd = hookPost("/refresh");
    const ensureCmd = hookPost("/ensure-sidebar", "#{client_tty}|#{session_name}|#{window_id}");

    const clientResizedCmd = hookPost("/client-resized");

    // client-session-changed: update focus AND ensure sidebar in the new session's window
    tmux.setGlobalHook("client-session-changed", `${focusCmd} ; ${ensureCmd}`);
    tmux.setGlobalHook("session-created", refreshCmd);
    tmux.setGlobalHook("session-closed", refreshCmd);
    tmux.setGlobalHook("after-select-window", ensureCmd);
    tmux.setGlobalHook("after-new-window", ensureCmd);
    // client-resized: terminal window changed size — enforce stored width back
    tmux.setGlobalHook("client-resized", clientResizedCmd);
    // pane-exited: a pane closed — kill orphaned sidebar panes (only pane left in window)
    const paneExitedCmd = hookPost("/pane-exited");
    tmux.setGlobalHook("pane-exited", paneExitedCmd);
  }

  cleanupHooks(): void {
    tmux.unsetGlobalHook("client-session-changed");
    tmux.unsetGlobalHook("session-created");
    tmux.unsetGlobalHook("session-closed");
    tmux.unsetGlobalHook("after-select-window");
    tmux.unsetGlobalHook("after-new-window");
    tmux.unsetGlobalHook("client-resized");
    tmux.unsetGlobalHook("pane-exited");
  }

  getAllPaneCounts(): Map<string, number> {
    return tmux.getAllPaneCounts();
  }

  listActiveWindows(): ActiveWindow[] {
    return tmux.listWindows()
      .filter((w) => w.sessionName !== STASH_SESSION)
      .map((w) => ({ id: w.id, sessionName: w.sessionName, active: w.active }));
  }

  getCurrentWindowId(): string | null {
    return tmux.getCurrentWindowId() || null;
  }

  cleanupSidebar(): void {
    // Kill the stash session used for hiding sidebar panes
    try {
      Bun.spawnSync(["tmux", "kill-session", "-t", STASH_SESSION], { stdout: "pipe", stderr: "pipe" });
    } catch {}
  }

  listSidebarPanes(sessionName?: string): SidebarPane[] {
    const panes = sessionName
      ? tmux.listPanes({ scope: "session", target: sessionName })
      : tmux.listPanes();
    const windowWidths = new Map<string, number>();
    for (const pane of panes) {
      windowWidths.set(pane.windowId, Math.max(windowWidths.get(pane.windowId) ?? 0, pane.right + 1));
    }

    return panes
      .filter((p) =>
        p.title === SIDEBAR_PANE_TITLE
        && p.sessionName !== STASH_SESSION
        // Exclude stale panes (sidebar title but plain shell — restored by
        // tmux-resurrect/continuum after restart with no TUI process).
        && !SHELL_COMMANDS.has(p.command)
      )
      .map((p) => ({
        paneId: p.id,
        sessionName: p.sessionName,
        windowId: p.windowId,
        width: p.width,
        windowWidth: windowWidths.get(p.windowId),
      }));
  }

  spawnSidebar(
    sessionName: string,
    windowId: string,
    width: number,
    position: SidebarPosition,
    scriptsDir: string,
  ): string | null {
    // Find the edge pane to split against
    const panes = tmux.listPanes({ scope: "window", target: windowId });
    plog("spawnSidebar", { windowId, paneCount: panes.length });
    if (panes.length === 0) return null;

    const targetPane = position === "left"
      ? panes.reduce((a, b) => (a.left <= b.left ? a : b))
      : panes.reduce((a, b) => (a.right >= b.right ? a : b));

    plog("spawnSidebar: spawning new", { target: targetPane.id, width, position });
    const newPane = tmux.splitWindow({
      target: targetPane.id,
      direction: "horizontal",
      before: position === "left",
      fullWindow: true,
      size: width,
      command: `REFOCUS_WINDOW=${windowId} exec ${scriptsDir}/start.sh`,
    });

    if (!newPane) {
      plog("spawnSidebar: splitWindow FAILED");
      return null;
    }

    tmux.setPaneTitle(newPane.id, SIDEBAR_PANE_TITLE);
    tmux.resetPane(newPane.id);
    tmux.clearPaneHistory(newPane.id);
    // Do NOT selectPane here for fresh spawns — the TUI's refocusMainPane()
    // handles it after terminal capability detection finishes. Refocusing
    // immediately causes capability query responses (DECRPM, DA1, Kitty
    // graphics) to be routed to the main pane as garbage escape sequences.
    return newPane.id;
  }

  hideSidebar(paneId: string): void {
    // Align tmux with zellij: hiding the global sidebar should tear down the
    // pane entirely so hidden sidebars do not keep a background TUI process
    // alive per window inside the hidden stash session.
    plog("hideSidebar: killing pane", { paneId });
    tmux.killPane(paneId);
  }

  killSidebarPane(paneId: string): void {
    tmux.killPane(paneId);
  }

  resizeSidebarPane(paneId: string, width: number): void {
    tmux.resizePane(paneId, { width });
  }

  /**
   * Force-resize a window to the given dimensions, triggering tmux to
   * re-layout its panes. Used after monitor switches to pre-layout
   * background windows so they don't need re-layout on focus.
   */
  resizeWindow(windowId: string, width: number, height: number): void {
    rawTmux(["resize-window", "-t", windowId, "-x", String(width), "-y", String(height)]);
    rawTmux(["set-window-option", "-t", windowId, "window-size", "latest"]);
  }

  /** Get the current client dimensions. */
  getClientSize(): { width: number; height: number } | null {
    const clients = tmux.listClients().filter((client) => client.tty.length > 0);
    if (clients.length === 0) return null;
    return { width: clients[0]!.width, height: clients[0]!.height };
  }

  killStaleSidebarPanes(): void {
    const allPanes = tmux.listPanes();
    for (const p of allPanes) {
      if (p.title !== SIDEBAR_PANE_TITLE) continue;
      if (p.sessionName === STASH_SESSION) continue;
      if (!SHELL_COMMANDS.has(p.command)) continue;
      plog("killStaleSidebarPanes: killing", { paneId: p.id, command: p.command, session: p.sessionName });
      tmux.killPane(p.id);
    }
  }

  killOrphanedSidebarPanes(): void {
    const allPanes = tmux.listPanes();
    // Count panes per window and collect sidebar panes by window.
    const windowPaneCounts = new Map<string, number>();
    const sidebarsByWindow = new Map<string, typeof allPanes>();
    for (const p of allPanes) {
      if (p.sessionName === STASH_SESSION) continue;
      windowPaneCounts.set(p.windowId, (windowPaneCounts.get(p.windowId) ?? 0) + 1);
      if (p.title !== SIDEBAR_PANE_TITLE) continue;
      const panes = sidebarsByWindow.get(p.windowId) ?? [];
      panes.push(p);
      sidebarsByWindow.set(p.windowId, panes);
    }

    for (const [windowId, sidebars] of sidebarsByWindow) {
      if (windowPaneCounts.get(windowId) === 1) {
        for (const pane of sidebars) tmux.killPane(pane.id);
        continue;
      }
      if (sidebars.length <= 1) continue;
      // Defensive cleanup: keep a single sidebar per window, kill extras.
      for (const pane of sidebars.slice(1)) {
        plog("killOrphanedSidebarPanes: killing duplicate sidebar", { paneId: pane.id, windowId });
        tmux.killPane(pane.id);
      }
    }
  }

  /**
   * spawn-shell counterpart to killOrphanedSidebarPanes. For each window
   * where a sidebar is the only pane left, spawn a fresh shell pane on
   * the opposite side so the window stays usable. Then defensively dedupe
   * any duplicate sidebars in windows that already have neighbours.
   *
   * Idempotent: re-running on a window that already has a shell + sidebar
   * is a no-op (the lonely-sidebar branch never fires there).
   */
  protectOrphanedSidebars(): void {
    const allPanes = tmux.listPanes();
    const windowPaneCounts = new Map<string, number>();
    const sidebarsByWindow = new Map<string, typeof allPanes>();
    for (const p of allPanes) {
      if (p.sessionName === STASH_SESSION) continue;
      windowPaneCounts.set(p.windowId, (windowPaneCounts.get(p.windowId) ?? 0) + 1);
      if (p.title !== SIDEBAR_PANE_TITLE) continue;
      const panes = sidebarsByWindow.get(p.windowId) ?? [];
      panes.push(p);
      sidebarsByWindow.set(p.windowId, panes);
    }

    for (const [windowId, sidebars] of sidebarsByWindow) {
      const totalPanes = windowPaneCounts.get(windowId) ?? 0;
      if (totalPanes === 1 && sidebars.length === 1) {
        const sidebar = sidebars[0]!;
        // Sidebar at column 0 → it's anchored left → put the new shell to
        // its right (before=false). Otherwise it's anchored right → put
        // the new shell to its left (before=true).
        const before = sidebar.left !== 0;
        plog("protectOrphanedSidebars: spawning shell next to lonely sidebar", {
          windowId,
          sidebarId: sidebar.id,
          before,
        });
        const spawned = tmux.splitWindow({
          target: sidebar.id,
          direction: "horizontal",
          before,
        });
        if (!spawned) {
          // splitWindow failed (e.g. window too narrow). Fall back to the
          // kill behaviour so we don't leave a broken state.
          plog("protectOrphanedSidebars: splitWindow FAILED, falling back to kill", {
            sidebarId: sidebar.id,
          });
          tmux.killPane(sidebar.id);
        }
        continue;
      }
      if (sidebars.length <= 1) continue;
      // Same defensive duplicate cleanup as killOrphanedSidebarPanes —
      // multi-sidebar windows never benefit from spawn-shell.
      for (const pane of sidebars.slice(1)) {
        plog("protectOrphanedSidebars: killing duplicate sidebar", { paneId: pane.id, windowId });
        tmux.killPane(pane.id);
      }
    }
  }
}
