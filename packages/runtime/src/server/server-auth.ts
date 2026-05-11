/**
 * Auth-token verification for the per-instance opensessions server.
 *
 * Each running server writes a random token to its TOKEN_FILE at startup
 * (mode 0600). Loopback callers — tmux hooks, the TUI, integrations —
 * must present that token via either the `x-opensessions-token` header
 * or a `?token=…` query parameter. The unauthenticated `GET /` liveness
 * probe is the only exception, and it explicitly rejects WebSocket
 * upgrades so it can't be used to bypass auth.
 *
 * The verification is a constant-time comparison so a slow attacker
 * can't recover the token byte-by-byte from response timings on a busy
 * loopback server. Tokens are 32-byte hex (64 chars), so the constant-
 * time path is cheap.
 */

import { AUTH_TOKEN_HEADER } from "../shared";

export { AUTH_TOKEN_HEADER };

export interface AuthorizationInput {
  /** The configured server token. */
  expected: string;
  /** Value of the `x-opensessions-token` header, if any. */
  headerToken?: string | null;
  /** Value of the `?token=…` query parameter, if any. */
  queryToken?: string | null;
}

/**
 * Constant-time equality for two strings. Returns false immediately on
 * length mismatch — token length is fixed (64 hex chars), so revealing
 * "wrong length" is not a meaningful side channel.
 */
export function constantTimeEquals(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i += 1) {
    diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  }
  return diff === 0;
}

/**
 * Returns true iff the caller presented a token matching `expected`.
 * The header takes precedence over the query parameter.
 */
export function isAuthorizedToken(input: AuthorizationInput): boolean {
  if (typeof input.expected !== "string" || input.expected.length === 0) {
    return false;
  }
  const presented = input.headerToken ?? input.queryToken ?? null;
  if (typeof presented !== "string") return false;
  return constantTimeEquals(presented, input.expected);
}

/**
 * Returns true iff a request should bypass auth (the GET / liveness
 * probe). WebSocket upgrades are explicitly excluded.
 */
export function isLivenessProbe(method: string, pathname: string, isUpgrade: boolean): boolean {
  return method === "GET" && pathname === "/" && !isUpgrade;
}
