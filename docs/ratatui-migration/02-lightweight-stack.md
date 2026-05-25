# 02 — Lightweight Library Stack (Crate Selection)

> **Goal:** every dep on this list is the smallest, fastest, lowest-RAM
> option available in the Rust ecosystem as of late-2024 / early-2025.
> No "convenient but heavy" crates. If something on this list isn't
> the leanest viable option, replace it.

## Final stack (TL;DR)

```toml
# apps/tui-rs/Cargo.toml
[package]
name = "opensessions-sidebar"
version = "0.1.0"
edition = "2024"

[dependencies]
# --- TUI core ---
ratatui            = { version = "0.30", default-features = false, features = ["crossterm"] }
crossterm          = { version = "0.29", default-features = false, features = ["events"] }
unicode-width      = "0.2"   # already required by ratatui; pin to dedupe

# --- Async runtime (smallest tokio config) ---
tokio              = { version = "1", default-features = false, features = ["rt", "net", "io-util", "macros", "time", "sync"] }
# NB: NO "rt-multi-thread", NO "fs", NO "process" (we shell out via std::process)
tokio-util         = { version = "0.7", default-features = false, features = [] }

# --- WebSocket client (fastest + smallest) ---
tokio-websockets   = { version = "0.13", default-features = false, features = ["client", "sha1_smol", "fastrand"] }
futures-util       = { version = "0.3", default-features = false }
http               = "1"

# --- JSON (the WS messages are small + protocol-stable) ---
serde              = { version = "1", default-features = false, features = ["derive"] }
serde_json         = { version = "1", default-features = false, features = ["alloc"] }
# Reasoning: messages are <10 KB; serde_json is fast enough and ~70 KB stripped.
# If profiling later shows JSON as a hotspot, swap for sonic-rs (x86_64) or
# simd-json. For binary-size obsessives, nanoserde drops ~150 KB by skipping
# proc macros entirely — but it loses union/tagged-enum ergonomics that our
# protocol leans on.

# --- Errors / args / logging ---
anyhow             = "1"     # zero-cost in release
clap               = { version = "4", default-features = false, features = ["std", "derive", "help"] }
# clap's "derive" pulls proc-macro work, but only at compile time.
# If we want sub-1MB total, swap for `lexopt` or hand-rolled std::env::args.

# --- Optional (gate behind features) ---
# tracing          = "0.1"   # only if we add a log file; otherwise eprintln!
# color-eyre       = "0.6"   # only in dev profile
```

## Per-component justification

### TUI: `ratatui` 0.30 + `crossterm` (no termion / termwiz / vaxis)

| Candidate | Verdict | Why |
|---|---|---|
| **ratatui** | ✅ chosen | De-facto standard. Modular as of 0.30 (`ratatui-core`, `ratatui-widgets`, `ratatui-crossterm` shipped separately so unused widgets are dropped at link-time). All widgets we need are first-party (List, Block, Paragraph, Sparkline, Scrollbar). |
| cursive | ❌ | Higher-level, drags in `pancurses` or `ncurses` C deps. Bigger binary, less control. |
| libvaxis (Rust port) | ❌ | Newer, smaller, but Zig-original; fewer widgets; would force us to write more from scratch. Worse DX with no upside for our scale. |
| dioxus-tui / freya | ❌ | Web-stack design, 10× heavier than we need. |

**Backend = `crossterm`**:
- Pure Rust, only deps: `bitflags`, `parking_lot`, `libc` (Unix), `mio`, `signal-hook`, `signal-hook-mio`. Roughly **5 transitive deps** when `events` feature is on, can drop further with `filedescriptor` feature flag (replaces `mio` with raw fd polling — saves ~20 KB).
- Termion is Unix-only (we may want Windows in the future) and has fewer modern features.
- Termwiz is built for Wezterm and pulls megabytes of rendering code we won't touch.

### WebSocket client: `tokio-websockets`, not `tokio-tungstenite`

**Implementation update (Phase 1):** after checking current crate docs and
benchmarks, the Rust client uses `tokio-websockets` instead of
`fastwebsockets`. It is Tokio-native, strict by default, actively maintained,
supports a tiny plaintext client feature set, and satisfies
`cargo tree --duplicates` with no vendored patch. `fastwebsockets` remains a
good high-performance crate, but version `0.10` pulls a `thiserror` version
that duplicates Ratatui's dependency graph unless patched locally.

The production dependency is:

```toml
tokio-websockets = { version = "0.13", default-features = false, features = ["client", "sha1_smol", "fastrand"] }
```

Historical comparison from the original stack selection:

| Candidate | Throughput rel. to fastwebsockets | Allocs | Verdict |
|---|---|---|---|
| fastwebsockets | 1.0× (baseline) | minimal, optional `simd` | ✅ viable, replaced to avoid vendoring |
| **tokio-websockets** | competitive | `Bytes` payloads, strict | ✅ chosen |
| websocket.rs (`web-socket`) | ~1.05× | smaller still | ❌ less mature, smaller community |
| tokio-tungstenite | ~0.5× | allocates per frame | ❌ ~2× slower, default frame allocations |
| soketto | ~0.7× | depends | ❌ less active |
| awc (Actix client) | n/a | drags Actix runtime | ❌ heavy |

**Why fastwebsockets:**
- Built and battle-tested by Deno (every Deno WS client uses it).
- `simd` feature for masking/UTF-8 validation = ~3× faster on payload processing.
- Returns raw `Frame` struct → zero-copy receive into our `&[u8]` → fed straight to `serde_json::from_slice`.
- ~Half the allocations of `tokio-tungstenite` per message.
- Client API is `handshake::client(...)` over a `tokio::net::TcpStream` + `hyper::Request`.

**Caveat:** the client-side handshake requires `hyper` (one-shot, only for the
HTTP upgrade). After upgrade we have a raw upgraded I/O object and `hyper` is
unused. Keep `hyper` features at `client + http1` only — drops most of hyper's
weight. Some folks have hand-rolled the WS upgrade themselves to avoid hyper
entirely (200 LOC of HTTP parsing) — viable Phase-2 optimization if our binary
is over budget.

### Async runtime: `tokio` minimal (not `smol`, not full tokio)

| Candidate | Binary cost | Why |
|---|---|---|
| **tokio (rt-current-thread, minimal features)** | ~700 KB | ✅ chosen — fastwebsockets, hyper, etc. are all tokio-native; using anything else means losing those crates or running an `async-compat` adapter. |
| smol | ~250 KB | Smallest async runtime, but ecosystem mismatch (would force `async-compat` shim around fastwebsockets/hyper, killing the size advantage). |
| async-std | (deprecated) | ❌ |
| Hand-rolled poll loop on `std::net::TcpStream` + `mio` | ~150 KB | ❌ Reinventing it: weeks of work, error-prone, not worth saving 500 KB. |

**Tokio feature surgery:**
- ❌ `rt-multi-thread` — we don't need work-stealing; one thread is enough for a sidebar.
- ❌ `fs` — no async filesystem; use `std::fs` (config is read once at startup).
- ❌ `process` — we shell out with `std::process::Command` (one-shot, no need for async).
- ❌ `signal` — `crossterm` already handles SIGWINCH via mio/signal-hook.
- ✅ `rt` — current-thread executor.
- ✅ `net` — `TcpStream`.
- ✅ `io-util` — `AsyncRead/Write` extension traits.
- ✅ `time` — `tokio::time::interval` for the spinner & frame loop.
- ✅ `sync` — `mpsc` channel between WS reader task and main render task.
- ✅ `macros` — only for `#[tokio::main]` and `tokio::select!`. Fine.

### JSON: `serde_json` (not `sonic-rs`, not `nanoserde`)

| Candidate | Deserialize speed (twitter.json) | Binary cost |
|---|---|---|
| **serde_json** | 2.3 ms | ~70 KB | ✅ chosen |
| simd-json | 1.0 ms | ~120 KB + runtime SIMD detection | overkill for <10 KB messages |
| sonic-rs | 0.7 ms | ~150 KB | x86_64 only without `target-cpu=native` |
| nanoserde | n/a | ~5 KB! | ❌ no enum-with-payload support; our protocol is full of tagged unions |
| miniserde | n/a | ~10 KB | ❌ same: no `#[serde(tag = "type")]` equivalent |

**Why serde_json wins for us:**
- Our WS messages are ~1–8 KB. Even at serde_json's "slowest" 2 ms/MB, that's
  ~16 µs per message. The 60 fps render loop has 16 ms of headroom — JSON is
  not the bottleneck.
- The protocol uses `#[serde(tag = "type")]` discriminated unions (every
  `ServerMessage` and `ClientCommand`). Only `serde` ecosystem supports that
  ergonomically.
- ~70 KB binary cost is fine.

### Args parsing: `clap` derive (or `lexopt` if we want sub-1 MB)

| Candidate | Binary cost | Verdict |
|---|---|---|
| **clap derive** | ~200 KB | ✅ default — only flag we currently need is `--server-port`; ergonomic for future flags |
| lexopt | ~5 KB | swap if binary budget tight |
| hand-rolled `std::env::args` | 0 | swap if binary budget tightest |

### Errors: `anyhow` (not `eyre`, not `thiserror` for app code)

- `anyhow::Result<T>` is zero-cost in release.
- `thiserror` only useful if we expose typed errors; this is a binary, not a lib.

## Cargo profile (release): aggressive size reduction

```toml
# Cargo.toml workspace root (or apps/tui-rs/Cargo.toml)
[profile.release]
opt-level     = "z"      # optimize for size (try "s" if "z" hurts perf measurably)
lto           = "fat"    # whole-program optimization, removes dead deps
codegen-units = 1        # let LLVM see everything at once
panic         = "abort"  # no unwind tables; sidebar crash → process exits → tmux respawns
strip         = "symbols" # strip debug symbols from final binary
incremental   = false    # CI-grade reproducibility for releases

[profile.release-dev]
inherits      = "release"
opt-level     = 3
strip         = false
debug         = "line-tables-only"  # keep enough for backtraces in CI
```

### Optional nightly-only further trim (deferred to Phase 8)

If we still need to claw back binary size after profiling:

```sh
RUSTFLAGS="-Zlocation-detail=none -Zfmt-debug=none" cargo +nightly build \
  -Z build-std=std,panic_abort \
  -Z build-std-features="optimize_for_size" \
  --target aarch64-apple-darwin --release
```

This is documented to drop a hello-world from ~400 KB → ~30 KB. Real apps
benefit less, but expect 30–50% reduction on top of the stable profile.

We do **not** ship a nightly-built artifact unless the binary is over budget;
stable gives ~1.5–3 MB stripped which is well under the ~5 MB esbuild-style ceiling.

## Expected per-process resource budget

Confirmed by the same patterns as `helix`, `gitui`, `bottom`, `bandwhich`:

| Phase | Per-process RSS | Per-process binary | 27 panes |
|---|---|---|---|
| Phase 1 (skeleton, just connect+render list) | ~6 MB | ~1.5 MB | ~160 MB |
| Phase 4 (full feature parity) | ~10–15 MB | ~2–3 MB | ~270–400 MB |
| Phase 8 (nightly + build-std) | ~8–12 MB | ~1.0–1.5 MB | ~220–325 MB |

Compared to the current ~80 MB Bun process: **80–90% RAM reduction, 95%
binary reduction**.

## Crates we are explicitly NOT pulling in

| Crate | Why not |
|---|---|
| `tokio-tungstenite` | 2× slower than fastwebsockets; allocates per frame |
| `reqwest` | We don't need a generic HTTP client; one-time `/quit` POST is one `tokio::net::TcpStream::connect` + 5 lines of HTTP/1.1 |
| `tracing` | Replace with `eprintln!` and a dedicated `/tmp/opensessions-tui-*.log` file via `std::fs`; we don't need spans/subscribers |
| `color-eyre` | Pretty backtraces; useful in dev only. Behind a `dev-deps` flag. |
| `crossbeam-channel` | `tokio::sync::mpsc` already in graph; no need to double up |
| `parking_lot` | Already pulled by crossterm; don't add explicitly |
| `chrono`/`time` | We don't format dates; use `std::time::SystemTime` + manual formatting if needed (~10 LOC) |
| `regex` | We don't parse text; if we ever do, use `regex-lite` (~50 KB vs `regex`'s ~600 KB) |
| `tokio-tungstenite` (revisit) | really, no |

## Validation gates

Before locking the stack:

1. **`cargo bloat --release --crates`** after Phase 1 skeleton. Top-10 crates by
   bytes; flag anything unexpected (>5% of binary that we can't justify).
2. **`cargo tree --duplicates`** — zero acceptable. If we get two versions of a
   crate (common with `crossterm` + `ratatui-crossterm`), pin one explicitly.
3. **`/usr/bin/time -l ./target/release/opensessions-sidebar`** at idle for
   60 s, capture peak RSS. Must be ≤ 20 MB.
4. **`cargo audit`** — zero unpatched advisories.

If any of these fail, swap the offending dep before proceeding to Phase 2.
