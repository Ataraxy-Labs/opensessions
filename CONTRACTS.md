# CONTRACTS.md — Agent Integration Guide

opensessions uses a simple HTTP contract. Any coding agent can report its status by POSTing to the server.

## Agent Event Contract

**Endpoint:** `POST http://127.0.0.1:7391/event`

**Payload:**

```json
{
  "agent": "your-agent-name",
  "session": "tmux-session-name",
  "status": "running",
  "ts": 1700000000000
}
```

**Fields:**

| Field     | Type   | Description                              |
|-----------|--------|------------------------------------------|
| `agent`   | string | Agent identifier (e.g. "amp", "claude-code", "aider") |
| `session` | string | Terminal multiplexer session name         |
| `status`  | string | One of: `running`, `idle`, `done`, `error`, `waiting`, `interrupted` |
| `ts`      | number | Unix timestamp in milliseconds           |

**Status meanings:**

| Status        | Meaning                                |
|---------------|----------------------------------------|
| `running`     | Agent is actively working              |
| `idle`        | Agent is ready, not processing         |
| `done`        | Agent completed successfully           |
| `error`       | Agent encountered an error             |
| `waiting`     | Agent is waiting for user input        |
| `interrupted` | Agent was manually interrupted         |

---

## Integration Examples

### Amp (Plugin API)

Amp uses its plugin system. Install the sidebar-status plugin:

```typescript
// ~/.config/amp/plugins/sidebar-status.ts
import type Amp from "@anthropic-ai/amp";

const SESSION = process.env.TMUX
  ? Bun.spawnSync(["tmux", "display-message", "-p", "#{session_name}"], {
      stdout: "pipe",
    }).stdout.toString().trim()
  : "";

function post(status: string) {
  if (!SESSION) return;
  fetch("http://127.0.0.1:7391/event", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ agent: "amp", session: SESSION, status, ts: Date.now() }),
  }).catch(() => {});
}

export default function plugin(amp: Amp) {
  amp.on("agent.start", () => post("running"));
  amp.on("agent.end", (ev) => post(ev.error ? "error" : "done"));
  amp.on("tool.call", () => post("running"));
}
```

### Claude Code (Hooks)

Add to `~/.claude/settings.json`:

```json
{
  "hooks": {
    "UserPromptSubmit": [
      {
        "type": "command",
        "command": "curl -s -o /dev/null -X POST http://127.0.0.1:7391/event -H 'Content-Type: application/json' -d '{\"agent\":\"claude-code\",\"session\":\"'$(tmux display-message -p '#{session_name}')'\",\"status\":\"running\",\"ts\":'$(date +%s000)'}'"
      }
    ],
    "Stop": [
      {
        "type": "command",
        "command": "curl -s -o /dev/null -X POST http://127.0.0.1:7391/event -H 'Content-Type: application/json' -d '{\"agent\":\"claude-code\",\"session\":\"'$(tmux display-message -p '#{session_name}')'\",\"status\":\"idle\",\"ts\":'$(date +%s000)'}'"
      }
    ],
    "PostToolUse": [
      {
        "type": "command",
        "command": "curl -s -o /dev/null -X POST http://127.0.0.1:7391/event -H 'Content-Type: application/json' -d '{\"agent\":\"claude-code\",\"session\":\"'$(tmux display-message -p '#{session_name}')'\",\"status\":\"running\",\"ts\":'$(date +%s000)'}'"
      }
    ],
    "Notification": [
      {
        "type": "command",
        "command": "curl -s -o /dev/null -X POST http://127.0.0.1:7391/event -H 'Content-Type: application/json' -d '{\"agent\":\"claude-code\",\"session\":\"'$(tmux display-message -p '#{session_name}')'\",\"status\":\"waiting\",\"ts\":'$(date +%s000)'}'"
      }
    ]
  }
}
```

### OpenCode (Plugin)

OpenCode emits session events. Add a plugin that maps them:

```typescript
// opencode-opensessions.ts
const SESSION = process.env.TMUX
  ? Bun.spawnSync(["tmux", "display-message", "-p", "#{session_name}"], {
      stdout: "pipe",
    }).stdout.toString().trim()
  : "";

function post(status: string) {
  if (!SESSION) return;
  fetch("http://127.0.0.1:7391/event", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ agent: "opencode", session: SESSION, status, ts: Date.now() }),
  }).catch(() => {});
}

// Map OpenCode events → opensessions statuses
// session.idle   → idle
// session.status → running
// session.error  → error
```

### Codex CLI (Hooks)

Codex CLI supports hooks (beta). Run the setup script:

```bash
bash integrations/codex/setup.sh
```

This installs `~/.codex/hooks.json` and enables `codex_hooks` in `~/.codex/config.toml`.

Events mapped:
- `SessionStart` → idle
- `UserPromptSubmit` → running
- `PostToolUse` → running
- `Stop` → idle

### Aider / Any Agent (Generic curl)

For any agent that supports shell hooks or custom commands:

```bash
# Report agent started working
curl -s -o /dev/null -X POST http://127.0.0.1:7391/event \
  -H 'Content-Type: application/json' \
  -d '{"agent":"aider","session":"'"$(tmux display-message -p '#{session_name}')"'","status":"running","ts":'"$(date +%s000)"'}'

# Report agent finished
curl -s -o /dev/null -X POST http://127.0.0.1:7391/event \
  -H 'Content-Type: application/json' \
  -d '{"agent":"aider","session":"'"$(tmux display-message -p '#{session_name}')"'","status":"done","ts":'"$(date +%s000)"'}'
```

### JSONL File Fallback

If the HTTP endpoint is unreachable, agents can append events to `/tmp/opensessions-events.jsonl`:

```bash
echo '{"agent":"my-agent","session":"my-session","status":"running","ts":'$(date +%s000)'}' >> /tmp/opensessions-events.jsonl
```

The server reads this file as a fallback.

---

## MuxProvider Interface

To add support for a new terminal multiplexer, implement the `MuxProvider` interface from `@opensessions/core`:

```typescript
import type { MuxProvider, MuxSessionInfo } from "@opensessions/core";

export class ZellijProvider implements MuxProvider {
  readonly name = "zellij";

  listSessions(): MuxSessionInfo[] {
    // Return array of { name, createdAt, dir, windows }
  }

  switchSession(name: string, clientTty?: string): void {
    // Switch to the named session
  }

  getCurrentSession(): string | null {
    // Return the currently focused session name
  }

  getSessionDir(name: string): string {
    // Return the working directory of the session
  }

  getPaneCount(name: string): number {
    // Return number of panes in the session
  }

  getClientTty(): string {
    // Return the client's TTY path
  }

  setupHooks(serverHost: string, serverPort: number): void {
    // Set up hooks that POST to the server on session changes
  }

  cleanupHooks(): void {
    // Remove hooks set up by setupHooks
  }
}
```

Then pass it to the server at startup, or contribute it to the `@opensessions/core` package.
