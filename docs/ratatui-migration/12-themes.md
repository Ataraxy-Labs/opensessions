# 12 — Themes and Colors

`packages/runtime/src/themes.ts` is the source of truth. The Rust client
**must not** hardcode palettes — it should consume themes from server state.

## Type port

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct ThemePalette {
    pub blue: Color,
    pub lavender: Color,
    pub pink: Color,
    pub mauve: Color,
    pub yellow: Color,
    pub green: Color,
    pub red: Color,
    pub peach: Color,
    pub teal: Color,
    pub sky: Color,
    pub text: Color,
    pub subtext0: Color,
    pub subtext1: Color,
    pub overlay0: Color,
    pub overlay1: Color,
    pub surface0: Color,
    pub surface1: Color,
    pub surface2: Color,
    pub base: Color,
    pub mantle: Color,
    pub crust: Color,
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub palette: ThemePalette,
    pub status: HashMap<AgentStatus, Color>,
    pub icons:  HashMap<AgentStatus, &'static str>,
}
```

## Hex string → ratatui Color

The wire format is hex strings (`"#cdd6f4"`). Use a custom deserializer:

```rust
use ratatui::style::Color;
use serde::{Deserialize, Deserializer};

fn deserialize_hex_color<'de, D: Deserializer<'de>>(d: D) -> Result<Color, D::Error> {
    let s = String::deserialize(d)?;
    let h = s.trim_start_matches('#');
    if h.len() != 6 { return Err(serde::de::Error::custom("expected #rrggbb")); }
    let r = u8::from_str_radix(&h[0..2], 16).map_err(serde::de::Error::custom)?;
    let g = u8::from_str_radix(&h[2..4], 16).map_err(serde::de::Error::custom)?;
    let b = u8::from_str_radix(&h[4..6], 16).map_err(serde::de::Error::custom)?;
    Ok(Color::Rgb(r, g, b))
}

// Apply via:
#[serde(deserialize_with = "deserialize_hex_color")]
pub blue: Color,
```

## Bundled themes (fallback only)

The Rust binary should ship a hardcoded copy of **only Catppuccin Mocha** as
a startup fallback (used before the first `state` message arrives). All other
themes come from server.

```rust
pub fn catppuccin_mocha() -> Theme {
    Theme {
        name: "catppuccin-mocha".into(),
        palette: ThemePalette {
            blue:     rgb(0x89, 0xb4, 0xfa),
            lavender: rgb(0xb4, 0xbe, 0xfe),
            pink:     rgb(0xcb, 0xa6, 0xf7),
            mauve:    rgb(0xcb, 0xa6, 0xf7),
            yellow:   rgb(0xf9, 0xe2, 0xaf),
            green:    rgb(0xa6, 0xe3, 0xa1),
            red:      rgb(0xf3, 0x8b, 0xa8),
            peach:    rgb(0xfa, 0xb3, 0x87),
            teal:     rgb(0x94, 0xe2, 0xd5),
            sky:      rgb(0x89, 0xdc, 0xeb),
            text:     rgb(0xcd, 0xd6, 0xf4),
            subtext0: rgb(0xa6, 0xad, 0xc8),
            subtext1: rgb(0xba, 0xc2, 0xde),
            overlay0: rgb(0x6c, 0x70, 0x86),
            overlay1: rgb(0x7f, 0x84, 0x9c),
            surface0: rgb(0x31, 0x32, 0x44),
            surface1: rgb(0x45, 0x47, 0x5a),
            surface2: rgb(0x58, 0x5b, 0x70),
            base:     rgb(0x1e, 0x1e, 0x2e),
            mantle:   rgb(0x18, 0x18, 0x25),
            crust:    rgb(0x11, 0x11, 0x1b),
        },
        status: HashMap::from([
            (AgentStatus::Idle,        rgb(0x58,0x5b,0x70)),
            (AgentStatus::Running,     rgb(0xf9,0xe2,0xaf)),
            (AgentStatus::ToolRunning, rgb(0x89,0xdc,0xeb)),
            (AgentStatus::Done,        rgb(0xa6,0xe3,0xa1)),
            (AgentStatus::Error,       rgb(0xf3,0x8b,0xa8)),
            (AgentStatus::Waiting,     rgb(0x89,0xb4,0xfa)),
            (AgentStatus::Interrupted, rgb(0xfa,0xb3,0x87)),
            (AgentStatus::Stale,       rgb(0xf9,0xe2,0xaf)),
        ]),
        icons: HashMap::from([
            (AgentStatus::Idle,        "○"),
            (AgentStatus::Running,     "●"),
            (AgentStatus::ToolRunning, "⚙"),
            (AgentStatus::Done,        "✓"),
            (AgentStatus::Error,       "✗"),
            (AgentStatus::Waiting,     "◉"),
            (AgentStatus::Interrupted, "⚠"),
            (AgentStatus::Stale,       "⚠"),
        ]),
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> Color { Color::Rgb(r, g, b) }
```

## Theme picker — names list

The list of available themes comes from `BUILTIN_THEMES` in TS. To avoid
hardcoding the list in Rust, **the server should broadcast the available
theme names** as part of `ServerState`:

```ts
// Add to ServerState:
themeNames: string[];
```

(Phase 0 additive change.) Until then, hardcode the names in Rust to match
TS:

```rust
const BUILTIN_THEME_NAMES: &[&str] = &[
    "catppuccin-mocha",
    "catppuccin-latte",
    "catppuccin-frappe",
    "catppuccin-macchiato",
    "tokyo-night",
    "gruvbox-dark",
    "gruvbox-light",
    "nord",
    "rose-pine",
    // ... whatever themes.ts has
];
```

(Generate this list at build time from `packages/runtime/src/themes.ts` if
possible — same codegen as protocol types.)

## Preview vs. apply

Theme picker lets you preview without committing:

```rust
// Up/down with selection change → previewTheme(name)
// 'enter' → applyTheme(name) → server persists + rebroadcasts
// 'esc' → revert preview using theme_before_preview

fn open_theme_picker(&mut self) {
    self.theme_before_preview = Some(self.theme.clone());
    self.modal = Modal::ThemePicker(ThemePickerState::default());
}

fn preview_theme(&mut self, name: &str) {
    if let Some(t) = self.bundled_or_fetched_theme(name) {
        self.theme = t;  // local-only; server unchanged
    }
}

fn apply_theme(&mut self, name: String) {
    self.send(ClientCommand::SetTheme { theme: name });
    self.theme_before_preview = None;
    self.modal = Modal::None;
    // Server will broadcast updated state; theme rerolls to whatever server says
}

fn close_theme_picker(&mut self) {
    if let Some(t) = self.theme_before_preview.take() {
        self.theme = t;  // revert
    }
    self.modal = Modal::None;
}
```

## Custom themes (PartialTheme)

The TS config supports inline partial themes:

```ts
{ "theme": { "palette": { "blue": "#abcdef" } } }
```

For Phase 1–7 we don't support this — it's a power-user feature. Once theme
state comes from server, **the server already resolves these to a full
`Theme`** before broadcasting, so the Rust client never sees the partial
form. Phase 7 work item: add to `themeNames` in ServerState a virtual
"custom" entry if config has a partial theme.

## Status icon font / Unicode width gotchas

Several status icons have ambiguous East-Asian width:

| Icon | Unicode | Standard width | OpenTUI width | Action |
|---|---|---|---|---|
| `●` | U+25CF | ambiguous (1 or 2) | 1 | Force width 1 in our padding logic |
| `○` | U+25CB | ambiguous (1 or 2) | 1 | Same |
| `⚙` | U+2699 | ambiguous (1 or 2) | 1 | Same |
| `✓` | U+2713 | 1 | 1 | OK |
| `✗` | U+2717 | 1 | 1 | OK |
| `◉` | U+25C9 | ambiguous | 1 | Same |
| `⚠` | U+26A0 | ambiguous (often 2 in modern terms!) | 1 in OpenTUI | Force width 1 manually |
| `▌` | U+258C | 1 | 1 | OK |
| `▸` | U+25B8 | 1 | 1 | OK |
| `…` | U+2026 | 1 | 1 | OK |
| `↑↓⏎⇥↪→←` | various | 1 each | 1 each | OK |
| Braille `⠋⠙...` | various | 1 each | 1 each | OK |

**Mitigation**: when computing layout columns for icons, hardcode width = 1
rather than calling `unicode_width::UnicodeWidthStr::width`. This matches
OpenTUI's default behavior. Document this divergence in `14-edge-cases.md`.
