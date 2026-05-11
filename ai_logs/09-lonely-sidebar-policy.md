# Lonely Sidebar Policy — Configurable pane-exited Behavior

## Summary

Resolves the open conflict from PRs #7-#9 between the canonical
`pane-exited` behaviour (kill the lonely sidebar so the window dies
the way native tmux dies) and the TPM-installed work's behaviour
(spawn a fresh shell pane next to the sidebar so the window stays
usable).

Both behaviours are now valid, selected at server start by a single
config option:

```json
{
  "lonelySidebarPolicy": "kill" | "spawn-shell"
}
```

`OPENSESSIONS_LONELY_SIDEBAR_POLICY` env var takes precedence for
testing. Default is `"kill"` — preserves the canonical behaviour for
every existing user.

## Why a config flag instead of picking one

Both behaviours have legitimate constituencies:

- **`kill`** — closing the last pane in a window closes the window. It's
  the principle of least surprise for anyone who's used tmux without a
  sidebar. The sidebar is "just another pane".
- **`spawn-shell`** — the sidebar is window chrome, not content. If you
  accidentally `exit` your shell, you should get a recovery path
  rather than losing the window (and possibly the session).

Forcing one onto everyone would break someone's workflow. A flag with
a conservative default is the cheapest correct answer.

## Implementation

### Config plumbing
- `packages/runtime/src/config.ts`:
  - New `LonelySidebarPolicy = "kill" | "spawn-shell"` type.
  - `OpensessionsConfig.lonelySidebarPolicy` field.
  - `DEFAULT_LONELY_SIDEBAR_POLICY = "kill"` constant.
  - `resolveLonelySidebarPolicy(value)` — defensive coercion that falls
    back to the default for null/undefined/unknown/non-string values.
- `packages/runtime/src/server/index.ts`:
  - Reads `OPENSESSIONS_LONELY_SIDEBAR_POLICY` env first, then
    `config.lonelySidebarPolicy`, then default.
  - Adds `lonelySidebarPolicy` to the `config loaded` startup log line.

### Provider contract
- `packages/mux/contract/src/types.ts`:
  - New optional `protectOrphanedSidebars?(): void` on `SidebarCapable`.
  - Must be idempotent (the hook can fire multiple times for one logical
    close) and must not spawn a shell next to a sidebar that already has
    neighbours.

### tmux provider
- `packages/mux/providers/tmux/src/provider.ts`:
  - Implements `protectOrphanedSidebars()`. For each window where the
    sidebar is the only pane, calls `tmux.splitWindow(...)` against the
    sidebar pane id. Picks `before=true` if the sidebar is right-anchored
    (`sidebar.left !== 0`), otherwise `before=false` so the new shell
    lands on the opposite side.
  - Falls back to killing the lonely sidebar if `splitWindow` fails
    (e.g. window too narrow), so a misconfigured environment never gets
    stuck in a broken state.
  - Defensively dedupes any duplicate sidebars in multi-pane windows
    (mirrors `killOrphanedSidebarPanes` exactly there — spawn-shell only
    matters when alone).

### Server dispatch
- `/pane-exited` handler chooses between
  `provider.protectOrphanedSidebars()` (if policy is `spawn-shell` and
  the provider implements the method) and the existing
  `provider.killOrphanedSidebarPanes()`. Optional method gate ensures
  providers that haven't implemented `protectOrphanedSidebars` (e.g.
  zellij) silently fall back to kill.
- Same dispatch applied at server startup, where the canonical code
  already calls `killOrphanedSidebarPanes` to clean up restored
  panes from `tmux-resurrect`/`tmux-continuum`. Spawn-shell users
  expect that cleanup to be protective there too.

## Tests

`packages/runtime/test/lonely-sidebar-policy.test.ts` (new) — 8 tests
covering the resolver:
- Accepts `"kill"`, `"spawn-shell"`.
- Falls back to default for `undefined`, `null`, empty string, unknown
  strings, non-string values.
- Locks in `DEFAULT_LONELY_SIDEBAR_POLICY === "kill"` so a future
  default flip is a deliberate test break, not silent.

All 438 tests pass (8 new). The provider-level `protectOrphanedSidebars`
behaviour isn't unit-tested at the provider level because the existing
codebase has no fake-tmux harness for `TmuxProvider` — the tmux SDK is a
module-level singleton, and the existing convention is to test the
`TmuxClient` primitives heavily and trust the provider thin layer. The
underlying `splitWindow` primitive is already covered in
`packages/mux/tmux-sdk/test/tmux-client.test.ts`.

## Live smoke verification

```
$ OPENSESSIONS_LONELY_SIDEBAR_POLICY=spawn-shell bun run apps/server/src/main.ts
[server] config loaded {…,"lonelySidebarPolicy":"spawn-shell",…}

$ bun run apps/server/src/main.ts
[server] config loaded {…,"lonelySidebarPolicy":"kill",…}
```

End-to-end "actually-spawn-a-shell" testing requires a real tmux session
with a sidebar; that path is exercised manually before merge.

## Docs

`docs/reference/configuration.md`:
- Added `lonelySidebarPolicy` row to the config field table.
- New "Lonely Sidebar Policy" section explaining what the two values do
  and how the env-var override works.

## Files Changed

- `packages/runtime/src/config.ts`
- `packages/runtime/src/server/index.ts`
- `packages/runtime/test/lonely-sidebar-policy.test.ts` (new)
- `packages/mux/contract/src/types.ts`
- `packages/mux/providers/tmux/src/provider.ts`
- `docs/reference/configuration.md`
