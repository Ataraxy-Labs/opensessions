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
  "theme": "tokyo-night",
  "sidebarWidth": 30,
  "sidebarPosition": "right"
}
```

## Config Fields

| Field | Type | Default | Runtime status | Description |
| --- | --- | --- | --- | --- |
| `mux` | `string` | auto-detect | active | Selects the preferred registered mux provider by name |
| `plugins` | `string[]` | `[]` | parsed only | Compatibility field from the old TypeScript plugin loader; the Rust server does not execute plugin packages today |
| `theme` | `string` | `catppuccin-mocha` | active | Built-in theme name persisted by the TUI |
| `sidebarWidth` | `number` | `26` | active | Sidebar width in columns |
| `sidebarPosition` | `"left" | "right"` | `"left"` | active | Sidebar placement |
| `port` | `number` | none | parsed only | Present in the config type; use `OPENSESSIONS_PORT`/tmux-scoped environment for runtime port overrides today |
| `keybinding` | `string` | none | parsed only | Present in the config type, but keybindings are configured outside this file today |

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
| `@opensessions-width` | deprecated | Use `sidebarWidth` in config or the in-sidebar width slider instead |

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

On first load, the plugin downloads the matching GitHub release bundle into `~/.tmux/plugins/opensessions/bin/`. The bundle contains `opensessions-sidebar`, `opensessions-server`, and the bundled `lazydiff` binary. You do not need Rust/Cargo for the normal TPM install path.

If you run from a local checkout instead, this is enough:

```tmux
source-file /absolute/path/to/opensessions/opensessions.tmux
```

Optional overrides:

```tmux
set -g @opensessions-prefix-key "o"   # default; change to remap the opensessions key table
```

All other tmux options fall back to the defaults shown in the table above.

- Use `@opensessions-focus-global-key` and `@opensessions-index-keys` only when you explicitly want no-prefix tmux bindings and know they do not conflict with your window manager or terminal.

## Environment Variables

| Variable | Used by | Notes |
| --- | --- | --- |
| `OPENCODE_DB_PATH` | OpenCode watcher | Overrides the default SQLite path |
| `OPENSESSIONS_DIR` | tmux helper scripts and server | Helps helper scripts find the repo checkout |
| `OPENSESSIONS_HOST` | server, sidebar, helper shell scripts | Runtime host override; normally `127.0.0.1` |
| `OPENSESSIONS_PORT` | server, sidebar, helper shell scripts | Runtime port override; normally derived from the tmux socket/server key |
| `OPENSESSIONS_SERVER_KEY` | server, sidebar, helper shell scripts | Stable key used to derive per-tmux-socket ports and PID files |
| `OPENSESSIONS_LAZYDIFF` | sidebar | Explicit lazydiff binary path override. By default the sidebar prefers the bundled sibling binary in `bin/`, then `lazydiff` on `PATH` |
| `OPENSESSIONS_SKIP_BINARY_DOWNLOAD` | TPM bootstrap | Set to `1` to skip prebuilt binary downloads and use a local `target/` build |
| `OPENSESSIONS_WIDTH` | ignored | Deprecated stale bootstrap variable; width is controlled by persisted `sidebarWidth` |
| `SESSIONIZER_DIR` | tmux sessionizer popup | Colon-separated directories searched for new-session candidates (e.g. `$HOME/Code:$HOME/.config`). Also checked via `tmux show-environment -g` when the shell variable is unset. Defaults to `$HOME/Documents` |
| `SESSIONIZER_MAXDEPTH` | tmux sessionizer popup | Maximum `find` depth when collecting new-session candidates. Also checked via `tmux show-environment -g` when the shell variable is unset. Defaults to `3` |

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

tmux is the only supported built-in mux today. Older zellij helper code exists in the repository but is not part of the documented support surface.
