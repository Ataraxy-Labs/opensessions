# Sidebar Behavior E2E Coverage Matrix

This file maps the product contract in [`sidebar-behavior.md`](./sidebar-behavior.md) to the real tmux E2E tests that protect it.

The intended test surface is product E2E only: each test creates real fake git repositories/worktrees, a real isolated tmux server/socket, a real opensessions server, real sidebar panes, and PTY-backed attached tmux clients.

## Covered by `apps/tui-rs/tests/tmux_e2e.rs`

| Behavior | E2E coverage |
| --- | --- |
| Session keyboard focus is local, `Tab` switches, `j`/`k` browse temporary focus, and worktree group focus rehomes to the chosen child session. | `tmux_sidebar_keyboard_focus_and_worktree_flow` |
| Explicit foreground sidebar resize persists once and fans out to every managed sidebar pane in the tmux server. | `tmux_sidebar_width_resize_fans_out_to_every_session_sidebar` |
| `q` in a connected sidebar shuts down the server and every connected sidebar client. | `tmux_sidebar_quit_closes_the_server_and_every_sidebar_client` |
| Two attached tmux clients can keep independent active rows instead of a global server focus row overriding every sidebar. | `tmux_sidebar_multiple_clients_keep_independent_active_rows` |
| Two different tmux sockets have isolated ports, servers, width state, and sidebar state. | `tmux_sidebar_state_is_isolated_per_tmux_socket` |
| `q` in a normal/main tmux pane does not quit opensessions. | `tmux_sidebar_q_in_main_pane_does_not_quit_opensessions` |
| Pane topology repair must not let tmux permanently donate freed space to the sidebar. | `tmux_sidebar_pane_exit_does_not_steal_sidebar_width` |
| Resizing and immediately switching sessions preserves the latest drag-owned width through handoff. | `tmux_sidebar_resize_immediately_before_switch_survives_handoff` |
| A single resize immediately followed by a switch is adopted from the source window even if no prior drag report established an owner. | `tmux_sidebar_single_resize_immediately_before_switch_is_adopted` |
| Session switching remains responsive while 100 websocket sidebar clients are connected and invalidation/refetch traffic is bursting. | `tmux_sidebar_switch_stays_responsive_with_100_connected_clients` |

## Important Invariants Covered Indirectly

- Startup/layout-settle resize reports are rejected and repaired back to the coordinator-owned width.
- Background sidebar width reports are treated as echoes, not new user intent.
- A continued drag owner may report a final width after session focus has moved.
- Width fanout skips stale self-fighting during active user drag, then converges all panes.
- Normal websocket clients coalesce broadcast state to latest-wins frame cadence so stale snapshots cannot congest high-priority input.
- E2E tests are serialized inside the process because tmux, PTYs, product binaries, and debug logs are external resources even when each test uses an isolated socket.

## Not Yet Fully Automated

These are still mostly protected by final-state assertions rather than visual frame-by-frame checks:

- no visible intermediate flicker during `Tab` and `Enter` switching
- full terminal resize across multiple actual terminal emulator window sizes
- control-mode client interference
- tmux `window-size latest` restoration after every possible resize path

If one of these regresses, add another product E2E in `tmux_e2e.rs` before changing lower-level code.
