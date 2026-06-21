# Herdr Plugin Architecture

Source analyzed: <https://github.com/ogulcancelik/herdr>

Local checkout used for inspection:

- Path: `/tmp/herdr-checkout.7v63Sl`
- Commit: `73b137a4ae75cb1465e8d4ddfc293ff7366d8133`

> Note: this document intentionally describes Herdr's plugin system on its own terms. It does not evaluate or adapt the design for any other project.

## Executive summary

Herdr's plugin system is a **manifest-declared, out-of-process workflow extension system**. A plugin is a local directory or GitHub-managed checkout containing a `herdr-plugin.toml` manifest. The manifest declares metadata and executable entrypoints. Herdr validates the manifest, persists an installed-plugin registry, injects runtime context/environment variables, launches commands, tracks command logs, and can open plugin-owned terminal panes.

Herdr plugins are not sandboxed, not dynamically loaded into the Herdr process, and not implemented through an in-process SDK. A plugin can be Bash, JavaScript, Lua, Python, Rust, Go, or any argv command available on the user's machine. The plugin calls back into Herdr through the Herdr CLI or raw local socket API.

The v1 host surface includes:

- local plugin linking
- GitHub plugin installation
- manifest validation
- platform filtering
- build commands for GitHub installs
- user-invoked actions
- event hooks
- managed plugin panes
- link handlers for clicked terminal URLs
- plugin command logs
- plugin config/state directory discovery
- keybinding integration for actions

## Architectural shape

```text
Plugin repository/directory
  herdr-plugin.toml
  scripts/binaries/assets
          |
          v
Herdr plugin host
  parse + validate manifest
  persist registry entry
  create config/state dirs
  resolve actions/events/panes/links
  inject context/env
  launch out-of-process commands
  record command logs
          |
          v
Plugin command process
  arbitrary executable code
  can call HERDR_BIN_PATH or HERDR_SOCKET_PATH
  owns its own files/config/state
```

## Main implementation files

| Area | Source file |
|---|---|
| API schema for plugin requests/responses | `src/api/schema/plugins.rs` |
| Socket method enum entries | `src/api/schema.rs` |
| Plugin API handlers | `src/app/api/plugins/mod.rs` |
| Manifest parsing/validation | `src/app/api/plugins/manifest.rs` |
| Runtime command spawning/logging | `src/app/api/plugins/runtime.rs` |
| Invocation context construction | `src/app/api/plugins/context.rs` |
| Plugin pane creation | `src/app/api/plugins/panes.rs` |
| Plugin env/path helpers | `src/app/api/plugins/env.rs` |
| Plugin registry persistence | `src/persist/plugin_registry.rs` |
| Plugin config/state paths | `src/plugin_paths.rs` |
| Cross-platform command spawning | `src/plugin_command.rs` |
| CLI wrapper | `src/cli/plugin.rs` |
| User docs | `website/src/content/docs/plugins.mdx` |
| Socket API docs | `website/src/content/docs/socket-api.mdx` |

## Plugin contract

Herdr's docs describe the plugin contract plainly:

- A plugin is a directory with `herdr-plugin.toml`.
- Runtime action registration is not part of v1.
- Native non-terminal plugin UI is not part of v1.
- Actions, event hooks, panes, and link handlers are all declared in the manifest.
- Commands are argv arrays, not shell strings.
- Herdr does not sandbox plugin commands.
- The whole Herdr CLI/socket API is available to plugin code.

The docs' key framing is:

> A plugin is not an SDK integration. It is a directory with a `herdr-plugin.toml` manifest and commands Herdr can launch. Herdr validates the manifest, injects runtime context, starts the declared commands, and records logs. The commands call back into Herdr through the CLI or socket when they need to do more work.

## Manifest structure

The raw manifest shape is defined in `src/app/api/plugins/manifest.rs`:

```rust
struct RawPluginManifest {
    id: String,
    name: String,
    version: String,
    min_herdr_version: Option<String>,
    description: Option<String>,
    platforms: Option<Vec<RawPlatform>>,
    build: Vec<RawPluginManifestBuild>,
    actions: Vec<RawPluginManifestAction>,
    events: Vec<RawPluginManifestEventHook>,
    panes: Vec<RawPluginManifestPane>,
    link_handlers: Vec<RawPluginManifestLinkHandler>,
}
```

The docs show this representative manifest:

```toml
id = "example.layout"
name = "Layout"
version = "0.1.0"
min_herdr_version = "0.7.0"
description = "Apply project layouts"
platforms = ["linux", "macos", "windows"]

[[build]]
command = ["npm", "ci"]

[[build]]
command = ["npm", "run", "build"]
platforms = ["linux", "macos"]

[[actions]]
id = "apply"
title = "Apply layout"
contexts = ["workspace"]
command = ["node", "dist/apply.js"]

[[events]]
on = "worktree.created"
command = ["herdr", "workspace", "list"]

[[panes]]
id = "board"
title = "Project board"
placement = "overlay"
command = ["herdr-board"]

[[link_handlers]]
id = "github-issue"
title = "Open GitHub issue"
pattern = "^https://github\\.com/[^/]+/[^/]+/(issues|pull)/[0-9]+$"
action = "apply"
```

## Manifest fields

### Required top-level metadata

Required fields:

- `id`
- `name`
- `version`
- `min_herdr_version`

Optional fields:

- `description`
- `platforms`
- `[[build]]`
- `[[actions]]`
- `[[events]]`
- `[[panes]]`
- `[[link_handlers]]`

`min_herdr_version` is mandatory. Herdr refuses to link/install a plugin when the required version is missing, invalid, or newer than the running binary.

### Platforms

Supported platform identifiers are:

- `linux`
- `macos`
- `windows`

Top-level `platforms` applies to the plugin as a whole. Build commands, actions, event hooks, panes, and link handlers may declare item-level `platforms`, which override the plugin-level list.

If a local plugin omits top-level `platforms`, linking still succeeds but produces a warning:

```text
manifest does not declare platforms; platform support unknown
```

### Commands

All command fields are argv arrays:

```toml
command = ["node", "dist/apply.js"]
```

Herdr does not run plugin commands through a shell. Shell behavior only occurs if the plugin explicitly invokes a shell in its command array.

On Windows, `src/plugin_command.rs` resolves common `PATHEXT` command shims and runs batch files through `cmd.exe /d /c` when needed.

## Link and install model

Herdr supports two installation modes:

1. local link
2. GitHub-managed install

### Local link

CLI:

```bash
herdr plugin link /path/to/plugin
```

Socket API:

```json
{"id":"req_plugin_link","method":"plugin.link","params":{"path":"/path/to/plugin","enabled":true}}
```

The path may be either:

- a directory containing `herdr-plugin.toml`
- a direct manifest path

Local link does not run build commands. It is intended for authoring/testing a plugin from a working tree.

### GitHub install

CLI:

```bash
herdr plugin install owner/repo[/subdir...] [--ref REF] [--yes]
```

GitHub install:

- accepts GitHub shorthand only
- clones with `git`
- shows a trust preview in interactive terminals
- runs supported manifest build commands
- stores the checkout under Herdr-managed plugin data
- registers the plugin
- supports reinstall by replacing the managed checkout
- refuses to install over a locally linked plugin

The socket `plugin.link` method accepts optional source metadata. GitHub installs use it so `plugin.list` can show origin, requested ref, resolved commit, and managed path.

Source metadata schema appears in `src/api/schema/plugins.rs` as `PluginSourceInfo`:

```rust
pub struct PluginSourceInfo {
    pub kind: PluginSourceKind,
    pub owner: Option<String>,
    pub repo: Option<String>,
    pub subdir: Option<String>,
    pub requested_ref: Option<String>,
    pub resolved_commit: Option<String>,
    pub managed_path: Option<String>,
    pub installed_unix_ms: Option<u64>,
}
```

Managed GitHub paths are checked against the expected Herdr plugin data path. The source normalization rejects GitHub plugin metadata whose `managed_path` does not match the plugin id or whose manifest is outside the managed checkout.

## Registry persistence

Installed and linked plugins persist across restarts in `plugins.json`, next to Herdr's session state.

Implementation: `src/persist/plugin_registry.rs`.

Important behavior:

- `save()` atomically writes pretty JSON through a temp file.
- `load()` returns an empty list on missing, unreadable, or corrupt registry files so startup is not blocked.
- On startup/reload, Herdr re-reads each plugin's manifest from its original path.
- If a manifest is missing or unparseable, the registry entry is kept and a warning is attached.

The reload behavior is intentionally tolerant:

```rust
/// If the manifest parses successfully, replace cached fields but keep the
/// stored `enabled` flag. If the file is gone or unparseable, keep the stored
/// entry and append a warning so `plugin.list` surfaces it.
```

Unlinking a plugin removes it from `installed_plugins` and saves the registry. If a plugin had plugin-pane ownership records, those records are dropped for the unlinked plugin, but the panes themselves keep running.

## User config and state directories

Implementation: `src/plugin_paths.rs` and `src/app/api/plugins/env.rs`.

Herdr gives each plugin:

- `HERDR_PLUGIN_CONFIG_DIR`
- `HERDR_PLUGIN_STATE_DIR`

Path helpers:

```rust
pub(crate) fn plugin_config_dir(plugin_id: &str) -> PathBuf {
    crate::config::config_dir()
        .join("plugins")
        .join("config")
        .join(plugin_config_path_component(plugin_id))
}

pub(crate) fn plugin_state_dir(plugin_id: &str) -> PathBuf {
    crate::config::state_dir()
        .join("plugins")
        .join(plugin_config_path_component(plugin_id))
}
```

Herdr creates these directories on link/install and before runtime command launch. It may seed config from legacy plugin config locations, but it does not manage schemas, migrations, cleanup, validation, or synchronization for plugin-owned files.

The plugin root is source code / installed checkout. Docs explicitly tell authors not to store credentials or durable state in `HERDR_PLUGIN_ROOT`, because GitHub-installed roots are managed source checkouts.

## Socket API surface

Plugin methods are listed in `src/api/schema.rs`:

```rust
#[serde(rename = "plugin.link")]
PluginLink(PluginLinkParams),
#[serde(rename = "plugin.list")]
PluginList(PluginListParams),
#[serde(rename = "plugin.unlink")]
PluginUnlink(PluginUnlinkParams),
#[serde(rename = "plugin.enable")]
PluginEnable(PluginSetEnabledParams),
#[serde(rename = "plugin.disable")]
PluginDisable(PluginSetEnabledParams),
#[serde(rename = "plugin.action.list")]
PluginActionList(PluginActionListParams),
#[serde(rename = "plugin.action.invoke")]
PluginActionInvoke(PluginActionInvokeParams),
#[serde(rename = "plugin.log.list")]
PluginLogList(PluginLogListParams),
#[serde(rename = "plugin.pane.open")]
PluginPaneOpen(PluginPaneOpenParams),
#[serde(rename = "plugin.pane.focus")]
PluginPaneFocus(PluginPaneFocusParams),
#[serde(rename = "plugin.pane.close")]
PluginPaneClose(PluginPaneCloseParams),
```

The CLI in `src/cli/plugin.rs` wraps these methods for humans and scripts.

## Manifest validation

Manifest loading is implemented by `load_plugin_manifest(path, enabled)` in `src/app/api/plugins/manifest.rs`.

Key validation steps:

1. Resolve directory path to `herdr-plugin.toml`, or accept direct manifest path.
2. Canonicalize manifest path.
3. Read and parse TOML.
4. Normalize plugin id.
5. Require non-empty `name` and `version`.
6. Require and validate `min_herdr_version`.
7. Normalize platforms.
8. Normalize build commands.
9. Normalize actions and reject duplicate action ids.
10. Normalize events and sort them.
11. Normalize panes and reject duplicate pane ids.
12. Normalize link handlers, validate regex patterns, reject duplicates.
13. Validate each link handler references an action declared by the same plugin.
14. Add warnings for unknown event names and missing top-level platform declarations.

### Id rules

Top-level plugin ids may use ASCII letters, digits, dot, colon, underscore, and hyphen. Action ids, pane ids, and link handler ids are local to the plugin and may use ASCII letters, digits, colon, underscore, and hyphen, but not dots.

Action ids become globally unique when qualified as:

```text
plugin.id.action
```

### Link handler validation

For link handlers, Herdr validates:

- non-empty id
- non-empty title
- non-empty regex pattern
- regex compiles
- action id is valid
- action exists in the same plugin
- link handler ids are unique within a plugin

## Actions

Actions are manifest-declared commands that users can list and invoke.

Schema from `src/api/schema/plugins.rs`:

```rust
pub struct PluginManifestAction {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub contexts: Vec<PluginActionContext>,
    pub platforms: Option<Vec<PluginPlatform>>,
    pub command: Vec<String>,
}
```

Action contexts:

```rust
pub enum PluginActionContext {
    Global,
    Workspace,
    Tab,
    Pane,
    Selection,
}
```

`plugin.action.list` returns all actions across installed plugins, optionally filtered by plugin id. `PluginActionInfo::qualified_id()` returns `plugin_id.action_id`.

`plugin.action.invoke`:

1. resolves the action by qualified or bare id
2. rejects disabled plugins
3. enforces platform support
4. merges supplied context with current Herdr context
5. starts the command asynchronously
6. returns action info, invocation context, and an initial running command log

Bare action ids are allowed only when unambiguous. If more than one plugin has the same local action id, invocation returns an `ambiguous_plugin_action` error and the caller must specify `plugin_id` or use a qualified id.

## Keybinding integration

The docs show this keybinding form:

```toml
[[keys.command]]
key = "prefix+l"
type = "plugin_action"
command = "example.layout.apply"
description = "apply layout"
```

At runtime, keybindings call `invoke_plugin_action_from_keybind`, which resolves the action, checks enabled/platform support, builds current context with `invocation_source = "keybinding"`, and starts the plugin command.

## Event hooks

Event hooks are manifest-declared commands that run when Herdr emits a matching event.

Schema:

```rust
pub struct PluginManifestEventHook {
    pub on: String,
    pub platforms: Option<Vec<PluginPlatform>>,
    pub command: Vec<String>,
}
```

Allowed plugin-hook event kinds are intentionally narrower than all event kinds. Defined in `src/api/schema/events.rs`:

```rust
pub const PLUGIN_HOOK_EVENT_KINDS: &[EventKind] = &[
    EventKind::WorkspaceCreated,
    EventKind::WorkspaceUpdated,
    EventKind::WorkspaceClosed,
    EventKind::WorkspaceRenamed,
    EventKind::WorkspaceFocused,
    EventKind::WorktreeCreated,
    EventKind::WorktreeOpened,
    EventKind::WorktreeRemoved,
    EventKind::TabCreated,
    EventKind::TabClosed,
    EventKind::TabRenamed,
    EventKind::TabFocused,
    EventKind::PaneCreated,
    EventKind::PaneClosed,
    EventKind::PaneFocused,
    EventKind::PaneMoved,
    EventKind::PaneExited,
    EventKind::PaneAgentDetected,
    EventKind::PaneAgentStatusChanged,
];
```

`pane.output_changed` exists as an event kind but is excluded from plugin hook events because it is high volume. The source comment says output-change hook semantics are intentionally deferred.

At link time, unknown event names are warnings, not hard errors. At runtime, `run_plugin_event_hooks`:

1. converts the emitted event to its dot-name, such as `worktree.created`
2. returns immediately if the event kind is not in `PLUGIN_HOOK_EVENT_KINDS`
3. finds enabled plugins with matching hooks
4. serializes the event envelope to JSON
5. builds event-specific plugin context
6. starts each matching hook command asynchronously

Event hooks receive:

- `HERDR_PLUGIN_EVENT`
- `HERDR_PLUGIN_EVENT_JSON`
- `HERDR_PLUGIN_CONTEXT_JSON`

## Link handlers

Link handlers route modified-click terminal URLs to plugin actions.

Schema:

```rust
pub struct PluginManifestLinkHandler {
    pub id: String,
    pub title: String,
    pub pattern: String,
    pub action: String,
    pub platforms: Option<Vec<PluginPlatform>>,
}
```

Docs define behavior:

- `pattern` is a Rust regex matched against the clicked URL.
- `action` must refer to an action declared by the same plugin.
- Modified-click uses Control on every platform, including macOS, because captured terminal mouse input does not distinguish Command/Super reliably.
- Handlers are checked in manifest order inside each plugin.

Implementation behavior:

- enabled plugins only
- unavailable manifests are skipped
- plugin-level and handler/action-level platform support are checked
- malformed regexes are rejected at link time; runtime also defensively skips regex compile failures
- matching handler invokes its action with `invocation_source = "link_click"`

Link handler action context includes:

- clicked URL
- link handler id
- focused pane context

Runtime env includes:

- `HERDR_PLUGIN_CLICKED_URL`
- `HERDR_PLUGIN_LINK_HANDLER_ID`

## Plugin panes

Plugin panes are manifest-declared terminal entrypoints.

Schema:

```rust
pub struct PluginManifestPane {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub platforms: Option<Vec<PluginPlatform>>,
    pub placement: PluginPanePlacement,
    pub command: Vec<String>,
}
```

Placements:

```rust
pub enum PluginPanePlacement {
    Overlay,
    Split,
    Tab,
    Zoomed,
}
```

`plugin.pane.open` requires:

- installed plugin
- manifest available
- plugin enabled
- valid entrypoint id
- matching `[[panes]]` entry
- platform compatibility

Placement behavior:

- `overlay`: targets the active pane; request must not provide workspace/target/direction
- `split`: targets an existing pane, defaulting to current pane; may specify split direction
- `zoomed`: implemented like split plus tab zooming/focus behavior
- `tab`: opens a new tab in a target or active workspace

Plugin pane commands receive plugin path env, socket env, context JSON, and `HERDR_PLUGIN_ENTRYPOINT_ID`.

Herdr records plugin pane ownership in `state.plugin_panes`:

```rust
PluginPaneRecord {
    plugin_id,
    entrypoint,
}
```

Plugin panes behave like normal Herdr panes after opening. They can be moved by normal pane/layout APIs, and Herdr keeps plugin-pane ownership attached to the pane record. `plugin.pane.focus` and `plugin.pane.close` only operate on panes Herdr knows were opened through the plugin API.

When unlinking a plugin, Herdr removes plugin-pane ownership records for that plugin but does not kill the underlying panes.

## Runtime command launch

Runtime command launch is implemented by `start_plugin_command` in `src/app/api/plugins/runtime.rs`.

Important constants:

```rust
const PLUGIN_COMMAND_OUTPUT_MAX_BYTES: usize = 64 * 1024;
pub(super) const MAX_PLUGIN_COMMANDS_IN_FLIGHT: usize = 32;
const PLUGIN_COMMAND_LOG_LIMIT: usize = 200;
```

Launch sequence:

1. Require non-empty command array.
2. Split program and args.
3. Serialize `PluginInvocationContext` to JSON.
4. Ensure plugin config/state dirs exist.
5. Allocate a log id.
6. Build runtime env.
7. Enforce max 32 concurrent plugin commands.
8. Push a `Running` log record.
9. Increment in-flight counter.
10. Spawn an OS thread.
11. Spawn the plugin child process with cwd set to plugin root.
12. Capture stdout/stderr with a 64 KiB cap each.
13. Wait for child completion.
14. Send `AppEvent::PluginCommandFinished` back to the app event loop.

Commands run with:

- `current_dir(plugin_root)`
- stdout piped
- stderr piped
- injected env

There is no sandboxing and no plugin-specific permission model beyond install/link trust.

## Runtime environment

Runtime commands receive:

```text
HERDR_SOCKET_PATH
HERDR_BIN_PATH
HERDR_ENV=1
HERDR_PLUGIN_ID
HERDR_PLUGIN_ROOT
HERDR_PLUGIN_CONFIG_DIR
HERDR_PLUGIN_STATE_DIR
HERDR_PLUGIN_CONTEXT_JSON
HERDR_WORKSPACE_ID
HERDR_TAB_ID
HERDR_PANE_ID
```

Action commands additionally receive:

```text
HERDR_PLUGIN_ACTION_ID
```

Event hooks additionally receive:

```text
HERDR_PLUGIN_EVENT
HERDR_PLUGIN_EVENT_JSON
```

Pane commands additionally receive:

```text
HERDR_PLUGIN_ENTRYPOINT_ID
```

Link handlers may receive:

```text
HERDR_PLUGIN_CLICKED_URL
HERDR_PLUGIN_LINK_HANDLER_ID
```

`HERDR_BIN_PATH` points to the running Herdr binary and is the recommended portable callback mechanism. `HERDR_SOCKET_PATH` is available for raw socket clients, but the socket transport is platform-specific: Unix socket on Unix, named pipe on Windows.

## Invocation context

Context schema is in `src/api/schema/plugins.rs`:

```rust
pub struct PluginInvocationContext {
    pub workspace_id: Option<String>,
    pub workspace_label: Option<String>,
    pub workspace_cwd: Option<String>,
    pub worktree: Option<WorkspaceWorktreeInfo>,
    pub tab_id: Option<String>,
    pub tab_label: Option<String>,
    pub focused_pane_id: Option<String>,
    pub focused_pane_cwd: Option<String>,
    pub focused_pane_agent: Option<String>,
    pub focused_pane_status: Option<AgentStatus>,
    pub selected_text: Option<String>,
    pub invocation_source: Option<String>,
    pub correlation_id: Option<String>,
    pub clicked_url: Option<String>,
    pub link_handler_id: Option<String>,
}
```

`merge_plugin_context` builds current context from the active Herdr state, then lets request-provided fields override missing fields.

Context can be built for:

- current active workspace/tab/pane
- a specific workspace id
- a specific tab id
- a specific pane id
- an event envelope
- a clicked URL/link handler invocation

Event-specific context tries to preserve useful identity even for closed/removed resources by falling back to snapshot data from the event envelope.

## Command logs

Plugin command logs are exposed via `plugin.log.list`.

Log schema:

```rust
pub struct PluginCommandLogInfo {
    pub log_id: String,
    pub plugin_id: String,
    pub action_id: Option<String>,
    pub event: Option<String>,
    pub command: Vec<String>,
    pub status: PluginCommandStatus,
    pub started_unix_ms: u64,
    pub finished_unix_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub error: Option<String>,
}
```

Statuses:

```rust
pub enum PluginCommandStatus {
    Running,
    Succeeded,
    Failed,
}
```

The in-memory log is capped to 200 entries. `plugin.log.list` defaults to 50 and clamps requested limits to 1..=200.

## Build commands

Build commands are declared with `[[build]]` and run during GitHub install only. They do not receive runtime Herdr socket/plugin context env. They are ordinary argv commands used to prepare the managed checkout before registration.

If any build command fails, install aborts and the plugin is not registered. Build failures include plugin id, build index, working directory, command, exit status/spawn error, and capped stdout/stderr.

## Trust and security model

Herdr's plugin security model is explicit trust, not sandboxing.

Important properties:

- Plugin code runs as the current user.
- Plugin code can access the user's environment.
- Plugin code can call the full Herdr CLI/socket API.
- Plugin code can run any local command available to the user.
- Herdr validates the manifest but does not review or constrain the implementation.
- GitHub install shows an interactive preview unless `--yes` is used.
- Users are expected to install/link only trusted plugins.

This is comparable to editor extensions, shell plugins, and local automation scripts.

## Boundaries and non-goals in plugin v1

Herdr plugin v1 does not include:

- sandboxing
- capability permissions
- in-process plugin loading
- runtime action registration
- native non-terminal plugin UI
- managed plugin storage/database API
- plugin code review or trust guarantees
- automatic plugin updates

## Design takeaways

Herdr's plugin architecture is intentionally simple and host-owned:

1. **Manifest first**: all extension points are declared statically.
2. **Out-of-process execution**: plugin code never runs inside the Herdr process.
3. **CLI/socket as API**: plugins compose existing Herdr capabilities instead of requiring a separate SDK.
4. **Context injection**: commands are given rich runtime context via JSON and environment variables.
5. **Platform-aware**: top-level and item-level platform filters avoid launching unsupported commands.
6. **Persisted registry**: linked/installed plugins survive restart, while broken manifests degrade to warnings.
7. **Command logs**: plugin execution is observable after launch.
8. **No sandbox promises**: security is handled through explicit user trust and install/link review.
9. **High-volume event caution**: output-change events are deliberately excluded from plugin hooks.
10. **Terminal UI via panes**: plugin UI is an ordinary terminal process managed by Herdr.
