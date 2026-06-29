import { existsSync, readFileSync, writeFileSync, mkdirSync } from "fs";
import { join } from "path";

import type { PartialTheme } from "./themes";

/** Session filter mode for the TUI sidebar */
export type SessionFilterMode = "all" | "active" | "running";

export interface OpensessionsConfig {
  /** Explicit mux provider name (overrides auto-detect) */
  mux?: string;
  /** Custom server port */
  port?: number;
  /** Community plugin package names to load (e.g. ["opensessions-mux-zellij"]) */
  plugins: string[];
  /** Theme: builtin name (e.g. "catppuccin-latte") or partial inline theme object */
  theme?: string | PartialTheme;
  /** Sidebar column width (default 26) */
  sidebarWidth?: number;
  /** Sidebar position relative to the terminal window (default "left") */
  sidebarPosition?: "left" | "right";
  /** Tmux prefix key for sidebar toggle (default "s") */
  keybinding?: string;
  /** Persisted detail panel heights keyed by mux session name */
  detailPanelHeights?: Record<string, number>;
  /** Default session filter: "all" (default), "active" (any agent), "running" (running agents only) */
  sessionFilter?: SessionFilterMode;
  /** Automatically stop idle live agent processes while keeping their session metadata visible. */
  autoHibernate?: AutoHibernateConfig;
  /**
   * What to do when a sidebar pane is the only pane left in its window
   * (e.g. the user closed their last shell). Default "kill" matches tmux's
   * native "closing the last pane closes the window" feel. "spawn-shell"
   * inserts a fresh shell pane next to the sidebar so the window stays
   * alive — useful if you treat the sidebar as window chrome and want a
   * recovery path after accidentally exiting your shell.
   */
  lonelySidebarPolicy?: LonelySidebarPolicy;
}

/**
 * Policy for what happens when a sidebar pane becomes the only pane in a
 * window. See `OpensessionsConfig.lonelySidebarPolicy`.
 */
export type LonelySidebarPolicy = "kill" | "spawn-shell";

export interface AutoHibernateConfig {
  /** Defaults to true. Set false to disable idle agent hibernation. */
  enabled?: boolean;
  /** Idle age in milliseconds before an alive idle/terminal agent is hibernated. Defaults to 6 hours. */
  idleAfterMs?: number;
}

export const DEFAULT_AUTO_HIBERNATE_IDLE_AFTER_MS = 6 * 60 * 60 * 1000;

export function resolveAutoHibernateConfig(value: unknown): Required<AutoHibernateConfig> {
  if (!value || typeof value !== "object") {
    return { enabled: true, idleAfterMs: DEFAULT_AUTO_HIBERNATE_IDLE_AFTER_MS };
  }
  const raw = value as AutoHibernateConfig;
  const idleAfterMs = typeof raw.idleAfterMs === "number" && Number.isFinite(raw.idleAfterMs) && raw.idleAfterMs > 0
    ? raw.idleAfterMs
    : DEFAULT_AUTO_HIBERNATE_IDLE_AFTER_MS;
  return { enabled: raw.enabled !== false, idleAfterMs };
}

export const DEFAULT_LONELY_SIDEBAR_POLICY: LonelySidebarPolicy = "kill";

/**
 * Parses an arbitrary value into a valid LonelySidebarPolicy, falling back
 * to the default for null/undefined/unknown values. Centralised so the
 * server, tests, and any future CLI override path stay consistent.
 */
export function resolveLonelySidebarPolicy(value: unknown): LonelySidebarPolicy {
  if (value === "kill" || value === "spawn-shell") return value;
  return DEFAULT_LONELY_SIDEBAR_POLICY;
}

const DEFAULTS: OpensessionsConfig = {
  plugins: [],
};

/**
 * Load config from ~/.config/opensessions/config.json
 * @param homeDir — override home directory (for testing)
 */
export function loadConfig(homeDir?: string): OpensessionsConfig {
  const home = homeDir ?? process.env.HOME ?? process.env.USERPROFILE ?? "";
  const configPath = join(home, ".config", "opensessions", "config.json");

  if (!existsSync(configPath)) {
    return { ...DEFAULTS };
  }

  try {
    const raw = readFileSync(configPath, "utf-8");
    const parsed = JSON.parse(raw) as Partial<OpensessionsConfig>;
    return {
      ...DEFAULTS,
      ...parsed,
      plugins: parsed.plugins ?? DEFAULTS.plugins,
    };
  } catch {
    return { ...DEFAULTS };
  }
}

/**
 * Save partial config updates to ~/.config/opensessions/config.json
 * Merges with existing config on disk to preserve fields.
 * @param updates — partial config fields to write
 * @param homeDir — override home directory (for testing)
 */
export function saveConfig(updates: Partial<OpensessionsConfig>, homeDir?: string): void {
  const home = homeDir ?? process.env.HOME ?? process.env.USERPROFILE ?? "";
  const configDir = join(home, ".config", "opensessions");
  const configPath = join(configDir, "config.json");

  const existing = loadConfig(homeDir);
  const merged = { ...existing, ...updates };

  mkdirSync(configDir, { recursive: true });
  writeFileSync(configPath, JSON.stringify(merged, null, 2) + "\n");
}
