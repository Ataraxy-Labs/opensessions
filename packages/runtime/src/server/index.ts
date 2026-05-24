import { existsSync, readFileSync, unlinkSync, writeFileSync, appendFileSync, watch, type FSWatcher } from "fs";
import { join } from "path";
import { homedir } from "os";
import type { MuxProvider } from "../contracts/mux";
import { isFullSidebarCapable, isBatchCapable } from "../contracts/mux";
import type { AgentEvent, AgentStatus, PanePresenceInput } from "../contracts/agent";
import type { AgentThreadOwner, AgentWatcher, AgentWatcherContext } from "../contracts/agent-watcher";
import { AgentTracker } from "../agents/tracker";
import { SessionOrder } from "./session-order";
import { SessionMetadataStore } from "./metadata-store";
import { canonicalizeAgentEvent } from "./agent-ownership";
import { PiLiveResolver } from "./pi-live-resolver";
import { parsePiRuntimeInfo } from "./pi-runtime-registry";
import { buildDirSessionMap, resolveSessionForProjectDir } from "./project-dir-session";
import { buildLocalLinks, loadPortlessState } from "./portless";
import {
  applySidebarWidthReport,
  areWidthReportsSuppressed,
  createSidebarCoordinator,
  isClientResizeReportGuardActive as isSidebarClientResizeReportGuardActive,
  isClientResizeSyncActive as isSidebarClientResizeSyncActive,
  isUserDragActive,
  readSidebarCoordinatorState,
} from "./sidebar-coordinator";
import { loadConfig, saveConfig } from "../config";
import type { SessionFilterMode } from "../config";
import {
  clampSidebarWidth,
} from "./sidebar-width-sync";
import {
  type ServerState,
  type SessionData,
  type ClientCommand,
  type FocusUpdate,
  PROTOCOL_VERSION,
  SERVER_PORT,
  SERVER_HOST,
  LOCAL_CLIENT_HOST,
  PID_FILE,
  SERVER_IDLE_TIMEOUT_MS,
  STUCK_RUNNING_TIMEOUT_MS,
} from "../shared";

const VALID_AGENT_STATUSES = new Set<AgentStatus>([
  "idle", "running", "tool-running", "done", "error", "waiting", "interrupted", "stale",
]);

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
const SPAWN_STAGGER_MS = 500;
const RESIZE_STAGGER_MS = 60;
const CLIENT_RESIZE_SETTLE_MS = 220;
const WIDTH_REPORT_SUPPRESSION_MS = 500;
const FOCUS_WIDTH_REPORT_GUARD_MS = 500;
const CLIENT_RESIZE_REPORT_GUARD_MS = 700;
const USER_DRAG_SETTLE_MS = 600;

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
  let currentFilter: SessionFilterMode | undefined = config.sessionFilter;
  const initialSidebarWidth = clampSidebarWidth(config.sidebarWidth ?? 26);
  let sidebarPosition: "left" | "right" = config.sidebarPosition ?? "left";
  const sidebarCoordinator = createSidebarCoordinator({ width: initialSidebarWidth });

  // The sidebar launcher lives with the TUI app, not the tmux integration layer.
  const scriptsDir = (() => {
    const envDir = process.env.OPENSESSIONS_DIR;
    if (envDir) return join(envDir, "apps", "tui", "scripts");
    // Fallback: relative to this file
    return join(import.meta.dir, "..", "..", "..", "..", "apps", "tui", "scripts");
  })();

  log("server", "config loaded", {
    sidebarWidth: initialSidebarWidth, sidebarPosition, scriptsDir,
    theme: currentTheme, configKeys: Object.keys(config),
  });

  // Bootstrap active sessions
  const currentSession = mux.getCurrentSession();
  if (currentSession) {
    tracker.setActiveSessions([currentSession]);
  }

  // --- Agent watcher context ---

  // Cache for dir→sessions resolution (rebuilt per scan cycle)
  let dirSessionCache: Map<string, string[]> | null = null;
  let dirSessionCacheTs = 0;
  const DIR_CACHE_TTL = 5000;

  function getDirSessionMap(): Map<string, string[]> {
    const now = Date.now();
    if (dirSessionCache && now - dirSessionCacheTs < DIR_CACHE_TTL) return dirSessionCache;
    const sessions: Array<{ dir: string; name: string }> = [];
    for (const p of allProviders) {
      for (const s of p.listSessions()) {
        if (s.dir) sessions.push({ dir: s.dir, name: s.name });
      }
    }
    const map = buildDirSessionMap(sessions);
    dirSessionCache = map;
    dirSessionCacheTs = now;
    return map;
  }

  const piLiveResolver = new PiLiveResolver({
    listPanes: () => {
      const raw = shell([
        "tmux", "list-panes", "-a",
        "-F", "#{session_name}|#{pane_id}|#{pane_pid}",
      ]);
      if (!raw) return [];
      return raw.split("\n")
        .filter(Boolean)
        .map((line) => {
          const idx1 = line.indexOf("|");
          const idx2 = line.indexOf("|", idx1 + 1);
          return {
            session: line.slice(0, idx1),
            paneId: line.slice(idx1 + 1, idx2),
            pid: parseInt(line.slice(idx2 + 1), 10),
          };
        })
        .filter((pane) => !Number.isNaN(pane.pid));
    },
    listSidebarPaneIds: function* () {
      for (const { panes } of listSidebarPanesByProvider()) {
        for (const pane of panes) yield pane.paneId;
      }
    },
    buildProcessTree,
  });

  function resolveThreadOwner(agent: string, threadId?: string, threadName?: string): AgentThreadOwner | null {
    if (agent === "pi" && threadId) {
      const owner = piLiveResolver.resolveThreadOwner(threadId);
      return owner ? { session: owner.session, paneId: owner.paneId } : null;
    }

    const panes = listNonSidebarTmuxPanes();
    if (panes.length === 0) return null;

    let paneId: string | undefined;
    if (agent === "claude-code" && threadId) {
      paneId = resolveClaudeCodePane(panes, threadId);
    } else if (agent === "codex" && threadId) {
      paneId = resolveCodexPane(panes, threadId);
    } else if (agent === "opencode" && threadId) {
      paneId = resolveOpenCodePane(panes, threadId);
    } else if (agent === "amp" && threadName) {
      paneId = resolveAmpPane(panes, threadName);
    }

    if (!paneId) return null;
    const pane = panes.find((entry) => entry.id === paneId);
    return pane ? { session: pane.session, paneId } : null;
  }

  function canonicalizeOwnedEvent(event: AgentEvent): AgentEvent {
    const nextEvent = canonicalizeAgentEvent(event, resolveThreadOwner);
    if (event.session !== nextEvent.session && nextEvent.threadId) {
      log("agent-emit", "thread owner override", {
        agent: nextEvent.agent,
        from: event.session,
        to: nextEvent.session,
        threadId: nextEvent.threadId.slice(0, 8),
      });
    }
    if (nextEvent.threadId) {
      tracker.dedupeInstanceToSession(nextEvent.session, nextEvent.agent, nextEvent.threadId);
    }
    return nextEvent;
  }

  const watcherCtx: AgentWatcherContext = {
    resolveSession(projectDir: string): string | null {
      return resolveSessionForProjectDir(projectDir, getDirSessionMap());
    },
    resolveThreadOwner,
    emit(event: AgentEvent) {
      const nextEvent = canonicalizeOwnedEvent(event);
      log("agent-emit", nextEvent.agent, { session: nextEvent.session, status: nextEvent.status, threadId: nextEvent.threadId?.slice(0, 8) });
      tracker.applyEvent(nextEvent, { seed: !watchersSeeded });
      // Broadcast immediately — broadcastState() is already microtask-coalesced
      // so bursts within a single tick collapse to one send. The previous 200ms
      // debounce was adding perceptible latency for push-driven events (plugin
      // POSTs, DTW WebSocket, Claude Code file watcher).
      broadcastState();
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
  let initializingTimer: ReturnType<typeof setTimeout> | null = null;
  let transientResizeTimer: ReturnType<typeof setTimeout> | null = null;
  let clientResizeSyncTimer: ReturnType<typeof setTimeout> | null = null;
  let programmaticAdjustmentTimer: ReturnType<typeof setTimeout> | null = null;
  let resizeStaggerTimers: ReturnType<typeof setTimeout>[] = [];
  let idleTimer: ReturnType<typeof setTimeout> | null = null;
  const clientTtys = new WeakMap<object, string>();
  const clientSessionNames = new WeakMap<object, string>();
  const clientWindowIds = new WeakMap<object, string>();
  const connectedClients = new Set<any>();
  const sessionProviders = new Map<string, MuxProvider>();
  // Map session name → client TTY (from hook context, for multi-client setups)
  const clientTtyBySession = new Map<string, string>();

  function getSidebarState() {
    return readSidebarCoordinatorState(sidebarCoordinator.getSnapshot());
  }

  function getSidebarWidth(): number {
    return getSidebarState().width;
  }

  function isSidebarVisible(): boolean {
    return getSidebarState().visible;
  }

  function suppressWidthReports(ms = WIDTH_REPORT_SUPPRESSION_MS): void {
    sidebarCoordinator.send({ type: "SUPPRESS_WIDTH_REPORTS", until: Date.now() + ms });
  }

  function noteClientResizeReportGuard(ms = CLIENT_RESIZE_REPORT_GUARD_MS): void {
    sidebarCoordinator.send({ type: "NOTE_CLIENT_RESIZE_GUARD", until: Date.now() + ms });
  }

  function noteFocusContextChange(ms = FOCUS_WIDTH_REPORT_GUARD_MS): void {
    sidebarCoordinator.send({ type: "FOCUS_CONTEXT_CHANGED" });
    suppressWidthReports(ms);
  }

  function beginSidebarWarmup(): void {
    sidebarCoordinator.send({ type: "BEGIN_WARMUP" });
  }

  function finishSidebarWarmup(): void {
    sidebarCoordinator.send({ type: "WARMUP_DONE" });
  }

  function markSidebarReady(): void {
    sidebarCoordinator.send({ type: "MARK_READY" });
  }

  function hideSidebarLifecycle(): void {
    sidebarCoordinator.send({ type: "HIDE" });
  }

  function clearTransientResizeTimer(): void {
    if (!transientResizeTimer) return;
    clearTimeout(transientResizeTimer);
    transientResizeTimer = null;
  }

  function clearClientResizeSyncTimer(): void {
    if (!clientResizeSyncTimer) return;
    clearTimeout(clientResizeSyncTimer);
    clientResizeSyncTimer = null;
  }

  function clearProgrammaticAdjustmentTimer(): void {
    if (!programmaticAdjustmentTimer) return;
    clearTimeout(programmaticAdjustmentTimer);
    programmaticAdjustmentTimer = null;
  }

  function clearResizeStaggerTimers(): void {
    for (const t of resizeStaggerTimers) clearTimeout(t);
    resizeStaggerTimers = [];
  }

  function isClientResizeSyncActive(): boolean {
    return isSidebarClientResizeSyncActive(getSidebarState());
  }

  function isClientResizeReportGuardActive(now = Date.now()): boolean {
    return isSidebarClientResizeReportGuardActive(getSidebarState(), now);
  }

  function startTransientSidebarResize(ms = USER_DRAG_SETTLE_MS): boolean {
    const extendingTransientResize = isUserDragActive(getSidebarState());
    clearTransientResizeTimer();

    transientResizeTimer = setTimeout(() => {
      transientResizeTimer = null;
      if (!isSidebarVisible() || isClientResizeSyncActive()) return;
      if (!isUserDragActive(getSidebarState())) return;
      sidebarCoordinator.send({ type: "FINISH_USER_DRAG" });
      broadcastState();
    }, ms);

    return !extendingTransientResize;
  }

  function startProgrammaticAdjustment(ms = 250): void {
    const sidebarState = getSidebarState();
    if (!sidebarState.visible) return;
    if (isUserDragActive(sidebarState) || isClientResizeSyncActive()) return;

    const alreadyAdjusting = sidebarState.resizeAuthority === "programmatic-adjust";
    sidebarCoordinator.send({ type: "BEGIN_PROGRAMMATIC_ADJUSTMENT" });
    if (!alreadyAdjusting) {
      broadcastState();
    }

    clearProgrammaticAdjustmentTimer();
    programmaticAdjustmentTimer = setTimeout(() => {
      programmaticAdjustmentTimer = null;
      if (getSidebarState().resizeAuthority !== "programmatic-adjust") return;
      sidebarCoordinator.send({ type: "FINISH_PROGRAMMATIC_ADJUSTMENT" });
      broadcastState();
    }, ms);
  }

  function beginClientResizeSync(): void {
    if (!isSidebarVisible()) return;
    const sidebarState = getSidebarState();
    if (isClientResizeSyncActive(sidebarState)) return;
    sidebarCoordinator.send({
      type: "BEGIN_CLIENT_RESIZE_SYNC",
      suppressUntil: Date.now() + WIDTH_REPORT_SUPPRESSION_MS,
      guardUntil: Date.now() + CLIENT_RESIZE_REPORT_GUARD_MS,
    });
    // Client-resize sync can overlap sidebar warmup while panes are still
    // spawning. Do not cancel the warmup completion timer here or the sidebar
    // can get stranded in a permanent "warming up" state.
    broadcastState();
  }

  function finishClientResizeSync(): void {
    clearResizeStaggerTimers();
    if (!isClientResizeSyncActive(getSidebarState())) return;
    sidebarCoordinator.send({ type: "FINISH_CLIENT_RESIZE_SYNC" });
    broadcastState();
  }

  function sendYourSession(ws: any, sessionName: string, clientTty?: string | null): void {
    clientSessionNames.set(ws, sessionName);
    ws.send(JSON.stringify({
      type: "your-session",
      name: sessionName,
      clientTty: clientTty ?? clientTtyBySession.get(sessionName) ?? null,
    }));
  }

  function syncClientSessionsForTty(clientTty: string | undefined, sessionName: string, windowId?: string): void {
    if (!clientTty) return;
    clientTtyBySession.set(sessionName, clientTty);
    for (const ws of connectedClients) {
      const existingTty = clientTtys.get(ws);
      const windowMatches = !!windowId && clientWindowIds.get(ws) === windowId;
      const ttyMatches = existingTty === clientTty;
      if (!ttyMatches && !windowMatches) continue;
      if (windowMatches && existingTty !== clientTty) {
        clientTtys.set(ws, clientTty);
      }
      sendYourSession(ws, sessionName, clientTty);
    }
  }

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
    const portlessState = loadPortlessState();

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
        localLinks: buildLocalLinks(getSessionPorts(name), portlessState),
        windows,
        uptime,
        agentState: tracker.getState(name),
        agents: tracker.getAgents(name),
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

    const sidebarState = getSidebarState();

    return {
      type: "state",
      sessions,
      focusedSession,
      currentSession,
      theme: currentTheme,
      sessionFilter: currentFilter,
      sidebarWidth: getSidebarWidth(),
      initializing: sidebarState.initializing,
      initLabel: sidebarState.initLabel,
      ts: Date.now(),
    };
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

  function getForegroundClientTtyForWindow(windowId?: string): string | null {
    if (!windowId) return null;
    const raw = shell(["tmux", "list-clients", "-F", "#{client_tty}|#{window_id}"]);
    if (!raw) return null;
    for (const line of raw.split("\n")) {
      if (!line) continue;
      const idx = line.indexOf("|");
      if (idx < 0) continue;
      const tty = line.slice(0, idx);
      const clientWindowId = line.slice(idx + 1);
      if (clientWindowId !== windowId || tty.length === 0) continue;
      return tty;
    }
    return null;
  }

  function normalizeTmuxWindowSize(windowId?: string): void {
    if (!windowId) return;
    shell(["tmux", "set-window-option", "-t", windowId, "window-size", "latest"]);
  }

  function isForegroundClient(ws: any): boolean {
    const senderSession = clientSessionNames.get(ws) ?? null;
    if (senderSession === "_os_stash") return false;

    const senderWindowId = clientWindowIds.get(ws);
    if (senderWindowId) {
      const foregroundTty = getForegroundClientTtyForWindow(senderWindowId);
      if (foregroundTty) return true;
      return false;
    }

    const current = getCachedCurrentSession();
    const currentTty = current ? clientTtyBySession.get(current) ?? null : null;
    const senderTty = clientTtys.get(ws) ?? null;
    if (currentTty && senderTty) return currentTty === senderTty;

    if (senderSession && current) return senderSession === current;
    return true;
  }

  function setFocus(name: string, sender?: any) {
    if (!lastState || !lastState.sessions.some((s) => s.name === name)) return;

    if (sender && !isForegroundClient(sender)) {
      log("focus-session", "ignored from background sidebar", {
        requested: name,
        senderSession: clientSessionNames.get(sender) ?? null,
        senderTty: clientTtys.get(sender) ?? null,
        current: getCachedCurrentSession(),
      });
      return;
    }

    focusedSession = name;
    broadcastFocusOnly(sender);
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

  function switchToVisibleIndex(
    index: number,
    clientTty?: string,
    sourceCtx?: { session?: string | null; windowId?: string | null },
  ): void {
    if (!lastState) {
      broadcastState();
    }

    if (!lastState) return;

    const sessions = lastState.sessions.filter((s) => {
      if (!currentFilter || currentFilter === "all") return true;
      if (currentFilter === "active") return s.agents.length > 0 || s.agentState !== null;
      // "running" — only sessions where an agent is currently running
      const st = s.agentState?.status;
      return st === "running" || st === "tool-running" || st === "waiting";
    });

    const idx = index - 1;
    if (idx < 0 || idx >= sessions.length) return;

    const name = sessions[idx]!.name;
    const p = sessionProviders.get(name) ?? mux;
    adoptSidebarWidthFromWindow(sourceCtx?.session, sourceCtx?.windowId, "switch-index");
    p.switchSession(name, clientTty);

    if (isSidebarVisible() && isFullSidebarCapable(p) && p.name === "zellij") {
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

  function adoptSidebarWidthFromWindow(
    sessionName?: string | null,
    windowId?: string | null,
    reason = "source-window",
  ): void {
    if (!sessionName || !windowId || !isSidebarVisible()) return;

    invalidateSidebarPaneCache();

    let sidebarPaneWidth: number | null = null;
    for (const { panes } of listSidebarPanesByProvider()) {
      const pane = panes.find((candidate) => candidate.windowId === windowId);
      if (!pane) continue;
      sidebarPaneWidth = pane.width;
      break;
    }

    if (sidebarPaneWidth === null) return;

    const current = getCachedCurrentSession();
    const provider = sessionProviders.get(sessionName) ?? mux;
    const currentWindowId = current === sessionName ? provider.getCurrentWindowId() : null;
    const decision = applySidebarWidthReport(sidebarCoordinator, {
      width: sidebarPaneWidth,
      session: sessionName,
      windowId,
      isActiveSession: current === sessionName,
      isForegroundClient: true,
      isCurrentWindow: currentWindowId === windowId,
    });

    if (!decision.accepted) {
      log("adopt-sidebar-width", `REJECTED — ${decision.reason}`, {
        reason,
        sessionName,
        windowId,
        reported: sidebarPaneWidth,
        sidebarWidth: getSidebarWidth(),
        current,
      });
      return;
    }

    const nextSidebarWidth = getSidebarWidth();
    log("adopt-sidebar-width", "ACCEPTED", {
      reason,
      sessionName,
      windowId,
      oldWidth: decision.previousWidth,
      sidebarWidth: nextSidebarWidth,
    });
    saveConfig({ sidebarWidth: nextSidebarWidth });
    if (!startTransientSidebarResize()) {
      broadcastState();
    }
    enforceSidebarWidth(windowId);
  }

  function reconcileSidebarPresence() {
    invalidateSidebarPaneCache();
    const panesByProvider = listSidebarPanesByProvider();
    return {
      panesByProvider,
      visible: panesByProvider.some(({ panes }) => panes.length > 0),
    };
  }

  const pendingSidebarSpawns = new Set<string>();

  function toggleSidebar(ctx?: { session: string; windowId: string }): void {
    const providers = getProvidersWithSidebar();
    if (providers.length === 0) {
      log("toggle", "SKIP — no providers with sidebar methods");
      return;
    }

    const { panesByProvider, visible: sidebarPresent } = reconcileSidebarPresence();
    const hasPaneInContextWindow = ctx
      ? panesByProvider.some(({ panes }) => panes.some((pane) => pane.windowId === ctx.windowId))
      : false;

    // If the server rebooted into a degraded state where only some sidebar
    // panes survived, treat toggle from a pane-less window as a recovery
    // request and restore missing panes instead of hiding the lone survivor.
    const recoverVisibleState = sidebarPresent && ctx && !hasPaneInContextWindow;

    if (sidebarPresent && !recoverVisibleState) {
      for (const p of providers) {
        const panes = p.listSidebarPanes();
        log("toggle", "OFF — hiding panes", { provider: p.name, count: panes.length });
        for (const pane of panes) {
          p.hideSidebar(pane.paneId);
        }
      }
      clearTransientResizeTimer();
      clearClientResizeSyncTimer();
      clearProgrammaticAdjustmentTimer();
      clearResizeStaggerTimers();
      hideSidebarLifecycle();
      if (initializingTimer) { clearTimeout(initializingTimer); initializingTimer = null; }
    } else {
      if (initializingTimer) clearTimeout(initializingTimer);
      clearClientResizeSyncTimer();
      clearProgrammaticAdjustmentTimer();
      clearResizeStaggerTimers();
      suppressWidthReports();
      beginSidebarWarmup();
      invalidateSidebarPaneCache();

      // Prioritized spawn order:
      // 1. Current active window (instant)
      // 2. Other windows in the current session
      // 3. Windows in other sessions (staggered)
      const curSession = ctx?.session ?? getCurrentSession();

      // Track max delay to know when all spawns are done
      let maxDelay = 0;

      for (const p of providers) {
        const allWindows = p.listActiveWindows();
        log("toggle", recoverVisibleState ? "RECOVER — ensuring all windows" : "ON — spawning in all windows", {
          provider: p.name,
          total: allWindows.length,
          currentSession: curSession,
        });

        // Tier 1: current active window (instant)
        const curWindowId = ctx?.windowId ?? p.getCurrentWindowId();
        if (curSession && curWindowId) {
          const activeWindow = allWindows.find((w) => w.sessionName === curSession && w.id === curWindowId);
          if (activeWindow) {
            log("toggle", "tier1: active window", { session: curSession, windowId: curWindowId });
            ensureSidebarInWindow(p, { session: activeWindow.sessionName, windowId: activeWindow.id });
          }
        }

        // Tier 2: other windows in current session (slight delay)
        const tier2 = allWindows.filter((w) => w.sessionName === curSession && w.id !== curWindowId);
        // Tier 3: windows in other sessions
        const tier3 = allWindows.filter((w) => w.sessionName !== curSession);

        log("toggle", "spawn plan", { tier2: tier2.length, tier3: tier3.length });

        // Stagger background spawns — each ensureSidebarInWindow blocks ~100ms
        // with sync tmux calls, so space them out to keep the event loop responsive.
        let delay = SPAWN_STAGGER_MS;
        for (const w of tier2) {
          const win = w;
          const prov = p;
          setTimeout(() => {
            if (isSidebarVisible()) ensureSidebarInWindow(prov, { session: win.sessionName, windowId: win.id });
          }, delay);
          delay += SPAWN_STAGGER_MS;
        }
        for (const w of tier3) {
          const win = w;
          const prov = p;
          setTimeout(() => {
            if (isSidebarVisible()) ensureSidebarInWindow(prov, { session: win.sessionName, windowId: win.id });
          }, delay);
          delay += SPAWN_STAGGER_MS;
        }
        if (delay > maxDelay) maxDelay = delay;
      }

      // Set initializing state during stagger
      if (maxDelay > 0) {
        initializingTimer = setTimeout(() => {
          initializingTimer = null;
          finishSidebarWarmup();
          log("toggle", "initializing complete");
          broadcastState();
        }, maxDelay + 500); // extra 500ms buffer for last spawn to finish
      } else {
        markSidebarReady();
      }

      scheduleSidebarWidthEnforcement();
      server.publish("sidebar", JSON.stringify({ type: "re-identify" }));
    }
    log("toggle", "done", { sidebarVisible: isSidebarVisible() });
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
    if (!p || !isSidebarVisible()) {
      log("ensure", "SKIP", { hasProvider: !!p, sidebarVisible: isSidebarVisible() });
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
    // Invalidate before check — staggered spawns change pane state between calls
    invalidateSidebarPaneCache();
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
      const sidebarWidth = getSidebarWidth();
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
    }
    // Always enforce width — session switches can change window width,
    // causing tmux to proportionally redistribute pane sizes.
    // Call directly (not scheduled) since we're already behind debouncedEnsureSidebar.
    suppressWidthReports();
    enforceSidebarWidth();
  }

  // Debounced ensure-sidebar — collapses rapid hook-fired calls during fast
  // session switching into a single batch after switching settles.
  let ensureSidebarTimer: ReturnType<typeof setTimeout> | null = null;
  const ensureSidebarPendingCtxs = new Map<string, { session: string; windowId: string }>();
  let ensureSidebarPendingCurrentWindow = false;

  function debouncedEnsureSidebar(ctx?: { session: string; windowId: string }): void {
   if (ctx) {
     ensureSidebarPendingCtxs.set(`${ctx.session}:${ctx.windowId}`, ctx);
   } else {
     ensureSidebarPendingCurrentWindow = true;
   }
   if (ensureSidebarTimer) clearTimeout(ensureSidebarTimer);
   ensureSidebarTimer = setTimeout(() => {
     ensureSidebarTimer = null;
     const pendingCtxs = [...ensureSidebarPendingCtxs.values()];
     ensureSidebarPendingCtxs.clear();
     const shouldEnsureCurrentWindow = ensureSidebarPendingCurrentWindow;
     ensureSidebarPendingCurrentWindow = false;

     if (pendingCtxs.length === 0) {
       if (shouldEnsureCurrentWindow) ensureSidebarInWindow();
       return;
     }

     for (const pendingCtx of pendingCtxs) {
       ensureSidebarInWindow(undefined, pendingCtx);
     }

     if (shouldEnsureCurrentWindow) ensureSidebarInWindow();
   }, 150);
  }

  function queueEnsureSidebarAcrossAllWindows(): void {
    if (!isSidebarVisible()) return;
    for (const provider of getProvidersWithSidebar()) {
      for (const window of provider.listActiveWindows()) {
        debouncedEnsureSidebar({ session: window.sessionName, windowId: window.id });
      }
    }
  }

  // Debounced width enforcement — collapses resize storms (monitor switch,
  // terminal resize) into a single tmux resize pass.
  let sidebarEnforceTimer: ReturnType<typeof setTimeout> | null = null;

  function scheduleSidebarWidthEnforcement(): void {
   if (!isSidebarVisible()) return;
   const sidebarWidth = getSidebarWidth();
   log("scheduleEnforce", "scheduling debounced enforcement", { sidebarWidth });
   suppressWidthReports();
   if (sidebarEnforceTimer) clearTimeout(sidebarEnforceTimer);
   sidebarEnforceTimer = setTimeout(() => {
     sidebarEnforceTimer = null;
     const sidebarWidth = getSidebarWidth();
     log("scheduleEnforce", "FIRING debounced enforcement", { sidebarVisible: isSidebarVisible(), sidebarWidth });
     if (isSidebarVisible()) {
       startProgrammaticAdjustment();
       enforceSidebarWidth();
     }
   }, 150);
  }

  function enforceSidebarWidthInWindow(
    provider: ReturnType<typeof getProvidersWithSidebar>[number],
    windowId: string,
  ): void {
    const sidebarWidth = getSidebarWidth();
    invalidateSidebarPaneCache();
    const providerEntry = listSidebarPanesByProvider().find((entry) => entry.provider === provider);
    if (!providerEntry) return;
    for (const pane of providerEntry.panes) {
      if (pane.windowId !== windowId) continue;
      if (pane.width === sidebarWidth) continue;
      log("enforce", `${pane.paneId} ${pane.width}→${sidebarWidth} (window ${windowId})`);
      provider.resizeSidebarPane(pane.paneId, sidebarWidth);
    }
  }

  function resizeSidebarPanes(
    provider: ReturnType<typeof getProvidersWithSidebar>[number],
    paneIds: readonly string[],
  ): void {
    const sidebarWidth = getSidebarWidth();
    for (const paneId of paneIds) {
      provider.resizeSidebarPane(paneId, sidebarWidth);
    }
  }

  function scheduleClientResizeSync(): void {
    if (!isSidebarVisible()) return;

    clearTransientResizeTimer();
    clearResizeStaggerTimers();
    clearClientResizeSyncTimer();
    beginClientResizeSync();

    // Keep the currently visible window at the persisted sidebar width while
    // the terminal itself is being resized. Background windows can wait for
    // the debounced sync pass, but the foreground window should not visually
    // drift until the user explicitly drags the sidebar.
    suppressWidthReports();
    for (const provider of getProvidersWithSidebar()) {
      const currentWindowId = provider.getCurrentWindowId();
      if (!currentWindowId) continue;
      enforceSidebarWidthInWindow(provider, currentWindowId);
    }

    clientResizeSyncTimer = setTimeout(() => {
      clientResizeSyncTimer = null;

      if (!isSidebarVisible()) {
        finishClientResizeSync();
        return;
      }

      let delay = 0;
      for (const provider of getProvidersWithSidebar()) {
        if (!("resizeWindow" in provider) || !("getClientSize" in provider)) continue;
        const resizableProvider = provider as typeof provider & {
          resizeWindow(windowId: string, width: number, height: number): void;
          getClientSize(): { width: number; height: number } | null;
        };
        const size = resizableProvider.getClientSize();
        if (!size) continue;

        const providerEntry = listSidebarPanesByProvider().find((entry) => entry.provider === provider);
        const sidebarPaneIdsByWindow = new Map<string, string[]>();
        for (const pane of providerEntry?.panes ?? []) {
          const paneIds = sidebarPaneIdsByWindow.get(pane.windowId) ?? [];
          paneIds.push(pane.paneId);
          sidebarPaneIdsByWindow.set(pane.windowId, paneIds);
        }

        const currentWindowId = provider.getCurrentWindowId();
        const backgroundWindows = provider.listActiveWindows().filter((window) => window.id !== currentWindowId);

        for (const window of backgroundWindows) {
          const targetWindow = window;
          const { width, height } = size;
          resizeStaggerTimers.push(setTimeout(() => {
            if (!isSidebarVisible()) return;
            suppressWidthReports();
            resizableProvider.resizeWindow(targetWindow.id, width, height);
            resizeSidebarPanes(provider, sidebarPaneIdsByWindow.get(targetWindow.id) ?? []);
          }, delay));
          delay += RESIZE_STAGGER_MS;
        }

        if (currentWindowId) {
          const currentPaneIds = sidebarPaneIdsByWindow.get(currentWindowId) ?? [];
          resizeStaggerTimers.push(setTimeout(() => {
            if (!isSidebarVisible()) {
              finishClientResizeSync();
              return;
            }
            suppressWidthReports();
            resizeSidebarPanes(provider, currentPaneIds);
          }, delay));
          delay += RESIZE_STAGGER_MS;
        }
      }

      resizeStaggerTimers.push(setTimeout(() => {
        if (!isSidebarVisible()) {
          finishClientResizeSync();
          return;
        }
        finishClientResizeSync();
      }, delay));
    }, CLIENT_RESIZE_SETTLE_MS);
  }

  let quitting = false;
  function quitAll(): void {
    // Re-entrancy guard: the TUI sends both a WS "quit" command and an
    // HTTP POST /quit as a belt-and-suspenders fallback; either can land
    // first. Without this, the second call restarts the teardown mid-way
    // through the first and we sometimes end up leaking the process.
    if (quitting) {
      log("quit", "already quitting — forcing exit");
      process.exit(0);
    }
    quitting = true;
    log("quit", "killing all sidebar panes");
    // Wrap every teardown step so a single throw (tmux command failure,
    // watcher.stop(), xstate send on a stopped actor, etc.) doesn't skip
    // process.exit and leave a zombie server behind — which is exactly
    // what the "q doesn't kill server" reports have been.
    try {
      for (const p of getProvidersWithSidebar()) {
        try {
          const panes = p.listSidebarPanes();
          log("quit", "found panes to kill", { provider: p.name, count: panes.length });
          for (const pane of panes) {
            try { p.killSidebarPane(pane.paneId); } catch (err) {
              log("quit", "killSidebarPane failed", { paneId: pane.paneId, error: String(err) });
            }
          }
        } catch (err) {
          log("quit", "listSidebarPanes failed", { provider: p.name, error: String(err) });
        }
      }
      for (const p of getProvidersWithSidebar()) {
        try { p.cleanupSidebar(); } catch (err) {
          log("quit", "cleanupSidebar failed", { provider: p.name, error: String(err) });
        }
      }
      try { server.publish("sidebar", JSON.stringify({ type: "quit" })); } catch {}
      try { hideSidebarLifecycle(); } catch (err) {
        log("quit", "hideSidebarLifecycle failed", { error: String(err) });
      }
      try { cleanup(); } catch (err) {
        log("quit", "cleanup failed", { error: String(err) });
      }
    } finally {
      process.exit(0);
    }
  }

  function startIdleTimerIfNeeded(reason: string): void {
    if (clientCount !== 0 || idleTimer) return;
    log("ws", "no clients remaining, starting idle timer", { timeoutMs: SERVER_IDLE_TIMEOUT_MS, reason });
    idleTimer = setTimeout(() => {
      log("ws", "idle timeout reached, shutting down", { reason });
      quitAll();
    }, SERVER_IDLE_TIMEOUT_MS);
  }

  // --- Sidebar width enforcement ---

  let enforcing = false;

  function enforceSidebarWidth(skipWindowId?: string) {
    if (enforcing) {
      log("enforce", "SKIPPED — re-entrancy guard");
      return;
    }
    enforcing = true;
    const sidebarWidth = getSidebarWidth();
    log("enforce", "START", {
      sidebarWidth,
      skipWindowId,
      widthReportsSuppressed: areWidthReportsSuppressed(getSidebarState()),
    });
    try {
      invalidateSidebarPaneCache();
      for (const { provider, panes } of listSidebarPanesByProvider()) {
        for (const pane of panes) {
          if (pane.width === sidebarWidth) continue;
          if (skipWindowId && pane.windowId === skipWindowId) continue;
          log("enforce", `${pane.paneId} ${pane.width}→${sidebarWidth}`);
          provider.resizeSidebarPane(pane.paneId, sidebarWidth);
        }
      }
    } finally {
      enforcing = false;
    }
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
      if (childCmd && patterns.some((pat) => commMatches(childCmd.toLowerCase(), pat))) return true;
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
  type SessionPaneEntry = PaneEntry & { session: string };

  function listNonSidebarTmuxPanes(): SessionPaneEntry[] {
    const hasTmux = allProviders.some((provider) => provider.name === "tmux");
    if (!hasTmux) return [];

    const raw = shell([
      "tmux", "list-panes", "-a",
      "-F", "#{session_name}|#{pane_id}|#{pane_pid}|#{pane_current_command}|#{pane_title}",
    ]);
    if (!raw) return [];

    const panes = raw.split("\n")
      .filter(Boolean)
      .map((line) => {
        const idx1 = line.indexOf("|");
        const idx2 = line.indexOf("|", idx1 + 1);
        const idx3 = line.indexOf("|", idx2 + 1);
        const idx4 = line.indexOf("|", idx3 + 1);
        return {
          session: line.slice(0, idx1),
          id: line.slice(idx1 + 1, idx2),
          pid: line.slice(idx2 + 1, idx3),
          cmd: line.slice(idx3 + 1, idx4),
          title: line.slice(idx4 + 1),
        };
      });

    const sidebarPaneIds = new Set<string>();
    for (const { panes: sidebarPanes } of listSidebarPanesByProvider()) {
      for (const sidebarPane of sidebarPanes) sidebarPaneIds.add(sidebarPane.paneId);
    }

    return panes.filter((pane) => !sidebarPaneIds.has(pane.id));
  }

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

  function resolveAmpPane(panes: PaneEntry[], threadName: string): string | undefined {
    const matches = panes
      .filter((pane) => pane.title.toLowerCase().startsWith("amp - ") && pane.title.includes(threadName));
    return matches.length === 1 ? matches[0]!.id : undefined;
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

  function getTrackedAlivePaneId(sessionName: string, agentName: string, threadId?: string): string | undefined {
    if (!threadId) return undefined;
    return tracker.getAgents(sessionName)
      .find((agent) => agent.agent === agentName && agent.threadId === threadId && agent.liveness === "alive" && !!agent.paneId)
      ?.paneId;
  }

  function resolvePiPane(threadId: string): string | undefined {
    return piLiveResolver.resolveThreadPane(threadId, { fresh: true });
  }

  /** Resolve a tmux pane ID for an agent using all available resolution strategies. */
  function resolveAgentPaneId(sessionName: string, agentName: string, threadId?: string, threadName?: string): string | undefined {
    const trackedPaneId = getTrackedAlivePaneId(sessionName, agentName, threadId);
    if (trackedPaneId) return trackedPaneId;

    const p = sessionProviders.get(sessionName) ?? mux;
    if (p.name !== "tmux") return undefined;

    const patterns = AGENT_TITLE_PATTERNS[agentName];
    if (!patterns && agentName !== "pi") return undefined;

    const raw = shell([
      "tmux", "list-panes", "-s", "-t", sessionName,
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
      targetPaneId = resolveAmpPane(nonSidebar, threadName);
    }
    if (!targetPaneId && agentName === "codex" && threadId) {
      targetPaneId = resolveCodexPane(nonSidebar, threadId);
    }
    if (!targetPaneId && agentName === "opencode" && threadId) {
      targetPaneId = resolveOpenCodePane(nonSidebar, threadId);
    }
    if (!targetPaneId && agentName === "pi" && threadId) {
      targetPaneId = resolvePiPane(threadId);
      if (targetPaneId && sidebarPaneIds.has(targetPaneId)) {
        targetPaneId = undefined;
      }
    }
    if (!targetPaneId && patterns) {
      targetPaneId = nonSidebar
        .find((p) => patterns.some((pat) => p.title.toLowerCase().includes(pat)))
        ?.id;
    }
    if (!targetPaneId && patterns) {
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

    // Switch to the window containing the target pane first,
    // otherwise select-pane alone won't work across windows
    const windowId = shell(["tmux", "display-message", "-t", targetPaneId, "-p", "#{window_id}"]);
    if (windowId) {
      shell(["tmux", "select-window", "-t", windowId.trim()]);
    }
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

  // Pane presence is now folded into the tracker via applyPanePresence().

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

  /** Match a comm string against a pattern as a whole word.
   *  "claude" matches "claude", "/usr/bin/claude", "claude-code"
   *  but NOT "tail-claude" or "my-claude-fork". The pattern must appear
   *  at the start of the comm or after a path separator (/). */
  function commMatches(comm: string, pat: string): boolean {
    const idx = comm.indexOf(pat);
    if (idx < 0) return false;
    // Pattern must be at start, or preceded by a path separator
    if (idx > 0 && comm[idx - 1] !== "/") return false;
    return true;
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
      if (comm && patterns.some((pat) => commMatches(comm, pat))) return true;
      if (matchProcessTreeFast(childPid, patterns, tree, depth + 1)) return true;
    }
    return false;
  }

  /** Scan all panes across all tmux sessions and identify running agents.
   *  Returns pane presence, optionally enriched with threadId when an agent-specific
   *  runtime registry can resolve a live process to a watcher thread. */
  function scanAllTmuxPaneAgents(): Map<string, PanePresenceInput[]> {
    const result = new Map<string, PanePresenceInput[]>();

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

    for (const pane of nonSidebar) {
      for (const [agentName, patterns] of Object.entries(AGENT_TITLE_PATTERNS)) {
        // Only use process tree matching — title matching produces false positives
        // (e.g. an Amp thread named "Detect Claude session names" matches "claude")
        if (!matchProcessTreeFast(pane.pid, patterns, tree)) continue;

        let sessionAgents = result.get(pane.session);
        if (!sessionAgents) {
          sessionAgents = [];
          result.set(pane.session, sessionAgents);
        }
        sessionAgents.push({ agent: agentName, paneId: pane.id });
      }

    }

    for (const [session, paneAgents] of piLiveResolver.scanPresenceBySession()) {
      let sessionAgents = result.get(session);
      if (!sessionAgents) {
        sessionAgents = [];
        result.set(session, sessionAgents);
      }
      sessionAgents.push(...paneAgents);
    }

    return result;
  }

  /** Refresh pane agent presence by scanning tmux panes and folding results into the tracker. */
  function refreshPaneAgents(): void {
    const hasTmux = allProviders.some((p) => p.name === "tmux");
    if (!hasTmux) {
      // No tmux provider — mark all previously-alive agents as exited
      // by applying empty presence for each tracked session
      // (applyPanePresence handles the exited transition internally)
      return;
    }

    const nextBySession = scanAllTmuxPaneAgents();
    let changed = false;

    // Apply presence for sessions that have pane agents
    for (const [session, paneAgents] of nextBySession) {
      for (const paneAgent of paneAgents) {
        if (paneAgent.threadId) {
          if (tracker.dedupeInstanceToSession(session, paneAgent.agent, paneAgent.threadId)) changed = true;
        }
      }
      if (tracker.applyPanePresence(session, paneAgents)) changed = true;
    }

    // For sessions NOT in the scan, apply empty presence to transition alive → exited
    if (lastState) {
      for (const s of lastState.sessions) {
        if (!nextBySession.has(s.name)) {
          if (tracker.applyPanePresence(s.name, [])) changed = true;
        }
      }
    }

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
        // Resolve TTY from the invoking WebSocket first.
        // A restored/background sidebar can have a stale clientSessionNames entry,
        // but its identify() handshake still carries the correct client TTY.
        const clientSess = clientSessionNames.get(ws);
        const senderWindowId = clientWindowIds.get(ws);
        const foregroundTtyForWindow = getForegroundClientTtyForWindow(senderWindowId);
        if (!isForegroundClient(ws)) {
          log("switch-session", "ignored from background sidebar", {
            target: cmd.name,
            senderSession: clientSess ?? null,
            senderTty: clientTtys.get(ws) ?? null,
            current: getCachedCurrentSession(),
          });
          break;
        }
        const tty = foregroundTtyForWindow
          ?? clientTtys.get(ws)
          ?? cmd.clientTty
          ?? (clientSess ? clientTtyBySession.get(clientSess) : undefined);
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

        adoptSidebarWidthFromWindow(clientSess, senderWindowId, "switch-session");
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
        if (isSidebarVisible() && isFullSidebarCapable(p) && p.name === "zellij") {
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
        if (!isForegroundClient(ws)) {
          log("switch-index", "ignored from background sidebar", {
            index: cmd.index,
            senderSession: clientSess ?? null,
            senderTty: clientTtys.get(ws) ?? null,
            current: getCachedCurrentSession(),
          });
          break;
        }
        const tty = getForegroundClientTtyForWindow(clientWindowIds.get(ws))
          ?? clientTtys.get(ws)
          ?? (clientSess ? clientTtyBySession.get(clientSess) : undefined);
        switchToVisibleIndex(cmd.index, tty, {
          session: clientSess,
          windowId: clientWindowIds.get(ws) ?? null,
        });
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
        // If killing the current session, switch to the adjacent session in sidebar order
        const currentBefore = getCurrentSession();
        if (currentBefore === cmd.name) {
          const allNames = p.listSessions().map((s) => s.name);
          const visible = sessionOrder.apply(allNames);
          const idx = visible.indexOf(cmd.name);
          // Prefer the session before, then after, in sidebar order
          const fallback = visible[idx - 1] ?? visible[idx + 1];
          if (fallback) {
            const tty = clientTtyBySession.get(cmd.name);
            p.switchSession(fallback, tty);
          }
        }
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
      case "set-filter":
        currentFilter = cmd.filter;
        saveConfig({ sessionFilter: cmd.filter });
        broadcastState();
        break;
      case "quit":
        quitAll();
        break;
      case "identify-pane":
        if (cmd.windowId) clientWindowIds.set(ws, cmd.windowId);
        // Hidden sidebar panes live in _os_stash temporarily; don't let that
        // transient session overwrite the client's logical session identity.
        if (cmd.sessionName === "_os_stash") break;
        // Store this client's session, reply with session + authoritative client TTY
        sendYourSession(ws, cmd.sessionName);
        break;
      case "focus-agent-pane":
        log("handleCommand", "focus-agent-pane received", { session: cmd.session, agent: cmd.agent, threadId: cmd.threadId, threadName: cmd.threadName });
        focusAgentPane(cmd.session, cmd.agent, cmd.threadId, cmd.threadName);
        break;
      case "kill-agent-pane":
        log("handleCommand", "kill-agent-pane received", { session: cmd.session, agent: cmd.agent, threadId: cmd.threadId, threadName: cmd.threadName });
        killAgentPane(cmd.session, cmd.agent, cmd.threadId, cmd.threadName);
        break;
      case "report-width": {
        const reported = clampSidebarWidth(cmd.width);
        const session = clientSessionNames.get(ws) ?? null;
        const senderWindowId = clientWindowIds.get(ws) ?? null;
        const current = getCachedCurrentSession();
        const sidebarStateBefore = getSidebarState();
        const sidebarWidth = sidebarStateBefore.width;
        const senderIsForeground = isForegroundClient(ws);
        const currentWindowId = session
          ? (sessionProviders.get(session) ?? mux).getCurrentWindowId()
          : null;

        const decision = applySidebarWidthReport(sidebarCoordinator, {
          width: reported,
          session,
          windowId: senderWindowId,
          isActiveSession: !!session && !!current && session === current,
          isForegroundClient: senderIsForeground,
          isCurrentWindow: !!senderWindowId && !!currentWindowId && senderWindowId === currentWindowId,
        });

        if (!decision.accepted) {
          log("report-width", `REJECTED — ${decision.reason}`, {
            reported,
            sidebarWidth,
            session,
            current,
            senderTty: clientTtys.get(ws) ?? null,
            senderWindowId,
            resizeAuthority: sidebarStateBefore.resizeAuthority,
            widthReportsSuppressed: areWidthReportsSuppressed(sidebarStateBefore),
            clientResizeGuardActive: isClientResizeReportGuardActive(),
          });
          break;
        }

        const nextSidebarWidth = getSidebarWidth();
        log("report-width", decision.continuedDrag ? "ACCEPTED — continuing user drag" : "ACCEPTED as user drag", {
          reported,
          oldWidth: decision.previousWidth,
          sidebarWidth: nextSidebarWidth,
          session,
        });
        saveConfig({ sidebarWidth: nextSidebarWidth });
        if (!startTransientSidebarResize()) {
          broadcastState();
        }
        enforceSidebarWidth(senderWindowId ?? undefined);
        break;
      }
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
    if (sidebarEnforceTimer) clearTimeout(sidebarEnforceTimer);
    clearClientResizeSyncTimer();
    clearProgrammaticAdjustmentTimer();
    if (portPollTimer) clearInterval(portPollTimer);
    if (paneScanTimer) clearInterval(paneScanTimer);
    for (const timer of pendingHighlightResets.values()) clearTimeout(timer);
    pendingHighlightResets.clear();
    for (const watcher of gitHeadWatchers.values()) watcher.close();
    gitHeadWatchers.clear();
    if (idleTimer) clearTimeout(idleTimer);
    if (initializingTimer) clearTimeout(initializingTimer);
    clearTransientResizeTimer();
    clearResizeStaggerTimers();
    sidebarCoordinator.stop();
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
        queueEnsureSidebarAcrossAllWindows();
        return new Response("ok", { status: 200 });
      }

      if (req.method === "POST" && url.pathname === "/focus") {
        try {
          const body = await req.text();
          const ctx = parseContext(body);
          if (ctx) {
            noteFocusContextChange();
            normalizeTmuxWindowSize(ctx.windowId);
            syncClientSessionsForTty(ctx.clientTty, ctx.session, ctx.windowId);
            handleFocus(ctx.session);
          } else {
            // Legacy: body is just the session name
            const name = body.trim().replace(/^"+|"+$/g, "");
            if (name) {
              noteFocusContextChange();
              handleFocus(name);
            }
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
          switchToVisibleIndex(index, ctx?.clientTty, {
            session: ctx?.session ?? null,
            windowId: ctx?.windowId ?? null,
          });
        } catch {}
        return new Response("ok", { status: 200 });
      }

      if (req.method === "POST" && url.pathname === "/ensure-sidebar") {
        try {
          const body = await req.text();
          const ctx = parseContext(body) ?? undefined;
          noteFocusContextChange();
          normalizeTmuxWindowSize(ctx?.windowId);
          log("http", "POST /ensure-sidebar", { sidebarVisible: isSidebarVisible(), ctx });
          // Enforce width immediately — window switch causes tmux to
          // proportionally redistribute panes, fix it before user sees it.
          if (isSidebarVisible()) enforceSidebarWidth();
          debouncedEnsureSidebar(ctx ?? undefined);
        } catch {}
        return new Response("ok", { status: 200 });
      }

      // client-resized hook: terminal window changed size — enforce stored width
      if (req.method === "POST" && url.pathname === "/suppress-width-reports") {
        const msParam = Number.parseInt(url.searchParams.get("ms") ?? "", 10);
        const ms = Number.isFinite(msParam) && msParam > 0
          ? Math.min(msParam, 10_000)
          : 2_000;
        suppressWidthReports(ms);
        log("http", "POST /suppress-width-reports", { ms });
        return new Response("ok", { status: 200 });
      }

      if (req.method === "POST" && url.pathname === "/client-resized") {
        log("http", "POST /client-resized", {
          sidebarVisible: isSidebarVisible(),
          sidebarWidth: getSidebarWidth(),
          widthReportsSuppressed: areWidthReportsSuppressed(getSidebarState()),
        });
        scheduleClientResizeSync();
        return new Response("ok", { status: 200 });
      }

      // pane-exited hook: a pane closed — kill orphaned sidebar panes
      if (req.method === "POST" && url.pathname === "/pane-exited") {
        if (isSidebarVisible()) {
          invalidateSidebarPaneCache();
          for (const { provider } of listSidebarPanesByProvider()) {
            provider.killOrphanedSidebarPanes();
          }
        }
        return new Response("ok", { status: 200 });
      }

      if (req.method === "POST" && url.pathname === "/api/runtime/pi/upsert") {
        try {
          const parsed = parsePiRuntimeInfo(await req.json());
          if (!parsed) {
            return new Response("invalid pi runtime payload", { status: 400 });
          }
          piLiveResolver.upsert(parsed);
          if (clientCount > 0) refreshPaneAgents();
          return new Response(null, { status: 204 });
        } catch {
          return new Response("invalid json", { status: 400 });
        }
      }

      if (req.method === "POST" && url.pathname === "/api/runtime/pi/delete") {
        try {
          const body = await req.json() as { pid?: number };
          if (typeof body.pid !== "number" || !Number.isInteger(body.pid) || body.pid <= 0) {
            return new Response("missing pid", { status: 400 });
          }
          piLiveResolver.delete(body.pid);
          if (clientCount > 0) refreshPaneAgents();
          return new Response(null, { status: 204 });
        } catch {
          return new Response("invalid json", { status: 400 });
        }
      }

      if (req.method === "POST" && url.pathname === "/api/agent-event") {
        try {
          const body = await req.json() as {
            agent?: string;
            status?: string;
            threadId?: string;
            threadName?: string;
            tmuxSession?: string;
            projectDir?: string;
            ts?: number;
          };
          log("agent-event", "received", {
            agent: body.agent,
            status: body.status,
            threadId: body.threadId?.slice(0, 8),
            tmuxSession: body.tmuxSession,
            projectDir: body.projectDir,
          });
          if (!body.agent || typeof body.agent !== "string") {
            return new Response("missing agent", { status: 400 });
          }
          if (!body.status || !VALID_AGENT_STATUSES.has(body.status as AgentStatus)) {
            return new Response("invalid status", { status: 400 });
          }

          // Resolve session: prefer the mux session name if it's one we know
          // about, otherwise fall back to projectDir-based resolution.
          let session: string | null = null;
          if (body.tmuxSession && typeof body.tmuxSession === "string") {
            const known = new Set<string>();
            for (const p of allProviders) {
              for (const s of p.listSessions()) known.add(s.name);
            }
            if (known.has(body.tmuxSession)) session = body.tmuxSession;
          }
          if (!session && body.threadId) {
            session = resolveThreadOwner(body.agent, body.threadId, body.threadName)?.session ?? null;
          }
          if (!session && body.projectDir && typeof body.projectDir === "string") {
            session = watcherCtx.resolveSession(body.projectDir);
          }
          if (!session) {
            return new Response("could not resolve session", { status: 404 });
          }

          // Tell the matching watcher that the plugin is driving this thread so
          // it skips cloud API / WebSocket calls for it. Duck-typed to avoid
          // leaking watcher internals into the shared interface.
          if (body.threadId) {
            for (const w of allWatchers) {
              if (w.name === body.agent && typeof (w as { markPluginOwned?: (id: string) => void }).markPluginOwned === "function") {
                (w as { markPluginOwned: (id: string) => void }).markPluginOwned(body.threadId);
              }
            }
          }

          watcherCtx.emit({
            agent: body.agent,
            session,
            status: body.status as AgentEvent["status"],
            ts: typeof body.ts === "number" ? body.ts : Date.now(),
            threadId: body.threadId,
            threadName: body.threadName,
          });
          return new Response(null, { status: 204 });
        } catch {
          return new Response("invalid json", { status: 400 });
        }
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
        connectedClients.add(ws);
        ws.subscribe("sidebar");
        clientCount++;
        log("ws", "client connected", { clientCount });
        ws.send(JSON.stringify({ type: "hello", protocol: PROTOCOL_VERSION, serverVersion: "0.2.0-alpha.5" }));
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
        connectedClients.delete(ws);
        ws.unsubscribe("sidebar");
        clientCount--;
        if (clientCount < 0) clientCount = 0;
        log("ws", "client disconnected", { clientCount });
        startIdleTimerIfNeeded("last websocket closed");
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

  // Local tmux hooks always curl loopback, regardless of what address
  // the server binds to. SERVER_HOST may be 0.0.0.0 or a bridge IP to
  // accept remote POSTs, but in-process hooks should never use that.
  for (const p of allProviders) p.setupHooks(LOCAL_CLIENT_HOST, SERVER_PORT);
  // Older builds stashed hidden tmux sidebars in _os_stash and kept them
  // running. Clear any legacy stash session on startup so hidden panes do not
  // keep bloating the tmux server after upgrades.
  for (const p of getProvidersWithSidebar()) p.cleanupSidebar();
  const sidebarPresence = reconcileSidebarPresence();
  if (sidebarPresence.visible) {
    for (const { provider } of listSidebarPanesByProvider()) {
      provider.killOrphanedSidebarPanes();
    }
    const panesByProvider = reconcileSidebarPresence();
    if (panesByProvider.visible) {
      markSidebarReady();
      const curSession = getCurrentSession();
      for (const p of getProvidersWithSidebar()) {
        const allWindows = p.listActiveWindows();
        const curWindowId = p.getCurrentWindowId();
        normalizeTmuxWindowSize(curWindowId ?? undefined);

        // Tier 1: current active window (instant)
        if (curSession && curWindowId) {
          const activeWindow = allWindows.find((w) => w.sessionName === curSession && w.id === curWindowId);
          if (activeWindow) {
            ensureSidebarInWindow(p, { session: activeWindow.sessionName, windowId: activeWindow.id });
          }
        }

        // Tier 2 + 3: staggered
        const tier2 = allWindows.filter((w) => w.sessionName === curSession && w.id !== curWindowId);
        const tier3 = allWindows.filter((w) => w.sessionName !== curSession);
        let delay = SPAWN_STAGGER_MS;
        for (const w of [...tier2, ...tier3]) {
          const win = w;
          const prov = p;
          setTimeout(() => {
            if (isSidebarVisible()) ensureSidebarInWindow(prov, { session: win.sessionName, windowId: win.id });
          }, delay);
          delay += SPAWN_STAGGER_MS;
        }
      }
      scheduleSidebarWidthEnforcement();
    }
  }
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

  // Start agent watchers after server is ready. Each watcher is isolated so
  // a failure in one doesn't block the others (pi, claude-code, etc. shouldn't
  // depend on the amp watcher booting cleanly).
  for (const w of allWatchers) {
    try {
      w.start(watcherCtx);
      log("server", `agent watcher started: ${w.name}`);
    } catch (err) {
      log("server", `agent watcher failed to start: ${w.name}`, { error: String(err) });
    }
  }

  startIdleTimerIfNeeded("server booted without clients");

  process.on("SIGINT", () => { cleanup(); process.exit(0); });
  process.on("SIGTERM", () => { cleanup(); process.exit(0); });

  const names = allProviders.map((p) => p.name).join(", ");
  console.log(`opensessions server listening on ${SERVER_HOST}:${SERVER_PORT} (mux: ${names})`);
}
