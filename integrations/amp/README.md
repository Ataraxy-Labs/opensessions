# opensessions plugin for Amp

Real-time agent status for Amp threads, driven by the Amp plugin API instead
of by polling Amp's cloud API.

## Why

The cloud API approach (built-in `AmpAgentWatcher`) works, but:

- It polls `/api/threads` every 10s for discovery.
- Local threads (`usesDtw: false`) require detail fetches on every version bump.
- Amp's DTW WebSocket protocol is "in flux" per the Amp team and may change.

When this plugin is installed, opensessions uses its events as the source of
truth and automatically suppresses cloud API calls for any thread the plugin
has reported on. If the plugin goes silent for 5 minutes, the watcher falls
back to polling.

## Install

Amp loads system plugins from `~/.config/amp/plugins/*.ts` and project plugins
from `.amp/plugins/*.ts`. For normal opensessions usage, install the plugin as
a system plugin from the checkout that is actually running the opensessions
server.

If you use the TPM install as the runtime (the usual local setup), copy from
that installed checkout:

```sh
mkdir -p ~/.config/amp/plugins
cp ~/.tmux/plugins/opensessions/integrations/amp/opensessions.ts ~/.config/amp/plugins/opensessions.ts
```

When developing from this source checkout, copy (or symlink) the edited file:

```sh
mkdir -p ~/.config/amp/plugins
cp integrations/amp/opensessions.ts ~/.config/amp/plugins/opensessions.ts
```

Reload Amp plugins with `plugins: reload` from the Amp command palette (or
restart Amp). A recent Amp build that exposes `event.thread.id` on lifecycle
and tool events is required.

## Environment

| Variable | Default | Purpose |
| --- | --- | --- |
| `OPENSESSIONS_URL` | `http://127.0.0.1:7391` | opensessions server base URL |
| `OPENSESSIONS_TOKEN_FILE` | `/tmp/opensessions.<server-key>.token` | server auth token file |

The plugin logs diagnostics to `/tmp/opensessions-plugin.log`. Server-side
event handling and Amp watcher decisions are logged in `/tmp/opensessions-debug.log`
when debug logging is enabled.

## Event mapping

| Amp event | opensessions status |
| --- | --- |
| `session.start` | `idle` |
| `agent.start` | `running` |
| `agent.end` (`status=done`) | `done` |
| `agent.end` (`status=error`) | `error` |
| `agent.end` (other) | `interrupted` |
| `tool.call` | `tool-running` |
| `tool.result` (`status=error`) | `error` |
| `tool.result` (`status=cancelled`) | `interrupted` |
| `tool.result` (success, other tools still running) | `tool-running` |
| `tool.result` (success, no other tools running) | `running` (agent streaming the reply) |

## Session resolution

Every event payload carries:

1. `tmuxSession` — from `tmux display-message -p '#S'`, if Amp is running
   inside a tmux pane.
2. `projectDir` — from `process.cwd()`.

The server prefers a known tmux session name; otherwise it resolves
`projectDir` against its session→directory map.
