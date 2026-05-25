# 01 — Overview

## Goal

Replace `apps/tui` (Bun + OpenTUI + Solid) with `apps/tui-rs` (Rust + Ratatui)
while keeping:

- `apps/server` (TS, Bun) — **unchanged**
- `packages/runtime` — **unchanged** (still source of truth for protocol & themes)
- `packages/mux/*` — **unchanged**
- WS protocol on `SERVER_PORT` — **frozen** (Rust client speaks the existing wire format)

## Why

| | Bun + OpenTUI today | Rust + Ratatui target |
|---|---|---|
| Per-process RSS | ~73–80 MB | ~10–15 MB |
| 27-pane total RSS | ~2.0 GB | ~270–400 MB |
| Cold start | ~150–250 ms | ~10–30 ms |
| GC pauses on resize | yes (Solid + V8 in Bun) | none |
| Distribution | bun + node_modules per host | single static binary (~3–5 MB) |
| Hot reload | instant | `cargo watch -x run` (~1–3 s) |

The single-process daemon approach was rejected (would require forking OpenTUI
or maintaining a custom virtual-framebuffer renderer). The data daemon already
exists (`apps/server`); the TUI is already a thin client. Per-pane RAM is the
fixed cost of "Bun + OpenTUI + Solid + module graph", and a Rust port collapses
that floor.

## Success criteria

A migration is complete when **all** of these hold:

1. **Pixel parity** — every reference snapshot in `reference-snapshots/`
   reproduces byte-for-byte at the same pane size & state. (See `14-testing.md`.)
2. **Behavioral parity** — the keybind table from the main README is honored 1:1,
   mouse clicks dispatch to the same `ClientCommand`s, modals open and close
   identically, theme picker preview/apply works.
3. **Protocol parity** — Rust client sends all 18 `ClientCommand` variants and
   handles all 6 `ServerMessage` variants. (See `03-protocol-and-types.md`.)
4. **Performance** — measured per-process RSS ≤ 20 MB after warm-up at 35×56,
   confirmed via `ps -o rss` on 27 simultaneous panes.
5. **Distribution** — `npm install opensessions` (or equivalent) on
   darwin-arm64, darwin-x64, linux-x64, linux-arm64 yields a working binary
   with no Rust toolchain on the host.

## Phased plan

### Phase 0 — Protocol freeze + codegen (TS-side, no Rust yet)

- Audit and document every `ServerMessage` / `ClientCommand` field. (Done in `03`.)
- Add a **schema version** to the WS handshake.
- Add `ts-rs` annotations to runtime types so Rust structs are generated at build time.
- Land a `packages/runtime` test that asserts the JSON shape of every variant.

**Acceptance:** `pnpm test` passes; generated Rust types compile in an empty crate.

### Phase 1 — Skeleton Rust client

- New crate at `apps/tui-rs/`.
- Stack: `ratatui`, `crossterm`, `tokio`, `tokio-tungstenite`, `serde`, `serde_json`,
  `clap`, `color-eyre`.
- Connect to `SERVER_HOST:SERVER_PORT`, read state, render a static dump of session names.
- `q` to quit; no other features.
- Goal: prove the WS roundtrip works and capture real RSS.

**Acceptance:** `cargo run` shows the session list; RSS at idle ≤ 20 MB.

### Phase 2 — Layout + read-only render

- Port the static layout (header, session list, separator, detail panel, footer).
- Apply Catppuccin Mocha (single hardcoded theme) for the first cut.
- No interactivity beyond `q`.

**Acceptance:** `cargo test` snapshot suite (using `TestBackend` + recorded
state) matches `reference-snapshots/pane-attached-session-list.ansi` byte-for-byte.

### Phase 3 — Keyboard + filter + focus

- Implement the keyboard FSM (`07-input-keyboard.md`).
- Wire `move-focus`, `cycle-filter`, `switch-session`, `quit`.
- Optimistic updates per the TS implementation.

**Acceptance:** all keybinds verified by recorded interaction tests.

### Phase 4 — Mouse + detail panel + drag-resize

- Implement mouse hit-testing & `open` spawns.
- Implement detail panel scroll + drag-resize.
- Persist detail-panel heights via the new server-side config endpoint
  (added in `10-config-and-persistence.md`).

**Acceptance:** click-to-open works; drag-resize feels smooth (debounced
log file matches what the TS version writes).

### Phase 5 — Modals + theme picker

- Confirm-kill modal.
- Theme picker with input field (use `tui-input` or hand-rolled).
- Theme preview vs. apply.

**Acceptance:** modal interactions verified by snapshot tests.

### Phase 6 — Mux integration + ensureServer

- `getLocalSessionName`, `getLocalWindowId`, `getClientTty`, `refocusMainPane`
  → either spawn `tmux` directly OR (preferred) move to server endpoints.
- Implement `ensureServer()` equivalent in Rust — spawn the TS server if not running.

**Acceptance:** Rust client launched with no server already running boots the
server and connects.

### Phase 7 — Multi-theme support + flag-gated default

- Pull theme palette from `packages/runtime` via WS state, not hardcoded.
- Ship as `OPENSESSIONS_TUI=rs` opt-in.

**Acceptance:** dogfood for 2 weeks; bug rate < 1/day.

### Phase 8 — Flip default + delete TS client

- New release: Rust client default, TS client kept as `OPENSESSIONS_TUI=ts` escape hatch.
- One release later: delete `apps/tui`.

## Out of scope

- Replacing the server. (TS, Bun, fine.)
- Replacing the mux providers. (TS, fine.)
- Removing OpenTUI from the repo's history. (Just delete `apps/tui` when done.)
- Cross-protocol-version coexistence. (Rust client targets one schema version
  at a time; server bumps version → both binaries rebuild.)
