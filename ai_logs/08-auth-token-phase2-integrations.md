# Auth Token System — Phase 2: Amp + pi-extension Integrations

## Summary

Phase 1 (PR #7) added an auth-token gate to the runtime server. The amp and
pi-extension integrations weren't updated in that PR because they live
outside the workspace (the user copies them into their own plugin directory)
and require independent rollouts. This phase plumbs the same token through
both integrations so they stop receiving 401s once Phase 1 is deployed.

## Why two phases

The amp plugin is sourced from `~/.config/amp/plugins/`, the pi-extension
file is loaded from wherever the pi user installs it. Bundling these as an
integration with separate semver lets each user upgrade on their own
schedule without breaking Phase 1's clean security boundary.

## Implementation

Both files mirror the same resolution logic that `packages/runtime/src/shared.ts`
uses (intentionally duplicated — these integrations are standalone files
the user copies into a plugin directory and can't import workspace
packages):

- `resolveServerKey()` — `OPENSESSIONS_SERVER_KEY` env, else hashed
  `$TMUX` socket path, else `null`.
- `resolveTokenFile(serverKey)` —
  `OPENSESSIONS_TOKEN_FILE` env, else `/tmp/opensessions.${serverKey}.token`,
  else `/tmp/opensessions.token`.
- `readAuthToken()` — reads the token file fresh on every POST so a server
  restart that rotates the token is picked up without restarting amp/pi.
- `post()` — skips the request silently when no token is readable rather
  than 401-spamming the server log; injects `x-opensessions-token: <token>`
  on every authenticated request.

### Files

**`integrations/amp/opensessions.ts`**
- Refactored `resolveServerPort` to take a `serverKey` parameter for symmetry
  with the runtime.
- Added `resolveServerKey`, `resolveTokenFile`, `readAuthToken`,
  `AUTH_TOKEN_HEADER`.
- `post(payload)` now reads the token, skips with a `SKIP no-token` log
  line if missing, otherwise sends the header.

**`integrations/pi-extension/opensessions-runtime.ts`**
- Same refactor: `resolveServerKey` + `resolveTokenFile` + `readAuthToken`.
- `post(path, body)` reads the token, returns silently if missing
  (preserves the previous "opensessions may not be running yet" semantics
  with no log noise), otherwise sends the header on both
  `/api/runtime/pi/upsert` and `/api/runtime/pi/delete`.

## Verification

All 430 runtime tests still pass (Phase 2 doesn't touch runtime code).

Live integration smoke test against a real server on port 17999:
```
POST /api/agent-event no auth                       → 401
POST /api/agent-event with x-opensessions-token: …  → 404 (could not
                                                          resolve session)
```
The 404 confirms the auth gate passed and the request reached the endpoint
logic; the test payload deliberately had an unknown projectDir so the
endpoint would short-circuit before mutating state.

Both files build cleanly with `bun build` (transpilation succeeds).

## Migration notes for users

Anyone running this plugin against an upgraded server MUST reinstall the
plugin file (`integrations/amp/opensessions.ts` →
`~/.config/amp/plugins/opensessions.ts`). Same story for
pi-extension users. The plugin file ships with no version pin, so
reinstall is the only path.

If the user upgrades the server first and forgets to refresh the plugin,
they'll see `401`s in `/tmp/opensessions-debug.log` for every
`agent-event` POST. The plugin itself stays alive — `post()` swallows the
response.

## Files Changed

- `integrations/amp/opensessions.ts`
- `integrations/pi-extension/opensessions-runtime.ts`
