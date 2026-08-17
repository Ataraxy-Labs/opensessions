# 11 — Config and Persistence

## Current shape

Path: `~/.config/opensessions/config.json`

Schema (from `packages/runtime/src/config.ts`):

```ts
interface OpensessionsConfig {
  mux?: string;
  port?: number;
  plugins: string[];
  theme?: string | PartialTheme;
  sidebarWidth?: number;
  sidebarPosition?: "left" | "right";
  keybinding?: string;
  detailPanelHeight?: number;
  sessionFilter?: SessionFilterMode;
}
```

The TUI client today reads/writes it directly (`loadConfig` / `saveConfig`).
The Rust TUI keeps detail-panel height server-owned so every sidebar sees the
same height and only the server writes `detailPanelHeight`.

## Target: server-mediated config (recommended)

### Phase 0 protocol additions (additive)

```ts
// Server → Client
interface ConfigBroadcast {
  type: "config";
  config: OpensessionsConfig;
}

// Client → Server
{ type: "set-config-key"; path: string; value: unknown }
{ type: "get-config" }
```

The server already writes config when it changes the theme; extending it for
detail-panel height is a small change. Server becomes single-writer.

### Rust client side

```rust
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpensessionsConfig {
    pub mux: Option<String>,
    pub port: Option<u16>,
    #[serde(default)]
    pub plugins: Vec<String>,
    pub theme: Option<serde_json::Value>,    // string or partial-theme; we don't introspect
    pub sidebar_width: Option<u16>,
    pub sidebar_position: Option<SidebarPosition>,
    pub keybinding: Option<String>,
    pub detail_panel_height: Option<u16>,
    pub session_filter: Option<SessionFilterMode>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarPosition { Left, Right }
```

On each `ConfigBroadcast`, the client updates its in-memory copy. To mutate:

```rust
self.send(ClientCommand::SetConfigKey {
    path: "detailPanelHeight".to_string(),
    value: serde_json::Value::Number(height.into()),
});
```

## Fallback: client-side direct writes (Phase ≤5)

If we ship Phase 1–5 before the server gets the new endpoints, the Rust
client reads/writes the file itself. **Use a file lock** to avoid the
27-way race that the TS client tolerates only because most writes are infrequent:

```rust
use std::fs::{File, OpenOptions};
use std::io::{Read, Write, Seek};
use std::path::PathBuf;

fn config_path() -> PathBuf {
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
    PathBuf::from(home).join(".config").join("opensessions").join("config.json")
}

pub fn load_config() -> OpensessionsConfig {
    let path = config_path();
    let Ok(s) = std::fs::read_to_string(&path) else { return Default::default() };
    serde_json::from_str(&s).unwrap_or_default()
}

pub fn save_partial(updates: serde_json::Value) -> std::io::Result<()> {
    use fs2::FileExt;  // tiny crate (~5 KB) for OS file locks
    let path = config_path();
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    let mut f = OpenOptions::new().create(true).read(true).write(true).open(&path)?;
    f.lock_exclusive()?;
    let mut buf = String::new();
    f.read_to_string(&mut buf)?;
    let mut existing: serde_json::Value = serde_json::from_str(&buf).unwrap_or(serde_json::json!({}));
    merge(&mut existing, &updates);
    f.set_len(0)?;
    f.seek(std::io::SeekFrom::Start(0))?;
    let s = serde_json::to_string_pretty(&existing)?;
    f.write_all(s.as_bytes())?;
    f.write_all(b"\n")?;
    f.unlock()?;
    Ok(())
}

fn merge(dst: &mut serde_json::Value, src: &serde_json::Value) {
    match (dst, src) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            for (k, v) in b { merge(a.entry(k.clone()).or_insert(serde_json::Value::Null), v); }
        }
        (a, b) => *a = b.clone(),
    }
}
```

> If we want to avoid `fs2` (~5 KB), use `flock(2)` directly via `libc` (~1 KB
> of inline code). Documented as TODO in code if binary budget tight.

## Detail-panel height persistence

The Rust server persists one shared height in `detailPanelHeight`:

```rust
fn persist_detail_panel_height(&self, height: u16) {
    save_config_to_home(&home, OpensessionsConfig {
        detail_panel_height: Some(height),
        ..OpensessionsConfig::default()
    })
}
```

## Theme persistence

`set-theme` already mutates server-side config. No change.

## Session filter

`set-filter` likewise. No change.

## Migration & forward-compat

The Rust client must tolerate fields it doesn't know (forward-compat). Use
`serde(default)` and `serde_json::Value` as a catch-all where shape is
uncertain (e.g., `theme` which can be a string OR an object).

## What about `OPENSESSIONS_*` env vars?

Read once at startup:
- `OPENSESSIONS_DIR` (server entry path resolution)
- `OPENSESSIONS_PORT` (override)
- `OPENSESSIONS_HOST` (override)
- `OPENSESSIONS_SERVER_KEY` (override)
- `OPENSESSIONS_PID_FILE` (override)
- `OPENSESSIONS_TUI` (selects ts vs rs client) — used by the launcher script,
  not the binary itself.

```rust
fn server_port() -> u16 {
    if let Ok(s) = std::env::var("OPENSESSIONS_PORT") {
        if let Ok(n) = s.parse() { return n; }
    }
    let key = server_key();
    match key {
        Some(k) => 17000 + k,
        None => 7391,  // DEFAULT_SERVER_PORT
    }
}

fn server_key() -> Option<u16> {
    if let Ok(s) = std::env::var("OPENSESSIONS_SERVER_KEY") {
        return s.parse().ok();
    }
    let tmux = std::env::var("TMUX").ok()?;
    let socket = tmux.split(',').next()?;
    Some(hash_server_key(socket))
}

fn hash_server_key(s: &str) -> u16 {
    // Hash UTF-8 bytes identically in Rust, TypeScript, and shell.
    let mut hash: u32 = 0;
    for (i, b) in s.bytes().enumerate() {
        hash = (hash + b as u32 * (i as u32 + 1)) % 20000;
    }
    hash as u16
}
```

Verify the hash matches by golden test vs TS implementation.
