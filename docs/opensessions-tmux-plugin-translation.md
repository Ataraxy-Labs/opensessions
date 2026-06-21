# Translating Herdr-Style Plugins to opensessions on tmux

This document evaluates whether a Herdr-style plugin system can be implemented for opensessions on top of tmux, given opensessions' constraints and tmux as the supported substrate.

The conclusion is: **yes, a useful plugin MVP is feasible on tmux**, with one important caveat: **server-side overlay/pop-up panes are not a reliable detached-server primitive**. Tmux supports split panes, new windows, stable pane/window/session identifiers, environment injection, pane metadata, capture/send/focus operations, creation hooks, and dead-pane exit-status observation. Tmux popups require a current client and failed in a detached dummy server.

## Tmux validation performed

Validation was run against an isolated dummy tmux server, not the user's normal tmux server.

Tmux version:

```text
tmux 3.6b
```

Dummy server pattern:

```bash
tmux -L <test-socket-name> -f /dev/null ...
```

The main validation run used a server named like:

```text
opensessions-plugin-test-4613
```

Artifacts from that run were written under:

```text
/tmp/opensessions-plugin-tmux-opensessions-plugin-test-4613
```

Additional isolated dummy servers were used to validate hook behavior, dead-pane observation, and popup behavior.

## Verified tmux primitives

### Stable session/window/pane ids

Command shape used:

```bash
tmux -L "$SOCK" -f /dev/null new-session -d -s osplug -n main -x 120 -y 40 '...'
tmux -L "$SOCK" display-message -p -t osplug:main '#{pane_id}'
tmux -L "$SOCK" display-message -p -t "$ROOT_PANE" '#{session_id}'
tmux -L "$SOCK" display-message -p -t "$ROOT_PANE" '#{window_id}'
```

Observed output:

```text
root_pane=%0 session_id=$0 window_id=@0
```

Implication: opensessions can address plugin-owned panes/windows by tmux id (`%pane`, `@window`, `$session`) rather than names that may change.

### Split plugin panes with env injection

Command shape used:

```bash
tmux split-window \
  -P -F '#{pane_id}' \
  -t "$ROOT_PANE" \
  -h \
  -c /tmp \
  -e OPENSESSIONS_PLUGIN_ID=example.tmux \
  -e OPENSESSIONS_CONTEXT_JSON='{"plugin_id":"example.tmux","action":"pane"}' \
  'printf "plugin-env:%s:%s:%s:%s\n" "$OPENSESSIONS_PLUGIN_ID" "$OPENSESSIONS_CONTEXT_JSON" "$PWD" "$TMUX_PANE"; sleep 60'
```

Observed metadata after creation:

```text
pane=%1 title=Plugin Board cwd=/private/tmp plugin=example.tmux entry=board
```

Implication: tmux supports launching a plugin terminal command in a split pane, with per-pane environment variables and cwd.

### New-window plugin panes with env injection

Command shape used:

```bash
tmux new-window \
  -P -F '#{pane_id}' \
  -t osplug: \
  -n plugin-tab \
  -c /tmp \
  -e OPENSESSIONS_PLUGIN_ID=example.tmux \
  -e OPENSESSIONS_PLUGIN_ENTRYPOINT_ID=tab \
  'printf "plugin-tab:%s:%s:%s\n" "$OPENSESSIONS_PLUGIN_ID" "$OPENSESSIONS_PLUGIN_ENTRYPOINT_ID" "$TMUX_PANE"; sleep 60'
```

Observed in `list-panes`:

```text
session=osplug window=plugin-tab pane=%2 active=1 title=Rahuls-MacBook-Air.local cwd=/private/tmp cmd=sleep plugin= entry=
```

Implication: tmux supports plugin pane placement as new windows. For tmux-native UX this maps more directly to Herdr's `tab` plugin pane placement than to Herdr's internal tab model.

### Pane titles and pane-local metadata

Command shape used:

```bash
tmux select-pane -t "$PLUGIN_PANE" -T 'Plugin Board'
tmux set-option -p -t "$PLUGIN_PANE" @opensessions_plugin_id example.tmux
tmux set-option -p -t "$PLUGIN_PANE" @opensessions_plugin_entrypoint board
tmux display-message -p -t "$PLUGIN_PANE" 'pane=#{pane_id} title=#{pane_title} cwd=#{pane_current_path} plugin=#{@opensessions_plugin_id} entry=#{@opensessions_plugin_entrypoint}'
```

Observed:

```text
pane=%1 title=Plugin Board cwd=/private/tmp plugin=example.tmux entry=board
```

Implication: opensessions can mark plugin-owned tmux panes with tmux pane options and/or maintain server-side durable ownership records. Pane options are useful as a tmux-visible backup/diagnostic marker.

### Pane discovery and state inspection

Command shape used:

```bash
tmux list-panes -a -F 'session=#{session_name} window=#{window_name} pane=#{pane_id} active=#{pane_active} title=#{pane_title} cwd=#{pane_current_path} cmd=#{pane_current_command} plugin=#{@opensessions_plugin_id} entry=#{@opensessions_plugin_entrypoint}'
```

Observed:

```text
session=osplug window=main pane=%0 active=0 title=Rahuls-MacBook-Air.local cwd=/Users/pullu/Documents/work/opensessions cmd=sleep plugin= entry=
session=osplug window=main pane=%1 active=1 title=Plugin Board cwd=/private/tmp cmd=sleep plugin=example.tmux entry=board
session=osplug window=plugin-tab pane=%2 active=1 title=Rahuls-MacBook-Air.local cwd=/private/tmp cmd=sleep plugin= entry=
```

Implication: opensessions can discover plugin panes, current commands, cwd, active state, pane titles, and pane-local plugin metadata through tmux formats.

### Pane capture and command input

Command shapes used:

```bash
tmux capture-pane -p -t "$PLUGIN_PANE"
tmux send-keys -t "$PLUGIN_PANE" 'echo sent-from-control-plane' C-m
```

Implication: plugin panes can be treated like normal tmux panes for read/send workflows. This supports dashboard panes, helper TUIs, review panes, and scripted setup panes.

### Tmux wait-for coordination

Command shape used:

```bash
tmux wait-for os-plugin-signal
tmux wait-for -S os-plugin-signal
```

Observed:

```text
wait-for-released
```

Implication: tmux has a native synchronization primitive that can be useful for tests and some plugin orchestration, though opensessions should normally expose higher-level waits through its server/API.

### Creation hooks

Available hooks were listed with:

```bash
tmux show-hooks -g
```

Relevant hook names included:

```text
after-new-session
after-new-window
after-split-window
after-kill-pane
after-select-pane
after-select-window
after-resize-pane
after-resize-window
session-created
session-closed
session-renamed
session-window-changed
window-linked
window-unlinked
```

Creation hook test:

```bash
tmux set-hook -g after-split-window 'set-option -Fgq @os_after_split "#{pane_id}:#{pane_current_command}"'
tmux set-hook -g after-new-window 'set-option -Fgq @os_after_new_window "#{window_id}:#{pane_id}:#{window_name}"'
```

Observed:

```text
after_split=%1:tmux
after_new_window=@1:%2:hookwin
```

Implication: tmux hooks can observe some creation/focus/layout events, but opensessions should not depend exclusively on tmux hooks. The server's own tmux polling/watch model is still the right source for durable state.

### Pane process exit observation

A direct `pane-died` hook was not available in this tmux version. `show-hooks -g` did not list `pane-died`.

However, tmux can retain exited panes and expose dead status when `remain-on-exit` is enabled:

```bash
tmux set-window-option -t remaintest remain-on-exit on
tmux split-window -P -F '#{pane_id}' -t "$ROOT" 'exit 7'
tmux list-panes -a -F 'pane=#{pane_id} dead=#{pane_dead} dead_status=#{pane_dead_status} current_command=#{pane_current_command}'
```

Observed:

```text
pane=%0 dead=0 dead_status= current_command=sleep
pane=%1 dead=1 dead_status=7 current_command=exit
```

Implication: opensessions can detect plugin pane exit status by enabling `remain-on-exit` for plugin panes/windows or by using process/pane polling. It should not assume a portable `pane-died` hook exists.

### Popup / overlay limitation

Tmux popup test from a detached dummy server:

```bash
tmux display-popup -t pop: -E 'echo popup-ok'
```

Observed:

```text
status=1
output=no current client
```

Implication: tmux popups are client-bound. A server-side opensessions plugin API should not promise Herdr-style overlays as a durable headless primitive. It can support split/window placements universally and maybe offer client-scoped popup placement later when a focused client is attached.

## Feasibility conclusion

Tmux supports enough primitives for a practical opensessions plugin MVP:

- command actions: yes, independent of tmux
- event hooks: yes, from opensessions server events; tmux hooks can assist but are not required
- managed plugin panes: yes, via `split-window` and `new-window`
- env/context injection into panes: yes, via `-e` and command environment
- plugin pane metadata: yes, via pane options and server state
- plugin pane focus/close: yes, via `select-pane` / `kill-pane`
- pane read/send: yes, via `capture-pane` / `send-keys`
- exit status observation: yes, with `remain-on-exit` or server process tracking
- overlay/popups: not as a detached-server guarantee

Therefore the second-stage design should include split/window plugin panes in the initial contract, and exclude or mark overlays/popups as client-scoped future work.

## Recommended opensessions interpretation

The Herdr model should translate to opensessions as **control-plane workflow plugins**:

- manifest-declared
- out-of-process
- CLI/API powered
- server-owned registry and lifecycle
- tmux-backed pane/window entrypoints
- opensessions-server events as the event source
- no in-process plugin runtime
- no sandbox promises in v1

This fits opensessions' architecture because the server already owns durable state, tmux/worktree mappings, launch jobs, watchers, and agent-facing APIs. Plugins should request work through the public control plane rather than mutate internals.

## Proposed plugin capabilities

### 1. Actions

Actions are arbitrary argv commands users can invoke manually, from keybindings, or through the API.

Example manifest:

```toml
id = "example.review"
name = "Review Tools"
version = "0.1.0"
min_opensessions_version = "0.1.0"
platforms = ["macos", "linux"]

[[actions]]
id = "review-all"
title = "Review all worktrees"
contexts = ["session", "worktree"]
command = ["./review-all.sh"]
```

Useful actions:

- create review layouts
- run `lazydiff`
- collect active worktree diffs
- launch sibling agents
- send prompts to selected agents
- start test watchers
- clean up completed sessions/worktrees
- export session summaries
- notify external systems

Actions do not require tmux support except for context and optional CLI/API calls back into opensessions.

### 2. Event hooks

Event hooks should run from opensessions server events, not raw tmux hooks. Tmux hooks are useful evidence that tmux can report some low-level events, but server-derived events are safer because opensessions already normalizes tmux state, worktrees, git state, and agent status.

Example:

```toml
[[events]]
on = "agent.done"
command = ["./notify.sh"]

[[events]]
on = "worktree.created"
command = ["./bootstrap-worktree.sh"]
```

Good initial event set:

- `session.created`
- `session.closed`
- `worktree.created`
- `worktree.removed`
- `tmux.pane.created`
- `tmux.pane.exited`
- `tmux.pane.focused`
- `agent.started`
- `agent.status_changed`
- `agent.done`
- `agent.blocked`
- `git.changed`
- `review.requested`
- `merge.completed`

Avoid high-volume events at first:

- raw pane output changed
- every focus tick
- every git cache refresh

If output hooks are ever added, they should be opt-in, throttled, and match-based.

### 3. Managed tmux plugin panes

Initial placements should be tmux-native:

- `split-right`
- `split-down`
- `window`

Optional later/client-scoped placement:

- `popup`

Avoid promising:

- durable detached `overlay`

because `tmux display-popup` failed in a detached dummy server with `no current client`.

Example manifest:

```toml
[[panes]]
id = "dashboard"
title = "Review dashboard"
placement = "split-right"
command = ["./dashboard"]

[[panes]]
id = "diff"
title = "Diff"
placement = "window"
command = ["lazydiff"]
```

Implementation mapping:

| Plugin placement | Tmux command |
|---|---|
| `split-right` | `tmux split-window -h ...` |
| `split-down` | `tmux split-window -v ...` |
| `window` | `tmux new-window ...` |
| `popup` future | `tmux display-popup ...`, only with current client |

### 4. Context injection

Plugin commands should receive environment variables plus a JSON context blob.

Recommended env:

```text
OPENSESSIONS_ENV=1
OPENSESSIONS_BIN
OPENSESSIONS_SOCKET
OPENSESSIONS_PLUGIN_ID
OPENSESSIONS_PLUGIN_ROOT
OPENSESSIONS_PLUGIN_CONFIG_DIR
OPENSESSIONS_PLUGIN_STATE_DIR
OPENSESSIONS_CONTEXT_JSON
OPENSESSIONS_SESSION_ID
OPENSESSIONS_TMUX_SESSION
OPENSESSIONS_TMUX_WINDOW_ID
OPENSESSIONS_TMUX_PANE_ID
OPENSESSIONS_WORKTREE_PATH
OPENSESSIONS_REPO_PATH
```

Actions additionally:

```text
OPENSESSIONS_PLUGIN_ACTION_ID
```

Events additionally:

```text
OPENSESSIONS_PLUGIN_EVENT
OPENSESSIONS_PLUGIN_EVENT_JSON
```

Pane entrypoints additionally:

```text
OPENSESSIONS_PLUGIN_ENTRYPOINT_ID
```

The tmux validation confirmed per-pane env injection works with `split-window -e` and `new-window -e`.

### 5. Plugin pane ownership

opensessions should track plugin pane ownership in server state and optionally mirror it into tmux pane options.

Server state should be authoritative:

```rust
PluginPaneRecord {
    plugin_id,
    entrypoint,
    tmux_session_id,
    tmux_window_id,
    tmux_pane_id,
}
```

Tmux pane options can serve as diagnostic/recovery hints:

```bash
tmux set-option -p -t %1 @opensessions_plugin_id example.tmux
tmux set-option -p -t %1 @opensessions_plugin_entrypoint board
```

Validation confirmed these can be read back with:

```bash
tmux display-message -p -t %1 '#{@opensessions_plugin_id} #{@opensessions_plugin_entrypoint}'
```

### 6. Plugin logs

Adopt Herdr's command log idea:

- log id
- plugin id
- action id or event name
- command argv
- started time
- finished time
- exit code
- stdout/stderr capped
- error string
- status: running/succeeded/failed

Logs are essential because plugins are arbitrary commands and often run in the background.

### 7. Plugin registry

Store linked plugins in a registry under opensessions state/config, separate from tmux. Tmux pane options are not enough for plugin registry persistence because actions and event hooks exist without panes.

Suggested MVP commands:

```bash
opensessions plugin link /path/to/plugin
opensessions plugin list
opensessions plugin enable <plugin_id>
opensessions plugin disable <plugin_id>
opensessions plugin unlink <plugin_id>
opensessions plugin action list [--plugin <plugin_id>]
opensessions plugin action run <action_id> [--plugin <plugin_id>]
opensessions plugin pane open --plugin <plugin_id> --entrypoint <id> [--placement split-right|split-down|window]
opensessions plugin pane focus <pane_id>
opensessions plugin pane close <pane_id>
opensessions plugin log list [--plugin <plugin_id>]
```

Defer GitHub install until local plugin linking proves useful.

## Important opensessions constraints

### Preserve tmux-native behavior

Plugins should not assume opensessions owns all tmux sessions/windows/panes. Plugin pane operations should be explicit and scoped:

- target a known opensessions-tracked pane
- target an opensessions-created session/window
- or create a new opensessions-managed session/window

Avoid rewriting user layouts unexpectedly.

### Server remains the control plane

Plugins should call the opensessions CLI/API. They should not directly edit state files or expect tmux options to be state authority.

Correct:

```bash
$OPENSESSIONS_BIN agent send ...
$OPENSESSIONS_BIN pane list ...
$OPENSESSIONS_BIN session create ...
```

Avoid:

```bash
sed -i ... ~/.local/share/opensessions/session.json
```

### No in-process plugin runtime

Keep plugin execution out-of-process. Do not load dynamic libraries or embed a JavaScript/TypeScript runtime into the server.

This preserves server stability and keeps the trust boundary obvious.

### Agent status authority must be explicit

If plugins can report agent status or metadata, reports must carry a source and authority level. Built-in watchers/API events should remain the source of truth unless a plugin is explicitly configured as authoritative for a given custom agent/source.

Recommended split:

- plugin metadata reports: safe by default
- plugin lifecycle reports: accepted only with explicit source/authority rules
- plugin pane ownership: server-owned

### Event hooks must be bounded

Do not run plugin commands for every high-frequency observation. Use an allowlist and rate limits. Herdr excludes `pane.output_changed` from plugin hooks; opensessions should make the same kind of choice.

## Recommended MVP design

### Manifest

```toml
id = "example.review"
name = "Review Tools"
version = "0.1.0"
min_opensessions_version = "0.1.0"
platforms = ["macos", "linux"]

[[actions]]
id = "review-all"
title = "Review all worktrees"
contexts = ["session", "worktree"]
command = ["./review-all.sh"]

[[events]]
on = "agent.done"
command = ["./notify-agent-done.sh"]

[[panes]]
id = "dashboard"
title = "Review dashboard"
placement = "split-right"
command = ["./dashboard"]
```

### API model

Start with:

- `plugin.link`
- `plugin.list`
- `plugin.unlink`
- `plugin.enable`
- `plugin.disable`
- `plugin.action.list`
- `plugin.action.run`
- `plugin.log.list`
- `plugin.pane.open`
- `plugin.pane.focus`
- `plugin.pane.close`

Defer:

- GitHub install
- marketplace
- link handlers
- popup placement
- output hooks
- plugin-managed storage API
- plugin permission prompts/capability model

### Tmux pane launch mapping

For a split plugin pane:

```bash
tmux split-window \
  -P -F '#{pane_id}' \
  -t "$TARGET_PANE" \
  -h \
  -c "$PLUGIN_CWD" \
  -e OPENSESSIONS_PLUGIN_ID="$PLUGIN_ID" \
  -e OPENSESSIONS_PLUGIN_ENTRYPOINT_ID="$ENTRYPOINT" \
  -e OPENSESSIONS_CONTEXT_JSON="$CONTEXT_JSON" \
  -- "$COMMAND"
```

For a new-window plugin pane:

```bash
tmux new-window \
  -P -F '#{pane_id}' \
  -t "$TMUX_SESSION:" \
  -n "$TITLE" \
  -c "$PLUGIN_CWD" \
  -e OPENSESSIONS_PLUGIN_ID="$PLUGIN_ID" \
  -e OPENSESSIONS_PLUGIN_ENTRYPOINT_ID="$ENTRYPOINT" \
  -e OPENSESSIONS_CONTEXT_JSON="$CONTEXT_JSON" \
  -- "$COMMAND"
```

After creation:

```bash
tmux select-pane -t "$PANE_ID" -T "$TITLE"
tmux set-option -p -t "$PANE_ID" @opensessions_plugin_id "$PLUGIN_ID"
tmux set-option -p -t "$PANE_ID" @opensessions_plugin_entrypoint "$ENTRYPOINT"
```

Implementation note: command quoting should go through the existing Rust tmux command runner APIs rather than shell string concatenation.

## Tmux limitations to document

### No detached overlay guarantee

`tmux display-popup` failed without a current client:

```text
no current client
```

So plugin panes should not promise Herdr's `overlay` behavior as a server-side primitive. Use split/window in v1.

### No direct pane-died hook in tested tmux version

`show-hooks -g` did not list `pane-died`. Process exit can still be observed by:

- opensessions' existing pane/process watcher model
- polling `list-panes`
- enabling `remain-on-exit` for plugin panes and reading `#{pane_dead}` / `#{pane_dead_status}`

Do not design the plugin lifecycle around a nonexistent universal pane-exit hook.

### User layout preservation

Splits and windows mutate tmux layout. The API should require explicit placement and target. For user-owned tmux sessions, default to non-destructive placement or require confirmation/explicit managed mode.

## Suggested implementation stages

### Stage 1: local action plugins

- parse `opensessions-plugin.toml`
- validate id/version/platform/commands
- link/list/enable/disable/unlink
- run actions out-of-process
- inject context/env
- capture logs

No tmux pane launch required beyond context discovery.

### Stage 2: server event hooks

- add event hook allowlist
- run hooks asynchronously
- rate-limit or suppress high-frequency events
- include event JSON and context JSON
- expose logs

### Stage 3: tmux plugin panes

- split-right / split-down / window placements
- env injection with `-e`
- cwd injection with `-c`
- server-side `PluginPaneRecord`
- tmux pane option mirror
- focus/close commands
- exit tracking through existing watcher/polling

### Stage 4: distribution and convenience

- optional GitHub install
- build commands
- config-dir command
- keybinding integration
- maybe link handlers

### Stage 5: client-scoped popup/overlay

- only when a client is attached
- gracefully fallback to split/window
- never required for headless server automation

## Final recommendation

Build a Herdr-inspired plugin system for opensessions, but make it tmux-native:

- **Yes** to manifest actions, event hooks, command logs, context injection, and managed tmux panes.
- **Yes** to split/window plugin panes backed by tmux; verified in dummy tmux servers.
- **Yes** to pane metadata through tmux options plus server state; verified.
- **Yes** to capture/send/focus/list operations; verified.
- **No** to promising detached overlays/popups in v1; tmux requires a current client.
- **No** to in-process plugin code.
- **No** to plugin state becoming implicit control-plane authority.

The right MVP is a control-plane workflow plugin system whose terminal UI surface is tmux split/window panes, not a generic UI extension runtime.
