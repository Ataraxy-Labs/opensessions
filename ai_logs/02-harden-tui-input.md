# Harden TUI Against tmux send-keys Injection - 2026-05-05

## Summary

Closed a vulnerability where any process inside the same tmux server could
invoke arbitrary opensessions UI actions — including a global, server-wide
quit that killed every sidebar pane and called `process.exit(0)` — by simply
running `tmux send-keys -t <opensessions-sidebar-pane>` to inject characters
into the TUI.

The original report described the symptom as "my tmux client gets switched to
the targeted session". After empirical reproduction the actual mechanism was:
characters injected via `send-keys` reached the sidebar TUI's keyboard handler
indistinguishably from real keystrokes; the single letter `q` (e.g. inside
the user's `tmux send-keys "mysql --version" Enter` command) immediately fired
`send({type:"quit"})`, which the server handled by tearing down every sidebar
across every session and exiting. Reproduced minimally with
`env -u TMUX tmux send-keys -t <sidebar-pane-id> q` from a non-tmux shell —
the server PID died, every sidebar pane was killed.

---

## Root Cause

Three independent issues stacked into one catastrophic outcome:

1. **The TUI cannot distinguish forged PTY input from human typing.** Every
   single-letter shortcut (`q`, `x`, `d`, `n`, `c`, `r`, `t`, `u`, `f`,
   digits, `Tab`, `Enter`, `Alt+Up/Down`) acted on injected bytes the same
   way it acted on real keystrokes.
2. **Destructive shortcuts had no confirmation strong enough to resist
   injection.** `q` immediately quit. `x` opened a confirm modal that a
   single subsequent `y` confirmed.
3. **`q` had global blast radius.** The server's `quitAll()` killed every
   sidebar pane in every session and then called `process.exit(0)`, so a
   single injected character took down the entire opensessions install for
   every session the user had open.

A separate but related bug — the sidebar TUI pane sometimes ends up the
*active* pane in its window, which is why `tmux send-keys -t SESSION:WINDOW`
(without `.PANE`) hit the sidebar instead of the user's intended target —
was handed off to a dedicated thread for investigation.

---

## Fix (layered defense)

### Layer 1 — Terminal focus tracking (primary)

The TUI now enables DECSET 1004 (`\x1b[?1004h`) on startup and listens for
focus-in (`\x1b[I`) / focus-out (`\x1b[O`) escape sequences on stdin. Real
human typing only reaches a tmux pane while it's focused; `tmux send-keys` to
a non-focused pane delivers raw bytes but does NOT cause a focus-in event.

A new signal `paneHasTerminalFocus` gates every destructive shortcut early in
the keyboard handler:

```ts
if (!paneHasTerminalFocus()) return;
```

Default value is `true` — graceful degradation in terminals that don't support
focus reporting (the user still gets defense-in-depth from layers 2 and 3).
After tmux's `refocusMainPane` selects the main pane post-spawn, tmux emits a
focus-out to the sidebar and `paneHasTerminalFocus` flips to `false`,
correctly tracking state from then on.

The provider's `setupHooks()` now sets `tmux set-option -g focus-events on`
so focus events propagate from the outer client into inner panes.

### Layer 2 — Modal confirmations require Enter, not bare letters

Both confirm modals (`confirm-kill`, new `confirm-quit`) now accept ONLY
`Enter` as a confirmation. Anything else cancels. Previously `confirm-kill`
treated any `y` keystroke as a confirm, so an injected string like
`"mysql --version"` containing `y` could slip through after an `x` injected
the modal open.

### Layer 3 — `q` is now scoped, not global

`q` no longer sends `{type:"quit"}`. After the user confirms the modal it
sends a new `{type:"close-sidebar"}` command. The server's `closeLocalSidebar`
handler kills only the requesting client's sidebar pane (looked up via the
`clientPaneIds` map populated by `identify-pane`). It never calls
`process.exit` and never touches sidebars in other sessions.

`quitAll()` is preserved for explicit plugin unload paths but is no longer
reachable from a single keystroke.

---

## Files Changed

- `packages/runtime/src/shared.ts` — added `close-sidebar` to ClientCommand union.
- `packages/runtime/src/server/index.ts` — new `clientPaneIds` WeakMap, new
  `closeLocalSidebar` function, new `case "close-sidebar"` in handleCommand,
  identify-pane now records pane id.
- `packages/mux/providers/tmux/src/provider.ts` — `setupHooks` now sets
  `focus-events on` globally.
- `apps/tui/src/index.tsx` — new `paneHasTerminalFocus` signal, stdin
  focus-event observer in `onMount`, `q` opens `confirm-quit` modal,
  `confirm-kill` requires Enter, destructive shortcuts gated on focus,
  new `confirm-quit` modal UI, kill modal label updated to `enter / esc`.
- `packages/runtime/test/close-sidebar.test.ts` — new contract tests covering
  every layer of the fix.

---

## Verification

- Empirically reproduced the original bug: `env -u TMUX tmux send-keys -t %62
  q` killed the server and all 11 sidebar panes (PID 14254 → dead).
- New tests pass (12/12 in close-sidebar.test.ts).
- Full runtime suite: 374 pass / 5 pre-existing failures unchanged
  (all in `even-horizontal shell helpers`).

To validate after deploying the fix:

1. Restart opensessions so the running server picks up the new build.
2. From a non-tmux shell: `env -u TMUX tmux send-keys -t <sidebar-pane-id> q`.
3. Server should still be alive; targeted sidebar pane should be unchanged
   (focus-event gating drops the keystroke).
4. Click into the sidebar pane manually, press `q` — confirm modal appears.
5. Press Enter — only that one sidebar pane closes; other sessions' sidebars
   and the server stay up.

---

## Out of Scope

- The "sidebar pane becomes the active pane" bug is being addressed in a
  separate thread (T-019df8bb-66e8-73f9-b1c4-aef842ed868e).
- Server-side per-action authorization (the WS auth token is sufficient for
  the reported threat model — accidental injection from agents that share
  the same tmux server). A determined attacker who can read the token can
  still send WS commands directly; defending against that needs a different
  trust boundary.
