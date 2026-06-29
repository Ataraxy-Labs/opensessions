# Idle Agent Hibernation - 2026-06-29

## Summary

Added automatic idle-agent hibernation to opensessions so old live agent processes can be stopped after six hours while preserving their sidebar metadata as `hibernated`.

---

## Details

- Added `hibernated` as an agent status with theme/icon support.
- Added `autoHibernate` config resolution, enabled by default with a 6-hour idle timeout.
- Added tracker APIs to find hibernation candidates and mark them hibernated without treating them as unseen terminal events.
- Added server polling that hibernates old live idle/terminal agent panes outside the active session.
- Hibernation terminates the agent process under the pane rather than killing the tmux pane/session, preserving the session shell/sidebar row.
- Because stale Amp processes were observed ignoring `SIGTERM`, hibernation now escalates to `SIGKILL` after a one-second grace period if the process is still alive.

---

## Files Changed

- `packages/runtime/src/contracts/agent.ts` - Added `hibernated` status.
- `packages/runtime/src/agents/tracker.ts` - Added hibernation candidate discovery and state transition.
- `packages/runtime/src/config.ts` - Added auto-hibernate config defaults and resolver.
- `packages/runtime/src/server/index.ts` - Added idle hibernation polling and process termination.
- `packages/runtime/src/shared.ts` and `packages/runtime/src/themes.ts` - Added status colors/icons.
- `apps/tui/src/index.tsx` - Renders hibernated agents distinctly.
- `packages/runtime/test/*` - Added/updated coverage for hibernation and status/theme contracts.

---

## Validation

```bash
bun test packages/runtime/test/agent-contract.test.ts packages/runtime/test/agent-tracker.test.ts packages/runtime/test/config.test.ts
cd packages/runtime && bun test
cd apps/tui && bun run build
git diff --check
```

All checks passed during the session.

---

## Runtime Cleanup Notes

Manually stopped old Amp parent processes that had been running for 11-33 days. These were terminated at the process level only; Amp thread history was not deleted.
