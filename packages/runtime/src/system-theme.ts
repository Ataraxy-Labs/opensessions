/**
 * macOS system-appearance helpers.
 *
 * On macOS, the global "Appearance" preference (System Settings → Appearance)
 * flips between Light and Dark. We expose three helpers:
 *   - `readMacSystemAppearance()` reads the current setting via `defaults`.
 *   - `themeForSystemMode()` maps a mode + configured theme names to the
 *     theme the server should apply.
 *   - `watchMacSystemAppearance()` invokes a callback on every detected
 *     appearance change. Push-based via kqueue file watch on the underlying
 *     plist; falls back to a slow safety poll for atomic-rename cases.
 */

import { watch } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

export type SystemAppearanceMode = "dark" | "light";

/**
 * Read the current macOS Appearance setting.
 *
 * `defaults read -g AppleInterfaceStyle` returns "Dark" when Dark mode is
 * active and exits non-zero with an empty stdout when Light is active
 * (the key is simply absent). We map both absent/unreadable cases to "light".
 *
 * Safe to call on non-macOS platforms — returns "light" and does not throw.
 */
export async function readMacSystemAppearance(): Promise<SystemAppearanceMode> {
  if (process.platform !== "darwin") return "light";
  try {
    const proc = Bun.spawn(["defaults", "read", "-g", "AppleInterfaceStyle"], {
      stdout: "pipe",
      stderr: "pipe",
    });
    const out = (await new Response(proc.stdout).text()).trim();
    return out === "Dark" ? "dark" : "light";
  } catch {
    return "light";
  }
}

/**
 * Map a detected system appearance to the theme name the server should set.
 * Pure — trivially testable.
 */
export function themeForSystemMode(
  mode: SystemAppearanceMode,
  darkTheme: string,
  lightTheme: string,
): string {
  return mode === "dark" ? darkTheme : lightTheme;
}

export interface SystemAppearanceWatcher {
  stop(): void;
}

/**
 * Watch the macOS Appearance setting and fire `onChange` when it flips.
 *
 * macOS rewrites `~/Library/Preferences/.GlobalPreferences.plist` whenever
 * any global preference (including AppleInterfaceStyle) changes. We watch
 * that file with kqueue (zero-overhead push) and re-read appearance on
 * every event. Most events are unrelated to appearance (e.g. other prefs
 * being written) so we suppress the callback unless the *value* actually
 * changed.
 *
 * A 60s safety poll covers the rare case where the plist is replaced via
 * atomic rename — kqueue loses the inode and the watcher goes silent.
 *
 * On non-darwin platforms returns a no-op watcher.
 */
export function watchMacSystemAppearance(
  onChange: (mode: SystemAppearanceMode) => void | Promise<void>,
  opts?: { safetyPollMs?: number },
): SystemAppearanceWatcher {
  if (process.platform !== "darwin") {
    return { stop() {} };
  }

  const plistPath = join(homedir(), "Library", "Preferences", ".GlobalPreferences.plist");
  let lastMode: SystemAppearanceMode | null = null;
  let stopped = false;

  async function check() {
    if (stopped) return;
    // All three call sites invoke this as `void check()`, so any rejection
    // (most plausibly from the consumer's onChange callback) would surface as
    // an unhandled promise rejection. The appearance watch is best-effort —
    // swallow so a failing callback can't take down the process.
    try {
      const mode = await readMacSystemAppearance();
      if (mode !== lastMode) {
        lastMode = mode;
        await onChange(mode);
      }
    } catch {
      // ignore — next file-watch event or safety poll will retry
    }
  }

  let watcher: ReturnType<typeof watch> | null = null;
  try {
    watcher = watch(plistPath, () => { void check(); });
  } catch {
    // fall through — safety poll alone keeps us correct
  }

  const safetyMs = opts?.safetyPollMs ?? 60_000;
  const safetyTimer = setInterval(() => { void check(); }, safetyMs);

  // Initial read so the consumer learns the starting mode without waiting.
  void check();

  return {
    stop() {
      stopped = true;
      try { watcher?.close(); } catch {}
      clearInterval(safetyTimer);
    },
  };
}
