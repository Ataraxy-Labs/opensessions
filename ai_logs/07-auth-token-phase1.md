# Auth Token System — Phase 1: Server, TUI, and tmux Plugin

## Summary

Added a per-server-instance shared-secret token that gates every HTTP and
WebSocket call into the opensessions runtime, except for the unauthenticated
`GET /` liveness probe. Loopback callers — tmux hooks, the sidebar TUI, the
zellij toggle script, and the uninstaller — now read the token from a
0600-mode file and present it via the `x-opensessions-token` header (or the
`?token=…` query parameter for the WebSocket upgrade).

The token is generated fresh on every server start, scoped per server-key
(matching the existing `PID_FILE` scheme), and removed on graceful shutdown.

## Why

The runtime previously trusted any process on the host to POST to its loopback
endpoints — including `quit`, `toggle`, and `notify`. On a multi-tenant box
(or any box where untrusted code can bind sockets) that meant any local
process could disrupt sidebars, ferry agent-status pings, or shut the server
down. The token closes that hole without losing the in-process latency
characteristics that the tmux hook integration depends on.

## Implementation

### Constants and exports
- `packages/runtime/src/shared.ts`: added `TOKEN_FILE` (per-instance,
  honors `OPENSESSIONS_TOKEN_FILE`) and `AUTH_TOKEN_HEADER` constants.
  Path scheme matches `PID_FILE`: `/tmp/opensessions.${SERVER_KEY}.token`
  when keyed, `/tmp/opensessions.token` otherwise.
- `packages/runtime/src/index.ts`: re-export both constants.

### Server
- `packages/runtime/src/server/server-auth.ts` (new): pure helpers
  `constantTimeEquals`, `isAuthorizedToken`, `isLivenessProbe`. Constant-time
  comparison protects against timing-based token recovery on busy loopback
  servers; length mismatch is fast-pathed because the token length is fixed.
- `packages/runtime/src/server/index.ts`:
  - Generate `randomBytes(32).toString("hex")` at startup.
  - Write to `TOKEN_FILE` with `mode: 0o600`.
  - Unlink in `cleanup()` alongside `PID_FILE`.
  - Reject everything that isn't `isLivenessProbe(...)` and lacks a matching
    `x-opensessions-token` header or `?token=` query parameter with `401`.
  - Pass `TOKEN_FILE` through to `provider.setupHooks(...)`.

### Mux contract
- `packages/mux/contract/src/types.ts`: extended `setupHooks` with optional
  third arg `tokenFile?: string`. Existing zellij provider ignores it.

### tmux provider
- `packages/mux/providers/tmux/src/provider.ts`: when `tokenFile` is given,
  the `run-shell -b "..."` body emits curl with
  `-H 'x-opensessions-token: '$(cat ${tokenFile} 2>/dev/null)`. The shell
  expands `$(cat …)` at hook fire time, so a server restart that rotates
  the token doesn't require re-installing every hook.

### tmux-plugin scripts
- `integrations/tmux-plugin/scripts/server-common.sh`: resolves
  `TOKEN_FILE` the same way as `PID_FILE`, exports `read_token()` and
  `auth_post()` helpers. `auth_post` no-ops if the token isn't readable
  (avoids retrying without auth and confusing logs).
- `ensure-sidebar.sh`, `switch-index.sh`, `focus.sh`, `toggle.sh`,
  `even-horizontal.sh`: switched from raw `curl … -X POST …` to
  `auth_post "/path"`.
- `zellij-toggle.sh`: doesn't source server-common (standalone bash with
  `set -euo pipefail`); inlined the token-read logic with explicit guards.
- `uninstall.sh`: replaced the always-broken `POST /shutdown` (no such
  endpoint existed) with an authenticated `auth_post "/quit"` inside a
  subshell so a missing server can't abort the rest of the uninstall.

### TUI
- `apps/tui/src/index.tsx`: read `TOKEN_FILE`, append
  `?token=<urlencoded>` to the WebSocket URL. Empty token falls through
  to a 401 — the existing reconnect loop will retry once the file lands,
  which matches the prior behaviour of "wait for the server to come up".

## Tests

`packages/runtime/test/server-auth.test.ts` (new) covers:
- `constantTimeEquals` true/false/length-mismatch cases
- `isAuthorizedToken` accepts header, accepts query, rejects mismatch,
  rejects missing presented token, rejects empty configured token, prefers
  header over query when both are present, rejects nullish values
- `isLivenessProbe` true for `GET /`, false for `GET /` with upgrade, false
  for non-root or non-GET
- Locks in the canonical header name `x-opensessions-token`

All 430 tests pass (16 new). Manual smoke verified at `OPENSESSIONS_PORT=17999`:
- `GET /` → 200 unauthenticated
- `POST /refresh` no token → 401
- `POST /refresh` valid header → 200
- `POST /refresh ?token=…` → 200
- `POST /refresh` wrong token → 401
- Token file is mode `-rw-------`, 64 hex chars (32 bytes of entropy)

## Out of Scope (Phase 2 / Phase 3)

- `integrations/amp/opensessions.ts` and
  `integrations/pi-extension/opensessions-runtime.ts` still post without a
  token. Their callers will start receiving 401s when they upgrade to a
  server built from this branch — Phase 2 will plumb the token through
  those integrations.
- The pane-exited behaviour conflict (TPM strategy: spawn a shell when the
  sidebar is the last pane; canonical strategy: kill the lonely sidebar) is
  unchanged. That's a separate decision and PR.

## Files Changed

- `packages/runtime/src/shared.ts`
- `packages/runtime/src/index.ts`
- `packages/runtime/src/server/index.ts`
- `packages/runtime/src/server/server-auth.ts` (new)
- `packages/runtime/test/server-auth.test.ts` (new)
- `packages/mux/contract/src/types.ts`
- `packages/mux/providers/tmux/src/provider.ts`
- `apps/tui/src/index.tsx`
- `integrations/tmux-plugin/scripts/server-common.sh`
- `integrations/tmux-plugin/scripts/ensure-sidebar.sh`
- `integrations/tmux-plugin/scripts/switch-index.sh`
- `integrations/tmux-plugin/scripts/focus.sh`
- `integrations/tmux-plugin/scripts/toggle.sh`
- `integrations/tmux-plugin/scripts/even-horizontal.sh`
- `integrations/tmux-plugin/scripts/zellij-toggle.sh`
- `integrations/tmux-plugin/scripts/uninstall.sh`
