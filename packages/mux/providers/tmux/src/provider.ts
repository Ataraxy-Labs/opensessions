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

  setupHooks(serverHost: string, serverPort: number): void {
    const base = `http://${serverHost}:${serverPort}`;
    const hookPost = (path: string, data?: string) => {
      const body = data ? ` -d '${data}'` : "";
      return `run-shell -b "curl -s -o /dev/null -m 0.2 --connect-timeout 0.1 -X POST -H 'x-opensessions-token: '$(cat /tmp/opensessions.token 2>/dev/null) ${base}${path}${body} >/dev/null 2>&1 || true"`;
    };
    // tmux expands #{} formats at hook-fire time — no need for $(tmux display-message)
    // Use | as field separator (safe for session names, window IDs, TTYs)
    const focusCmd = hookPost("/focus", "#{client_tty}|#{session_name}|#{window_id}");
    const refreshCmd = hookPost("/refresh");
    const ensureCmd = hookPost("/ensure-sidebar", "#{client_tty}|#{session_name}|#{window_id}");

    const clientResizedCmd = hookPost("/client-resized");

    // client-session-changed: update focus AND ensure sidebar in the new session's window
    tmux.setGlobalHook("client-session-changed", `${focusCmd} ; ${ensureCmd}`);
    tmux.setGlobalHook("session-created", `${refreshCmd} ; ${ensureCmd}`);
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
    tmux.unsetGlobalHook("after-resize-pane");
    tmux.unsetGlobalHook("pane-exited");
  }

  getAllPaneCounts(): Map<string, number> {
    return tmux.getAllPaneCounts();
  }

  listActiveWindows(): ActiveWindow[] {
    return tmux.listWindows()
      .filter((w) => w.active && w.sessionName !== STASH_SESSION)
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

  /** Ensure the invisible stash session exists for hiding sidebar panes */
  private ensureStash(): void {
    const r = Bun.spawnSync(["tmux", "has-session", "-t", STASH_SESSION], { stdout: "pipe", stderr: "pipe" });
    if (r.exitCode !== 0) {
      rawTmux(["new-session", "-d", "-s", STASH_SESSION, "-x", "80", "-y", "24"]);
    }
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

    // --- Try to restore a stashed sidebar pane ---
    try {
      const stashPanes = tmux.listPanes({ scope: "session", target: STASH_SESSION });
      const stashedPane = stashPanes.find((p) => p.title === SIDEBAR_PANE_TITLE);
      if (stashedPane) {
        plog("spawnSidebar: restoring from stash", { paneId: stashedPane.id, target: targetPane.id });
        const joinFlag = position === "left" ? "-hb" : "-h";
        rawTmux(["join-pane", joinFlag, "-f", "-l", String(width), "-s", stashedPane.id, "-t", targetPane.id]);
        tmux.setPaneTitle(stashedPane.id, SIDEBAR_PANE_TITLE);
        // Do NOT selectPane here — same as fresh spawns. The TUI's
        // restoreTerminalModes fires on focus-in after join-pane, generating
        // capability query responses. Refocusing the main pane immediately
        // causes those responses to leak as garbage escape sequences.
        return stashedPane.id;
      }
    } catch { /* stash session doesn't exist yet — spawn fresh */ }

    // --- No stashed pane, spawn fresh ---
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
    // Do NOT selectPane here for fresh spawns — the TUI's refocusMainPane()
    // handles it after terminal capability detection finishes. Refocusing
    // immediately causes capability query responses (DECRPM, DA1, Kitty
    // graphics) to be routed to the main pane as garbage escape sequences.
    return newPane.id;
  }

  hideSidebar(paneId: string): void {
    this.ensureStash();
    // Ensure the stash window is large enough to accept another pane.
    // join-pane fails with "pane too small" when stash panes fill up.
    rawTmux(["resize-window", "-t", `${STASH_SESSION}:`, "-x", "200", "-y", "200"]);
    plog("hideSidebar: stashing pane", { paneId });
    rawTmux(["join-pane", "-d", "-s", paneId, "-t", `${STASH_SESSION}:`]);
  }

  killSidebarPane(paneId: string): void {
    tmux.killPane(paneId);
  }

  resizeSidebarPane(paneId: string, width: number): void {
    tmux.resizePane(paneId, { width });
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
    // Count panes per window
    const windowPaneCounts = new Map<string, number>();
    for (const p of allPanes) {
      if (p.sessionName === STASH_SESSION) continue;
      windowPaneCounts.set(p.windowId, (windowPaneCounts.get(p.windowId) ?? 0) + 1);
    }
    // Find sidebar panes that are the only pane in their window
    for (const p of allPanes) {
      if (p.title !== SIDEBAR_PANE_TITLE || p.sessionName === STASH_SESSION) continue;
      if (windowPaneCounts.get(p.windowId) === 1) {
        tmux.killPane(p.id);
      }
    }
  }
}
