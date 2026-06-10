# Sidebar UI column logic

This document records the Rust TUI sidebar layout rules that came from the old
TS sidebar and recent Ratatui spacing work. The goal is a dense tmux-native radar:
session identity, worktree structure, and agent attention should be visible in
the session list before the user focuses a detail panel.

## Section rhythm

The sidebar uses terminal-native spacing, not bordered cards:

```text
 sessions                       ⚡2 ●1 4

› 01 ○ learning
      main

▌ 04 ⚙ opensessions
      ratatui-migration

 ───────────────────────────────────────
 agents 1                        current
 /tmp/opensessions

  ⚙ Query tmux for open sessions
    working · amp
    “make the grouped session tree tighter”
```

Rules:

- Section headers are lowercase with one leading cell: ` sessions`, ` agents`.
- Session entries are two rows plus one spacer row.
- The detail panel keeps two-line agent entries by default, with an optional
  third intent line for the latest user prompt. There is still one spacer row
  between agents.
- Separators are single horizontal rules. No card borders.

## Top-level session columns

Top-level sessions use a tight four-part row:

```text
[marker][space][index][space][signal][space][name]
```

Examples:

```text
› 01 ○ learning
  02 ⠹ background-export
▌ 04 ⚙ opensessions
```

The marker column is one cell:

- `▌`: current tmux session for this sidebar client.
- `›`: focused session in the sidebar.
- space: neither current nor focused.

The index always starts in the same column. This is why the current row is
`▌ 04`, not `▌  04`.

Detail rows align under the index/signal/name columns:

```text
  02 ⠹ background-export
      feat/export
```

## Agent attention signals

Each session row shows the highest-priority signal across `agent_state` and all
per-pane/per-thread `agents`.

Priority, highest first:

1. `✗` error
2. `⚠` stale
3. `⚠` interrupted
4. `◉` waiting / blocked
5. `●` done but unseen
6. `⚙` tool-running
7. spinner (`⠋`, `⠙`, `⠹`, …) running
8. `✓` done and seen
9. `○` idle / no agent / unknown

Header counts use the same vocabulary:

```text
 sessions                       ⚡2 ●1 4
```

- `⚡N`: visible sessions with active agents (`running`, `tool-running`,
  `waiting`, `interrupted`, `stale`, or `error`).
- `●N`: sessions with unseen completions.
- final number: visible session count after filtering.

## Worktree group tree columns

Expanded worktree groups use tree glyphs so the group remains continuous across
spacer rows:

```text
  ▾    ● plane-wt        3wt ⚡1 ●
  │
  │ 01 ○ edit-pages
  │      feat/edit-pages
  │
▌ │ 02 ⠹ background-export
  │      feat-background-exports
  │
  ╰ 03 ● pdf-word-formatting
         chore-relation-pqls
```

Rules:

- Group header: `  ▾    <signal> <label>` when expanded.
- Collapsed group: `  ▸    <signal> <label>`.
- Middle children use a continuous `│` gutter, not a branch glyph. This keeps
  the row grid calm in narrow panes.
- Current/focused markers are rendered in a reserved left gutter for grouped
  children (`▌ │ 02 ...`, `› │ 02 ...`) so the index/signal/name columns do not
  shift.
- The final child uses `╰`; its detail row drops the rail so the tree visibly
  ends.
- Group signals use the same highest-priority agent attention as sessions,
  computed across all child sessions.

Collapsed groups hide children but keep the aggregate signal:

```text
  ▸    ◉ plane-wt        2wt ⚡1
```

## Agent panel flow

The detail panel is an action list. In `current` scope, primary text is the
thread/task name when available:

```text
 agents 1                        current
 /tmp/opensessions

  ⚙ Query tmux for open sessions
    working · amp
    “make the grouped session tree tighter”
```

The optional prompt line is deliberately detail-panel-only. Session rows keep a
stable two-row shape so a new prompt does not shift the session list. Prompt
text comes from `AgentEvent.lastUserPrompt` and is truncated on the right.

In `all` scope, rows start with the tmux session so users can jump by where the
work lives:

```text
 agents 2                            all

  ⚙ opensessions · Query tmux for op…

  ● plane · Review PR
```

Interaction remains unchanged:

- Enter/click an agent focuses its tmux pane.
- `a` toggles `current` / `all` scope.
- `d` dismisses the focused agent item.
- `x` kills the focused agent pane.

## Test coverage

The concrete rendering rules above are covered by `opensessions-sidebar`
unit tests that build the real render model:

- `session_rows_show_inline_agent_signals_and_spaced_header`
- `expanded_worktree_groups_render_a_continuous_tree_with_child_spacing`
- `collapsed_worktree_groups_keep_the_highest_priority_agent_signal`
- `agent_panel_uses_clean_current_and_all_scope_labels`
- `agent_detail_panel_shows_last_user_prompt_without_polluting_session_rows`
- `initializing_loader_keeps_spinner_and_detail_copy`
