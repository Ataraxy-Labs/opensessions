/**
 * macOS system-appearance helpers.
 *
 * On macOS, the global "Appearance" preference (System Settings → Appearance)
 * flips between Light and Dark. We expose two helpers:
 *   - `readMacSystemAppearance()` reads the current setting via `defaults`.
 *   - `themeForSystemMode()` maps a mode + configured theme names to the
 *     theme the server should apply.
 *
 * The pair is enough for a simple polling loop in the server. macOS does not
 * expose a CLI change-notification, so polling every few seconds is the
 * pragmatic approach; the calls are cheap (one `defaults` subprocess).
 */

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
