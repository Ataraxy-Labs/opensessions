import { describe, expect, test } from "bun:test";
import {
  AUTH_TOKEN_HEADER,
  constantTimeEquals,
  isAuthorizedToken,
  isLivenessProbe,
} from "../src/server/server-auth";

const TOKEN = "a".repeat(64);

describe("constantTimeEquals", () => {
  test("returns true for identical strings", () => {
    expect(constantTimeEquals("abc", "abc")).toBe(true);
  });

  test("returns false for differing strings of equal length", () => {
    expect(constantTimeEquals("abc", "abd")).toBe(false);
  });

  test("returns false for strings of different length", () => {
    expect(constantTimeEquals("abc", "abcd")).toBe(false);
  });

  test("returns true for two empty strings", () => {
    // Distinct from the empty-token case in isAuthorizedToken; this is
    // just the primitive's contract.
    expect(constantTimeEquals("", "")).toBe(true);
  });
});

describe("isAuthorizedToken", () => {
  test("accepts a matching header token", () => {
    expect(
      isAuthorizedToken({
        expected: TOKEN,
        headerToken: TOKEN,
      }),
    ).toBe(true);
  });

  test("accepts a matching query token", () => {
    expect(
      isAuthorizedToken({
        expected: TOKEN,
        queryToken: TOKEN,
      }),
    ).toBe(true);
  });

  test("rejects a mismatched header token", () => {
    expect(
      isAuthorizedToken({
        expected: TOKEN,
        headerToken: "b".repeat(64),
      }),
    ).toBe(false);
  });

  test("rejects when no token is presented", () => {
    expect(
      isAuthorizedToken({
        expected: TOKEN,
      }),
    ).toBe(false);
  });

  test("rejects when expected token is empty", () => {
    // Defence in depth: if the server somehow boots with no token,
    // treat every request as unauthenticated rather than authenticating
    // every request.
    expect(
      isAuthorizedToken({
        expected: "",
        headerToken: "",
      }),
    ).toBe(false);
  });

  test("prefers the header over the query parameter when both are present", () => {
    // A correct header should authorize even if the query is wrong.
    expect(
      isAuthorizedToken({
        expected: TOKEN,
        headerToken: TOKEN,
        queryToken: "bogus",
      }),
    ).toBe(true);
    // A wrong header should NOT be overridden by a correct query param.
    expect(
      isAuthorizedToken({
        expected: TOKEN,
        headerToken: "bogus",
        queryToken: TOKEN,
      }),
    ).toBe(false);
  });

  test("rejects null/undefined token values", () => {
    expect(
      isAuthorizedToken({
        expected: TOKEN,
        headerToken: null,
        queryToken: null,
      }),
    ).toBe(false);
  });
});

describe("isLivenessProbe", () => {
  test("true for GET / without upgrade", () => {
    expect(isLivenessProbe("GET", "/", false)).toBe(true);
  });

  test("false for GET / WITH upgrade (websocket)", () => {
    // Websocket upgrades on `/` must be authenticated; otherwise the
    // liveness exemption would be a hole big enough to drive a TUI
    // through.
    expect(isLivenessProbe("GET", "/", true)).toBe(false);
  });

  test("false for non-root GET", () => {
    expect(isLivenessProbe("GET", "/refresh", false)).toBe(false);
  });

  test("false for non-GET method on /", () => {
    expect(isLivenessProbe("POST", "/", false)).toBe(false);
  });
});

describe("AUTH_TOKEN_HEADER", () => {
  test("is the canonical header name", () => {
    // Lock in the wire-format constant so external integrations
    // (tmux scripts, amp, pi-extension) don't drift from the server.
    expect(AUTH_TOKEN_HEADER).toBe("x-opensessions-token");
  });
});
