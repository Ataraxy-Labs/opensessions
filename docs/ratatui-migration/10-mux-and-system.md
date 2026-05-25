# 10 — Mux Integration & System Calls

The TS client makes several `tmux` subprocess calls and `Bun.spawn(["open",
…])` calls. Two strategies for the Rust port:

| Call | Best path | Why |
|---|---|---|
| `getLocalSessionName()` | **server-side** (new WS endpoint) | Already in server's mux provider; eliminates per-client tmux spawn |
| `getLocalWindowId()` | **server-side** | Same |
| `getClientTty()` | **server-side** | Same |
| `refocusMainPane()` | **server-side** | Race-free; server knows pane ID and target |
| `Bun.spawn(["open", url])` (mouse click) | **client-side** (must be) | Inherits user's GUI session; `open` only works locally on macOS Aqua session |
| `Bun.spawnSync(["open", dir])` | **client-side** (must be) | Same |
| HTTP `POST /quit` fallback | **client-side** | Already a fallback path; trivial |

## Detection at startup

```rust
pub enum MuxContext {
    Tmux  { pane_id: String },
    Zellij { session_name: String, pane_id: String },
    None,
}

impl MuxContext {
    pub fn detect() -> Self {
        if std::env::var("TMUX").is_ok() {
            if let Ok(p) = std::env::var("TMUX_PANE") {
                return Self::Tmux { pane_id: p };
            }
        }
        if let Ok(s) = std::env::var("ZELLIJ_SESSION_NAME") {
            let p = std::env::var("ZELLIJ_PANE_ID").unwrap_or_default();
            return Self::Zellij { session_name: s, pane_id: p };
        }
        Self::None
    }
}
```

## Server-side mux query (Phase 6)

Add to `ServerMessage` (Phase 0 friendly — additive only):

```ts
interface MuxInfo {
  type: "mux-info";
  paneId: string;
  sessionName: string | null;
  windowId: string | null;
  clientTty: string | null;
}
```

And to `ClientCommand`:

```ts
{ type: "query-mux-info"; paneId: string }
{ type: "refocus-main-pane"; paneId: string }
```

The server's existing `MuxProvider` has all the data; just expose it.

```rust
// In Rust client, on startup after WS connect:
let MuxContext::Tmux { pane_id } = &self.mux else { return };
self.send(ClientCommand::QueryMuxInfo { pane_id: pane_id.clone() });
```

The `MuxInfo` response populates `App.my_session`, `App.window_id`,
`App.client_tty`.

## Spawning `open` (must stay client-side)

```rust
pub fn spawn_open(target: &str) {
    use std::process::{Command, Stdio};
    let target = target.to_string();

    // Detached, no I/O kept open, no zombie reap
    let _ = Command::new("open")
        .arg(&target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();   // intentionally not .wait()
}
```

> Don't use `tokio::process::Command` here — we don't want to await the result,
> and `std::process::Command::spawn` returns immediately. Pulling tokio's
> process feature also drags signal handling we don't need.

### Cross-platform

`open` is macOS-specific. For Linux: `xdg-open`. For Windows: `start`.
The repo currently has 1k stars and primarily macOS users, but to be safe:

```rust
pub fn spawn_open(target: &str) {
    #[cfg(target_os = "macos")]
    let bin = "open";
    #[cfg(target_os = "linux")]
    let bin = "xdg-open";
    #[cfg(target_os = "windows")]
    let bin = "explorer";

    let _ = std::process::Command::new(bin)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
```

## HTTP `/quit` fallback (one-shot, no `reqwest`)

Hand-roll HTTP/1.1 over `tokio::net::TcpStream` to avoid pulling reqwest:

```rust
async fn http_post_quit(host: &str, port: u16) -> std::io::Result<()> {
    let mut s = tokio::net::TcpStream::connect((host, port)).await?;
    let req = format!(
        "POST /quit HTTP/1.1\r\n\
         Host: {host}:{port}\r\n\
         Content-Length: 0\r\n\
         Connection: close\r\n\
         \r\n"
    );
    use tokio::io::AsyncWriteExt;
    s.write_all(req.as_bytes()).await?;
    s.flush().await?;
    // Don't bother reading the response; we're shutting down anyway.
    Ok(())
}
```

## tmux subprocess spawn (fallback if server-side is deferred)

If we want Phase 1–5 to ship before the server gains the new endpoints, we can
spawn `tmux` directly on the client side (matches TS behavior 1:1):

```rust
fn tmux_display(target: &str, fmt: &str) -> Option<String> {
    let out = std::process::Command::new("tmux")
        .args(["display-message", "-t", target, "-p", fmt])
        .output().ok()?;
    if !out.status.success() { return None; }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn get_local_session_name(pane_id: &str) -> Option<String> {
    tmux_display(pane_id, "#{session_name}")
}

pub fn get_local_window_id(pane_id: &str) -> Option<String> {
    tmux_display(pane_id, "#{window_id}")
}
```

But this regresses the data-daemon goal (each client spawns tmux on a poll
interval). **Prefer server-side** as the long-term solution.

## Polling cadence

The TS client calls `maybeReIdentify()` whenever focus changes (and elsewhere).
On the Rust side, do the same checks driven by a periodic task:

```rust
let mut reidentify_tick = tokio::time::interval(Duration::from_secs(2));

tokio::select! {
    _ = reidentify_tick.tick() => app.maybe_reidentify(),
    // ...
}
```

`maybe_reidentify` queries the server for current mux info; if the session
name or window id changed, send `IdentifyPane`.

## Zellij parity

Identical pattern, different command line: `zellij action move-focus right`
instead of `tmux select-pane`. Defer until tmux path works; zellij is a
secondary mux.

## Why we don't pull `which` or `command_exists` crates

`std::process::Command::new("open").spawn()` returns `Err(NotFound)` if the
binary doesn't exist. Just check the error.
