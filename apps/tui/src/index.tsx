import { render } from "@opentui/solid";
import { appendFileSync, readFileSync } from "fs";
import { createSignal, createEffect, onCleanup, onMount, batch, For, Show, createMemo, createSelector, type Accessor } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { useKeyboard, useRenderer } from "@opentui/solid";
import { TextAttributes, type MouseEvent, type InputRenderable, type KeyEvent } from "@opentui/core";
import { resolveSyncedFocus } from "./focus-sync";

import { ensureServer } from "@opensessions/runtime";
import {
  type ServerMessage,
  type SessionData,
  type ClientCommand,
  type Theme,
  type MetadataTone,
  type SessionFilterMode,
  TERMINAL_STATUSES,
  SERVER_PORT,
  SERVER_HOST,
  TOKEN_FILE,
  BUILTIN_THEMES,
  loadConfig,
  resolveTheme,
  saveConfig,
} from "@opensessions/runtime";
import { TmuxClient } from "@opensessions/mux-tmux";

// Detect which mux we're running inside
type MuxContext =
  | { type: "tmux"; sdk: TmuxClient; paneId: string }
  | { type: "zellij"; sessionName: string; paneId: string }
  | { type: "none" };

type BufferedShortcutKey = Pick<KeyEvent, "name" | "number" | "shift" | "meta" | "option">;

function detectMuxContext(): MuxContext {
  if (process.env.TMUX_PANE && process.env.TMUX) {
    return { type: "tmux", sdk: new TmuxClient(), paneId: process.env.TMUX_PANE };
  }
  if (process.env.ZELLIJ_SESSION_NAME) {
    return {
      type: "zellij",
      sessionName: process.env.ZELLIJ_SESSION_NAME,
      paneId: process.env.ZELLIJ_PANE_ID ?? "",
    };
  }
  return { type: "none" };
}

const muxCtx = detectMuxContext();

const SPINNERS = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const UNSEEN_ICON = "●";
const BOLD = TextAttributes.BOLD;
const DIM = TextAttributes.DIM;
const SPARK_BLOCKS = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
const PRINTABLE_SHORTCUT_DELAY_MS = 45;
const PRINTABLE_BURST_SUPPRESSION_MS = 180;

const THEME_NAMES = Object.keys(BUILTIN_THEMES);
const DEFAULT_DETAIL_PANEL_HEIGHT = 10;
const MIN_DETAIL_PANEL_HEIGHT = 4;
const RESIZE_DEBUG_LOG = "/tmp/opensessions-tui-resize.log";

const TONE_ICONS: Record<MetadataTone, string> = {
  neutral: "·",
  info: "ℹ",
  success: "✓",
  warn: "⚠",
  error: "✗",
};

const FILTER_CYCLE: SessionFilterMode[] = ["all", "active", "running"];
const FILTER_LABELS: Record<SessionFilterMode, string> = {
  all: "all",
  active: "agents",
  running: "running",
};

function toneColor(tone: MetadataTone | undefined, palette: ReturnType<() => Theme["palette"]>): string {
  switch (tone) {
    case "success": return palette.green;
    case "error": return palette.red;
    case "warn": return palette.yellow;
    case "info": return palette.blue;
    default: return palette.overlay0;
  }
}

function logResizeDebug(message: string, data?: Record<string, unknown>): void {
  const ts = new Date().toISOString();
  const extra = data ? ` ${JSON.stringify(data)}` : "";
  try {
    appendFileSync(RESIZE_DEBUG_LOG, `[${ts}] [pid:${process.pid}] ${message}${extra}\n`);
  } catch {}
}

function cloneShortcutKey(key: KeyEvent): BufferedShortcutKey {
  return {
    name: key.name,
    number: key.number,
    shift: key.shift,
    meta: key.meta,
    option: key.option,
  };
}

function isPlainPrintableKey(key: BufferedShortcutKey): boolean {
  return !key.meta && !key.option && key.name.length === 1;
}

function isBurstGuardedShortcut(key: BufferedShortcutKey): boolean {
  if (!isPlainPrintableKey(key)) return false;
  if (key.number) return true;
  return ["c", "d", "f", "n", "q", "r", "t", "u", "x"].includes(key.name);
}

function clampDetailPanelHeight(height: number): number {
  return Math.max(MIN_DETAIL_PANEL_HEIGHT, Math.round(height));
}

function getStoredDetailPanelHeight(sessionName: string): number {
  const stored = loadConfig().detailPanelHeights?.[sessionName];
  return typeof stored === "number" ? clampDetailPanelHeight(stored) : DEFAULT_DETAIL_PANEL_HEIGHT;
}

function persistDetailPanelHeight(sessionName: string, height: number): void {
  const config = loadConfig();
  saveConfig({
    detailPanelHeights: {
      ...(config.detailPanelHeights ?? {}),
      [sessionName]: clampDetailPanelHeight(height),
    },
  });
}

function localLinkText(link: SessionData["localLinks"][number]): string {
  if (link.kind === "direct") return String(link.port);

  try {
    const url = new URL(link.url);
    return url.port && url.port !== "80" && url.port !== "443" && url.port !== "1355"
      ? url.host
      : url.hostname;
  } catch {
    return link.label;
  }
}

function sessionDirLabel(dir: string, sessionName: string): string {
  if (!dir) return "";

  const parts = dir.replace(/\/+$/, "").split("/").filter(Boolean);
  const name = parts[parts.length - 1] || "";
  if (!name) return "";

  if (name !== sessionName) return name;

  const parent = parts[parts.length - 2];
  return parent ? `${parent}/${name}` : name;
}

function wrapLocalLinks(links: SessionData["localLinks"], maxWidth: number): SessionData["localLinks"][] {
  if (links.length === 0) return [];

  const rows: SessionData["localLinks"][] = [];
  let currentRow: SessionData["localLinks"] = [];
  let currentWidth = 0;

  for (const link of links) {
    const textWidth = localLinkText(link).length;
    const itemWidth = currentRow.length === 0 ? textWidth : textWidth + 3;

    if (currentRow.length > 0 && currentWidth + itemWidth > maxWidth) {
      rows.push(currentRow);
      currentRow = [link];
      currentWidth = textWidth;
      continue;
    }

    currentRow.push(link);
    currentWidth += itemWidth;
  }

  if (currentRow.length > 0) rows.push(currentRow);
  return rows;
}

/** Refocus the main (non-sidebar) pane after TUI capability detection finishes.
 *  This must happen from the TUI process — doing it from start.sh races with
 *  capability query responses and leaks escape sequences to the main pane. */
function refocusMainPane(): boolean {
  if (muxCtx.type === "tmux") {
    try {
      const selfPaneId = muxCtx.paneId;
      // Use the TUI's own pane ID to find its current window (handles stash restore
      // where the pane may have moved to a different window than the original).
      const windowId = process.env.REFOCUS_WINDOW
        || Bun.spawnSync(
            ["tmux", "display-message", "-t", selfPaneId, "-p", "#{window_id}"],
            { stdout: "pipe", stderr: "pipe" },
          ).stdout.toString().trim();
      if (!windowId) {
        logResizeDebug("refocusMainPane:no-window-id", { selfPaneId });
        return false;
      }
      const r = Bun.spawnSync(
        ["tmux", "list-panes", "-t", windowId, "-F", "#{pane_id}"],
        { stdout: "pipe", stderr: "pipe" },
      );
      const paneIds = r.stdout.toString().trim().split("\n").filter(Boolean);
      // Identify "main" as any pane in the window that is NOT this TUI's own pane.
      // Title-based detection races with the parent server's setPaneTitle call:
      // when the sidebar is on the left, list-panes orders the new sidebar pane
      // first (lower pane_index), and if its title hasn't been set yet the find
      // would pick the sidebar itself and re-select it.
      const main = paneIds.find((id) => id && id !== selfPaneId);
      logResizeDebug("refocusMainPane:tmux", { windowId, selfPaneId, paneIds, main });
      if (main) {
        const sel = Bun.spawnSync(
          ["tmux", "select-pane", "-t", main],
          { stdout: "pipe", stderr: "pipe" },
        );
        if (sel.exitCode !== 0) {
          logResizeDebug("refocusMainPane:select-failed", {
            main,
            stderr: sel.stderr.toString().trim(),
          });
          return false;
        }
        return true;
      }
    } catch (err) {
      logResizeDebug("refocusMainPane:error", { error: String(err) });
    }
  } else if (muxCtx.type === "zellij") {
    // Zellij: move focus to the right (away from the sidebar on the left)
    try {
      Bun.spawnSync(["zellij", "action", "move-focus", "right"], { stdout: "pipe", stderr: "pipe" });
      return true;
    } catch {}
  }

  return false;
}

function getClientTty(): string {
  if (muxCtx.type === "tmux") {
    const { sdk, paneId } = muxCtx;
    const sessName = sdk.display("#{session_name}", { target: paneId });
    if (sessName) {
      const clients = sdk.listClients();
      const client = clients.find((c) => c.sessionName === sessName);
      if (client) return client.tty;
    }
    return "";
  }
  // Zellij doesn't expose client TTY
  return "";
}

function getLocalSessionName(): string | null {
  if (muxCtx.type === "tmux") {
    const sessionName = muxCtx.sdk.display("#{session_name}", { target: muxCtx.paneId });
    return sessionName || null;
  }

  if (muxCtx.type === "zellij") {
    return muxCtx.sessionName || null;
  }

  return null;
}

function getLocalWindowId(): string | null {
  if (muxCtx.type === "tmux") {
    const windowId = muxCtx.sdk.display("#{window_id}", { target: muxCtx.paneId });
    return windowId || null;
  }

  return null;
}

function getLocalPaneActive(): boolean | null {
  if (muxCtx.type !== "tmux") return null;
  const active = muxCtx.sdk.display("#{pane_active}", { target: muxCtx.paneId });
  if (active === "1") return true;
  if (active === "0") return false;
  return null;
}

function App() {
  const renderer = useRenderer();
  const startupSessionName = getLocalSessionName();
  const startupWindowId = getLocalWindowId();

  // --- Theme state (driven by server) ---
  const [theme, setTheme] = createSignal<Theme>(resolveTheme(undefined));
  const P = () => theme().palette;
  const S = () => theme().status;

  const [sessions, setSessions] = createStore<SessionData[]>([]);
  const [focusedSession, setFocusedSession] = createSignal<string | null>(null);
  const [currentSession, setCurrentSession] = createSignal<string | null>(startupSessionName);
  const [mySession, setMySession] = createSignal<string | null>(startupSessionName);
  const [connected, setConnected] = createSignal(false);
  const [spinIdx, setSpinIdx] = createSignal(0);
  const [terminalWidth, setTerminalWidth] = createSignal(Math.max(0, renderer.terminalWidth));
  const [detailPanelHeight, setDetailPanelHeight] = createSignal(DEFAULT_DETAIL_PANEL_HEIGHT);
  const [isDetailResizeHover, setIsDetailResizeHover] = createSignal(false);
  const [isDetailResizing, setIsDetailResizing] = createSignal(false);
  const detailPanelSessionName = createMemo(() => focusedSession() ?? mySession());

  // --- Session filter (synced from server) ---
  const [sessionFilter, setSessionFilter] = createSignal<SessionFilterMode>("all");
  const [initializing, setInitializing] = createSignal(false);
  const [initLabel, setInitLabel] = createSignal("");

  const filteredSessions = createMemo(() => {
    const mode = sessionFilter();
    if (mode === "all") return sessions;
    return sessions.filter((s) => {
      if (mode === "active") return s.agents.length > 0 || s.agentState !== null;
      // "running" — only sessions where an agent is currently running
      const st = s.agentState?.status;
      return st === "running" || st === "tool-running" || st === "waiting";
    });
  });

  function cycleSessionFilter() {
    const idx = FILTER_CYCLE.indexOf(sessionFilter());
    const next = FILTER_CYCLE[(idx + 1) % FILTER_CYCLE.length]!;
    setSessionFilter(next);
    send({ type: "set-filter", filter: next });
    flash(`filter: ${FILTER_LABELS[next]}`);
    // If the focused session is no longer visible, refocus
    const list = filteredSessions();
    if (list.length > 0 && !list.some((s) => s.name === focusedSession())) {
      setFocusedSession(list[0]!.name);
      send({ type: "focus-session", name: list[0]!.name });
    }
  }

  // --- Panel focus: sessions list vs agent detail ---
  type PanelFocus = "sessions" | "agents";
  const [panelFocus, setPanelFocus] = createSignal<PanelFocus>("sessions");
  const [focusedAgentIdx, setFocusedAgentIdx] = createSignal(0);

  // --- Modal state ---
  const [modal, setModal] = createSignal<"none" | "theme-picker" | "confirm-kill" | "confirm-quit">("none");
  const [killTarget, setKillTarget] = createSignal<string | null>(null);

  // --- Terminal focus tracking (defends against tmux send-keys injection) ---
  // Real human keystrokes only reach the sidebar pane while it has terminal
  // focus. tmux send-keys to a non-focused pane delivers raw bytes but does
  // NOT cause a focus-in escape sequence. By gating destructive UI shortcuts
  // (q, x, n, c, d, digits, Tab, Enter, etc.) on this signal we make
  // accidental keystroke injection from other tmux clients/agents a no-op.
  // See ai_logs/02-harden-tui-input.md (and the Oracle review) for context.
  //
  // Default `true`: on startup, before any focus event has been observed,
  // permit input. After tmux sends the first focus-out (which it does when
  // refocusMainPane runs after capability detection), this flips to false
  // and stays correct from then on. Terminals that don't support focus
  // reporting will simply leave this true (graceful degradation — destructive
  // actions still require Enter confirmation thanks to the modal hardening).
  const [paneHasTerminalFocus, setPaneHasTerminalFocus] = createSignal(true);
  let themeBeforePreview: Theme | null = null;

  // --- Flash message (brief feedback after actions like refresh) ---
  const [flashMessage, setFlashMessage] = createSignal<string | null>(null);
  let flashTimer: ReturnType<typeof setTimeout> | null = null;
  function flash(msg: string, ms = 1200) {
    if (flashTimer) clearTimeout(flashTimer);
    setFlashMessage(msg);
    flashTimer = setTimeout(() => setFlashMessage(null), ms);
  }

  const [clientTty, setClientTty] = createSignal(getClientTty());
  let ws: WebSocket | null = null;
  let startupFocusSynced = false;
  let lastIdentifiedSessionName: string | null = startupSessionName;
  let lastIdentifiedWindowId: string | null = startupWindowId;
  let detailResizeStartY = 0;
  let detailResizeStartHeight = DEFAULT_DETAIL_PANEL_HEIGHT;

  const focusedData = createMemo(() =>
    sessions.find((s) => s.name === focusedSession()) ?? null,
  );

  function send(cmd: ClientCommand) {
    if (connected() && ws) ws.send(JSON.stringify(cmd));
  }

  function switchToSession(name: string) {
    // Optimistic local update — makes rapid Tab repeat instant by removing
    // the server/hook round-trip from the next-Tab decision.
    // The server's focus/state broadcast will reconcile if needed.
    // Also update mySession optimistically: after a successful tmux switch,
    // this sidebar client now belongs to the target session. Without this,
    // resolveSyncedFocus can snap focus back to the previous local session
    // during the handoff window before the server replies with your-session.
    setMySession(name);
    setCurrentSession(name);
    setFocusedSession(name);
    setPanelFocus("sessions");
    setFocusedAgentIdx(0);
    send({ type: "switch-session", name });
  }

  function reIdentify() {
    const sessionName = getLocalSessionName();
    if (!sessionName || sessionName === "_os_stash") return;
    const windowId = getLocalWindowId() ?? undefined;

    lastIdentifiedSessionName = sessionName;
    lastIdentifiedWindowId = windowId ?? null;

    if (muxCtx.type === "tmux") {
      send({ type: "identify-pane", paneId: muxCtx.paneId, sessionName, windowId });
    } else if (muxCtx.type === "zellij") {
      send({ type: "identify-pane", paneId: muxCtx.paneId, sessionName });
    }
  }

  function maybeReIdentify() {
    const sessionName = getLocalSessionName();
    const windowId = getLocalWindowId();
    if (!sessionName || sessionName === "_os_stash") return;
    if (sessionName !== lastIdentifiedSessionName || windowId !== lastIdentifiedWindowId) {
      reIdentify();
    }
  }

  function moveLocalFocus(delta: -1 | 1) {
    const list = filteredSessions();
    if (list.length === 0) return;

    const current = focusedSession();
    const currentIdx = Math.max(0, list.findIndex((s) => s.name === current));
    const nextIdx = Math.max(0, Math.min(list.length - 1, currentIdx + delta));
    const next = list[nextIdx]?.name ?? null;

    if (!next || next === current) return;

    setFocusedSession(next);
    send({ type: "focus-session", name: next });
  }

  function moveAgentFocus(delta: -1 | 1) {
    const data = focusedData();
    const agents = data?.agents ?? [];
    if (agents.length === 0) return;
    const idx = focusedAgentIdx();
    const next = Math.max(0, Math.min(agents.length - 1, idx + delta));
    setFocusedAgentIdx(next);
  }

  function activateFocusedAgent() {
    const data = focusedData();
    const agents = data?.agents ?? [];
    const agent = agents[focusedAgentIdx()];
    if (!agent || !data) return;
    appendFileSync("/tmp/opensessions-tui-agent-click.log",
      `[${new Date().toISOString()}] keyboard focus-agent-pane session=${data.name} agent=${agent.agent} threadId=${agent.threadId} threadName=${agent.threadName}\n`);
    // Switch to the agent's session first so the tmux client is attached
    setCurrentSession(data.name);
    send({ type: "switch-session", name: data.name });
    // Then focus the specific agent pane within that session
    send({
      type: "focus-agent-pane",
      session: data.name,
      agent: agent.agent,
      threadId: agent.threadId,
      threadName: agent.threadName,
    });
  }

  function dismissFocusedAgent() {
    const data = focusedData();
    const agents = data?.agents ?? [];
    const agent = agents[focusedAgentIdx()];
    if (!agent || !data) return;
    send({
      type: "dismiss-agent",
      session: data.name,
      agent: agent.agent,
      threadId: agent.threadId,
    });
    // Adjust index if we dismissed the last item
    if (focusedAgentIdx() >= agents.length - 1 && agents.length > 1) {
      setFocusedAgentIdx(agents.length - 2);
    }
    // If no agents left, go back to sessions
    if (agents.length <= 1) setPanelFocus("sessions");
  }

  function killFocusedAgentPane() {
    const data = focusedData();
    const agents = data?.agents ?? [];
    const agent = agents[focusedAgentIdx()];
    if (!agent || !data) return;
    send({
      type: "kill-agent-pane",
      session: data.name,
      agent: agent.agent,
      threadId: agent.threadId,
      threadName: agent.threadName,
    });
  }

  function togglePanelFocus() {
    const data = focusedData();
    const agents = data?.agents ?? [];
    if (panelFocus() === "sessions" && agents.length > 0) {
      setPanelFocus("agents");
      setFocusedAgentIdx((idx) => Math.min(idx, agents.length - 1));
    } else {
      setPanelFocus("sessions");
    }
  }

  function applyTheme(themeName: string) {
    send({ type: "set-theme", theme: themeName });
  }

  function previewTheme(themeName: string) {
    setTheme(resolveTheme(themeName));
  }

  function resizeDetailPanel(delta: -1 | 1) {
    const nextHeight = clampDetailPanelHeight(detailPanelHeight() + delta);
    if (nextHeight === detailPanelHeight()) return;

    setDetailPanelHeight(nextHeight);

    const sessionName = detailPanelSessionName();
    if (sessionName) {
      persistDetailPanelHeight(sessionName, nextHeight);
    }
  }

  function beginDetailResize(event: MouseEvent) {
    logResizeDebug("beginDetailResize", {
      button: event.button,
      x: event.x,
      y: event.y,
      currentHeight: detailPanelHeight(),
      session: detailPanelSessionName(),
      target: event.target?.id ?? null,
    });
    if (event.button !== 0) return;
    (renderer as any).setCapturedRenderable?.(event.target ?? undefined);
    detailResizeStartY = event.y;
    detailResizeStartHeight = detailPanelHeight();
    setIsDetailResizing(true);
    event.stopPropagation();
  }

  function handleDetailResizeDrag(event: MouseEvent) {
    logResizeDebug("handleDetailResizeDrag", {
      x: event.x,
      y: event.y,
      isResizing: isDetailResizing(),
      startY: detailResizeStartY,
      startHeight: detailResizeStartHeight,
      currentHeight: detailPanelHeight(),
      session: detailPanelSessionName(),
    });
    if (!isDetailResizing()) return;
    const delta = detailResizeStartY - event.y;
    const nextHeight = clampDetailPanelHeight(detailResizeStartHeight + delta);
    setDetailPanelHeight(nextHeight);
    logResizeDebug("handleDetailResizeDrag:applied", {
      delta,
      nextHeight,
      session: detailPanelSessionName(),
    });
    event.stopPropagation();
  }

  function endDetailResize(event?: MouseEvent) {
    logResizeDebug("endDetailResize", {
      x: event?.x,
      y: event?.y,
      isResizing: isDetailResizing(),
      currentHeight: detailPanelHeight(),
      session: detailPanelSessionName(),
      target: event?.target?.id ?? null,
    });
    if (!isDetailResizing()) return;
    (renderer as any).setCapturedRenderable?.(undefined);
    setIsDetailResizing(false);
    setIsDetailResizeHover(false);

    const sessionName = detailPanelSessionName();
    if (sessionName) {
      persistDetailPanelHeight(sessionName, detailPanelHeight());
      logResizeDebug("endDetailResize:persisted", {
        session: sessionName,
        height: detailPanelHeight(),
      });
    }

    event?.stopPropagation();
  }

  function createNewSession() {
    if (muxCtx.type !== "tmux") {
      send({ type: "new-session" });
      return;
    }
    const scriptPath = new URL("../scripts/sessionizer.sh", import.meta.url).pathname;
    muxCtx.sdk.displayPopup({
      command: `bash "${scriptPath}"`,
      title: " new session ",
      width: "60%",
      height: "60%",
      env: {
        OPENSESSIONS_HOST: SERVER_HOST,
        OPENSESSIONS_PORT: String(SERVER_PORT),
        OPENSESSIONS_TOKEN_FILE: TOKEN_FILE,
      },
      closeOnExit: true,
    });
  }

  onMount(() => {
    setTerminalWidth(Math.max(0, renderer.terminalWidth));
    logResizeDebug("mount", {
      startupSessionName,
      localSessionName: getLocalSessionName(),
      muxType: muxCtx.type,
      tmuxPane: process.env.TMUX_PANE ?? null,
    });
    // Refocus the main pane once terminal capability detection finishes.
    // This avoids the race where start.sh refocuses too early and capability
    // responses leak as garbage text into the main pane.
    let startupRefocused = false;
    const doStartupRefocus = () => {
      if (startupRefocused) return;
      startupRefocused = true;
      if (refocusMainPane()) {
        // We just selected the main pane away from this TUI. Mark ourselves
        // unfocused immediately instead of waiting for tmux/terminal focus-out
        // bytes; otherwise `tmux send-keys -t session:window ...` can target a
        // stale-active sidebar pane and have printable command text interpreted
        // as shortcuts such as `n`/`c` (new-session popup).
        setPaneHasTerminalFocus(false);
      }
    };
    renderer.on("capabilities", doStartupRefocus);
    // Fallback: if no capability response arrives within 2s, refocus anyway
    const refocusTimeout = setTimeout(doStartupRefocus, 2000);

    // --- Terminal focus reporting (DECSET 1004) ---
    // Enable focus events so we can distinguish real user keystrokes (which
    // arrive while the pane is focused) from tmux send-keys injection from
    // other clients (which delivers bytes WITHOUT a focus-in event). This
    // is the primary defense against accidental UI-action injection.
    //
    // We attach a parallel `data` listener on stdin: Node allows multiple
    // listeners on the same Readable, so OpenTUI's input parser still gets
    // every byte. We don't touch raw mode or pause/resume the stream.
    let focusEventsEnabled = false;
    try {
      process.stdout.write("\x1b[?1004h");
      focusEventsEnabled = true;
    } catch { /* terminal doesn't support write — leave gating as default-true */ }

    let focusSeqTail = "";
    const onStdinData = (chunk: Buffer | string) => {
      const data = focusSeqTail + (typeof chunk === "string" ? chunk : chunk.toString("binary"));
      let i = 0;
      while (i < data.length) {
        if (data.charCodeAt(i) === 0x1b && i + 2 < data.length && data[i + 1] === "[") {
          const code = data[i + 2];
          if (code === "I") { setPaneHasTerminalFocus(true); i += 3; continue; }
          if (code === "O") { setPaneHasTerminalFocus(false); i += 3; continue; }
        }
        i++;
      }
      // Retain a small tail in case a focus sequence is split across chunks.
      focusSeqTail = data.slice(-3);
    };
    process.stdin.on("data", onStdinData);

    onCleanup(() => {
      clearTimeout(refocusTimeout);
      renderer.removeListener("capabilities", doStartupRefocus);
      try { process.stdin.off("data", onStdinData); } catch {}
      if (focusEventsEnabled) {
        try { process.stdout.write("\x1b[?1004l"); } catch {}
      }
    });

    // Read the per-instance auth token written by the server at startup.
    // Empty string falls through to a 401 on the upgrade — server logs make
    // that obvious, and the reconnect loop will retry once the file lands.
    let authToken = "";
    try { authToken = readFileSync(TOKEN_FILE, "utf-8").trim(); } catch {}
    const wsQuery = authToken ? `?token=${encodeURIComponent(authToken)}` : "";
    const socket = new WebSocket(`ws://${SERVER_HOST}:${SERVER_PORT}/${wsQuery}`);
    ws = socket;

    socket.onopen = () => {
      setConnected(true);
      const tty = clientTty();
      if (tty) send({ type: "identify", clientTty: tty });
      reIdentify();

      // Report sidebar width on SIGWINCH (terminal resize / pane drag)
      // Only the TUI in the current session reports — other TUIs' resizes
      // are always enforcement echoes, never user drags.
      let lastReportedWidth = renderer.terminalWidth;
      const onResize = () => {
        const width = renderer.terminalWidth;
        setTerminalWidth(Math.max(0, width));
        if (width !== lastReportedWidth) {
          lastReportedWidth = width;
          const liveSession = getLocalSessionName();
          const current = currentSession();
          if (!liveSession || liveSession === "_os_stash") return;
          if (current && liveSession !== current) return;
          send({ type: "report-width", width });
        }
      };
      renderer.on("resize", onResize);
      onCleanup(() => renderer.removeListener("resize", onResize));
    };

    socket.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data as string) as ServerMessage;
        let startupFocusToPublish: string | null = null;
        batch(() => {
          const liveLocalSessionName = getLocalSessionName();
          const localSessionName = liveLocalSessionName && liveLocalSessionName !== "_os_stash"
            ? liveLocalSessionName
            : mySession() ?? startupSessionName;

          if (msg.type === "state") {
            const startupFocus = !startupFocusSynced
              && startupSessionName
              && msg.sessions.some((session) => session.name === startupSessionName)
              ? startupSessionName
              : msg.focusedSession;
            const nextFocusedSession = resolveSyncedFocus(
              startupFocus,
              msg.currentSession,
              localSessionName,
            );

            if (startupFocus === startupSessionName) {
              startupFocusSynced = true;
              if (msg.focusedSession !== startupSessionName) {
                startupFocusToPublish = startupSessionName;
              }
            }

            setSessions(reconcile(msg.sessions, { key: "name" }));
            // Don't override user's focused session during initialization —
            // staggered spawns cause frequent state broadcasts that would
            // keep yanking focus away from what the user is looking at.
            if (!msg.initializing) {
              setFocusedSession(nextFocusedSession);
            }
            setCurrentSession(msg.currentSession);
            setTheme(resolveTheme(msg.theme));
            setSessionFilter(msg.sessionFilter ?? "all");
            setInitializing(msg.initializing ?? false);
            setInitLabel(msg.initLabel ?? "");
          } else if (msg.type === "focus") {
            if (!initializing()) {
              const nextFocusedSession = resolveSyncedFocus(msg.focusedSession, msg.currentSession, localSessionName);
              setFocusedSession(nextFocusedSession);
              setCurrentSession(msg.currentSession);
            }
          } else if (msg.type === "your-session") {
            setMySession(msg.name);
            setCurrentSession(msg.name);
            if (msg.clientTty) setClientTty(msg.clientTty);

            if (!startupFocusSynced && sessions.some((session) => session.name === msg.name)) {
              startupFocusSynced = true;
              setFocusedSession(msg.name);
              if (focusedSession() !== msg.name) {
                startupFocusToPublish = msg.name;
              }
            }
          } else if (msg.type === "re-identify") {
            reIdentify();
          }
        });

        maybeReIdentify();

        if (startupFocusToPublish) {
          send({ type: "focus-session", name: startupFocusToPublish });
        }
      } catch {}
    };

    socket.onclose = () => {
      setConnected(false);
      renderer.destroy();
    };

    onCleanup(() => socket.close());

    // Listen for quit messages from server
    socket.addEventListener("message", (event) => {
      try {
        const msg = JSON.parse(event.data as string);
        if (msg.type === "quit") {
          if (ws) ws.close();
          renderer.destroy();
        }
      } catch {}
    });
  });

  const hasRunning = createMemo(() =>
    sessions.some((s) => s.agentState?.status === "running"),
  );

  createEffect(() => {
    if (!hasRunning() && !initializing()) return;
    const interval = setInterval(() => {
      setSpinIdx((i) => (i + 1) % SPINNERS.length);
    }, 120);
    onCleanup(() => clearInterval(interval));
  });

  createEffect(() => {
    const sessionName = detailPanelSessionName();
    if (!sessionName) return;
    const storedHeight = getStoredDetailPanelHeight(sessionName);
    logResizeDebug("loadStoredDetailPanelHeight", {
      session: sessionName,
      storedHeight,
    });
    setDetailPanelHeight(storedHeight);
  });

  createEffect(() => {
    logResizeDebug("detailPanelHeight:changed", {
      height: detailPanelHeight(),
      session: detailPanelSessionName(),
      isResizing: isDetailResizing(),
    });
  });

  let printableShortcutTimer: ReturnType<typeof setTimeout> | null = null;
  let pendingPrintableShortcut: BufferedShortcutKey | null = null;
  let suppressPrintableShortcutsUntil = 0;

  function clearPendingPrintableShortcut() {
    if (printableShortcutTimer) clearTimeout(printableShortcutTimer);
    printableShortcutTimer = null;
    pendingPrintableShortcut = null;
  }

  onCleanup(clearPendingPrintableShortcut);

  function handleNormalKey(key: BufferedShortcutKey) {
    // --- Normal mode keybindings ---
    // Alt+Up / Alt+Down → reorder session
    if ((key.meta || key.option) && (key.name === "up" || key.name === "down")) {
      const focused = focusedSession();
      if (focused) {
        const delta: -1 | 1 = key.name === "up" ? -1 : 1;
        send({ type: "reorder-session", name: focused, delta });
      }
      return;
    }

    switch (key.name) {
      case "q":
        // Open a confirm modal instead of immediately quitting. The modal
        // requires Enter to confirm, so injected printable characters cannot
        // tear the sidebar down. The confirmed action is `close-sidebar`,
        // which is scoped to this client's pane (no global blast radius).
        setModal("confirm-quit");
        break;
      case "escape":
        if (panelFocus() === "agents") {
          setPanelFocus("sessions");
        }
        break;
      case "up":
      case "k":
        if (panelFocus() === "agents") {
          moveAgentFocus(-1);
        } else {
          moveLocalFocus(-1);
        }
        break;
      case "down":
      case "j":
        if (panelFocus() === "agents") {
          moveAgentFocus(1);
        } else {
          moveLocalFocus(1);
        }
        break;
      case "left":
      case "h":
        if (panelFocus() === "agents") {
          setPanelFocus("sessions");
        } else {
          resizeDetailPanel(-1);
        }
        break;
      case "right":
      case "l":
        if (panelFocus() === "sessions") {
          const data = focusedData();
          const agents = data?.agents ?? [];
          if (agents.length > 0) {
            setPanelFocus("agents");
            setFocusedAgentIdx((idx) => Math.min(idx, agents.length - 1));
          } else {
            resizeDetailPanel(1);
          }
        }
        break;
      case "return": {
        if (panelFocus() === "agents") {
          activateFocusedAgent();
        } else {
          const focused = focusedSession();
          if (focused) switchToSession(focused);
        }
        break;
      }
      case "tab": {
        const list = filteredSessions();
        if (list.length === 0) break;
        const cur = currentSession();
        const idx = list.findIndex((s) => s.name === cur);
        const next = list[(idx + (key.shift ? list.length - 1 : 1)) % list.length];
        if (next) switchToSession(next.name);
        break;
      }
      case "r":
        send({ type: "refresh" });
        flash("refreshed");
        break;
      case "t":
        themeBeforePreview = theme();
        setModal("theme-picker");
        break;
      case "u":
        send({ type: "show-all-sessions" });
        break;
      case "f":
        cycleSessionFilter();
        break;
      case "d": {
        if (panelFocus() === "agents") {
          dismissFocusedAgent();
        } else {
          const focused = focusedSession();
          if (focused) send({ type: "hide-session", name: focused });
        }
        break;
      }
      case "x": {
        if (panelFocus() === "agents") {
          killFocusedAgentPane();
        } else {
          const focused = focusedSession();
          if (focused) {
            setKillTarget(focused);
            setModal("confirm-kill");
          }
        }
        break;
      }
      case "n":
      case "c":
        createNewSession();
        break;
      default: {
        if (key.number) {
          const idx = parseInt(key.name, 10) - 1;
          const target = filteredSessions()[idx];
          if (target) switchToSession(target.name);
        }
        break;
      }
    }
  }

  function handlePrintableBurstGuard(key: KeyEvent): boolean {
    if (!isPlainPrintableKey(key)) return false;

    if (printableShortcutTimer) {
      clearPendingPrintableShortcut();
      suppressPrintableShortcutsUntil = Date.now() + PRINTABLE_BURST_SUPPRESSION_MS;
      return true;
    }

    if (Date.now() < suppressPrintableShortcutsUntil) {
      suppressPrintableShortcutsUntil = Date.now() + PRINTABLE_BURST_SUPPRESSION_MS;
      return true;
    }

    if (!isBurstGuardedShortcut(key)) return false;

    pendingPrintableShortcut = cloneShortcutKey(key);
    printableShortcutTimer = setTimeout(() => {
      const pending = pendingPrintableShortcut;
      printableShortcutTimer = null;
      pendingPrintableShortcut = null;
      if (!pending || Date.now() < suppressPrintableShortcutsUntil) return;
      handleNormalKey(pending);
    }, PRINTABLE_SHORTCUT_DELAY_MS);

    return true;
  }

  useKeyboard((key) => {
    const currentModal = modal();

    // --- Theme picker modal: input handles all keys via onKeyDown ---
    if (currentModal === "theme-picker") {
      return;
    }

    // --- Confirm kill modal: Enter confirms, anything else cancels ---
    // Previously a single `y` keystroke confirmed, which made `tmux send-keys`
    // a one-shot kill-session attack vector once the modal was open. Requiring
    // Enter means injected printable text can no longer slip through.
    if (currentModal === "confirm-kill") {
      if (key.name === "return") {
        const target = killTarget();
        if (target) send({ type: "kill-session", name: target });
      }
      setKillTarget(null);
      setModal("none");
      return;
    }

    // --- Confirm quit modal: Enter confirms close-sidebar, anything else cancels ---
    if (currentModal === "confirm-quit") {
      if (key.name === "return") {
        // Local-only close: kills only THIS sidebar pane via the server's
        // close-sidebar handler. Other sessions' sidebars stay alive and the
        // server keeps running. Compare to the legacy `quit` path which tore
        // down every sidebar globally and called process.exit(0).
        send({ type: "close-sidebar" });
        // Tear down the local renderer so the pane closes promptly even if
        // the server reply lags. Don't fire-and-forget HTTP /quit anymore —
        // it triggers the destructive global path.
        setTimeout(() => renderer.destroy(), 250);
      }
      setModal("none");
      return;
    }

    // --- Defense against tmux send-keys injection ---
    // Prefer tmux's authoritative pane_active bit when available. Focus-event
    // reporting is still useful as a fallback, but tmux does not always deliver
    // a focus-in sequence when the user re-selects a sidebar pane after the TUI
    // programmatically refocused the main pane. Requiring pane_active=false to
    // ignore input keeps real selected-sidebar shortcuts working while dropping
    // `tmux send-keys` delivered to a non-active sidebar pane.
    const localPaneActive = getLocalPaneActive();
    if (localPaneActive === false) return;
    if (localPaneActive === null && !paneHasTerminalFocus()) return;

    if (handlePrintableBurstGuard(key)) return;
    handleNormalKey(key);
  });

  const runningCount = createMemo(() =>
    sessions.filter((s) => s.agentState?.status === "running" || s.agentState?.status === "tool-running").length,
  );

  const errorCount = createMemo(() =>
    sessions.filter((s) => s.agentState?.status === "error").length,
  );

  const unseenCount = createMemo(() =>
    sessions.filter((s) => s.unseen).length,
  );

  const isFocused = createSelector(focusedSession);

  return (
    <box flexDirection="column" flexGrow={1} backgroundColor={P().crust}>
      {/* Header */}
      <box flexDirection="column" paddingLeft={1} paddingTop={1} paddingBottom={0} flexShrink={0}>
        <text>
          <span style={{ fg: P().overlay1 }}>{"  "}</span>
          <span style={{ fg: P().subtext0, attributes: BOLD }}>Sessions</span>
          <span style={{ fg: P().overlay0 }}>{" "}{String(filteredSessions().length)}{sessionFilter() !== "all" ? `/${sessions.length}` : ""}</span>
          <Show when={sessionFilter() !== "all"}>
            <span style={{ fg: P().lavender, attributes: DIM }}>{" "}{"⏻ "}{FILTER_LABELS[sessionFilter()]}</span>
          </Show>
          {runningCount() > 0 ? <span style={{ fg: P().yellow }}>{" "}{"⚡"}{runningCount()}</span> : ""}
          <Show when={initializing()}><span style={{ fg: P().peach, attributes: DIM }}>{" "}{"◐◓◑◒"[spinIdx() % 4]} {initLabel() || "warming up…"}</span></Show>
          <Show when={flashMessage()}><span style={{ fg: P().overlay0, attributes: DIM }}>{" "}{flashMessage()}</span></Show>
          {errorCount() > 0 ? <span style={{ fg: P().red }}>{" "}{"✗"}{errorCount()}</span> : ""}
          {unseenCount() > 0 ? <span style={{ fg: P().teal }}>{" "}{"●"}{" "}{unseenCount()}</span> : ""}
        </text>
      </box>

      {/* Session list */}
      <scrollbox flexGrow={1} flexShrink={1} paddingTop={1}>
        <For each={filteredSessions()}>
          {(session, i) => (
            <SessionCard
              session={session}
              index={i() + 1}
              isFocused={isFocused(session.name)}
              isCurrent={session.name === currentSession()}
              spinIdx={spinIdx}
              theme={theme}
              statusColors={S}
              onSelect={() => {
                switchToSession(session.name);
              }}
            />
          )}
        </For>
      </scrollbox>

      {/* Local URLs for focused session — above detail panel */}
      <Show when={focusedData()?.localLinks?.length}>
        {(_) => {
          const linkRows = () => {
            const links = focusedData()?.localLinks ?? [];
            const availableWidth = Math.max(12, terminalWidth() - 10);
            return wrapLocalLinks(links, availableWidth);
          };
          return (
            <box flexDirection="column" flexShrink={0} paddingLeft={2}>
              <For each={linkRows()}>
                {(links, rowIndex) => (
                  <box flexDirection="row" paddingRight={1}>
                    <text flexShrink={0}>
                      <span style={{ fg: rowIndex() === 0 ? P().overlay0 : P().surface2, attributes: DIM }}>
                        {rowIndex() === 0 ? "local " : "      "}
                      </span>
                    </text>
                    <For each={links}>
                      {(link, linkIndex) => (
                        <box flexDirection="row" flexShrink={0}>
                          <text onMouseDown={() => {
                            Bun.spawn(["open", link.url], { stdout: "ignore", stderr: "ignore" });
                          }}>
                            <span style={{ fg: P().sky, attributes: BOLD }}>{localLinkText(link)}</span>
                          </text>
                          <Show when={linkIndex() < links.length - 1}>
                            <text>
                              <span style={{ fg: P().surface2 }}>{" · "}</span>
                            </text>
                          </Show>
                        </box>
                      )}
                    </For>
                  </box>
                )}
              </For>
            </box>
          );
        }}
      </Show>

      {/* Detail panel — focused session info, draggable height */}
      <Show when={focusedData()}>
        {(data) => (
          <scrollbox height={detailPanelHeight()} maxHeight={detailPanelHeight()} flexShrink={0}>
            <DetailPanel
              session={data()}
              theme={theme}
              statusColors={S}
              spinIdx={spinIdx}
              focusedAgentIdx={panelFocus() === "agents" ? focusedAgentIdx() : -1}
              onDismissAgent={(agent) => {
                send({
                  type: "dismiss-agent",
                  session: data().name,
                  agent: agent.agent,
                  threadId: agent.threadId,
                });
              }}
              onFocusAgentPane={(agent) => {
                appendFileSync("/tmp/opensessions-tui-agent-click.log",
                  `[${new Date().toISOString()}] sending focus-agent-pane session=${data().name} agent=${agent.agent} threadId=${agent.threadId} threadName=${agent.threadName}\n`);
                setCurrentSession(data().name);
                send({ type: "switch-session", name: data().name });
                send({
                  type: "focus-agent-pane",
                  session: data().name,
                  agent: agent.agent,
                  threadId: agent.threadId,
                  threadName: agent.threadName,
                });
              }}
              isResizeHover={isDetailResizeHover()}
              isResizing={isDetailResizing()}
              onResizeStart={beginDetailResize}
              onResizeDrag={handleDetailResizeDrag}
              onResizeEnd={endDetailResize}
              onResizeHoverChange={setIsDetailResizeHover}
            />
          </scrollbox>
        )}
      </Show>

      {/* Footer */}
      <box flexDirection="column" paddingLeft={1} paddingBottom={1} paddingTop={0} flexShrink={0}>
        <box height={1}><text style={{ fg: P().surface2 }}>{"─".repeat(200)}</text></box>
        <Show when={panelFocus() === "sessions"} fallback={
          <text>
            <span style={{ fg: P().overlay0 }}>{"←"}</span>
            <span style={{ fg: P().overlay1 }}>{" back  "}</span>
            <span style={{ fg: P().overlay0 }}>{"⏎"}</span>
            <span style={{ fg: P().overlay1 }}>{" focus  "}</span>
            <span style={{ fg: P().overlay0 }}>{"d"}</span>
            <span style={{ fg: P().overlay1 }}>{" dismiss  "}</span>
            <span style={{ fg: P().overlay0 }}>{"x"}</span>
            <span style={{ fg: P().overlay1 }}>{" kill"}</span>
          </text>
        }>
          <text>
            <span style={{ fg: P().overlay0 }}>{"⇥"}</span>
            <span style={{ fg: P().overlay1 }}>{" cycle  "}</span>
            <span style={{ fg: P().overlay0 }}>{"⏎"}</span>
            <span style={{ fg: P().overlay1 }}>{" go  "}</span>
            <span style={{ fg: P().overlay0 }}>{"→"}</span>
            <span style={{ fg: P().overlay1 }}>{" agents  "}</span>
            <span style={{ fg: P().overlay0 }}>{"f"}</span>
            <span style={{ fg: P().overlay1 }}>{" filter  "}</span>
            <span style={{ fg: P().overlay0 }}>{"d"}</span>
            <span style={{ fg: P().overlay1 }}>{" hide  "}</span>
            <span style={{ fg: P().overlay0 }}>{"x"}</span>
            <span style={{ fg: P().overlay1 }}>{" kill"}</span>
          </text>
        </Show>
      </box>

      {/* Theme picker overlay */}
      <Show when={modal() === "theme-picker"}>
        <ThemePicker
          palette={P}
          onSelect={(name) => {
            themeBeforePreview = null;
            applyTheme(name);
            setModal("none");
          }}
          onPreview={(name) => {
            previewTheme(name);
          }}
          onClose={() => {
            if (themeBeforePreview) {
              setTheme(themeBeforePreview);
              themeBeforePreview = null;
            }
            setModal("none");
          }}
        />
      </Show>

      {/* Kill confirmation overlay */}
      <Show when={modal() === "confirm-kill"}>
        <box
          position="absolute"
          top={0} left={0} right={0} bottom={0}
          justifyContent="center"
          alignItems="center"
          backgroundColor="transparent"
        >
          <box
            border
            borderStyle="rounded"
            borderColor={P().red}
            backgroundColor={P().mantle}
            padding={1}
            paddingX={2}
            flexDirection="column"
            alignItems="center"
          >
            <text>
              <span style={{ fg: P().red, attributes: BOLD }}>Kill session?</span>
            </text>
            <text>
              <span style={{ fg: P().text }}>{killTarget() ?? ""}</span>
            </text>
            <text>
              <span style={{ fg: P().overlay0 }}>enter</span>
              <span style={{ fg: P().overlay1 }}>{" / "}</span>
              <span style={{ fg: P().overlay0 }}>esc</span>
            </text>
          </box>
        </box>
      </Show>

      {/* Quit (close sidebar) confirmation overlay */}
      <Show when={modal() === "confirm-quit"}>
        <box
          position="absolute"
          top={0} left={0} right={0} bottom={0}
          justifyContent="center"
          alignItems="center"
          backgroundColor="transparent"
        >
          <box
            border
            borderStyle="rounded"
            borderColor={P().yellow}
            backgroundColor={P().mantle}
            padding={1}
            paddingX={2}
            flexDirection="column"
            alignItems="center"
          >
            <text>
              <span style={{ fg: P().yellow, attributes: BOLD }}>Close sidebar?</span>
            </text>
            <text>
              <span style={{ fg: P().overlay0 }}>enter</span>
              <span style={{ fg: P().overlay1 }}>{" / "}</span>
              <span style={{ fg: P().overlay0 }}>esc</span>
            </text>
          </box>
        </box>
      </Show>
    </box>
  );
}

// --- Theme Picker ---

interface ThemePickerProps {
  palette: Accessor<Theme["palette"]>;
  onSelect: (name: string) => void;
  onPreview: (name: string) => void;
  onClose: () => void;
}

function ThemePicker(props: ThemePickerProps) {
  let inputRef: InputRenderable;

  const [query, setQuery] = createSignal("");
  const [selected, setSelected] = createSignal(0);

  const filtered = createMemo(() => {
    const q = query().toLowerCase();
    if (!q) return THEME_NAMES;
    return THEME_NAMES.filter((name) => name.toLowerCase().includes(q));
  });

  function move(direction: -1 | 1) {
    const list = filtered();
    if (!list.length) return;
    let next = selected() + direction;
    if (next < 0) next = list.length - 1;
    if (next >= list.length) next = 0;
    setSelected(next);
    const name = list[next];
    if (name) props.onPreview(name);
  }

  function confirm() {
    const name = filtered()[selected()];
    if (name) props.onSelect(name);
  }

  function handleKeyDown(e: KeyEvent) {
    if (e.name === "up") {
      e.preventDefault();
      move(-1);
    } else if (e.name === "down") {
      e.preventDefault();
      move(1);
    } else if (e.name === "return") {
      e.preventDefault();
      confirm();
    } else if (e.name === "escape") {
      e.preventDefault();
      props.onClose();
    }
  }

  function handleInput(value: string) {
    setQuery(value);
    setSelected(0);
  }

  const MAX_VISIBLE = 12;

  const scrollOffset = createMemo(() => {
    const sel = selected();
    if (sel < MAX_VISIBLE) return 0;
    return sel - MAX_VISIBLE + 1;
  });

  const visibleItems = createMemo(() => {
    const list = filtered();
    return list.slice(scrollOffset(), scrollOffset() + MAX_VISIBLE);
  });

  return (
    <box
      position="absolute"
      top={0} left={0} right={0} bottom={0}
      justifyContent="center"
      alignItems="center"
      backgroundColor="transparent"
    >
      <box
        border
        borderStyle="rounded"
        borderColor={props.palette().blue}
        backgroundColor={props.palette().mantle}
        padding={1}
        flexDirection="column"
        width={30}
      >
        <text>
          <span style={{ fg: props.palette().blue, attributes: BOLD }}>Select Theme</span>
        </text>
        <box height={1}><text style={{ fg: props.palette().surface2 }}>{"─".repeat(200)}</text></box>
        <box border borderColor={props.palette().surface1} marginBottom={1}>
          <input
            ref={(r: InputRenderable) => { inputRef = r; inputRef.focus(); }}
            value={query()}
            onInput={handleInput}
            onKeyDown={handleKeyDown}
            placeholder="Search themes…"
            backgroundColor={props.palette().surface0}
            focusedBackgroundColor={props.palette().surface0}
            textColor={props.palette().text}
            cursorColor={props.palette().blue}
            placeholderColor={props.palette().overlay0}
          />
        </box>
        <Show when={filtered().length > 0} fallback={
          <box paddingLeft={1}><text style={{ fg: props.palette().overlay0 }}>No matches</text></box>
        }>
          <For each={visibleItems()}>
            {(name) => {
              const idx = createMemo(() => filtered().indexOf(name));
              const isSel = createMemo(() => idx() === selected());
              return (
                <box
                  paddingLeft={1}
                  paddingRight={1}
                  backgroundColor={isSel() ? props.palette().surface0 : undefined}
                >
                  <text style={{ fg: isSel() ? props.palette().text : props.palette().subtext0 }}>
                    {isSel() ? "▸ " : "  "}{name}
                  </text>
                </box>
              );
            }}
          </For>
          <Show when={filtered().length > MAX_VISIBLE}>
            <text style={{ fg: props.palette().overlay0, attributes: DIM }}>
              {"  "}↕ {filtered().length - MAX_VISIBLE} more
            </text>
          </Show>
        </Show>
        <box height={1}><text style={{ fg: props.palette().surface2 }}>{"─".repeat(200)}</text></box>
        <text style={{ fg: props.palette().overlay0 }}>
          <span style={{ attributes: DIM }}>↑↓</span>{" browse  "}
          <span style={{ attributes: DIM }}>⏎</span>{" select  "}
          <span style={{ attributes: DIM }}>esc</span>{" close"}
        </text>
      </box>
    </box>
  );
}

// --- Sparkline ---

function buildSparkline(timestamps: number[], width: number, windowMs: number = 30 * 60 * 1000): string {
  if (timestamps.length === 0 || width <= 0) return "";
  const now = Date.now();
  const start = now - windowMs;
  const bucketSize = windowMs / width;
  const buckets = new Array(width).fill(0);

  for (const ts of timestamps) {
    if (ts < start) continue;
    const idx = Math.min(width - 1, Math.floor((ts - start) / bucketSize));
    buckets[idx]++;
  }

  const max = Math.max(...buckets, 1);
  return buckets.map((count: number) => {
    const level = Math.round((count / max) * (SPARK_BLOCKS.length - 1));
    return SPARK_BLOCKS[level];
  }).join("");
}

// --- Detail Panel ---

interface DetailPanelProps {
  session: SessionData;
  theme: Accessor<Theme>;
  statusColors: Accessor<Theme["status"]>;
  spinIdx: Accessor<number>;
  focusedAgentIdx: number;
  onDismissAgent: (agent: SessionData["agents"][number]) => void;
  onFocusAgentPane: (agent: SessionData["agents"][number]) => void;
  isResizeHover: boolean;
  isResizing: boolean;
  onResizeStart: (event: MouseEvent) => void;
  onResizeDrag: (event: MouseEvent) => void;
  onResizeEnd: (event?: MouseEvent) => void;
  onResizeHoverChange: (hovered: boolean) => void;
}

function DetailPanel(props: DetailPanelProps) {
  const P = () => props.theme().palette;

  const agents = () => props.session.agents ?? [];
  const hasAgents = () => agents().length > 0;
  const meta = () => props.session.metadata;
  const hasMeta = () => !!meta();
  const visibleLogs = () => {
    const m = meta();
    if (!m || m.logs.length === 0) return [];
    return m.logs.slice(-8);
  };

  const truncDir = () => {
    const d = props.session.dir;
    if (!d) return "";
    const home = process.env.HOME ?? "";
    const short = home && d.startsWith(home) ? "~" + d.slice(home.length) : d;
    return short.length > 24 ? "…" + short.slice(short.length - 23) : short;
  };

  return (
    <box flexDirection="column" flexShrink={0} paddingLeft={1}>
      <box height={1}>
        <text
          selectable={false}
          onMouseDown={(event) => {
            logResizeDebug("separator:onMouseDown", { x: event.x, y: event.y, button: event.button, session: props.session.name });
            event.preventDefault();
            props.onResizeStart(event);
          }}
          onMouseDrag={(event) => {
            logResizeDebug("separator:onMouseDrag", { x: event.x, y: event.y, button: event.button, session: props.session.name });
            event.preventDefault();
            props.onResizeDrag(event);
          }}
          onMouseDragEnd={(event) => {
            logResizeDebug("separator:onMouseDragEnd", { x: event.x, y: event.y, button: event.button, session: props.session.name });
            event.preventDefault();
            props.onResizeEnd(event);
          }}
          onMouseUp={(event) => {
            logResizeDebug("separator:onMouseUp", { x: event.x, y: event.y, button: event.button, session: props.session.name });
            event.preventDefault();
            props.onResizeEnd(event);
          }}
          onMouseOver={() => props.onResizeHoverChange(true)}
          onMouseOut={() => {
            if (!props.isResizing) props.onResizeHoverChange(false);
          }}
          style={{
            fg: props.isResizing
              ? P().blue
              : props.isResizeHover
                ? P().overlay1
                : P().surface2,
          }}
        >
          {"─".repeat(200)}
        </text>
      </box>

      {/* Directory */}
      <text truncate>
        <span style={{ fg: P().overlay0, attributes: DIM }}>{truncDir()}</span>
      </text>

      {/* Agent instances */}
      <Show when={hasAgents()}>
        <For each={agents()}>
          {(agent, i) => (
            <AgentListItem
              agent={agent}
              palette={P}
              statusColors={props.statusColors}
              spinIdx={props.spinIdx}
              isKeyboardFocused={i() === props.focusedAgentIdx}
              onDismiss={() => props.onDismissAgent(agent)}
              onFocusPane={() => props.onFocusAgentPane(agent)}
            />
          )}
        </For>
      </Show>

      {/* Metadata: status, progress, logs */}
      <Show when={hasMeta()}>
        {(_) => {
          const m = meta()!;
          const progressText = () => {
            const p = m.progress;
            if (!p) return "";
            if (p.current != null && p.total != null) return `${p.current}/${p.total}`;
            if (p.percent != null) return `${Math.round(p.percent * 100)}%`;
            return "";
          };
          return (
            <box flexDirection="column">
              <box height={1} />

              {/* Status + progress on one line */}
              <Show when={m.status || m.progress}>
                <box flexDirection="row" paddingRight={1}>
                  <Show when={m.status}>
                    <text truncate flexGrow={1}>
                      <span style={{ fg: toneColor(m.status!.tone, P()) }}>{TONE_ICONS[m.status!.tone ?? "neutral"]} {m.status!.text}</span>
                    </text>
                  </Show>
                  <Show when={m.progress}>
                    <text flexShrink={0}>
                      <span style={{ fg: P().sky }}>
                        {m.status ? " · " : ""}{progressText()}{m.progress!.label ? ` ${m.progress!.label}` : ""}
                      </span>
                    </text>
                  </Show>
                </box>
              </Show>

              {/* Log entries */}
              <Show when={visibleLogs().length > 0}>
                <For each={visibleLogs()}>
                  {(entry) => (
                    <text truncate>
                      <span style={{ fg: toneColor(entry.tone, P()), attributes: DIM }}>
                        {TONE_ICONS[entry.tone ?? "neutral"]}
                      </span>
                      <Show when={entry.source}>
                        <span style={{ fg: P().surface2, attributes: DIM }}>{` [${entry.source}]`}</span>
                      </Show>
                      <span style={{ fg: P().overlay0 }}>{" "}{entry.message}</span>
                    </text>
                  )}
                </For>
              </Show>
            </box>
          );
        }}
      </Show>
    </box>
  );
}

interface AgentListItemProps {
  agent: SessionData["agents"][number];
  palette: Accessor<Theme["palette"]>;
  statusColors: Accessor<Theme["status"]>;
  spinIdx: Accessor<number>;
  isKeyboardFocused: boolean;
  onDismiss: () => void;
  onFocusPane: () => void;
}

function AgentListItem(props: AgentListItemProps) {
  const P = () => props.palette();
  const SC = () => props.statusColors();
  const [isDismissHover, setIsDismissHover] = createSignal(false);
  const [isFlash, setIsFlash] = createSignal(false);

  const isTerminal = () =>
    TERMINAL_STATUSES.has(props.agent.status) && props.agent.liveness !== "alive";
  const isUnseen = () => isTerminal() && props.agent.unseen === true;

  const icon = () => {
    if (isUnseen()) return UNSEEN_ICON;
    if (isTerminal()) return props.agent.status === "done" ? "✓" : props.agent.status === "error" ? "✗" : "⚠";
    if (props.agent.status === "tool-running") return "⚙";
    if (props.agent.status === "running") return SPINNERS[props.spinIdx() % SPINNERS.length]!;
    if (props.agent.status === "waiting") return "◉";
    return "○";
  };

  const color = () => {
    if (isTerminal()) {
      if (props.agent.status === "error") return P().red;
      if (props.agent.status === "stale") return P().yellow;
      if (props.agent.status === "interrupted") return P().peach;
      return isUnseen() ? P().teal : P().green;
    }
    return SC()[props.agent.status];
  };

  const statusText = () => {
    if (props.agent.status === "tool-running") return "tools";
    if (props.agent.status === "running") return "running";
    // Alive + done = idle at prompt, not finished
    if (props.agent.status === "done" && props.agent.liveness === "alive") return "idle";
    if (props.agent.status === "done") return "done";
    if (props.agent.status === "error") return "error";
    if (props.agent.status === "stale") return "stale";
    if (props.agent.status === "interrupted" && props.agent.liveness === "alive") return "idle";
    if (props.agent.status === "interrupted") return "stopped";
    if (props.agent.status === "waiting") return "waiting";
    return "";
  };

  const triggerFlash = () => {
    setIsFlash(true);
    setTimeout(() => setIsFlash(false), 150);
  };

  const bgColor = () => {
    if (isFlash()) return P().surface1;
    if (props.isKeyboardFocused) return P().surface0;
    return "transparent";
  };

  return (
    <box flexDirection="column" flexShrink={0} onMouseDown={(event) => {
      // Don't trigger focus if clicking the dismiss button
      if ((event.target as any)?.id === "dismiss") return;
      appendFileSync("/tmp/opensessions-tui-agent-click.log",
        `[${new Date().toISOString()}] clicked agent=${props.agent.agent} thread=${props.agent.threadName ?? "?"}\n`);
      triggerFlash();
      props.onFocusPane();
    }}>
      <box height={1} />
      <box
        flexDirection="row"
        backgroundColor={bgColor()}
        paddingLeft={1}
      >
        {/* Content column — name row + thread name row */}
        <box flexDirection="column" flexGrow={1} paddingRight={1}>
          {/* Row 1: icon + agent name + status + dismiss */}
          <box flexDirection="row">
            <text flexGrow={1} truncate>
              <span style={{ fg: color() }}>{icon()}</span>
              <span style={{ fg: props.isKeyboardFocused ? P().text : P().subtext1, attributes: props.isKeyboardFocused ? BOLD : undefined }}>{" "}{props.agent.agent}</span>
            </text>
            <Show when={!isTerminal() || !isUnseen()}>
              <text flexShrink={0}>
                <span style={{ fg: color(), attributes: DIM }}>{statusText()}</span>
              </text>
            </Show>
            <text
              flexShrink={0}
              onMouseDown={(event) => {
                event.preventDefault();
                event.stopPropagation();
                props.onDismiss();
              }}
              onMouseOver={() => setIsDismissHover(true)}
              onMouseOut={() => setIsDismissHover(false)}
            >
              <span style={{ fg: isDismissHover() ? P().red : P().overlay0 }}>{" ✕"}</span>
            </text>
          </box>

          {/* Row 2: thread name */}
          <Show when={props.agent.threadName}>
            <text truncate>
              <span style={{ fg: isUnseen() ? color() : P().overlay0 }}>{props.agent.threadName}</span>
            </text>
          </Show>
        </box>
      </box>
    </box>
  );
}

// --- Session Card ---

interface SessionCardProps {
  session: SessionData;
  index: number;
  isFocused: boolean;
  isCurrent: boolean;
  spinIdx: Accessor<number>;
  theme: Accessor<Theme>;
  statusColors: Accessor<Theme["status"]>;
  onSelect: () => void;
}

function SessionCard(props: SessionCardProps) {
  const P = () => props.theme().palette;
  const SC = () => props.statusColors();

  const status = () => props.session.agentState?.status ?? "idle";
  const unseen = () => props.session.unseen;

  const isUnseenTerminal = () =>
    unseen() && TERMINAL_STATUSES.has(status());

  const accentColor = () => {
    if (props.isCurrent) return P().green;
    if (isUnseenTerminal()) return unseenAccentColor();
    const s = status();
    if (s === "error") return P().red;
    if (s === "stale") return P().yellow;
    if (s === "interrupted") return P().peach;
    if (s === "tool-running" || s === "waiting") return SC()[s];
    if (s === "running") return P().yellow;
    if (props.isFocused) return P().lavender;
    return "transparent";
  };

  const unseenAccentColor = () => {
    const s = status();
    if (s === "error") return P().red;
    if (s === "stale") return P().yellow;
    if (s === "interrupted") return P().peach;
    return P().teal;
  };

  const statusIcon = () => {
    const s = status();
    if (isUnseenTerminal()) return UNSEEN_ICON;
    if (s === "done") return "✓";
    if (s === "error") return "✗";
    if (s === "stale" || s === "interrupted") return "⚠";
    if (s === "tool-running") return "⚙";
    if (s === "running") return SPINNERS[props.spinIdx() % SPINNERS.length]!;
    if (s === "waiting") return "◉";
    return "";
  };

  const statusColor = () => {
    if (isUnseenTerminal()) return unseenAccentColor();
    return SC()[status()];
  };

  const nameColor = () => {
    if (props.isFocused) return P().text;
    if (props.isCurrent) return P().subtext1;
    return P().subtext0;
  };

  const indexColor = () => {
    if (props.isFocused) return P().subtext0;
    return P().surface2;
  };

  const truncName = () => props.session.name;

  const truncBranch = () => props.session.branch ?? "";

  const dirName = () => {
    return sessionDirLabel(props.session.dir, props.session.name);
  };

  const portHint = () => {
    const ports = props.session.ports ?? [];
    if (ports.length === 0) return "";
    if (ports.length === 1) return `  ⌁${ports[0]}`;
    return `  ⌁${ports[0]}+${ports.length - 1}`;
  };

  const metaSummary = () => {
    const meta = props.session.metadata;
    if (!meta) return "";
    const parts: string[] = [];
    if (meta.status) parts.push(meta.status.text);
    if (meta.progress) {
      if (meta.progress.current != null && meta.progress.total != null) {
        parts.push(`${meta.progress.current}/${meta.progress.total}`);
      } else if (meta.progress.percent != null) {
        parts.push(`${Math.round(meta.progress.percent * 100)}%`);
      }
      if (meta.progress.label) parts.push(meta.progress.label);
    }
    return parts.join(" · ");
  };

  const metaTone = () => props.session.metadata?.status?.tone;

  const bgColor = () => {
    if (props.isFocused) return P().surface1;
    return "transparent";
  };

  return (
    <box flexDirection="column" flexShrink={0}>
      <box
        flexDirection="row"
        backgroundColor={bgColor()}
        onMouseDown={props.onSelect}
        paddingLeft={1}
      >
        {/* Left accent — space-preserving, only colored for meaningful states */}
        <text style={{ fg: accentColor() }}>{accentColor() === "transparent" ? " " : "▌"}</text>

        {/* Index */}
        <box width={3} flexShrink={0}>
          <text style={{ fg: indexColor() }}>{String(props.index).padStart(2)}</text>
        </box>

        {/* Content */}
        <box flexDirection="column" flexGrow={1} flexShrink={1} overflow="hidden" paddingRight={1}>
          {/* Row 1: name + status icons (right) */}
          <box flexDirection="row">
            <text truncate wrapMode="none" flexGrow={1} fg={nameColor()}>
              <span style={{ fg: nameColor(), attributes: props.isFocused || props.isCurrent ? BOLD : undefined }}>
                {truncName()}
              </span>
            </text>
            <box flexGrow={1} />
            <Show when={statusIcon()}>
              <text flexShrink={0}>
                <span style={{ fg: statusColor() }}>{" "}{statusIcon()}</span>
              </text>
            </Show>
          </box>

          {/* Row 2: repo/dir name (click to open in Finder when focused) */}
          <Show when={dirName()}>
            <text truncate wrapMode="none" fg={props.isFocused ? P().teal : P().overlay1}
              onMouseDown={() => {
                if (!props.isFocused) return;
                const d = props.session.dir;
                if (d) Bun.spawnSync(["open", d]);
              }}>
              <span style={{ fg: props.isFocused ? P().teal : P().overlay1 }}>
                {dirName()}
              </span>
            </text>
          </Show>

          {/* Row 3: branch + port hint */}
          <Show when={props.session.branch || portHint()}>
            <text truncate wrapMode="none" fg={props.isFocused ? P().pink : P().overlay0}>
              <span style={{ fg: props.isFocused ? P().pink : P().overlay0 }}>
                {truncBranch()}
              </span>
              <Show when={portHint()}>
                <span style={{ fg: props.isFocused ? P().sky : P().overlay0 }}>
                  {portHint()}
                </span>
              </Show>
            </text>
          </Show>

          {/* Row 3: metadata summary (status + progress) */}
          <Show when={metaSummary()}>
            <text truncate>
              <span style={{ fg: toneColor(metaTone(), P()), attributes: DIM }}>{metaSummary()}</span>
            </text>
          </Show>
        </box>
      </box>

      {/* Breathing room — 1 empty line between cards */}
      <box height={1} />
    </box>
  );
}

async function main() {
  await ensureServer();
  render(() => <App />, {
    exitOnCtrlC: true,
    targetFPS: 30,
    useMouse: true,
  });
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
