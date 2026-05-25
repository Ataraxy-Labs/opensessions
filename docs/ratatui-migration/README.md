# Ratatui Migration — Visual Reference

This directory contains the **canonical visual reference** for the OpenTUI sidebar
as it exists today. The Rust + Ratatui port must reproduce this output **pixel-for-pixel**:
same glyphs at the same column/row, same fg/bg RGB triples, same attributes (bold/dim),
same hover/focus behavior, same keybinds.

If the new client renders even a single character differently from these snapshots
(at the same pane size, theme, and state), the migration is **not** complete.

## What's captured

| File | Pane size | State |
|---|---|---|
| `reference-snapshots/pane-attached-session-list.png` | 35×56 | **Visual ground truth** (rendered PNG) — use as the human-eye comparison |
| `reference-snapshots/pane-attached-session-list.txt` / `.ansi` | 35×56 | Currently-attached session highlighted, focused agent in detail panel (Catppuccin Mocha) |
| `reference-snapshots/pane-opensessions-self.txt` / `.ansi` | 35×55 | The opensessions repo itself focused, "Query tmux for open sessions" agent thread |
| `reference-snapshots/pane-multi-window.txt` / `.ansi` | 35×56 | A pane in a many-window session (5+ panes) |

- `.png` = pixel-perfect visual reference (the **must-match-this** image)
- `.txt` = plain text (column-accurate, glyph-accurate)
- `.ansi` = full ANSI with truecolor SGR sequences (color-accurate)
- `.meta` / `all-panes.meta` = pane geometry context

### Things visible in the PNG that the .ansi alone doesn't make obvious

- **Focused row has a filled `surface1` (#45475a) background** spanning the full pane width
  (not just behind the text). The `▌` focus bar in front is `green` (#a6e3a1), suggesting
  it doubles as the "currently-attached session" indicator (matches the bar color seen
  in the screenshot for session 4 — opensessions itself).
- **Status icons on the right edge**: `✓` in `green` for `done`, `○` in `overlay0` for `idle`.
- **Footer glyph** for cycle is `↪` (curved-arrow), not `⇥`. (The text capture lost the
  exact glyph due to ambiguous-width handling.) Actual sequence per `themes.ts` and
  the .ansi: `↪ cycle  ⏎ go  → agents  f filter` then `d hide  x kill` on next line.
- **Vertical rhythm**: 1 blank line between each session row group (3 lines + 1 blank
  = 4 rows per session). Detail panel separated by a `─` rule with one blank line
  above and below.
- **Detail panel sub-header** uses `…` as the truncation marker for long paths
  (`…ments/work/opensessions`).

## Color palette in use (Catppuccin Mocha — currently active theme)

Decoded from `\033[38;2;R;G;Bm` SGR sequences in the snapshots:

| RGB | Hex | Catppuccin token | Where it appears |
|---|---|---|---|
| 205,214,244 | `#cdd6f4` | `text` | default fg |
| 186,194,222 | `#bac2de` | `subtext1` | secondary text |
| 166,173,200 | `#a6adc8` | `subtext0` | tertiary |
| 127,132,156 | `#7f849c` | `overlay1` | dim labels |
| 108,112,134 | `#6c7086` | `overlay0` | very dim |
| 88,91,112 | `#585b70` | `surface2` | muted |
| 69,71,90 | `#45475a` | `surface1` | **bg** of focused row |
| 137,220,235 | `#89dceb` | `sky` | links / port hints |
| 148,226,213 | `#94e2d5` | `teal` | repo name when focused |
| 166,227,161 | `#a6e3a1` | `green` | done / success status |
| 180,190,254 | `#b4befe` | `lavender` | (alt accent) |
| 203,166,247 | `#cba6f7` | `mauve`/`pink` | branch / accent |
| 249,226,175 | `#f9e2af` | `yellow` | running spinner |
| 255,255,255 | `#ffffff` | (raw white) | reset/default |
| 0,0,0 | `#000000` | (reset) | hard reset |

## Source of truth for ALL themes

`packages/runtime/src/themes.ts` — `BUILTIN_THEMES` object. The Rust port must:

1. Read the same `Theme` config from server (already broadcast over WS as part of state).
2. Translate `#rrggbb` → `ratatui::style::Color::Rgb(r, g, b)` at runtime.
3. Apply the same status icon table (`status.icons`) and status color table (`status.color`).

**Do not hardcode the palette in Rust.** Themes change at runtime when the user
opens the theme picker; the client just renders whatever palette the server
hands over.

## Layout invariants the Rust port must preserve

From inspection of the snapshots + `apps/tui/src/index.tsx`:

1. **Header** (line 2): `   Sessions <count> ⚡<active> ● <unseen>` — left-padded 3 cols,
   icons in their respective tone colors (`yellow` for ⚡, `green` for ●).
2. **Session rows**: 3 lines per session
   - Row 1: ` <focus-bar> <index> <session-name>          <status-icon>`
     - `▌` left bar shown only when row is focused or attached
     - session-name in `text` when focused, `subtext0` otherwise
   - Row 2: 4-space indent + `<dirname>` (truncated, `teal` when focused else `overlay1`)
   - Row 3: 4-space indent + `<branch>` (truncated, `pink`/`mauve` when focused else `overlay0`),
     followed by ` <portHint>` in `sky` when present
3. **Separator**: full-width `─` rule in `overlay0`.
4. **Detail panel** (toggleable, height persisted per session):
   - Sub-header: `…<truncated dir>` in dim
   - Per-agent rows: `<icon> <agent-name>` followed by status icon on the right
   - Below each: agent thread title in `text` (truncated)
5. **Footer**: `⇥ cycle  ⏎ go  → agents  f filter` in dim, then `d hide  x kill`
   on a second line.
6. **Spinner**: 10-frame Braille cycle `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` at 120ms cadence, only when
   `hasRunning()` is true.
7. **Truecolor required** — no 256-color or 16-color fallback (the current TUI
   doesn't support them either; `$TERM` must be a truecolor terminal).

## Hover / mouse behavior to preserve

- **`open` URL on click** — every link in the local-links row is mouse-clickable
  (`Bun.spawn(["open", url])` in TS → `std::process::Command::new("open")` in Rust).
- **`open` dir on click** — when a session row is *focused*, clicking its dirname
  opens the directory in Finder.
- **Detail-panel resize** — drag the divider; height persisted to config per
  session. Debug log written to `/tmp/opensessions-tui-resize.log`.
- **Mouse selection of session row** — focus follows click.

## Keybind table (must be 1:1)

| Key | Action |
|---|---|
| `q` | quit (sends WS `{type:"quit"}` + HTTP fallback to `/quit`) |
| `tab` | cycle focus |
| `enter` | go to session |
| `→` | open agents view |
| `f` | cycle filter (all → agents → running) |
| `d` | hide session |
| `x` | confirm-kill modal |
| `y` (in confirm-kill) | confirm |
| any other key (in confirm-kill) | cancel |
| `Alt+↑` / `Alt+↓` | reorder session |
| `escape` | dismiss modal |

## Verification protocol after Rust port

For each reference snapshot:

```sh
# 1. Set the same theme + state in the running daemon
# 2. Run the Rust client at the same pane size
opensessions-sidebar --width 35 --height 56 > out.ansi

# 3. Diff against reference
diff <(strip-ansi reference-snapshots/pane-attached-session-list.ansi) <(strip-ansi out.ansi)
diff reference-snapshots/pane-attached-session-list.ansi out.ansi
```

Both diffs must be empty for that snapshot to be considered ported.

Add a `cargo test` integration suite that:
1. Boots a fake server with a fixed snapshot of `ServerState`.
2. Renders to an in-memory `TestBackend` of size 35×56.
3. Compares the buffer cell-by-cell against the captured `.ansi` (after parsing
   ANSI back into a cell grid — `vt100` crate).

## Out of scope (intentionally)

- Themes other than Catppuccin Mocha — they should "just work" since palette
  comes from the server. We capture only one for the visual baseline.
- Modal screens (theme picker, confirm-kill) — capture those during Phase 2
  once base layout is solid.
- Resize transitions (debounced log) — capture once Phase 2 lands.
