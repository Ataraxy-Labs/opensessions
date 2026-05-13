# opensessions — AI Agent Instructions

You are working on **opensessions**, an agent-agnostic, mux-agnostic terminal session manager.

## Project Structure

```
opensessions/
├── apps/
│   ├── server/        # @opensessions/server — bootstrap entrypoint for the Bun server
│   └── tui/           # @opensessions/tui — OpenTUI terminal sidebar (Solid)
│       ├── src/
│       │   └── index.tsx    # Main TUI app
│       ├── scripts/
│       │   └── start.sh     # Canonical sidebar launcher used by mux providers
│       ├── build.ts         # Bun build with Solid plugin
│       └── bunfig.toml      # Required: preload for Solid JSX transform
├── integrations/
│   └── tmux-plugin/  # tmux-facing scripts and host integration glue
├── packages/
│   ├── runtime/       # @opensessions/runtime — runtime, watcher logic, config, plugins, server internals
│   │   ├── src/
│   │   │   ├── contracts/   # AgentEvent, AgentStatus, AgentWatcher, MuxProvider, MuxSessionInfo
│   │   │   ├── agents/      # AgentTracker (state management for agent events)
│   │   │   │   └── watchers/  # Built-in agent watchers
│   │   │   │       ├── amp.ts
│   │   │   │       ├── claude-code.ts
│   │   │   │       ├── codex.ts
│   │   │   │       └── opencode.ts
│   │   │   ├── mux/         # Mux registry and detection helpers
│   │   │   ├── server/      # WebSocket server internals and launcher
│   │   │   ├── shared.ts    # Shared types, constants, palette
│   │   │   └── index.ts     # Barrel export
│   │   └── test/            # Tests (bun:test)
│   └── mux/
│       ├── contract/        # @opensessions/mux — mux contracts and capability guards
│       ├── providers/
│       │   ├── tmux/        # @opensessions/mux-tmux — tmux provider
│       │   └── zellij/      # @opensessions/mux-zellij — experimental zellij provider
│       └── tmux-sdk/        # @opensessions/tmux-sdk — lower-level tmux command wrapper
├── ai_logs/           # AI-assisted dev session logs (NN-kebab-case-name.md)
├── CONTRACTS.md       # Agent integration guide (Amp, Claude Code, OpenCode, Aider)
├── turbo.json         # Turborepo config
├── opensessions.tmux  # Root TPM entrypoint
└── package.json       # Bun workspace root
```

## Key Architecture Decisions

1. **Monorepo**: Turborepo + Bun workspaces, with `apps/` for runnable entrypoints and `packages/` for reusable libraries.
2. **Built-in agent watchers**: Core ships with `AmpAgentWatcher`, `ClaudeCodeAgentWatcher`, `CodexAgentWatcher`, and `OpenCodeAgentWatcher` that watch agent data directories directly. External agents integrate via the `AgentWatcher` plugin interface.
3. **Mux-agnostic**: `MuxProvider` interface abstracts all mux operations. `TmuxProvider` is the reference implementation.
4. **MuxProvider is SYNC**: All methods use `Bun.spawnSync` — matches the existing pattern and keeps the server simple.
5. **Auto-detect mux**: `detectMux()` checks `$TMUX`, `$ZELLIJ_SESSION_NAME` env vars. Config file override planned.
6. **TDD**: All contracts and tracker logic have tests. Use `bun test` in `packages/runtime/`.

## Contracts

### AgentEvent
```typescript
{ agent: string, session: string, status: AgentStatus, ts: number, threadId?: string, threadName?: string, unseen?: number }
```
`AgentStatus = "running" | "idle" | "done" | "error" | "waiting" | "interrupted"`

### MuxProvider Interface
```typescript
interface MuxProvider {
  name: string;
  listSessions(): MuxSessionInfo[];        // {name, createdAt, dir, windows}[]
  switchSession(name, clientTty?): void;
  getCurrentSession(): string | null;
  getSessionDir(name): string;
  getPaneCount(name): number;
  getClientTty(): string;
  setupHooks(host, port): void;
  cleanupHooks(): void;
}
```

### AgentWatcher Interface
```typescript
interface AgentWatcher {
  name: string;
  watch(callback: (event: AgentEvent) => void): void;
  stop(): void;
}
```

## Stack

- **Runtime**: Bun (not Node)
- **Language**: TypeScript (strict)
- **TUI**: OpenTUI with Solid reconciler (`@opentui/solid`, `@opentui/core`, `solid-js`)
- **Tests**: `bun:test` — run with `bun test` in `packages/runtime/`
- **Build**: `@opentui/solid/bun-plugin` for TUI builds

## Development Guidelines

- **TDD**: Red-green-refactor, vertical slices, one test at a time. Tests verify behavior through public interfaces.
- **Sync mux calls**: MuxProvider methods are synchronous. Don't make them async.
- **Preserve optimizations**: Batched tmux calls, 5s git cache with HEAD watchers, lightweight focus-only broadcasts.
- **Sidebar resize work**: Before changing sidebar spawning, width sync, tmux resize handling, or `sidebar-coordinator`, read `docs/explanation/sidebar-behavior.md` and preserve those invariants unless you update the doc in the same change.
- **Built-in watchers in runtime**: Amp, Claude Code, Codex, and OpenCode have built-in watchers in `packages/runtime/src/agents/watchers/`. Community agents use the `AgentWatcher` plugin interface.
- **OpenTUI Solid**: JSX needs `bunfig.toml` preload and `jsxImportSource: "@opentui/solid"` in tsconfig. Build needs `solidPlugin`.
- **Never call `process.exit()` directly in TUI**: Use `renderer.destroy()`.
- **AI session logs**: Always write AI-assisted dev session logs to `ai_logs/` at the project root (this directory — `~/repos/github/opensessions/ai_logs/`), not inside any TPM-installed clone (e.g. `~/.tmux/plugins/opensessions/`). Use the `ai-session-logger` skill's `NN-kebab-case-name.md` convention.

## Local Runtime vs Source Checkout

- **Keep the installed runtime and source checkout separate.** For local tmux usage, the running TPM install is expected to live at `~/.tmux/plugins/opensessions`, while this repository checkout (`~/repos/github/opensessions`) is the development/source tree.
- **Do not assume tests against the sidebar are using this checkout.** Before debugging live runtime behavior, verify the active install with `tmux show-environment -g OPENSESSIONS_DIR` and `ps -axo pid,ppid,command | grep opensessions/apps/server`.
- **When live behavior depends on new source changes**, update the TPM clone intentionally (for example, fast-forward `~/.tmux/plugins/opensessions` to the desired branch/commit), run `bun install --frozen-lockfile` there if dependencies changed, and restart opensessions from the installed location.
- **Current server builds use per-instance auth tokens.** Integration clients such as the Amp plugin should read `/tmp/opensessions.<server-key>.token` (or `OPENSESSIONS_TOKEN_FILE`) and send it as `x-opensessions-token`; unauthenticated runtime POSTs are expected to return `401`.

## Amp Integration Notes

- The Amp plugin belongs in Amp's plugin search path, usually `~/.config/amp/plugins/opensessions.ts` for a system-wide local install or `.amp/plugins/opensessions.ts` for a project-local install. After edits, reload Amp plugins from the command palette with `plugins: reload`.
- Prefer installing/copying the plugin from the runtime checkout that is actually serving opensessions when doing live sidebar tests, typically `~/.tmux/plugins/opensessions/integrations/amp/opensessions.ts`.
- Useful diagnostics: `cat /tmp/opensessions-plugin.log` for plugin POST/auth/thread resolution and `grep -i "agent-event\|amp-watcher" /tmp/opensessions-debug.log` for server-side watcher/plugin ownership behavior.
- Amp Neo may return `404` for the old `/api/durable-thread-workers` path even when `GET /api/threads/:id` works. The built-in Amp watcher must treat that as a non-DTW thread and fall back to thread-detail polling rather than retrying DTW forever.

## Common Commands

```bash
bun install                          # Install all workspace deps
bun test                             # Run all tests (from root via turbo)
cd packages/runtime && bun test      # Run runtime tests directly
cd apps/tui && bun run start         # Start TUI (requires tmux)
cd apps/tui && bun run build         # Build TUI for distribution
cd apps/server && bun run start      # Start the server bootstrap directly
```

## Adding a New Mux Provider

1. Create a new package under `packages/mux/providers/<your-mux>/`
2. Implement the `MuxProvider` interface
3. Register it from the server bootstrap in `apps/server/src/main.ts` if it should be built in
4. Add tests in the provider package or `packages/runtime/test/` at the highest useful layer
5. Export the provider from its package entrypoint

## Adding Agent Support

1. Create `packages/runtime/src/agents/watchers/your-agent.ts`
2. Implement the `AgentWatcher` interface
3. Register via `PluginAPI.registerWatcher()` in your plugin
4. Add tests in `packages/runtime/test/`
5. See `CONTRACTS.md` for integration examples
