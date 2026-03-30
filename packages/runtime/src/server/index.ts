import { existsSync, readFileSync, unlinkSync, writeFileSync, appendFileSync, watch, type FSWatcher } from "fs";
import { join } from "path";
import { homedir } from "os";
import type { MuxProvider } from "../contracts/mux";
import { isFullSidebarCapable, isBatchCapable } from "../contracts/mux";
import type { AgentEvent } from "../contracts/agent";
import type { AgentWatcher, AgentWatcherContext } from "../contracts/agent-watcher";
import { AgentTracker, instanceKey } from "../agents/tracker";
import { SessionOrder } from "./session-order";
import { SessionMetadataStore } from "./metadata-store";
import { loadConfig, saveConfig } from "../config";
import {
  resolveSidebarWidthFromResizeContext,
  snapshotSidebarWindows,
  type SidebarResizeContext,
  type SidebarResizeSuppression,
} from "./sidebar-width-sync";
import {
  type ServerState,
  type SessionData,
  type ClientCommand,
  type FocusUpdate,
  SERVER_PORT,
  SERVER_HOST,
  PID_FILE,
  SERVER_IDLE_TIMEOUT_MS,
  STUCK_RUNNING_TIMEOUT_MS,
} from "../shared";

// --- Debug logger ---

const DEBUG_LOG = "/tmp/opensessions-debug.log";

function log(category: string, msg: string, data?: Record<string, unknown>) {
  const ts = new Date().toISOString().slice(11, 23);
  const extra = data ? " " + JSON.stringify(data) : "";
  const line = `[${ts}] [${category}] ${msg}${extra}\n`;
  try { appendFileSync(DEBUG_LOG, line); } catch {}
}

// --- Shell helper (for git commands only) ---

function shell(cmd: string[]): string {
  try {
    const result = Bun.spawnSync(cmd, { stdout: "pipe", stderr: "pipe" });
    return result.stdout.toString().trim();
  } catch {
    return "";
  }
}

// --- Git helpers ---

interface GitInfo {
  branch: string;
  dirty: boolean;
  isWorktree: boolean;
}

const gitInfoCache = new Map<string, { info: GitInfo; ts: number }>();
const GIT_CACHE_TTL_MS = 5000;

function getGitInfo(dir: string): GitInfo {
  if (!dir) return { branch: "", dirty: false, isWorktree: false };

  const cached = gitInfoCache.get(dir);
  if (cached && Date.now() - cached.ts < GIT_CACHE_TTL_MS) return cached.info;

  const out = shell([
    "sh", "-c",
    `cd "${dir}" 2>/dev/null && git rev-parse --abbrev-ref HEAD --git-dir 2>/dev/null && echo "---" && git status --porcelain 2>/dev/null`,
  ]);
  if (!out) return { branch: "", dirty: false, isWorktree: false };
  const sepIdx = out.indexOf("---");
  const headerPart = sepIdx >= 0 ? out.slice(0, sepIdx).trim() : out.trim();
  const statusPart = sepIdx >= 0 ? out.slice(sepIdx + 3).trim() : "";
  const lines = headerPart.split("\n");
  const branch = lines[0] ?? "";
  const gitDir = lines[1] ?? "";
  const info: GitInfo = {
    branch,
    dirty: statusPart.length > 0,
    isWorktree: gitDir.includes("/worktrees/"),
  };
  gitInfoCache.set(dir, { info, ts: Date.now() });
  return info;
}

function invalidateGitCache(dir?: string) {
  if (dir) gitInfoCache.delete(dir);
  else gitInfoCache.clear();
}

// --- Port detection ---

// Global port snapshot — refreshed by the port poll timer, read by computeState.
// Runs lsof + ps once for ALL sessions instead of per-session.
let portSnapshot = new Map<string, number[]>();

function refreshPortSnapshot(sessionNames: string[]): boolean {
  try {
    // 1. Gather pane PIDs for all sessions in one tmux call per session
    //    (tmux doesn't support multi-session list-panes, so we batch via a single format string)
    const panePidsBySession = new Map<string, number[]>();
    for (const name of sessionNames) {
      const r = Bun.spawnSync(
        ["tmux", "list-panes", "-s", "-t", name, "-F", "#{pane_pid}"],
        { stdout: "pipe", stderr: "pipe" },
      );
      const pids = r.stdout.toString().trim().split("\n").filter(Boolean).map(Number).filter((n) => !isNaN(n));
      if (pids.length > 0) panePidsBySession.set(name, pids);
    }

    if (panePidsBySession.size === 0) {
      portSnapshot = new Map();
      return false;
    }

    // 2. Build parent→children map from a single ps call
    const childrenOf = new Map<number, number[]>();
    const psResult = Bun.spawnSync(["ps", "-eo", "pid=,ppid="], { stdout: "pipe", stderr: "pipe" });
    for (const line of psResult.stdout.toString().trim().split("\n")) {
      const parts = line.trim().split(/\s+/);
      if (parts.length < 2) continue;
      const pid = parseInt(parts[0], 10);
      const ppid = parseInt(parts[1], 10);
      if (isNaN(pid) || isNaN(ppid)) continue;
      let arr = childrenOf.get(ppid);
      if (!arr) { arr = []; childrenOf.set(ppid, arr); }
      arr.push(pid);
    }

    // 3. BFS from pane PIDs to get full descendant tree per session
    //    Also build a reverse map: pid → session name(s)
    const pidToSessions = new Map<number, string[]>();
    for (const [name, panePids] of panePidsBySession) {
      const allPids = new Set<number>(panePids);
      const queue = [...panePids];
      while (queue.length > 0) {
        const pid = queue.pop()!;
        const kids = childrenOf.get(pid);
        if (!kids) continue;
        for (const kid of kids) {
          if (!allPids.has(kid)) {
            allPids.add(kid);
            queue.push(kid);
          }
        }
      }
      for (const pid of allPids) {
        let arr = pidToSessions.get(pid);
        if (!arr) { arr = []; pidToSessions.set(pid, arr); }
        arr.push(name);
      }
    }

    // 4. Single lsof call for all listening TCP ports
    const lsofResult = Bun.spawnSync(
      ["/usr/sbin/lsof", "-iTCP", "-sTCP:LISTEN", "-nP", "-F", "pn"],
      { stdout: "pipe", stderr: "pipe" },
    );
    if (lsofResult.exitCode !== 0) {
      log("ports", "lsof failed", { exitCode: lsofResult.exitCode, stderr: lsofResult.stderr.toString().slice(0, 200) });
      return false;
    }

    // 5. Parse and attribute ports to sessions
    const sessionPorts = new Map<string, Set<number>>();
    let currentPid = 0;
    for (const line of lsofResult.stdout.toString().split("\n")) {
      if (line.startsWith("p")) {
        currentPid = parseInt(line.slice(1), 10);
      } else if (line.startsWith("n")) {
        const sessions = pidToSessions.get(currentPid);
        if (!sessions) continue;
        const match = line.match(/:(\d+)$/);
        if (!match) continue;
        const port = parseInt(match[1], 10);
        if (isNaN(port)) continue;
        for (const name of sessions) {
          let set = sessionPorts.get(name);
          if (!set) { set = new Set(); sessionPorts.set(name, set); }
          set.add(port);
        }
      }
    }

    // 6. Build the new snapshot
    const next = new Map<string, number[]>();
    for (const name of sessionNames) {
      const set = sessionPorts.get(name);
      next.set(name, set ? [...set].sort((a, b) => a - b) : []);
    }

    const changed = !mapsEqual(portSnapshot, next);
    portSnapshot = next;
    return changed;
  } catch (err) {
    log("ports", "refreshPortSnapshot failed", { error: String(err) });
    return false;
  }
}

function mapsEqual(a: Map<string, number[]>, b: Map<string, number[]>): boolean {
  if (a.size !== b.size) return false;
  for (const [k, v] of a) {
    const bv = b.get(k);
    if (!bv || bv.length !== v.length || v.some((n, i) => n !== bv[i])) return false;
  }
  return true;
}

function getSessionPorts(sessionName: string): number[] {
  return portSnapshot.get(sessionName) ?? [];
}

// --- Git HEAD file watchers ---

const gitHeadWatchers = new Map<string, FSWatcher>();

function resolveGitHeadPath(dir: string): string | null {
  if (!dir) return null;
  const gitDir = shell(["git", "-C", dir, "rev-parse", "--git-dir"]);
  if (!gitDir) return null;
  const absGitDir = gitDir.startsWith("/") ? gitDir : join(dir, gitDir);
  const headPath = join(absGitDir, "HEAD");
  return existsSync(headPath) ? headPath : null;
}

let debounceTimer: ReturnType<typeof setTimeout> | null = null;

function onGitHeadChange(broadcastFn: () => void) {
  if (debounceTimer) return;
  debounceTimer = setTimeout(() => {
    debounceTimer = null;
    invalidateGitCache();
    broadcastFn();
  }, 200);
}

function syncGitWatchers(sessions: SessionData[], broadcastFn: () => void) {
  const currentDirs = new Set<string>();
  for (const s of sessions) {
    if (s.dir) currentDirs.add(s.dir);
  }

  for (const [dir, watcher] of gitHeadWatchers) {
    if (!currentDirs.has(dir)) {
      watcher.close();
      gitHeadWatchers.delete(dir);
    }
  }

  for (const dir of currentDirs) {
    if (gitHeadWatchers.has(dir)) continue;
    const headPath = resolveGitHeadPath(dir);
    if (!headPath) continue;
    try {
      const watcher = watch(headPath, () => onGitHeadChange(broadcastFn));
      gitHeadWatchers.set(dir, watcher);
    } catch { /* ignore */ }
  }
}

// --- Server startup ---

export function startServer(mux: MuxProvider, extraProviders?: MuxProvider[], watchers?: AgentWatcher[]): void {
  const allProviders = [mux, ...(extraProviders ?? [])];
  const allWatchers = watchers ?? [];
  const tracker = new AgentTracker();
  const metadataStore = new SessionMetadataStore();
  const home = process.env.HOME ?? process.env.USERPROFILE ?? "";
  const sessionOrderPath = join(home, ".config", "opensessions", "session-order.json");
  const sessionOrder = new SessionOrder(sessionOrderPath);

  // Clear previous log on server start
  try { writeFileSync(DEBUG_LOG, ""); } catch {}
  log("server", "starting", { providers: allProviders.map((p) => p.name) });

  // Load initial theme from config
  const config = loadConfig();
  let currentTheme: string | undefined = typeof config.theme === "string" ? config.theme : undefined;
  let sidebarWidth = config.sidebarWidth ?? 26;
  let sidebarPosition: "left" | "right" = config.sidebarPosition ?? "left";
  let sidebarVisible = false;

  // The sidebar launcher lives with the TUI app, not the tmux integration layer.
  const scriptsDir = (() => {
    const envDir = process.env.OPENSESSIONS_DIR;
    if (envDir) return join(envDir, "apps", "tui", "scripts");
    // Fallback: relative to this file
    return join(import.meta.dir, "..", "..", "..", "..", "apps", "tui", "scripts");
  })();

  log("server", "config loaded", {
    sidebarWidth, sidebarPosition, scriptsDir,
    theme: currentTheme, configKeys: Object.keys(config),
  });

  // Bootstrap active sessions
  const currentSession = mux.getCurrentSession();
  if (currentSession) {
    tracker.setActiveSessions([currentSession]);
  }

  // --- Agent watcher context ---

  let watcherBroadcastTimer: ReturnType<typeof setTimeout> | null = null;

  function debouncedBroadcast() {
    if (watcherBroadcastTimer) return;
    watcherBroadcastTimer = setTimeout(() => {
      watcherBroadcastTimer = null;
      broadcastState();
    }, 200);
  }

  // Cache for dir→session resolution (rebuilt per scan cycle)
  let dirSessionCache: Map<string, string> | null = null;
  let dirSessionCacheTs = 0;
  const DIR_CACHE_TTL = 5000;

  function getDirSessionMap(): Map<string, string> {
    const now = Date.now();
    if (dirSessionCache && now - dirSessionCacheTs < DIR_CACHE_TTL) return dirSessionCache;
    const map = new Map<string, string>();
    for (const p of allProviders) {
      for (const s of p.listSessions()) {
        if (s.dir) map.set(s.dir, s.name);
      }
    }
    dirSessionCache = map;
    dirSessionCacheTs = now;
    return map;
  }

  const watcherCtx: AgentWatcherContext = {
    resolveSession(projectDir: string): string | null {
      const map = getDirSessionMap();
      const direct = map.get(projectDir);
      if (direct) return direct;
      for (const [dir, name] of map) {
        if (projectDir.startsWith(dir + "/") || dir.startsWith(projectDir + "/")) return name;
      }
      return null;
    },
    emit(event: AgentEvent) {
      tracker.applyEvent(event, { seed: !watchersSeeded });
      debouncedBroadcast();
    },
  };

  // Flag to track when initial watcher seeding is complete
  let watchersSeeded = false;
  setTimeout(() => {
    watchersSeeded = true;
    // Re-apply focus for the current session to clear seed-unseen flags
    // (handleFocus already ran before seed events arrived)
    const current = getCurrentSession();
    if (current && tracker.handleFocus(current)) {
      broadcastState();
    }
  }, 3000);

  let focusedSession: string | null = null;
  let lastState: ServerState | null = null;
  let clientCount = 0;
  let idleTimer: ReturnType<typeof setTimeout> | null = null;
  const clientTtys = new WeakMap<object, string>();
  const clientSessionNames = new WeakMap<object, string>();
  const sessionProviders = new Map<string, MuxProvider>();
  // Map session name → client TTY (from hook context, for multi-client setups)
  const clientTtyBySession = new Map<string, string>();

  function getCurrentSession(): string | null {
    // Try all providers until one returns a session
    for (const p of allProviders) {
      const result = p.getCurrentSession();
      if (result) {
        log("getCurrentSession", "result", { result, provider: p.name });
        return result;
      }
    }
    log("getCurrentSession", "no provider returned a session");
    return null;
  }

  /** Merge pane-detected agents into watcher-provided agents for a session.
   *  Watcher events take precedence — pane presence only adds synthetic entries
   *  for agents that aren't already tracked by watchers. */
  function mergeAgentsWithPanePresence(sessionName: string, watcherAgents: AgentEvent[]): AgentEvent[] {
    const paneAgents = paneAgentsBySession.get(sessionName);
    if (!paneAgents || paneAgents.size === 0) return watcherAgents;

    const result = [...watcherAgents];
    // Build a set of tracked agent:threadId keys for matching
    const trackedByKey = new Set(watcherAgents.map((a) => instanceKey(a.agent, a.threadId)));
    // Also track which agent names + threadIds are covered by watchers
    const trackedThreadIds = new Set(
      watcherAgents.filter((a) => a.threadId).map((a) => `${a.agent}:${a.threadId}`),
    );

    for (const [_key, presence] of paneAgents) {
      // If the pane scanner resolved a threadId, check if watcher already tracks it
      if (presence.threadId && trackedThreadIds.has(`${presence.agent}:${presence.threadId}`)) continue;
      // Check by instanceKey as well
      if (trackedByKey.has(instanceKey(presence.agent, presence.threadId))) continue;
      // If we have no threadId from pane scan and watcher tracks any instance of this agent, skip
      if (!presence.threadId && watcherAgents.some((a) => a.agent === presence.agent)) continue;

      result.push({
        agent: presence.agent,
        session: sessionName,
        status: presence.status ?? "idle",
        ts: presence.lastSeenTs,
        threadId: presence.threadId,
        threadName: presence.threadName,
        paneId: presence.paneId,
      });
    }

    return result;
  }

  function computeState(): ServerState {
    // Merge sessions from all providers
    const allMuxSessions: (import("../contracts/mux").MuxSessionInfo & { provider: MuxProvider })[] = [];
    for (const p of allProviders) {
      for (const s of p.listSessions()) {
        allMuxSessions.push({ ...s, provider: p });
      }
    }
    allMuxSessions.sort((a, b) => {
      if (a.createdAt !== b.createdAt) return a.createdAt - b.createdAt;
      return a.name.localeCompare(b.name);
    });

    const currentSession = getCurrentSession();

    // Sync custom ordering with current session list
    sessionOrder.sync(allMuxSessions.map((s) => s.name));
    if (currentSession) {
      sessionOrder.show(currentSession);
    }

    // Apply custom ordering
    const orderedNames = sessionOrder.apply(allMuxSessions.map((s) => s.name));
    const sessionByName = new Map(allMuxSessions.map((s) => [s.name, s]));
    const orderedMuxSessions = orderedNames.map((n) => sessionByName.get(n)!);

    // Batch pane counts per provider (uses BatchCapable type guard)
    const paneCountMaps = new Map<MuxProvider, Map<string, number>>();
    for (const p of allProviders) {
      if (isBatchCapable(p)) {
        paneCountMaps.set(p, p.getAllPaneCounts());
      }
    }

    const sessions: SessionData[] = orderedMuxSessions.map(({ name, createdAt, windows, dir, provider }) => {
      sessionProviders.set(name, provider);
      const git = getGitInfo(dir);
      const providerPaneCounts = paneCountMaps.get(provider);
      const panes = providerPaneCounts?.get(name) ?? provider.getPaneCount(name);

      let uptime = "";
      const diff = Math.floor(Date.now() / 1000) - createdAt;
      if (!isNaN(diff) && diff >= 0) {
        const days = Math.floor(diff / 86400);
        const hours = Math.floor((diff % 86400) / 3600);
        const mins = Math.floor((diff % 3600) / 60);
        if (days > 0) uptime = `${days}d${hours}h`;
        else if (hours > 0) uptime = `${hours}h${mins}m`;
        else uptime = `${mins}m`;
      }

      return {
        name,
        createdAt,
        dir,
        branch: git.branch,
        dirty: git.dirty,
        isWorktree: git.isWorktree,
        unseen: tracker.isUnseen(name),
        panes,
        ports: getSessionPorts(name),
        windows,
        uptime,
        agentState: tracker.getState(name),
        agents: mergeAgentsWithPanePresence(name, tracker.getAgents(name)),
        eventTimestamps: tracker.getEventTimestamps(name),
        metadata: metadataStore.get(name),
      };
    });

    metadataStore.pruneSessions(new Set(sessions.map((s) => s.name)));

    if (sessions.length === 0) {
      focusedSession = null;
    } else if (!focusedSession || !sessions.some((s) => s.name === focusedSession)) {
      focusedSession = sessions.find((s) => s.name === currentSession)?.name ?? sessions[0]!.name;
    }

    return { type: "state", sessions, focusedSession, currentSession, theme: currentTheme, sidebarWidth, ts: Date.now() };
  }

  let broadcastPending = false;

  function broadcastState() {
    if (broadcastPending) return;
    broadcastPending = true;
    queueMicrotask(() => {
      broadcastPending = false;
      broadcastStateImmediate();
    });
  }

  function broadcastStateImmediate() {
    invalidateCurrentSessionCache();
    tracker.pruneStuck(STUCK_RUNNING_TIMEOUT_MS);
    tracker.pruneTerminal();
    lastState = computeState();
    syncGitWatchers(lastState.sessions, broadcastState);
    const msg = JSON.stringify(lastState);
    server.publish("sidebar", msg);
  }

  // Lightweight current-session cache — avoids a tmux subprocess per focus update
  let cachedCurrentSession: string | null = null;
  let cachedCurrentSessionTs = 0;
  const CURRENT_SESSION_CACHE_TTL = 500; // ms — short TTL, just enough to coalesce rapid switches

  function getCachedCurrentSession(): string | null {
    const now = Date.now();
    if (now - cachedCurrentSessionTs < CURRENT_SESSION_CACHE_TTL) return cachedCurrentSession;
    cachedCurrentSession = getCurrentSession();
    cachedCurrentSessionTs = now;
    return cachedCurrentSession;
  }

  function invalidateCurrentSessionCache(): void {
    cachedCurrentSessionTs = 0;
  }

  function broadcastFocusOnly(sender?: any) {
    if (!lastState) return;
    const currentSession = getCachedCurrentSession();
    lastState = { ...lastState, focusedSession, currentSession };
    const msg: FocusUpdate = { type: "focus", focusedSession, currentSession };
    const payload = JSON.stringify(msg);
    if (sender) {
      sender.publish("sidebar", payload);
    } else {
      server.publish("sidebar", payload);
    }
  }

  function moveFocus(delta: -1 | 1, sender?: any) {
    if (!lastState || lastState.sessions.length === 0) return;
    const sessions = lastState.sessions;
    const currentIdx = sessions.findIndex((s) => s.name === focusedSession);
    const newIdx = Math.max(0, Math.min(sessions.length - 1, (currentIdx === -1 ? 0 : currentIdx) + delta));
    focusedSession = sessions[newIdx]!.name;
    broadcastFocusOnly(sender);
  }

  function setFocus(name: string, sender?: any) {
    if (lastState && lastState.sessions.some((s) => s.name === name)) {
      focusedSession = name;
      broadcastFocusOnly(sender);
    }
  }

  function handleFocus(name: string): void {
    focusedSession = name;
    invalidateCurrentSessionCache();
    // Rescan pane agents when session focus changes
    refreshPaneAgents();
    const hadUnseen = tracker.handleFocus(name);
    if (hadUnseen && lastState) {
      // Patch unseen flags in-place — avoids a full computeState with many subprocesses
      const currentSession = getCachedCurrentSession();
      const updatedSessions = lastState.sessions.map((s) => {
        if (s.name !== name) return s;
        return {
          ...s,
          unseen: false,
          agents: s.agents.map((a) => ({ ...a, unseen: false })),
        };
      });
      lastState = { ...lastState, sessions: updatedSessions, focusedSession, currentSession };
      server.publish("sidebar", JSON.stringify(lastState));
    } else if (hadUnseen) {
      broadcastState();
    } else {
      broadcastFocusOnly();
    }
  }

  function switchToVisibleIndex(index: number, clientTty?: string): void {
    if (!lastState) {
      broadcastState();
    }

    if (!lastState) return;

    const idx = index - 1;
    if (idx < 0 || idx >= lastState.sessions.length) return;

    const name = lastState.sessions[idx]!.name;
    const p = sessionProviders.get(name) ?? mux;
    p.switchSession(name, clientTty);

    if (sidebarVisible && isFullSidebarCapable(p) && p.name === "zellij") {
      const activeWindows = p.listActiveWindows();
      const targetWindow = activeWindows.find((w) => w.sessionName === name);
      if (targetWindow) {
        setTimeout(() => {
          ensureSidebarInWindow(p, { session: name, windowId: targetWindow.id });
        }, 500);
      }
    }
  }

  // --- Sidebar management ---

  function getProvidersWithSidebar() {
    return allProviders.filter(isFullSidebarCapable);
  }

  /** Parse "clientTty|session|windowId" or legacy "session:windowId" context from POST body */
  function parseContext(body: string): { clientTty?: string; session: string; windowId: string } | null {
    const trimmed = body.trim().replace(/^"+|"+$/g, "").replace(/^'+|'+$/g, "");

    // New format: pipe-separated "clientTty|session|windowId"
    const pipeParts = trimmed.split("|");
    if (pipeParts.length === 3 && pipeParts[1] && pipeParts[2]) {
      const ctx = { clientTty: pipeParts[0] || undefined, session: pipeParts[1], windowId: pipeParts[2] };
      if (ctx.clientTty && ctx.session) {
        clientTtyBySession.set(ctx.session, ctx.clientTty);
      }
      return ctx;
    }

    // Legacy format: "session:windowId"
    const colonIdx = trimmed.indexOf(":");
    if (colonIdx < 1) return null;
    const session = trimmed.slice(0, colonIdx);
    const windowId = trimmed.slice(colonIdx + 1);
    if (!session || !windowId) return null;
    return { session, windowId };
  }

  function parseResizeContext(body: string): SidebarResizeContext | null {
    const trimmed = body.trim().replace(/^"+|"+$/g, "").replace(/^'+|'+$/g, "");
    if (!trimmed) return null;

    const [paneId, sessionName, windowId, widthRaw, windowWidthRaw] = trimmed.split("|");
    if (!paneId) return null;

    const width = Number.parseInt(widthRaw ?? "", 10);
    const windowWidth = Number.parseInt(windowWidthRaw ?? "", 10);

    return {
      paneId,
      sessionName: sessionName || undefined,
      windowId: windowId || undefined,
      width: Number.isNaN(width) ? undefined : width,
      windowWidth: Number.isNaN(windowWidth) ? undefined : windowWidth,
    };
  }

  // Short-lived cache for sidebar pane listings — avoid repeated tmux list-panes -a
  let sidebarPaneCache: ReturnType<typeof listSidebarPanesByProviderUncached> | null = null;
  let sidebarPaneCacheTs = 0;
  const SIDEBAR_PANE_CACHE_TTL = 300; // ms

  function listSidebarPanesByProviderUncached() {
    return getProvidersWithSidebar().map((provider) => ({
      provider,
      panes: provider.listSidebarPanes(),
    }));
  }

  function listSidebarPanesByProvider() {
    const now = Date.now();
    if (sidebarPaneCache && now - sidebarPaneCacheTs < SIDEBAR_PANE_CACHE_TTL) return sidebarPaneCache;
    sidebarPaneCache = listSidebarPanesByProviderUncached();
    sidebarPaneCacheTs = now;
    return sidebarPaneCache;
  }

  function invalidateSidebarPaneCache(): void {
    sidebarPaneCache = null;
    sidebarPaneCacheTs = 0;
  }

  const pendingSidebarSpawns = new Set<string>();
  const suppressedSidebarResizeAcks = new Map<string, SidebarResizeSuppression>();
  let sidebarSnapshots = new Map<string, { width?: number; windowWidth?: number }>();
  let pendingSidebarResize: ReturnType<typeof setTimeout> | null = null;
  let pendingSidebarResizeCtx: SidebarResizeContext | undefined;

  function scheduleSidebarResize(ctx?: SidebarResizeContext): void {
    if (ctx) pendingSidebarResizeCtx = ctx;
    resizeSidebars(ctx);
    if (pendingSidebarResize) clearTimeout(pendingSidebarResize);
    // tmux/zellij can finish layout changes slightly after the pane appears.
    pendingSidebarResize = setTimeout(() => {
      const nextCtx = pendingSidebarResizeCtx;
      pendingSidebarResizeCtx = undefined;
      pendingSidebarResize = null;
      resizeSidebars(nextCtx);
    }, 120);
  }

  function toggleSidebar(ctx?: { session: string; windowId: string }): void {
    const providers = getProvidersWithSidebar();
    if (providers.length === 0) {
      log("toggle", "SKIP — no providers with sidebar methods");
      return;
    }

    invalidateSidebarPaneCache();
    if (sidebarVisible) {
      for (const p of providers) {
        const panes = p.listSidebarPanes();
        log("toggle", "OFF — hiding panes", { provider: p.name, count: panes.length });
        for (const pane of panes) {
          p.hideSidebar(pane.paneId);
        }
      }
      sidebarVisible = false;
    } else {
      sidebarVisible = true;
      for (const p of providers) {
        const allWindows = p.listActiveWindows();
        log("toggle", "ON — spawning in active windows", { provider: p.name, count: allWindows.length });
        for (const w of allWindows) {
          ensureSidebarInWindow(p, { session: w.sessionName, windowId: w.id });
        }
      }
      scheduleSidebarResize();
      server.publish("sidebar", JSON.stringify({ type: "re-identify" }));
    }
    log("toggle", "done", { sidebarVisible });
  }

  function ensureSidebarInWindow(provider?: ReturnType<typeof getProvidersWithSidebar>[number], ctx?: { session: string; windowId: string }): void {
    // If no specific provider, try to find one for the session
    const p = provider ?? (() => {
      const providers = getProvidersWithSidebar();
      if (ctx?.session) {
        const sessionProvider = sessionProviders.get(ctx.session);
        return providers.find((pp) => pp === sessionProvider) ?? providers[0];
      }
      return providers[0];
    })();
    if (!p || !sidebarVisible) {
      log("ensure", "SKIP", { hasProvider: !!p, sidebarVisible });
      return;
    }

    const curSession = ctx?.session ?? getCurrentSession();
    if (!curSession) {
      log("ensure", "SKIP — no current session");
      return;
    }

    const windowId = ctx?.windowId ?? p.getCurrentWindowId();
    if (!windowId) {
      log("ensure", "SKIP — could not get window_id");
      return;
    }

    const spawnKey = `${p.name}:${windowId}`;
    if (pendingSidebarSpawns.has(spawnKey)) {
      log("ensure", "SKIP — spawn already in progress", { curSession, windowId, provider: p.name });
      return;
    }

    // Use cached pane listing to avoid redundant tmux list-panes -a calls
    const allPanesByProvider = listSidebarPanesByProvider();
    const providerEntry = allPanesByProvider.find((e) => e.provider === p);
    const existingPanes = providerEntry?.panes ?? [];
    const hasInWindow = existingPanes.some((ep) => ep.windowId === windowId);
    log("ensure", "checking window", {
      curSession, windowId, existingPanes: existingPanes.length,
      hasInWindow, paneIds: existingPanes.map((x) => `${x.paneId}@${x.windowId}`),
    });

    if (!hasInWindow) {
      invalidateSidebarPaneCache();
      pendingSidebarSpawns.add(spawnKey);
      log("ensure", "SPAWNING sidebar", { curSession, windowId, sidebarWidth, sidebarPosition, scriptsDir });
      try {
        const newPaneId = p.spawnSidebar(curSession, windowId, sidebarWidth, sidebarPosition, scriptsDir);
        log("ensure", "spawn result", { newPaneId });
        // Do NOT refocus the main pane here — the TUI handles it.
        // For fresh spawns, the TUI refocuses after capability detection.
        // For stash restores, the TUI refocuses after restoreTerminalModes
        // responses settle. Refocusing immediately from the server causes
        // capability query responses to leak as garbage escape sequences.
      } finally {
        pendingSidebarSpawns.delete(spawnKey);
      }
      // Only schedule resize when we actually spawned — layout changed
      scheduleSidebarResize();
    }
    // When sidebar already exists, no layout change — skip resize
  }

  // Debounced ensure-sidebar — collapses rapid hook-fired calls during fast
  // session switching into a single check after switching settles.
  let ensureSidebarTimer: ReturnType<typeof setTimeout> | null = null;
  let ensureSidebarPendingCtx: { session: string; windowId: string } | undefined;

  function debouncedEnsureSidebar(ctx?: { session: string; windowId: string }): void {
    if (ctx) ensureSidebarPendingCtx = ctx;
    if (ensureSidebarTimer) clearTimeout(ensureSidebarTimer);
    ensureSidebarTimer = setTimeout(() => {
      ensureSidebarTimer = null;
      const nextCtx = ensureSidebarPendingCtx;
      ensureSidebarPendingCtx = undefined;
      ensureSidebarInWindow(undefined, nextCtx);
    }, 150);
  }

  function quitAll(): void {
    log("quit", "killing all sidebar panes");
    for (const p of getProvidersWithSidebar()) {
      const panes = p.listSidebarPanes();
      log("quit", "found panes to kill", { provider: p.name, count: panes.length });
      for (const pane of panes) {
        p.killSidebarPane(pane.paneId);
      }
    }
    // Provider-specific cleanup (uses type guard)
    for (const p of getProvidersWithSidebar()) {
      p.cleanupSidebar();
    }
    server.publish("sidebar", JSON.stringify({ type: "quit" }));
    sidebarVisible = false;
    cleanup();
    process.exit(0);
  }

  // --- Sidebar resize enforcement ---

  function resizeSidebars(ctx?: SidebarResizeContext) {
    const panesByProvider = listSidebarPanesByProvider();
    const allPanes = panesByProvider.flatMap(({ panes }) => panes);

    if (allPanes.length === 0) {
      sidebarSnapshots = new Map();
      return;
    }

    const nextSidebarWidth = resolveSidebarWidthFromResizeContext({
      ctx,
      panes: allPanes,
      previousByWindow: sidebarSnapshots,
      suppressedByPane: suppressedSidebarResizeAcks,
    });

    if (nextSidebarWidth != null && nextSidebarWidth !== sidebarWidth) {
      sidebarWidth = nextSidebarWidth;
      saveConfig({ sidebarWidth });
      log("resize", "adopted sidebar width from pane resize", {
        paneId: ctx?.paneId ?? null,
        sessionName: ctx?.sessionName ?? null,
        windowId: ctx?.windowId ?? null,
        sidebarWidth,
      });
      broadcastState();
    }

    const now = Date.now();
    for (const { provider, panes } of panesByProvider) {
      log("resize", "enforcing width on all panes", {
        provider: provider.name,
        sidebarWidth,
        count: panes.length,
        triggerPaneId: ctx?.paneId ?? null,
      });
      for (const pane of panes) {
        if (pane.width === sidebarWidth) continue;
        suppressedSidebarResizeAcks.set(pane.paneId, { width: sidebarWidth, expiresAt: now + 1_000 });
        provider.resizeSidebarPane(pane.paneId, sidebarWidth);
      }
    }

    // After resizing, invalidate the sidebar pane cache since widths changed,
    // and use the already-fetched list for the snapshot (avoid another tmux call).
    if (panesByProvider.some(({ panes }) => panes.some((pane) => pane.width !== sidebarWidth))) {
      invalidateSidebarPaneCache();
    }
    sidebarSnapshots = snapshotSidebarWindows(allPanes);
  }

  // --- Focus agent pane (click-to-focus from TUI) ---

  /** Walk up to 3 levels of child processes looking for a command matching any pattern */
  function matchProcessTree(pid: string, patterns: string[], depth = 0): boolean {
    if (depth > 2) return false;
    const children = shell(["pgrep", "-P", pid]);
    if (!children) return false;
    for (const childPid of children.split("\n")) {
      const trimmed = childPid.trim();
      if (!trimmed) continue;
      const childCmd = shell(["ps", "-p", trimmed, "-o", "comm="]);
      if (childCmd && patterns.some((pat) => childCmd.toLowerCase().includes(pat))) return true;
      if (matchProcessTree(trimmed, patterns, depth + 1)) return true;
    }
    return false;
  }

  const AGENT_TITLE_PATTERNS: Record<string, string[]> = {
    amp: ["amp"],
    "claude-code": ["claude"],
    codex: ["codex"],
    opencode: ["opencode"],
  };

  const PANE_HIGHLIGHT_BORDER = "fg=#fab387,bold";
  const PANE_HIGHLIGHT_MS = 300;
  const pendingHighlightResets = new Map<string, ReturnType<typeof setTimeout>>();

  /** Walk child processes (up to 3 levels) to find a process matching `name`, returning its PID. */
  function findChildPid(pid: string, name: string, depth = 0): string | undefined {
    if (depth > 2) return undefined;
    const children = shell(["pgrep", "-P", pid]);
    if (!children) return undefined;
    for (const childPid of children.split("\n")) {
      const trimmed = childPid.trim();
      if (!trimmed) continue;
      const childCmd = shell(["ps", "-p", trimmed, "-o", "comm="]);
      if (childCmd?.trim().toLowerCase().includes(name)) return trimmed;
      const found = findChildPid(trimmed, name, depth + 1);
      if (found) return found;
    }
    return undefined;
  }

  type PaneEntry = { id: string; pid: string; cmd: string; title: string };

  /** Claude Code: ~/.claude/sessions/<pid>.json → sessionId */
  function resolveClaudeCodePane(panes: PaneEntry[], threadId: string): string | undefined {
    const sessionsDir = join(homedir(), ".claude", "sessions");
    for (const pane of panes) {
      const agentPid = findChildPid(pane.pid, "claude");
      if (!agentPid) continue;
      try {
        const data = JSON.parse(readFileSync(join(sessionsDir, `${agentPid}.json`), "utf-8"));
        if (data.sessionId === threadId) return pane.id;
      } catch {}
    }
    return undefined;
  }

  /** Codex: logs_1.sqlite process_uuid='pid:<PID>:*' → thread_id */
  function resolveCodexPane(panes: PaneEntry[], threadId: string): string | undefined {
    const dbPath = join(process.env.CODEX_HOME ?? join(homedir(), ".codex"), "logs_1.sqlite");
    let db: any;
    try {
      const { Database } = require("bun:sqlite");
      db = new Database(dbPath, { readonly: true });
    } catch { return undefined; }

    try {
      for (const pane of panes) {
        const agentPid = findChildPid(pane.pid, "codex");
        if (!agentPid) continue;
        const row = db.query(
          `SELECT thread_id FROM logs WHERE process_uuid LIKE ? AND thread_id IS NOT NULL ORDER BY ts DESC LIMIT 1`,
        ).get(`pid:${agentPid}:%`);
        if (row?.thread_id === threadId) return pane.id;
      }
    } finally { try { db.close(); } catch {} }
    return undefined;
  }

  /** OpenCode: lsof → log file → grep session ID */
  function resolveOpenCodePane(panes: PaneEntry[], threadId: string): string | undefined {
    for (const pane of panes) {
      const agentPid = findChildPid(pane.pid, "opencode");
      if (!agentPid) continue;
      const lsofOut = shell(["lsof", "-p", agentPid]);
      if (!lsofOut) continue;
      // Find the log file path from open file descriptors
      const logLine = lsofOut.split("\n").find((l) => l.includes("/opencode/log/") && l.endsWith(".log"));
      if (!logLine) continue;
      // Extract absolute path — lsof NAME column starts at the last recognized path
      const pathMatch = logLine.match(/\s(\/\S+\.log)$/);
      if (!pathMatch) continue;
      try {
        const logText = readFileSync(pathMatch[1], "utf-8");
        const match = logText.match(/ses_[A-Za-z0-9]+/);
        if (match?.[0] === threadId) return pane.id;
      } catch {}
    }
    return undefined;
  }

  /** Resolve a tmux pane ID for an agent using all available resolution strategies. */
  function resolveAgentPaneId(sessionName: string, agentName: string, threadId?: string, threadName?: string): string | undefined {
    const p = sessionProviders.get(sessionName) ?? mux;
    if (p.name !== "tmux") return undefined;

    const patterns = AGENT_TITLE_PATTERNS[agentName];
    if (!patterns) return undefined;

    const raw = shell([
      "tmux", "list-panes", "-t", sessionName,
      "-F", "#{pane_id}|#{pane_pid}|#{pane_current_command}|#{pane_title}",
    ]);
    if (!raw) return undefined;

    const panes = raw.split("\n")
      .map((line) => {
        const idx1 = line.indexOf("|");
        const idx2 = line.indexOf("|", idx1 + 1);
        const idx3 = line.indexOf("|", idx2 + 1);
        return {
          id: line.slice(0, idx1),
          pid: line.slice(idx1 + 1, idx2),
          cmd: line.slice(idx2 + 1, idx3),
          title: line.slice(idx3 + 1),
        };
      });

    const sidebarPaneIds = new Set<string>();
    for (const { panes: sbPanes } of listSidebarPanesByProvider()) {
      for (const sb of sbPanes) sidebarPaneIds.add(sb.paneId);
    }
    const nonSidebar = panes.filter((p) => !sidebarPaneIds.has(p.id));

    let targetPaneId: string | undefined;

    if (agentName === "claude-code" && threadId) {
      targetPaneId = resolveClaudeCodePane(nonSidebar, threadId);
    }
    if (!targetPaneId && agentName === "amp" && threadName) {
      targetPaneId = nonSidebar
        .find((p) => p.title.toLowerCase().startsWith("amp - ") && p.title.includes(threadName))
        ?.id;
    }
    if (!targetPaneId && agentName === "codex" && threadId) {
      targetPaneId = resolveCodexPane(nonSidebar, threadId);
    }
    if (!targetPaneId && agentName === "opencode" && threadId) {
      targetPaneId = resolveOpenCodePane(nonSidebar, threadId);
    }
    if (!targetPaneId) {
      targetPaneId = nonSidebar
        .find((p) => patterns.some((pat) => p.title.toLowerCase().includes(pat)))
        ?.id;
    }
    if (!targetPaneId) {
      for (const pane of nonSidebar) {
        if (matchProcessTree(pane.pid, patterns)) {
          targetPaneId = pane.id;
          break;
        }
      }
    }
    return targetPaneId;
  }

  function focusAgentPane(sessionName: string, agentName: string, threadId?: string, threadName?: string): void {
    log("focus-agent-pane", "received", { sessionName, agentName, threadId, threadName });
    const targetPaneId = resolveAgentPaneId(sessionName, agentName, threadId, threadName);
    if (!targetPaneId) return;

    log("focus-agent-pane", "focusing", { sessionName, agentName, paneId: targetPaneId });
    shell(["tmux", "select-pane", "-t", targetPaneId]);

    const existing = pendingHighlightResets.get(targetPaneId);
    if (existing) clearTimeout(existing);

    shell(["tmux", "set-option", "-p", "-t", targetPaneId, "pane-active-border-style", PANE_HIGHLIGHT_BORDER]);
    shell(["tmux", "select-pane", "-t", targetPaneId, "-P", "bg=#2a2a4a"]);
    pendingHighlightResets.set(
      targetPaneId,
      setTimeout(() => {
        shell(["tmux", "set-option", "-p", "-t", targetPaneId, "-u", "pane-active-border-style"]);
        shell(["tmux", "select-pane", "-t", targetPaneId, "-P", ""]);
        pendingHighlightResets.delete(targetPaneId);
      }, PANE_HIGHLIGHT_MS),
    );
  }

  function killAgentPane(sessionName: string, agentName: string, threadId?: string, threadName?: string): void {
    log("kill-agent-pane", "received", { sessionName, agentName, threadId, threadName });
    const targetPaneId = resolveAgentPaneId(sessionName, agentName, threadId, threadName);
    if (!targetPaneId) return;

    log("kill-agent-pane", "killing", { sessionName, agentName, paneId: targetPaneId });
    shell(["tmux", "kill-pane", "-t", targetPaneId]);
  }

  // --- Pane agent scanning (detect agents running in current session panes) ---

  interface PaneAgentPresence {
    agent: string;
    session: string;
    paneId: string;
    threadId?: string;
    threadName?: string;
    status?: import("../contracts/agent").AgentStatus;
    lastSeenTs: number;
  }

  // Pane agent presence per session: sessionName → Map<instanceKey, PaneAgentPresence>
  let paneAgentsBySession = new Map<string, Map<string, PaneAgentPresence>>();

  /** Build parent→children map from a single ps snapshot (avoids per-pane pgrep calls). */
  function buildProcessTree(): { childrenOf: Map<number, number[]>; commOf: Map<number, string> } {
    const childrenOf = new Map<number, number[]>();
    const commOf = new Map<number, string>();
    const psResult = Bun.spawnSync(["ps", "-eo", "pid=,ppid=,comm="], { stdout: "pipe", stderr: "pipe" });
    for (const line of psResult.stdout.toString().trim().split("\n")) {
      const parts = line.trim().split(/\s+/);
      if (parts.length < 3) continue;
      const pid = parseInt(parts[0], 10);
      const ppid = parseInt(parts[1], 10);
      const comm = parts.slice(2).join(" ").toLowerCase();
      if (isNaN(pid) || isNaN(ppid)) continue;
      commOf.set(pid, comm);
      let arr = childrenOf.get(ppid);
      if (!arr) { arr = []; childrenOf.set(ppid, arr); }
      arr.push(pid);
    }
    return { childrenOf, commOf };
  }

  /** Walk up to 3 levels of child processes using a pre-built process tree. */
  function matchProcessTreeFast(
    pid: number, patterns: string[],
    tree: ReturnType<typeof buildProcessTree>, depth = 0,
  ): boolean {
    if (depth > 2) return false;
    const children = tree.childrenOf.get(pid);
    if (!children) return false;
    for (const childPid of children) {
      const comm = tree.commOf.get(childPid);
      if (comm && patterns.some((pat) => comm.includes(pat))) return true;
      if (matchProcessTreeFast(childPid, patterns, tree, depth + 1)) return true;
    }
    return false;
  }

  /** Find child PID matching a name pattern using pre-built process tree. */
  function findChildPidFast(
    pid: number, name: string,
    tree: ReturnType<typeof buildProcessTree>, depth = 0,
  ): number | undefined {
    if (depth > 2) return undefined;
    const children = tree.childrenOf.get(pid);
    if (!children) return undefined;
    for (const childPid of children) {
      const comm = tree.commOf.get(childPid);
      if (comm?.includes(name)) return childPid;
      const found = findChildPidFast(childPid, name, tree, depth + 1);
      if (found) return found;
    }
    return undefined;
  }

  /** Resolve threadId/threadName for an amp pane from its title. */
  function resolveAmpPaneInfo(title: string): { threadId?: string; threadName?: string } {
    // Amp pane title format: "amp - <threadName> - <dir>"
    if (!title.toLowerCase().startsWith("amp - ")) return {};
    const rest = title.slice(6);
    const dashIdx = rest.lastIndexOf(" - ");
    const threadName = dashIdx > 0 ? rest.slice(0, dashIdx) : rest;
    return { threadName: threadName || undefined };
  }

  /** Resolve threadId/threadName/status for a Claude Code pane via ~/.claude/sessions/<pid>.json + journal. */
  function resolveClaudeCodePaneInfo(
    panePid: number, tree: ReturnType<typeof buildProcessTree>,
  ): { threadId?: string; threadName?: string; status?: import("../contracts/agent").AgentStatus } {
    const agentPid = findChildPidFast(panePid, "claude", tree);
    if (!agentPid) return {};
    const sessionsDir = join(homedir(), ".claude", "sessions");
    try {
      const data = JSON.parse(readFileSync(join(sessionsDir, `${agentPid}.json`), "utf-8"));
      const threadId: string | undefined = data.sessionId;
      if (!threadId) return {};
      // Try to get thread name and status from the journal
      const journalInfo = resolveClaudeCodeJournalInfo(threadId);
      return { threadId, ...journalInfo };
    } catch { return {}; }
  }

  /** Read the JSONL journal to extract thread name and current status. */
  function resolveClaudeCodeJournalInfo(threadId: string): { threadName?: string; status?: import("../contracts/agent").AgentStatus } {
    const projectsDir = join(homedir(), ".claude", "projects");
    try {
      const dirs = require("fs").readdirSync(projectsDir) as string[];
      for (const dir of dirs) {
        const filePath = join(projectsDir, dir, `${threadId}.jsonl`);
        try {
          const text = readFileSync(filePath, "utf-8");
          const lines = text.split("\n").filter(Boolean);
          let threadName: string | undefined;
          let lastStatus: import("../contracts/agent").AgentStatus = "idle";

          for (const line of lines) {
            try {
              const entry = JSON.parse(line);

              // Prefer custom title from /rename command
              if (entry.type === "custom-title" && entry.customTitle) {
                threadName = entry.customTitle;
                continue;
              }

              const msg = entry.message;
              if (!msg?.role) continue;

              // Extract thread name from first user message (fallback)
              if (!threadName && msg.role === "user") {
                const content = msg.content;
                let t: string | undefined;
                if (typeof content === "string") t = content;
                else if (Array.isArray(content)) t = content.find((c: any) => c.type === "text" && c.text)?.text;
                if (t && !t.startsWith("<") && !t.startsWith("{")) threadName = t.slice(0, 80);
              }

              // Determine status from last entry (same logic as ClaudeCodeAgentWatcher)
              if (msg.role === "assistant") {
                const items = Array.isArray(msg.content) ? msg.content : [];
                lastStatus = items.some((c: any) => c.type === "tool_use") ? "running" : "done";
              } else if (msg.role === "user") {
                lastStatus = "running";
              }
            } catch { continue; }
          }

          return { threadName, status: lastStatus };
        } catch { continue; }
      }
    } catch {}
    return {};
  }

  /** Resolve threadId for a Codex pane via logs_1.sqlite. */
  function resolveCodexPaneInfo(
    panePid: number, tree: ReturnType<typeof buildProcessTree>,
  ): { threadId?: string; threadName?: string } {
    const agentPid = findChildPidFast(panePid, "codex", tree);
    if (!agentPid) return {};
    const dbPath = join(process.env.CODEX_HOME ?? join(homedir(), ".codex"), "logs_1.sqlite");
    let db: any;
    try {
      const { Database } = require("bun:sqlite");
      db = new Database(dbPath, { readonly: true });
    } catch { return {}; }
    try {
      const row = db.query(
        `SELECT thread_id FROM logs WHERE process_uuid LIKE ? AND thread_id IS NOT NULL ORDER BY ts DESC LIMIT 1`,
      ).get(`pid:${agentPid}:%`);
      if (row?.thread_id) return { threadId: row.thread_id };
    } catch {} finally { try { db.close(); } catch {} }
    return {};
  }

  /** Scan all panes across all tmux sessions and identify running agents.
   *  Uses a single `tmux list-panes -a` call for efficiency. */
  function scanAllTmuxPaneAgents(): Map<string, Map<string, PaneAgentPresence>> {
    const result = new Map<string, Map<string, PaneAgentPresence>>();

    const raw = shell([
      "tmux", "list-panes", "-a",
      "-F", "#{session_name}|#{pane_id}|#{pane_pid}|#{pane_current_command}|#{pane_title}",
    ]);
    if (!raw) return result;

    const panes = raw.split("\n").filter(Boolean).map((line) => {
      const idx1 = line.indexOf("|");
      const idx2 = line.indexOf("|", idx1 + 1);
      const idx3 = line.indexOf("|", idx2 + 1);
      const idx4 = line.indexOf("|", idx3 + 1);
      return {
        session: line.slice(0, idx1),
        id: line.slice(idx1 + 1, idx2),
        pid: parseInt(line.slice(idx2 + 1, idx3), 10),
        cmd: line.slice(idx3 + 1, idx4),
        title: line.slice(idx4 + 1),
      };
    });

    // Exclude sidebar panes
    const sidebarPaneIds = new Set<string>();
    for (const { panes: sbPanes } of listSidebarPanesByProvider()) {
      for (const sb of sbPanes) sidebarPaneIds.add(sb.paneId);
    }

    const nonSidebar = panes.filter((p) => !sidebarPaneIds.has(p.id));
    if (nonSidebar.length === 0) return result;

    // Build process tree once for all panes
    const tree = buildProcessTree();
    const now = Date.now();

    for (const pane of nonSidebar) {
      for (const [agentName, patterns] of Object.entries(AGENT_TITLE_PATTERNS)) {
        // Only use process tree matching — title matching produces false positives
        // (e.g. an Amp thread named "Detect Claude session names" matches "claude")
        if (!matchProcessTreeFast(pane.pid, patterns, tree)) continue;

        let threadId: string | undefined;
        let threadName: string | undefined;
        let status: import("../contracts/agent").AgentStatus | undefined;

        // Resolve thread info per agent type
        if (agentName === "amp") {
          const info = resolveAmpPaneInfo(pane.title);
          threadName = info.threadName;
        } else if (agentName === "claude-code") {
          const info = resolveClaudeCodePaneInfo(pane.pid, tree);
          threadId = info.threadId;
          threadName = info.threadName;
          status = info.status;
        } else if (agentName === "codex") {
          const info = resolveCodexPaneInfo(pane.pid, tree);
          threadId = info.threadId;
        }

        const key = `${agentName}:pane:${pane.id}`;
        let sessionAgents = result.get(pane.session);
        if (!sessionAgents) {
          sessionAgents = new Map();
          result.set(pane.session, sessionAgents);
        }
        sessionAgents.set(key, {
          agent: agentName,
          session: pane.session,
          paneId: pane.id,
          threadId,
          threadName,
          status,
          lastSeenTs: now,
        });
      }
    }

    return result;
  }

  /** Refresh pane agent cache for all tmux sessions. */
  function refreshPaneAgents(): void {
    // Check if any provider is tmux
    const hasTmux = allProviders.some((p) => p.name === "tmux");
    if (!hasTmux) {
      if (paneAgentsBySession.size > 0) {
        paneAgentsBySession.clear();
        tracker.setPinnedInstancesMulti(new Map());
        broadcastState();
      }
      return;
    }

    const nextBySession = scanAllTmuxPaneAgents();
    const allPinnedKeys = new Map<string, string[]>();
    for (const [session, agents] of nextBySession) {
      allPinnedKeys.set(session, [...agents.keys()]);
    }

    // Check if anything changed
    let changed = paneAgentsBySession.size !== nextBySession.size;
    if (!changed) {
      for (const [session, agents] of nextBySession) {
        const prev = paneAgentsBySession.get(session);
        if (!prev || prev.size !== agents.size) { changed = true; break; }
        for (const [key, agent] of agents) {
          const prevAgent = prev.get(key);
          if (!prevAgent || prevAgent.threadName !== agent.threadName) { changed = true; break; }
        }
        if (changed) break;
      }
    }

    paneAgentsBySession = nextBySession;

    // Update tracker pinning for all sessions
    tracker.setPinnedInstancesMulti(allPinnedKeys);

    if (changed) broadcastState();
  }

  // --- Pane agent polling (detect agents in current session every 3s) ---

  const PANE_SCAN_INTERVAL_MS = 3_000;
  let paneScanTimer: ReturnType<typeof setInterval> | null = null;

  function startPaneScan() {
    paneScanTimer = setInterval(() => {
      if (clientCount === 0) return;
      refreshPaneAgents();
    }, PANE_SCAN_INTERVAL_MS);
  }

  function handleCommand(cmd: ClientCommand, ws: any) {
    switch (cmd.type) {
      case "identify":
        clientTtys.set(ws, cmd.clientTty);
        break;
      case "switch-session": {
        // Resolve TTY: hook-derived (authoritative) > client-provided > stored
        const clientSess = clientSessionNames.get(ws);
        const tty = (clientSess ? clientTtyBySession.get(clientSess) : undefined)
          ?? cmd.clientTty ?? clientTtys.get(ws);
        log("switch-session", "switching", { target: cmd.name, tty, clientSess });
        const p = sessionProviders.get(cmd.name) ?? mux;

        // Detect cross-mux switch (e.g., zellij→tmux or tmux→zellij)
        const sourceProvider = clientSess ? sessionProviders.get(clientSess) : null;
        if (sourceProvider && sourceProvider.name !== p.name) {
          log("switch-session", "cross-mux detected", {
            source: sourceProvider.name, target: p.name, sourceSession: clientSess,
          });
          if (sourceProvider.name === "zellij" && p.name === "tmux") {
            // Write reattach target for the bash wrapper
            writeFileSync("/tmp/opensessions-reattach", cmd.name);
            // Detach from zellij — the wrapper script will auto-attach to tmux
            Bun.spawnSync(["zellij", "--session", clientSess!, "action", "detach"], {
              stdout: "pipe", stderr: "pipe",
            });
            break; // Don't call p.switchSession — the wrapper handles it
          }
        }

        p.switchSession(cmd.name, tty);

        // Optimistic server-side focus update — so other TUI instances see the
        // change immediately via broadcastFocusOnly, without waiting for the
        // tmux hook round-trip. The hook's /focus POST will reconcile if needed.
        focusedSession = cmd.name;
        cachedCurrentSession = cmd.name;
        cachedCurrentSessionTs = Date.now();
        const hadUnseen = tracker.handleFocus(cmd.name);
        if (hadUnseen) {
          broadcastState();
        } else {
          broadcastFocusOnly();
        }

        // Auto-ensure sidebar in the target session if sidebar is visible.
        // In tmux, hooks handle this — but zellij has no hooks, so we do it here.
        // Use listActiveWindows() to find the target session's active tab
        // (getCurrentWindowId() won't work from the server since ZELLIJ_SESSION_NAME isn't set).
        if (sidebarVisible && isFullSidebarCapable(p) && p.name === "zellij") {
          const activeWindows = p.listActiveWindows();
          const targetWindow = activeWindows.find((w) => w.sessionName === cmd.name);
          log("switch-session", "auto-ensure sidebar", {
            target: cmd.name, provider: p.name,
            activeWindows: activeWindows.length, targetWindow: targetWindow?.id ?? null,
          });
          if (targetWindow) {
            // 1.5s delay — zellij needs time to attach the client before we can spawn panes
            setTimeout(() => {
              ensureSidebarInWindow(p, { session: cmd.name, windowId: targetWindow.id });
            }, 1500);
          }
        }
        break;
      }
      case "switch-index": {
        const clientSess = clientSessionNames.get(ws);
        const tty = (clientSess ? clientTtyBySession.get(clientSess) : undefined)
          ?? clientTtys.get(ws);
        switchToVisibleIndex(cmd.index, tty);
        break;
      }
      case "new-session":
        mux.createSession();
        broadcastState();
        break;
      case "hide-session":
        sessionOrder.hide(cmd.name);
        broadcastState();
        break;
      case "show-all-sessions":
        sessionOrder.showAll();
        broadcastState();
        break;
      case "kill-session": {
        const p = sessionProviders.get(cmd.name) ?? mux;
        p.killSession(cmd.name);
        broadcastState();
        break;
      }
      case "reorder-session":
        sessionOrder.reorder(cmd.name, cmd.delta);
        broadcastState();
        break;
      case "refresh":
        broadcastState();
        break;
      case "move-focus":
        moveFocus(cmd.delta, ws);
        break;
      case "focus-session":
        setFocus(cmd.name, ws);
        break;
      case "mark-seen":
        if (tracker.markSeen(cmd.name)) broadcastState();
        break;
      case "dismiss-agent":
        if (tracker.dismiss(cmd.session, cmd.agent, cmd.threadId)) broadcastState();
        break;
      case "set-theme":
        currentTheme = cmd.theme;
        saveConfig({ theme: cmd.theme });
        broadcastState();
        break;
      case "report-width":
        // No-op: sidebar width is config-only, not auto-saved from drag
        break;
      case "quit":
        quitAll();
        break;
      case "identify-pane":
        // Store this client's session, reply with session + authoritative client TTY
        clientSessionNames.set(ws, cmd.sessionName);
        ws.send(JSON.stringify({
          type: "your-session",
          name: cmd.sessionName,
          clientTty: clientTtyBySession.get(cmd.sessionName) ?? null,
        }));
        break;
      case "focus-agent-pane":
        log("handleCommand", "focus-agent-pane received", { session: cmd.session, agent: cmd.agent, threadId: cmd.threadId, threadName: cmd.threadName });
        focusAgentPane(cmd.session, cmd.agent, cmd.threadId, cmd.threadName);
        break;
      case "kill-agent-pane":
        log("handleCommand", "kill-agent-pane received", { session: cmd.session, agent: cmd.agent, threadId: cmd.threadId, threadName: cmd.threadName });
        killAgentPane(cmd.session, cmd.agent, cmd.threadId, cmd.threadName);
        break;
    }
  }

  // --- Port polling (detect new/stopped listeners every 10s) ---

  const PORT_POLL_INTERVAL_MS = 10_000;
  let portPollTimer: ReturnType<typeof setInterval> | null = null;

  function startPortPoll() {
    // Run initial snapshot immediately so first broadcast has ports
    if (lastState) {
      refreshPortSnapshot(lastState.sessions.map((s) => s.name));
    }
    portPollTimer = setInterval(() => {
      if (!lastState || clientCount === 0) return;
      const changed = refreshPortSnapshot(lastState.sessions.map((s) => s.name));
      if (changed) broadcastState();
    }, PORT_POLL_INTERVAL_MS);
  }

  function cleanup() {
    for (const w of allWatchers) w.stop();
    if (watcherBroadcastTimer) clearTimeout(watcherBroadcastTimer);
    if (debounceTimer) clearTimeout(debounceTimer);
    if (portPollTimer) clearInterval(portPollTimer);
    if (paneScanTimer) clearInterval(paneScanTimer);
    if (pendingSidebarResize) clearTimeout(pendingSidebarResize);
    for (const timer of pendingHighlightResets.values()) clearTimeout(timer);
    pendingHighlightResets.clear();
    for (const watcher of gitHeadWatchers.values()) watcher.close();
    gitHeadWatchers.clear();
    if (idleTimer) clearTimeout(idleTimer);
    try { unlinkSync(PID_FILE); } catch {}
    for (const p of allProviders) p.cleanupHooks();
  }

  // --- Write PID + start server ---

  writeFileSync(PID_FILE, String(process.pid));

  const server = Bun.serve({
    port: SERVER_PORT,
    hostname: SERVER_HOST,
    async fetch(req, server) {
      const url = new URL(req.url);

      if (req.method === "POST" && url.pathname === "/refresh") {
        broadcastState();
        return new Response("ok", { status: 200 });
      }

      if (req.method === "POST" && url.pathname === "/resize-sidebars") {
        const body = await req.text();
        const ctx = parseResizeContext(body) ?? undefined;
        log("http", "POST /resize-sidebars", { sidebarWidth, ctx });
        scheduleSidebarResize(ctx);
        return new Response("ok", { status: 200 });
      }

      if (req.method === "POST" && url.pathname === "/focus") {
        try {
          const body = await req.text();
          const ctx = parseContext(body);
          if (ctx) {
            handleFocus(ctx.session);
          } else {
            // Legacy: body is just the session name
            const name = body.trim().replace(/^"+|"+$/g, "");
            if (name) handleFocus(name);
          }
        } catch {}
        return new Response("ok", { status: 200 });
      }

      if (req.method === "POST" && url.pathname === "/toggle") {
        try {
          const body = await req.text();
          const ctx = parseContext(body) ?? undefined;
          log("http", "POST /toggle", { ctx });
          toggleSidebar(ctx);
          broadcastState();
        } catch {}
        return new Response("ok", { status: 200 });
      }

      if (req.method === "POST" && url.pathname === "/quit") {
        log("http", "POST /quit");
        quitAll();
        return new Response("ok", { status: 200 });
      }

      if (req.method === "POST" && url.pathname === "/switch-index") {
        try {
          const index = Number.parseInt(url.searchParams.get("index") ?? "", 10);
          if (Number.isNaN(index)) {
            return new Response("missing index", { status: 400 });
          }
          const body = await req.text();
          const ctx = parseContext(body) ?? undefined;
          log("http", "POST /switch-index", { index, ctx });
          switchToVisibleIndex(index, ctx?.clientTty);
        } catch {}
        return new Response("ok", { status: 200 });
      }

      if (req.method === "POST" && url.pathname === "/ensure-sidebar") {
        try {
          const body = await req.text();
          const ctx = parseContext(body) ?? undefined;
          log("http", "POST /ensure-sidebar", { sidebarVisible, ctx });
          // Debounce ensure-sidebar during rapid switching — intermediate sessions
          // don't need full sidebar validation immediately.
          debouncedEnsureSidebar(ctx ?? undefined);
        } catch {}
        return new Response("ok", { status: 200 });
      }

      if (req.method === "POST" && url.pathname === "/set-status") {
        try {
          const body = await req.json() as { session?: string; text?: string | null; tone?: string };
          if (!body.session || typeof body.session !== "string") {
            return new Response("missing session", { status: 400 });
          }
          if (body.text === null || body.text === undefined) {
            metadataStore.setStatus(body.session, null);
          } else if (typeof body.text !== "string") {
            return new Response("text must be a string or null", { status: 400 });
          } else {
            metadataStore.setStatus(body.session, { text: body.text, tone: body.tone as any });
          }
          broadcastState();
          return new Response(null, { status: 204 });
        } catch {
          return new Response("invalid json", { status: 400 });
        }
      }

      if (req.method === "POST" && url.pathname === "/set-progress") {
        try {
          const body = await req.json() as { session?: string; current?: number; total?: number; percent?: number; label?: string; clear?: boolean };
          if (!body.session || typeof body.session !== "string") {
            return new Response("missing session", { status: 400 });
          }
          if (body.clear) {
            metadataStore.setProgress(body.session, null);
          } else {
            metadataStore.setProgress(body.session, {
              current: body.current,
              total: body.total,
              percent: body.percent,
              label: body.label,
            });
          }
          broadcastState();
          return new Response(null, { status: 204 });
        } catch {
          return new Response("invalid json", { status: 400 });
        }
      }

      if (req.method === "POST" && url.pathname === "/log") {
        try {
          const body = await req.json() as { session?: string; message?: string; tone?: string; source?: string };
          if (!body.session || typeof body.session !== "string") {
            return new Response("missing session", { status: 400 });
          }
          if (!body.message || typeof body.message !== "string") {
            return new Response("missing message", { status: 400 });
          }
          metadataStore.appendLog(body.session, {
            message: body.message,
            tone: body.tone as any,
            source: body.source,
          });
          broadcastState();
          return new Response(null, { status: 204 });
        } catch {
          return new Response("invalid json", { status: 400 });
        }
      }

      if (req.method === "POST" && url.pathname === "/clear-log") {
        try {
          const body = await req.json() as { session?: string };
          if (!body.session || typeof body.session !== "string") {
            return new Response("missing session", { status: 400 });
          }
          metadataStore.clearLogs(body.session);
          broadcastState();
          return new Response(null, { status: 204 });
        } catch {
          return new Response("invalid json", { status: 400 });
        }
      }

      if (req.method === "POST" && url.pathname === "/notify") {
        try {
          const body = await req.json() as { session?: string; message?: string; tone?: string; source?: string };
          if (!body.session || typeof body.session !== "string") {
            return new Response("missing session", { status: 400 });
          }
          if (!body.message || typeof body.message !== "string") {
            return new Response("missing message", { status: 400 });
          }
          metadataStore.appendLog(body.session, {
            message: body.message,
            tone: body.tone as any,
            source: body.source,
          });
          broadcastState();
          return new Response(null, { status: 204 });
        } catch {
          return new Response("invalid json", { status: 400 });
        }
      }

      if (server.upgrade(req, { data: {} })) return;
      return new Response("opensessions server", { status: 200 });
    },
    websocket: {
      open(ws) {
        ws.subscribe("sidebar");
        clientCount++;
        log("ws", "client connected", { clientCount });
        if (idleTimer) {
          clearTimeout(idleTimer);
          idleTimer = null;
        }
        if (lastState) {
          ws.send(JSON.stringify(lastState));
        } else {
          broadcastState();
        }
      },
      close(ws) {
        ws.unsubscribe("sidebar");
        clientCount--;
        if (clientCount < 0) clientCount = 0;
        log("ws", "client disconnected", { clientCount });
        if (clientCount === 0 && !idleTimer) {
          log("ws", "no clients remaining, starting idle timer", { timeoutMs: SERVER_IDLE_TIMEOUT_MS });
          idleTimer = setTimeout(() => {
            log("ws", "idle timeout reached, shutting down");
            quitAll();
          }, SERVER_IDLE_TIMEOUT_MS);
        }
      },
      message(ws, msg) {
        try {
          const cmd = JSON.parse(msg as string) as ClientCommand;
          log("ws", "command", { type: cmd.type });
          handleCommand(cmd, ws);
        } catch {}
      },
    },
  });

  // --- Bootstrap ---

  for (const p of allProviders) p.setupHooks(SERVER_HOST, SERVER_PORT);
  // Seed port snapshot before first broadcast so clients see ports immediately
  {
    const allMuxSessions: string[] = [];
    for (const p of allProviders) {
      for (const s of p.listSessions()) allMuxSessions.push(s.name);
    }
    refreshPortSnapshot(allMuxSessions);
  }
  broadcastState();
  startPortPoll();
  startPaneScan();
  // Run initial pane scan
  refreshPaneAgents();

  // Start agent watchers after server is ready
  for (const w of allWatchers) {
    w.start(watcherCtx);
    log("server", `agent watcher started: ${w.name}`);
  }

  process.on("SIGINT", () => { cleanup(); process.exit(0); });
  process.on("SIGTERM", () => { cleanup(); process.exit(0); });

  const names = allProviders.map((p) => p.name).join(", ");
  console.log(`opensessions server listening on ${SERVER_HOST}:${SERVER_PORT} (mux: ${names})`);
}
