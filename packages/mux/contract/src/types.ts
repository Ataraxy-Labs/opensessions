// ─── Specification version ───────────────────────────────────────────────────
// Like ai-sdk's specificationVersion — a literal discriminant for version compat.

export type MuxSpecificationVersion = "v1";

// ─── Core data types ─────────────────────────────────────────────────────────

export interface MuxSessionInfo {
  readonly name: string;
  readonly createdAt: number;
  readonly dir: string;
  readonly windows: number;
}

export interface ActiveWindow {
  readonly id: string;
  readonly sessionName: string;
  readonly active: boolean;
}

export interface SidebarPane {
  readonly paneId: string;
  readonly sessionName: string;
  readonly windowId: string;
  readonly width?: number;
  readonly windowWidth?: number;
}

/** Position for sidebar placement */
export type SidebarPosition = "left" | "right";

/** Provider-specific metadata (escape hatch — like ai-sdk's providerMetadata) */
export type MuxProviderMetadata = Record<string, Record<string, unknown>>;

// ─── Capability interfaces ───────────────────────────────────────────────────
// Split from one monolith into composable traits. Providers implement what they
// support. The server narrows with type guards, not NonNullable hacks.

/**
 * Core mux operations — every provider MUST implement this.
 *
 * Like ai-sdk's ProviderV4 required methods (languageModel, embeddingModel).
 */
export interface MuxProviderV1 {
  readonly specificationVersion: "v1";
  readonly name: string;

  // Session CRUD
  listSessions(): MuxSessionInfo[];
  switchSession(name: string, clientTty?: string): void;
  getCurrentSession(): string | null;
  getSessionDir(name: string): string;
  getPaneCount(name: string): number;
  getClientTty(): string;
  createSession(name?: string, dir?: string): void;
  killSession(name: string): void;

  // Hooks
  // tokenFile (optional): absolute path to the per-instance auth-token
  // file; providers that wire up out-of-band callbacks (e.g. tmux hooks
  // running curl) must include this token on each request so the
  // server doesn't reject them.
  setupHooks(serverHost: string, serverPort: number, tokenFile?: string): void;
  cleanupHooks(): void;
}

/**
 * Window/tab awareness — providers that can enumerate their windows/tabs.
 */
export interface WindowCapable {
  listActiveWindows(): ActiveWindow[];
  getCurrentWindowId(): string | null;
}

/**
 * Sidebar management — providers that can spawn/manage sidebar panes.
 */
export interface SidebarCapable {
  listSidebarPanes(sessionName?: string): SidebarPane[];
  spawnSidebar(
    sessionName: string,
    windowId: string,
    width: number,
    position: SidebarPosition,
    scriptsDir: string,
  ): string | null;
  hideSidebar(paneId: string): void;
  killSidebarPane(paneId: string): void;
  resizeSidebarPane(paneId: string, width: number): void;
  /** Kill sidebar panes that are the only pane left in their window (orphaned). */
  killOrphanedSidebarPanes(): void;
  /**
   * Optional counterpart to `killOrphanedSidebarPanes` for the
   * `spawn-shell` lonely-sidebar policy. Instead of killing a sidebar
   * that's the only pane left in its window, spawn a fresh shell pane
   * next to it so the window stays alive. Providers that don't implement
   * this fall back to the kill behaviour.
   *
   * Implementations must be idempotent (the hook can fire multiple times
   * for one logical close) and must not spawn a shell next to a sidebar
   * that already has neighbours.
   */
  protectOrphanedSidebars?(): void;
  /**
   * Kill stale sidebar panes — panes that retain the sidebar title (e.g. restored
   * by tmux-resurrect/continuum after a system restart) but no longer have the
   * TUI process running. Should be called once at server startup.
   */
  killStaleSidebarPanes(): void;
  cleanupSidebar(): void;
}

/**
 * Batch operations — providers that can fetch data in bulk for performance.
 */
export interface BatchCapable {
  getAllPaneCounts(): Map<string, number>;
}

// ─── Composite types ─────────────────────────────────────────────────────────

/**
 * A fully-featured provider with all capabilities.
 * Most providers won't implement everything — use the type guards below to narrow.
 */
export type FullMuxProvider = MuxProviderV1 & WindowCapable & SidebarCapable & BatchCapable;

/**
 * The union type the server accepts — core is required, capabilities are optional.
 *
 * Like ai-sdk's LanguageModel = V2 | V3 | V4 — accepts any level of capability.
 */
export type MuxProvider = MuxProviderV1 & Partial<WindowCapable & SidebarCapable & BatchCapable>;

// ─── Type guards ─────────────────────────────────────────────────────────────
// Runtime narrowing — like ai-sdk's isInstance() pattern, but for capabilities.

/** Check if a provider supports window operations */
export function isWindowCapable(p: MuxProvider): p is MuxProviderV1 & WindowCapable {
  return typeof p.listActiveWindows === "function" && typeof p.getCurrentWindowId === "function";
}

/** Check if a provider supports sidebar operations */
export function isSidebarCapable(p: MuxProvider): p is MuxProviderV1 & SidebarCapable {
  return (
    typeof p.listSidebarPanes === "function" &&
    typeof p.spawnSidebar === "function" &&
    typeof p.hideSidebar === "function" &&
    typeof p.killSidebarPane === "function" &&
    typeof p.resizeSidebarPane === "function" &&
    typeof p.killOrphanedSidebarPanes === "function" &&
    typeof p.killStaleSidebarPanes === "function" &&
    typeof p.cleanupSidebar === "function"
  );
}

/** Check if a provider supports batch operations */
export function isBatchCapable(p: MuxProvider): p is MuxProviderV1 & BatchCapable {
  return typeof p.getAllPaneCounts === "function";
}

/** Check if a provider supports full sidebar management (window + sidebar) */
export function isFullSidebarCapable(
  p: MuxProvider,
): p is MuxProviderV1 & WindowCapable & SidebarCapable {
  return isWindowCapable(p) && isSidebarCapable(p);
}

// ─── Provider settings ───────────────────────────────────────────────────────
// Like ai-sdk's OpenAIProviderSettings — each provider can extend this.

export interface MuxProviderSettings {
  /** Override the provider name (for custom/wrapped providers) */
  name?: string;
}
