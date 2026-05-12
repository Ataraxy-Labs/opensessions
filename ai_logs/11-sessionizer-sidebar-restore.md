# Sessionizer Sidebar Restore - 2026-05-12

## Summary

Fixed a tmux sidebar regression where creating or switching to a session from the `opensessions` new-session popup could leave the user in the target session without a sidebar.

---

## Details

The popup sessionizer script creates and switches tmux sessions directly from inside the popup. That bypasses the TUI's normal WebSocket `switch-session` path, so the server did not get an immediate, authenticated request to refresh state and ensure the sidebar in the destination window.

The fix passes the active opensessions server connection details into the popup and has the sessionizer notify the server after `tmux switch-client` succeeds. The notification is best-effort and uses the existing authenticated HTTP endpoints:

- `POST /refresh` so the new/existing session list is up to date.
- `POST /ensure-sidebar` with `client_tty|session|window_id` so the sidebar is restored in the target window.

This runs for both paths: switching to an already-existing session and creating a brand-new detached session.

---

## Files Changed

- `apps/tui/src/index.tsx` - Passes `OPENSESSIONS_HOST`, `OPENSESSIONS_PORT`, and `OPENSESSIONS_TOKEN_FILE` into the tmux popup environment.
- `apps/tui/scripts/sessionizer.sh` - Adds `notify_opensessions` and calls it after switching to the target session.
- `packages/runtime/test/close-sidebar.test.ts` - Adds source-level regression coverage for the popup/server notification wiring.

---

## Verification

```bash
bun test packages/runtime/test/close-sidebar.test.ts
bun run build  # from apps/tui
bash -n apps/tui/scripts/sessionizer.sh
```
