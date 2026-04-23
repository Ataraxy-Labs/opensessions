import { describe, test, expect } from "bun:test";

import { readMacSystemAppearance, themeForSystemMode } from "../src/system-theme";

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
