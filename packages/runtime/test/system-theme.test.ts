import { describe, test, expect } from "bun:test";

import {
  readMacSystemAppearance,
  themeForSystemMode,
  watchMacSystemAppearance,
} from "../src/system-theme";

describe("themeForSystemMode", () => {
  test("dark mode → dark theme", () => {
    expect(themeForSystemMode("dark", "catppuccin-mocha", "catppuccin-latte"))
      .toBe("catppuccin-mocha");
  });

  test("light mode → light theme", () => {
    expect(themeForSystemMode("light", "catppuccin-mocha", "catppuccin-latte"))
      .toBe("catppuccin-latte");
  });

  test("respects custom theme names", () => {
    expect(themeForSystemMode("dark", "tokyo-night", "github-light")).toBe("tokyo-night");
    expect(themeForSystemMode("light", "tokyo-night", "github-light")).toBe("github-light");
  });
});

describe("readMacSystemAppearance", () => {
  test("returns 'light' on non-darwin without throwing", async () => {
    // The helper short-circuits on non-darwin platforms, so this is a
    // portable sanity check. On darwin it will read the real setting.
    const result = await readMacSystemAppearance();
    expect(["dark", "light"]).toContain(result);
  });

  test("on non-darwin platforms returns 'light' deterministically", async () => {
    const original = process.platform;
    Object.defineProperty(process, "platform", { value: "linux", configurable: true });
    try {
      expect(await readMacSystemAppearance()).toBe("light");
    } finally {
      Object.defineProperty(process, "platform", { value: original, configurable: true });
    }
  });
});

describe("watchMacSystemAppearance", () => {
  test("returns a no-op watcher on non-darwin", () => {
    const original = process.platform;
    Object.defineProperty(process, "platform", { value: "linux", configurable: true });
    try {
      let calls = 0;
      const w = watchMacSystemAppearance(() => { calls++; });
      expect(typeof w.stop).toBe("function");
      w.stop();
      expect(calls).toBe(0);
    } finally {
      Object.defineProperty(process, "platform", { value: original, configurable: true });
    }
  });

  test("stop() is idempotent on non-darwin", () => {
    const original = process.platform;
    Object.defineProperty(process, "platform", { value: "linux", configurable: true });
    try {
      const w = watchMacSystemAppearance(() => {});
      w.stop();
      w.stop();
    } finally {
      Object.defineProperty(process, "platform", { value: original, configurable: true });
    }
  });

  test("on darwin, fires callback with the initial mode", async () => {
    if (process.platform !== "darwin") return;
    let received: "dark" | "light" | null = null;
    const w = watchMacSystemAppearance((mode) => { received = mode; }, { safetyPollMs: 60_000 });
    // Initial check is queued via void check() — give it a tick to land.
    await new Promise((r) => setTimeout(r, 100));
    w.stop();
    expect(received === "dark" || received === "light").toBe(true);
  });
});
