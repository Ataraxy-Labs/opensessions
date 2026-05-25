# 13 — Server Bootstrap (`ensureServer`)

The TS client calls `ensureServer()` at startup. It:

1. Checks `PID_FILE` for a live PID.
2. Probes the port to verify the server is responsive.
3. If not, spawns `bun run apps/server/src/main.ts` and polls until the port
   opens (up to 3 s).

Port to Rust:

## Rust port

```rust
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;

pub async fn ensure_server(host: &str, port: u16, pid_file: &PathBuf) -> anyhow::Result<()> {
    if is_alive(pid_file) && port_open(host, port).await {
        return Ok(());
    }
    spawn_server()?;

    // Poll for up to 3 s
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if port_open(host, port).await {
            return Ok(());
        }
    }
    anyhow::bail!("server failed to start within 3 seconds");
}

fn is_alive(pid_file: &PathBuf) -> bool {
    let Ok(s) = std::fs::read_to_string(pid_file) else { return false; };
    let Ok(pid) = s.trim().parse::<i32>() else { return false; };
    // kill -0 is a "does this PID exist?" probe
    unsafe { libc::kill(pid, 0) == 0 }
}

async fn port_open(host: &str, port: u16) -> bool {
    tokio::time::timeout(
        Duration::from_millis(200),
        TcpStream::connect((host, port)),
    ).await.is_ok_and(|r| r.is_ok())
}

fn spawn_server() -> std::io::Result<()> {
    let bun = std::env::var("OPENSESSIONS_BUN").unwrap_or_else(|_| "bun".into());
    let server_path = resolve_server_entry()?;
    use std::process::{Command, Stdio};
    let _child = Command::new(&bun)
        .arg("run")
        .arg(&server_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Detach: don't keep parent process tied to child
        .spawn()?;
    // Don't store the handle — we want to outlive this process.
    std::mem::forget(_child);
    Ok(())
}

fn resolve_server_entry() -> std::io::Result<PathBuf> {
    if let Ok(d) = std::env::var("OPENSESSIONS_DIR") {
        let p = PathBuf::from(d).join("apps/server/src/main.ts");
        if p.exists() { return Ok(p); }
    }
    // When binary is installed, the server entry sits next to or via PATH.
    // For now, fall back to a packaged path or fail.
    Err(std::io::Error::new(std::io::ErrorKind::NotFound,
        "OPENSESSIONS_DIR not set; cannot find server entry"))
}
```

## Distribution implications

The Rust binary still depends on **bun being installed** to launch the
server. Two paths:

1. **Status quo**: ship the server as TS, expect users to install bun.
   `npm install opensessions` postinstall ensures bun via `bunx` if absent.
2. **Compile server too**: use `bun build --compile` to produce a single-file
   server binary, ship both binaries side-by-side.

For Phase 6, option 1 is fine. Option 2 is a nice-to-have once we're
production-grade. Doesn't affect the Rust client.

## Server discovery race

If 27 clients all start at once and none find the server, all 27 try to
spawn it. The TS code has the same race; mitigate with a **lockfile**:

```rust
async fn ensure_server_locked(host: &str, port: u16, pid_file: &PathBuf) -> anyhow::Result<()> {
    use fs2::FileExt;
    let lock_path = pid_file.with_extension("lock");
    let lock = std::fs::OpenOptions::new()
        .create(true).write(true).open(&lock_path)?;
    lock.lock_exclusive()?;
    // Re-check now that we hold the lock
    if is_alive(pid_file) && port_open(host, port).await { return Ok(()); }
    spawn_server()?;
    // ...poll as before
    lock.unlock()?;
    Ok(())
}
```

This is **a real upgrade over the current TS behavior** (which has the race).
Worth doing as part of the port.

## Connection retry loop

After `ensure_server`, connect to WS with backoff:

```rust
async fn connect_ws(url: &str) -> anyhow::Result<WebSocket<...>> {
    let mut delay = Duration::from_millis(50);
    for attempt in 0..10 {
        match try_connect(url).await {
            Ok(ws) => return Ok(ws),
            Err(_) if attempt < 9 => {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_millis(500));
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}
```

## Server-initiated reconnect

If the server sends `quit` and exits, clients should attempt a reconnect
unless they were the ones who triggered the quit. Add a flag:

```rust
struct App {
    self_initiated_quit: bool,
    // ...
}

// On `Quit` message:
if self.self_initiated_quit {
    self.should_quit = true;  // we asked for it
} else {
    // Server died unexpectedly; don't auto-reconnect, just exit cleanly.
    // (User probably ran `tmux kill-server` or similar.)
    self.should_quit = true;
}
```

For now, behavior matches TS: quit on any server `quit` message.

## Health check / heartbeat

The server doesn't currently send pings. We can add:

- Server side: send a `ping` (or use WS PING frames) every 10 s.
- Client side: if no message in 30 s → assume dead, attempt one reconnect.

This is **out of scope** for the migration but worth tracking; add only if
real-world flakiness materializes.
