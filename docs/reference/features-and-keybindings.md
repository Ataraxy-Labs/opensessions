# Features And Keybindings Reference

This page lists the user-visible features currently implemented in the sidebar.

## Session List

Each session row can show:

- Session index for number-key switching
- Session name
- Branch name, truncated when needed
- Running spinner or terminal-state marker
- Focused state highlight
- Current client session emphasis
- Unseen accent for `done`, `error`, or `interrupted` states

## Detail Panel

The focused session detail panel can show:

- Truncated working directory
- Detected localhost ports
- Agent instances in that session
- Thread names when watchers provide them

Clicking a detected port opens `http://localhost:<port>`.

## Agent Features

- Multiple agent instances per session when a watcher emits `threadId`
- Per-instance unseen tracking
- Amp thread seen-state integration via Amp's `session.json`
- Status values: `idle`, `running`, `done`, `error`, `waiting`, `interrupted`
- Automatic pruning of stale running and seen terminal states

## Session Metadata Features

- Git branch lookup from the session directory
- Dirty-worktree detection
- Git worktree detection
- Pane count and window count per session
- Session uptime display
- Session ordering persisted across restarts
- Drag-and-drop session and worktree-group ordering in the unfiltered session list

## Sidebar Management Features

### tmux-specific

- Global hooks for session changes, window selection, window creation, resize, and refresh
- Sidebar stash session named `_os_stash` so hidden sidebars can be restored
- Session creation popup using the bundled `sessionizer.sh` script

## Keyboard Shortcuts

### Inside the sidebar

| Key | Action |
| --- | --- |
| `j`, `Down` | Move focus down |
| `k`, `Up` | Move focus up |
| `Enter` | Switch to focused session |
| `Tab` | Cycle to next session |
| `Shift+Tab` | Cycle to previous session |
| `1`-`9` | Switch to session by visible index |
| `Alt+Up` | Move focused session up in persisted order |
| `Alt+Down` | Move focused session down in persisted order |
| `n`, `c` | Create a new session |
| `d`, `x` | Open kill-session confirmation for focused session |
| `t` | Open theme picker |
| `r` | Refresh state |
| `q` | Quit the server and all sidebar panes |
| `Esc` | Close only the current sidebar client |

### tmux plugin shortcuts

| Key | Action |
| --- | --- |
| `prefix o → s` | Reveal and focus the sidebar pane |
| `prefix o → t` | Toggle the sidebar |
| `prefix o → e` | Spread non-sidebar panes in the current window using `even-horizontal` |
| `prefix o → 1` through `prefix o → 9` | Switch directly to the visible session indices |
| Configurable `@opensessions-focus-global-key` such as `Alt-s` | Reveal and focus the sidebar pane from any tmux pane |
| Configurable `@opensessions-index-keys` such as `Alt-1` through `Alt-9` | Switch directly to the visible session indices from any tmux pane |

## Session Creation Behavior

- The server exposes `new-session` through the mux provider interface.
- The TUI uses a tmux popup sessionizer when it is running inside tmux.
- The bundled sessionizer searches directories listed in `SESSIONIZER_DIR` (colon-separated, e.g. `$HOME/Code:$HOME/.config`) or `$HOME/Documents` if unset. The variable is also read from the tmux global environment (`tmux set-environment -g`) as a fallback.
- Search depth is controlled by `SESSIONIZER_MAXDEPTH` (defaults to `3`).
- If `fzf` is unavailable, the tmux sessionizer exits with a prompt explaining that dependency.

## Session Switching Behavior

- Switching routes through the server so the server can use authoritative client TTY information.
- tmux is the only supported mux today.

## Refresh And Discovery Behavior

- Git info is cached for 5 seconds.
- Listening localhost ports are re-polled every 10 seconds while clients are connected.
- Amp and Claude Code watchers poll every 2 seconds in addition to file watching.
- OpenCode polls every 3 seconds.

## Files And Paths The UI Depends On

- `~/.local/share/amp/threads/`
- `~/.local/share/amp/session.json`
- `~/.claude/projects/`
- `~/.local/share/opencode/opencode.db`
- `~/.config/opensessions/config.json`

## Related Docs

- [Configuration reference](./configuration.md)
- [Architecture explanation](../explanation/architecture.md)
- [Contracts and extension interfaces](../../CONTRACTS.md)
