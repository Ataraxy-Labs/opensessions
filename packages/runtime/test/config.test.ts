import { describe, test, expect, beforeEach } from "bun:test";
import { loadConfig, resolveAutoHibernateConfig, type OpensessionsConfig } from "../src/config";
import { resolveTheme, BUILTIN_THEMES, type Theme } from "../src/themes";
import { resolve, join } from "path";

describe("Config", () => {
  test("loadConfig returns defaults when no config file exists", () => {
    const config = loadConfig("/tmp/nonexistent-dir-" + Date.now());
    expect(config.mux).toBeUndefined();
    expect(config.port).toBeUndefined();
    expect(config.plugins).toEqual([]);
    expect(config.theme).toBeUndefined();
  });

  test("loadConfig reads sidebar settings", async () => {
    const tmpDir = `/tmp/opensessions-test-${Date.now()}`;
    const configDir = join(tmpDir, ".config", "opensessions");
    await Bun.write(
      join(configDir, "config.json"),
      JSON.stringify({ sidebarWidth: 30, sidebarPosition: "right", keybinding: "b" }),
    );

    const config = loadConfig(tmpDir);
    expect(config.sidebarWidth).toBe(30);
    expect(config.sidebarPosition).toBe("right");
    expect(config.keybinding).toBe("b");

    const { rmSync } = require("fs");
    rmSync(tmpDir, { recursive: true, force: true });
  });

  test("loadConfig returns undefined for unset sidebar settings", () => {
    const config = loadConfig("/tmp/nonexistent-dir-" + Date.now());
    expect(config.sidebarWidth).toBeUndefined();
    expect(config.sidebarPosition).toBeUndefined();
    expect(config.keybinding).toBeUndefined();
  });

  test("resolveAutoHibernateConfig defaults to enabled after six hours", () => {
    expect(resolveAutoHibernateConfig(undefined)).toEqual({
      enabled: true,
      idleAfterMs: 6 * 60 * 60 * 1000,
    });
  });

  test("resolveAutoHibernateConfig accepts explicit disable and timeout", () => {
    expect(resolveAutoHibernateConfig({ enabled: false, idleAfterMs: 1000 })).toEqual({
      enabled: false,
      idleAfterMs: 1000,
    });
  });

  test("loadConfig reads from config file", async () => {
    const tmpDir = `/tmp/opensessions-test-${Date.now()}`;
    const configDir = join(tmpDir, ".config", "opensessions");
    await Bun.write(
      join(configDir, "config.json"),
      JSON.stringify({ mux: "zellij", plugins: ["opensessions-mux-zellij"] }),
    );

    const config = loadConfig(tmpDir);
    expect(config.mux).toBe("zellij");
    expect(config.plugins).toEqual(["opensessions-mux-zellij"]);

    const { rmSync } = require("fs");
    rmSync(tmpDir, { recursive: true, force: true });
  });

  test("loadConfig merges defaults for missing fields", async () => {
    const tmpDir = `/tmp/opensessions-test-${Date.now()}`;
    const configDir = join(tmpDir, ".config", "opensessions");
    await Bun.write(
      join(configDir, "config.json"),
      JSON.stringify({ mux: "tmux" }),
    );

    const config = loadConfig(tmpDir);
    expect(config.mux).toBe("tmux");
    expect(config.plugins).toEqual([]);

    const { rmSync } = require("fs");
    rmSync(tmpDir, { recursive: true, force: true });
  });

  test("loadConfig reads theme as string name", async () => {
    const tmpDir = `/tmp/opensessions-test-${Date.now()}`;
    const configDir = join(tmpDir, ".config", "opensessions");
    await Bun.write(
      join(configDir, "config.json"),
      JSON.stringify({ theme: "catppuccin-latte" }),
    );

    const config = loadConfig(tmpDir);
    expect(config.theme).toBe("catppuccin-latte");

    const { rmSync } = require("fs");
    rmSync(tmpDir, { recursive: true, force: true });
  });

  test("loadConfig reads theme as inline object", async () => {
    const tmpDir = `/tmp/opensessions-test-${Date.now()}`;
    const configDir = join(tmpDir, ".config", "opensessions");
    await Bun.write(
      join(configDir, "config.json"),
      JSON.stringify({ theme: { palette: { base: "#000000", text: "#ffffff" } } }),
    );

    const config = loadConfig(tmpDir);
    expect(typeof config.theme).toBe("object");
    expect((config.theme as any).palette.base).toBe("#000000");

    const { rmSync } = require("fs");
    rmSync(tmpDir, { recursive: true, force: true });
  });
});

describe("Themes", () => {
  test("BUILTIN_THEMES has catppuccin-mocha as default", () => {
    expect(BUILTIN_THEMES["catppuccin-mocha"]).toBeDefined();
    expect(BUILTIN_THEMES["catppuccin-mocha"].palette.base).toBe("#1e1e2e");
  });

  test("BUILTIN_THEMES has catppuccin-latte", () => {
    expect(BUILTIN_THEMES["catppuccin-latte"]).toBeDefined();
    expect(BUILTIN_THEMES["catppuccin-latte"].palette.base).toBe("#eff1f5");
  });

  test("BUILTIN_THEMES has tokyo-night", () => {
    expect(BUILTIN_THEMES["tokyo-night"]).toBeDefined();
    expect(BUILTIN_THEMES["tokyo-night"].palette.base).toBe("#1a1b26");
  });

  test("resolveTheme returns default when no theme configured", () => {
    const theme = resolveTheme(undefined);
    expect(theme.palette.base).toBe("#1e1e2e"); // catppuccin-mocha
  });

  test("resolveTheme resolves builtin by name", () => {
    const theme = resolveTheme("catppuccin-latte");
    expect(theme.palette.base).toBe("#eff1f5");
  });

  test("resolveTheme falls back to default for unknown name", () => {
    const theme = resolveTheme("nonexistent-theme");
    expect(theme.palette.base).toBe("#1e1e2e");
  });

  test("resolveTheme merges partial inline theme over default", () => {
    const theme = resolveTheme({ palette: { base: "#000000", text: "#ffffff" } });
    expect(theme.palette.base).toBe("#000000");
    expect(theme.palette.text).toBe("#ffffff");
    // Non-overridden colors come from default
    expect(theme.palette.blue).toBe("#89b4fa");
  });

  test("resolveTheme merges partial status colors", () => {
    const theme = resolveTheme({ status: { running: "#ff0000" } });
    expect(theme.status.running).toBe("#ff0000");
    // Non-overridden statuses come from default
    expect(theme.status.done).toBe("#a6e3a1");
  });

  test("every builtin theme has all required palette keys", () => {
    const requiredKeys = ["blue", "yellow", "green", "red", "peach", "teal", "text", "subtext0", "overlay0", "overlay1", "surface0", "surface1", "surface2", "base", "mantle", "crust"];
    for (const [name, theme] of Object.entries(BUILTIN_THEMES)) {
      for (const key of requiredKeys) {
        expect(theme.palette).toHaveProperty(key);
      }
    }
  });

  test("every builtin theme has all status colors", () => {
    const statuses = ["idle", "running", "tool-running", "done", "error", "waiting", "interrupted", "stale", "hibernated"];
    for (const [name, theme] of Object.entries(BUILTIN_THEMES)) {
      for (const s of statuses) {
        expect(theme.status).toHaveProperty(s);
      }
    }
  });
});
