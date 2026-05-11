# Configuration Reference

This page documents the configuration inputs that opensessions reads today.

## Config File Location

User config is loaded from:

```text
~/.config/opensessions/config.json
```

If the file does not exist, opensessions falls back to defaults.

## Recommended Config Shape

```json
{
  "mux": "tmux",
  "plugins": [],
  "theme": "tokyo-night",
  "sidebarWidth": 30,
  "sidebarPosition": "right"
}
```

## Config Fields

| Field | Type | Default | Runtime status | Description |
| --- | --- | --- | --- | --- |
| `mux` | `string` | auto-detect | active | Selects the preferred registered mux provider by name |
| `plugins` | `string[]` | `[]` | active | Package names to load through the plugin loader |
| `theme` | `string` | `catppuccin-mocha` | active | Built-in theme name persisted by the TUI |
| `sidebarWidth` | `number` | `26` | active | Sidebar width in columns |
| `sidebarPosition` | `"left" | "right"` | `"left"` | active | Sidebar placement |
| `lonelySidebarPolicy` | `"kill" | "spawn-shell"` | `"kill"` | active | What to do when the sidebar is the only pane left in a window — see below |
| `port` | `number` | none | parsed only | Present in the config type, but the current server and TUI still use the fixed `7391` constant |
| `keybinding` | `string` | none | parsed only | Present in the config type, but keybindings are configured outside this file today |

## Lonely Sidebar Policy

When you close the last non-sidebar pane in a window (e.g. `exit` your shell), the sidebar is briefly the only pane left. `lonelySidebarPolicy` controls what happens next:

- `"kill"` (default) — kill the lonely sidebar. The window then closes itself the way native tmux behaves when the last pane exits. Best when you treat the sidebar like just another pane.
- `"spawn-shell"` — insert a fresh shell pane next to the sidebar so the window stays alive. Useful if you treat the sidebar as window chrome and want a recovery path after accidentally closing your shell.

The setting can also be supplied via the `OPENSESSIONS_LONELY_SIDEBAR_POLICY` environment variable, which takes precedence over the config file (handy for one-off testing without editing `~/.config/opensessions/config.json`).

## Built-In Themes

These theme names resolve in the running app today:

- `catppuccin-mocha`
- `catppuccin-latte`
- `catppuccin-frappe`
- `catppuccin-macchiato`
- `tokyo-night`
- `gruvbox-dark`
- `nord`
- `dracula`
- `github-dark`
- `one-dark`
- `kanagawa`
- `everforest`
- `material`
- `cobalt2`
- `flexoki`
- `ayu`
- `aura`
- `matrix`

## Inline Theme Objects

The core config type and theme resolver also support partial inline theme objects such as:

```json
{
  "theme": {
    "palette": {
      "base": "#000000",
      "text": "#ffffff"
    }
  }
}
```

That shape is valid for the core APIs, but the current server startup path only applies string theme names end-to-end.

## tmux Plugin Options

The tmux integration reads these tmux options instead of `config.json`:

| tmux option | Default | Used by |
| --- | --- | --- |
| `@opensessions-prefix-key` | `o` | Key after the tmux prefix that enters the opensessions key table (`prefix <key>`) |
| `@opensessions-focus-global-key` | unset | Optional no-prefix tmux keybinding that reveals and focuses the sidebar pane |
| `@opensessions-index-keys` | unset | Optional space-separated no-prefix tmux keys mapped in order to visible sessions `1` through `9` |
| `@opensessions-width` | `26` | exported as `OPENSESSIONS_WIDTH` by the tmux bootstrap script |

The plugin registers these prefix bindings automatically:

| Binding | Action |
| --- | --- |
| `prefix o → s` | Reveal and focus the sidebar |
| `prefix o → t` | Toggle the sidebar |
| `prefix o → e` | Spread non-sidebar panes in the current window using `even-horizontal` |
| `prefix o → 1` through `prefix o → 9` | Switch to visible session by index |

Minimal install:

If you use TPM, this is enough:

```tmux
set -g @plugin 'Ataraxy-Labs/opensessions'
```

After adding it, reload tmux and ask TPM to install plugins:

```bash
tmux source-file ~/.tmux.conf
~/.tmux/plugins/tpm/bin/install_plugins
```

If you run from a local checkout instead, this is enough:

```tmux
source-file /absolute/path/to/opensessions/opensessions.tmux
```

Optional overrides:

```tmux
set -g @opensessions-width "30"
set -g @opensessions-prefix-key "o"   # default; change to remap the opensessions key table
```

All other tmux options fall back to the defaults shown in the table above.

- Use `@opensessions-focus-global-key` and `@opensessions-index-keys` only when you explicitly want no-prefix tmux bindings and know they do not conflict with your window manager or terminal.

## Environment Variables

| Variable | Used by | Notes |
| --- | --- | --- |
| `OPENCODE_DB_PATH` | OpenCode watcher | Overrides the default SQLite path |
| `OPENSESSIONS_DIR` | tmux helper scripts and server | Helps helper scripts find the repo checkout |
| `OPENSESSIONS_HOST` | helper shell scripts | Script-level override only; the app runtime still uses `127.0.0.1` |
| `OPENSESSIONS_PORT` | helper shell scripts | Script-level override only; the app runtime still uses `7391` |
| `SESSIONIZER_DIR` | tmux sessionizer popup | Colon-separated directories searched for new-session candidates (e.g. `$HOME/Code:$HOME/.config`). Also checked via `tmux show-environment -g` when the shell variable is unset. Defaults to `$HOME/Documents` |
| `SESSIONIZER_MAXDEPTH` | tmux sessionizer popup | Maximum `find` depth when collecting new-session candidates. Also checked via `tmux show-environment -g` when the shell variable is unset. Defaults to `3` |
| `BUN_PATH` | helper scripts | Explicit Bun binary path for helper scripts |

## Related Files Written By The Runtime

| Path | Purpose |
| --- | --- |
| `~/.config/opensessions/session-order.json` | Persisted custom session ordering |
| `/tmp/opensessions.pid` | PID file used by server bootstrap logic |
| `/tmp/opensessions-debug.log` | Best-effort debug log written by the server and providers |

## Mux Detection Rules

If `mux` is unset, the supported built-in auto-detection path is:

1. `$TMUX` -> provider named `tmux`
2. no supported match -> `null`

Experimental or plugin-provided providers may implement their own detection behavior, but tmux is the only supported built-in mux today.
