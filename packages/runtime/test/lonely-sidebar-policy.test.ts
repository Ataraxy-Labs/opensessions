import { describe, expect, test } from "bun:test";
import {
  DEFAULT_LONELY_SIDEBAR_POLICY,
  resolveLonelySidebarPolicy,
} from "../src/config";

describe("resolveLonelySidebarPolicy", () => {
  test('accepts "kill"', () => {
    expect(resolveLonelySidebarPolicy("kill")).toBe("kill");
  });

  test('accepts "spawn-shell"', () => {
    expect(resolveLonelySidebarPolicy("spawn-shell")).toBe("spawn-shell");
  });

  test("falls back to default for undefined", () => {
    expect(resolveLonelySidebarPolicy(undefined)).toBe(DEFAULT_LONELY_SIDEBAR_POLICY);
  });

  test("falls back to default for null", () => {
    expect(resolveLonelySidebarPolicy(null)).toBe(DEFAULT_LONELY_SIDEBAR_POLICY);
  });

  test("falls back to default for empty string", () => {
    // The server-side env-var path uses `process.env.X || config.X`, but a
    // user might still write `""` in their config.json — treat it as
    // "unset" rather than crashing.
    expect(resolveLonelySidebarPolicy("")).toBe(DEFAULT_LONELY_SIDEBAR_POLICY);
  });

  test("falls back to default for unknown strings", () => {
    expect(resolveLonelySidebarPolicy("destroy-everything")).toBe(DEFAULT_LONELY_SIDEBAR_POLICY);
    expect(resolveLonelySidebarPolicy("KILL")).toBe(DEFAULT_LONELY_SIDEBAR_POLICY);
  });

  test("falls back to default for non-string values", () => {
    expect(resolveLonelySidebarPolicy(42)).toBe(DEFAULT_LONELY_SIDEBAR_POLICY);
    expect(resolveLonelySidebarPolicy({ policy: "kill" })).toBe(DEFAULT_LONELY_SIDEBAR_POLICY);
    expect(resolveLonelySidebarPolicy(true)).toBe(DEFAULT_LONELY_SIDEBAR_POLICY);
  });
});

describe("DEFAULT_LONELY_SIDEBAR_POLICY", () => {
  test('is "kill" — preserve canonical native-tmux behaviour', () => {
    // Locked in so a future change to the default doesn't silently flip
    // every existing user's behaviour.
    expect(DEFAULT_LONELY_SIDEBAR_POLICY).toBe("kill");
  });
});
