# Perf notes — 2026-05-06

Session of targeted runtime fixes against `feat/auto-theme-follows-system`.
Focus: idle CPU, session-switch latency, agent-emit fanout, and the
restart-race that produced multi-server zombies.

## Headline numbers

| Metric | Before | After | Change |
|---|---|---|---|
| Steady-state idle CPU (4 TUI clients) | 1.8 – 9.8% with 3 s pulse | 0.2 – 4.1% no pulse | ~3× lower mean, ~5× lower peak |
| Theme detection | `defaults read` every 3 s (~28,800 spawns/day) | kqueue file watch on `~/Library/Preferences/.GlobalPreferences.plist`, push-driven | subprocess work eliminated |
| Session-switch enforce dance (`ensure-sidebar` → `enforce START` → `ensure checking window`) | ~645 ms | ~175 ms | ~3.7× faster |
| User-felt session switch (`/switch-index` → `/ensure-sidebar` settled) | ~940 ms | ~200 ms | ~4.7× faster (residual is tmux's own switch-client redraw) |
| Broadcasts per `agent-emit` storm | 1 : 1 | 5 : 21 (~76% suppressed) | hash-dedup catches no-op status pings |
| EADDRINUSE on respawn | hit on every restart inside TIME_WAIT | impossible (singleton PID-file probe) | clean restarts |
| Idle-timeout grace window | 30 s | 5 min | enough room for `ensure_server` to bring the sidebar up after a code change |

RSS sat at 60 MB before, 65–72 MB after — within noise; the slight bump is
from the additional fs watcher and the larger broadcast hash buffer.

## Changes

| Commit | Layer | What it does |
|---|---|---|
| `a733f28` | runtime | Push-based macOS appearance watcher; broadcast hash-dedup over the serialized state |
| `540dee6` | runtime | Drop dangling `watcherBroadcastTimer` ref in `cleanup()` (latent bug, every shutdown threw) |
| `7aa9903` | runtime | `enforceSidebarWidth(reuseCache)` honored from `ensureSidebarInWindow`, halving `tmux list-panes -a` calls per switch |
| `fb3bb5c` | wire | Drop `eventTimestamps` from `SessionData` broadcast — unused by the TUI, prevented hash-dedup from working under chatty agents |
| `d48cb30` (reverted by `a082440`) | server | Tried `reusePort: true`; broke singleton invariant on macOS Bun |
| `8b7d9a0` | server | `SERVER_IDLE_TIMEOUT_MS` 30 s → 5 min |
| `a082440` | server | Singleton guard via PID-file `process.kill(pid, 0)` probe; revert reusePort |

## How the wins were measured

- **CPU / RSS:** `/bin/ps -o %cpu,rss -p $PID` sampled at 5 s intervals over a 30 s window with 4 TUI clients connected and ambient agent activity (Claude Code in `personal_assistant`, opencode in `warp` and `arcwave`).
- **Session-switch latency:** real switches captured in `/tmp/opensessions-debug.log`. Compared `[http] POST /switch-index` → first `[http] POST /ensure-sidebar` → final `[ensure] checking window` timestamps.
- **Dedup ratio:** 30 s windows of `[agent-emit]` vs `[getCurrentSession]` lines after the `eventTimestamps` removal. Pre-fix runs from a prior 8-day-warm process showed every `agent-emit` triggering a `getCurrentSession`; post-fix shows the inverse.
- **Singleton:** verified by attempting `bun run apps/server/src/main.ts` twice in succession; second invocation prints `opensessions: another server is already running (pid X). Exiting.` and exits cleanly.

## Residual cost

What remains in the user-felt session-switch latency (~200 ms) is dominated
by tmux's own `switch-client` redraw on long-running sessions with deep
scrollback. Server-side has been pushed about as far as it goes without a
protocol change. If the next painful target is shaving more off this, the
options are:

- Reduce `tmux history-limit` for everyday work,
- Cull background panes the user no longer needs in long-running sessions,
- Or move to a protocol that ships state diffs instead of full state snapshots (the natural follow-up to Palani's Ratatui PR #36, which already preserves the WS contract by design).
