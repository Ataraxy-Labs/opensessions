# Recent Changes Review - 2026-05-11

## Summary

Reviewed the recent fork changes against `origin/main` with a skeptical focus on reliability and security regressions. The review covered auth-token plumbing, tmux hook behavior, per-instance scoping, lonely-sidebar policy, sessionizer changes, and TUI input hardening.

---

## Key Findings

- Modal confirmations for destructive actions currently run before the TUI focus gate, so an unfocused sidebar can still accept an injected/background Enter once a confirm modal is already open.
- Server hook cleanup is suppressed whenever any other live opensessions PID exists, even if that PID belongs to a different tmux socket; this can leave dead hooks installed on the exiting server's tmux socket.
- tmux hook command construction still embeds unescaped tmux-format values in shell single quotes and parses context using `|`, which breaks on valid session names containing `'` or `|`.
- PID/token files are predictable `/tmp` paths written with plain `writeFileSync`, which follows symlinks and is unsafe under a hostile same-host local-user model.
- The `spawn-shell` lonely-sidebar policy likely creates the replacement shell in the sidebar process directory (`apps/tui`) rather than the user's session/project directory.
- TUI WebSocket auth startup has no reconnect loop despite the comment saying it will retry after the token appears.

---

## Verification

```bash
bun test packages/runtime/test/server-auth.test.ts packages/runtime/test/server-instance-scope.test.ts packages/runtime/test/lonely-sidebar-policy.test.ts packages/runtime/test/close-sidebar.test.ts
```

Result: 48 pass, 0 fail.

---

## Notes

The focused tests mostly validate helper behavior and source-shape contracts. They do not exercise the most concerning runtime edge cases: modal ordering under focus loss, multi-socket hook cleanup, hook quoting with unusual session names, or lonely-sidebar shell cwd restoration.
