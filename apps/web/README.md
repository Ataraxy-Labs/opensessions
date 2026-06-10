# opensessions · web (fleet control plane)

The cloud layer: machines push agent status to Convex, the web UI renders the
whole fleet reactively, and commands flow back down to each machine.

```
each machine                          Convex                clients
┌──────────────────────┐
│ server-rs (watchers)  │  snapshot   ┌──────────┐ reactive  ┌──────────┐
│        │              │ ──────────► │ agents   │ ────────► │ web UI   │
│   ┌────▼─────┐        │             │ machines │           │  render  │
│   │  bridge  │        │  command    │ commands │  insert   │  + send  │
│   └──────────┘ ◄──────│ ◄────────── └──────────┘ ◄──────── └──────────┘
│   send-keys           │
└──────────────────────┘
```

## What exists now

- **`convex/schema.ts`** — `machines`, `agents`, `commands`. The `agents` table
  mirrors `runtime-rs::AgentWatcherSnapshot` field-for-field.
- **`convex/agents.ts`** — `ingestSnapshot` (upsert + machine heartbeat),
  `listAgents` (the global sidebar query).
- **`convex/commands.ts`** — `enqueue` (web → cloud), `inbox` (the per-machine
  bridge's reactive work queue), `update` (delivered/acked/failed).
- **`src/App.tsx`** — renders agents grouped by machine, one send-input box per
  agent that calls `enqueue`.

## Run it

```bash
cd apps/web
npm install
npx convex dev        # creates the deployment, writes VITE_CONVEX_URL into .env.local, watches convex/
npm run dev           # in a second terminal — serves the web UI
```

Open the URL. It'll say "No machines yet" until a bridge pushes a snapshot.

## Smoke-test the pipe without the bridge

In the Convex dashboard (or `npx convex run`), call `ingestSnapshot` with a fake
row and watch it appear in the browser instantly:

```bash
npx convex run agents:ingestSnapshot '{"account":"dev","machineId":"laptop","hostname":"my-mac","agent":"claude-code","threadId":"t1","threadName":"wire up convex","status":"waiting","ts":0}'
```

Type into that agent's box → a `commands` row appears (drained by the bridge later).

## AgentWatcherSnapshot → ingestSnapshot (the contract)

The Rust bridge serializes each snapshot straight into `ingestSnapshot` args:

| AgentWatcherSnapshot (Rust) | ingestSnapshot arg | note                          |
| --------------------------- | ------------------ | ----------------------------- |
| `agent`                     | `agent`            |                               |
| `thread_id`                 | `threadId`         | `""` when `None`              |
| `thread_name`               | `threadName`       |                               |
| `last_user_prompt`          | `lastUserPrompt`   |                               |
| `project_dir`               | `projectDir`       |                               |
| `status` (camelCase serde)  | `status`           | already matches the union     |
| `ts`                        | `ts`               |                               |
| — (added by bridge)         | `account`          | identity                      |
| — (added by bridge)         | `machineId`        | stable per box                |
| — (added by bridge)         | `hostname`         |                               |
| — (from tmux mapping)       | `paneId`           | needed for `sendInput` target |

## Not built yet (next steps)

1. **The bridge** (`bridge/`, Node) — holds the Convex reactive subscription,
   reads agent state from the local `server-rs` websocket, pushes snapshots,
   drains the `commands` inbox.
2. **`send_keys`** in `server-rs` — `ClientCommand::SendInput { pane_id, text }`
   → `tmux send-keys`. The only new code on the dangerous write path; gate it.
3. **Auth** — `ACCOUNT = "dev"` is a placeholder. Real per-machine pairing +
   revocation before `sendInput` ships to anyone.
