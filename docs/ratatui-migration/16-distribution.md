# 16 — Distribution

The Rust binary needs to land on user machines without requiring them to
install Rust. Same UX as `npm install opensessions` today.

## Build matrix

| Target triple | Platform |
|---|---|
| `aarch64-apple-darwin` | macOS Apple Silicon (primary) |
| `x86_64-apple-darwin` | macOS Intel |
| `x86_64-unknown-linux-gnu` | Linux x64 (most common) |
| `aarch64-unknown-linux-gnu` | Linux ARM64 (cloud, RPi) |
| `x86_64-unknown-linux-musl` | Linux static (Alpine, scratch containers) |
| `x86_64-pc-windows-msvc` | Windows (low priority) |

## GitHub Actions release pipeline

```yaml
# .github/workflows/release.yml
on:
  push:
    tags: ['v*']

jobs:
  build:
    strategy:
      matrix:
        include:
          - target: aarch64-apple-darwin
            os: macos-latest
          - target: x86_64-apple-darwin
            os: macos-latest
          - target: x86_64-unknown-linux-gnu
            os: ubuntu-latest
          - target: aarch64-unknown-linux-gnu
            os: ubuntu-latest
            cross: true
          - target: x86_64-unknown-linux-musl
            os: ubuntu-latest
            cross: true
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { targets: ${{ matrix.target }} }
      - if: matrix.cross
        run: cargo install cross --git https://github.com/cross-rs/cross
      - run: |
          if [ "${{ matrix.cross }}" = "true" ]; then
            cross build --release --target ${{ matrix.target }} -p opensessions-sidebar
          else
            cargo build --release --target ${{ matrix.target }} -p opensessions-sidebar
          fi
      - run: |
          mkdir -p artifacts
          cp target/${{ matrix.target }}/release/opensessions-sidebar artifacts/
          tar -czf artifacts/opensessions-sidebar-${{ matrix.target }}.tar.gz -C artifacts opensessions-sidebar
      - uses: softprops/action-gh-release@v2
        with:
          files: artifacts/*.tar.gz
```

## npm package layout (postinstall download)

```
opensessions/
├── package.json
├── bin/
│   ├── opensessions          # entry script (Node) → execs the right binary
│   └── opensessions-sidebar  # symlink resolved by postinstall
├── scripts/
│   └── postinstall.js        # downloads correct binary from GH releases
└── apps/
    ├── server/               # TS server (still run via bun)
    └── tui-rs/               # Rust source (skipped in published tarball)
```

`scripts/postinstall.js` (sketch):

```js
const os = require('os');
const fs = require('fs');
const path = require('path');
const https = require('https');
const { execSync } = require('child_process');
const { version } = require('../package.json');

function targetTriple() {
  const a = os.arch();   // 'x64' | 'arm64' | ...
  const p = os.platform(); // 'darwin' | 'linux' | 'win32'
  if (p === 'darwin' && a === 'arm64') return 'aarch64-apple-darwin';
  if (p === 'darwin' && a === 'x64')   return 'x86_64-apple-darwin';
  if (p === 'linux'  && a === 'x64')   return 'x86_64-unknown-linux-gnu';
  if (p === 'linux'  && a === 'arm64') return 'aarch64-unknown-linux-gnu';
  if (p === 'win32'  && a === 'x64')   return 'x86_64-pc-windows-msvc';
  throw new Error(`Unsupported: ${p}-${a}`);
}

const url = `https://github.com/.../releases/download/v${version}/opensessions-sidebar-${targetTriple()}.tar.gz`;
const dest = path.join(__dirname, '..', 'bin', 'opensessions-sidebar');
const tarball = path.join(__dirname, '..', 'bin', 'tmp.tar.gz');

await download(url, tarball);
execSync(`tar -xzf ${tarball} -C ${path.dirname(dest)}`);
fs.unlinkSync(tarball);
fs.chmodSync(dest, 0o755);
```

Pattern is borrowed from `esbuild`, `swc`, `rg` (ripgrep). Robust.

## Cargo profile (final)

```toml
# apps/tui-rs/Cargo.toml or workspace root
[profile.release]
opt-level     = "z"
lto           = "fat"
codegen-units = 1
panic         = "abort"
strip         = "symbols"
incremental   = false
```

Expected stripped binary sizes (estimate, will refine post-Phase-1):

| Target | Size |
|---|---|
| aarch64-apple-darwin | ~2.0 MB |
| x86_64-apple-darwin | ~2.2 MB |
| x86_64-linux-gnu | ~2.8 MB |
| x86_64-linux-musl | ~3.5 MB (static) |

After UPX (optional): ~1.0–1.5 MB. Skip UPX — antivirus heuristics flag it.

## Workspace layout

```
opensessions/
├── apps/
│   ├── server/        # TS, unchanged
│   ├── tui/           # TS (deprecated, kept until Phase 8)
│   └── tui-rs/        # Rust (new)
│       ├── Cargo.toml
│       ├── src/
│       │   ├── main.rs
│       │   ├── app.rs
│       │   ├── render/
│       │   ├── input/
│       │   ├── ws/
│       │   ├── mux.rs
│       │   ├── theme.rs
│       │   └── generated/
│       │       └── protocol.rs   # codegen from ts-rs
│       ├── tests/
│       └── fixtures/
├── packages/
│   └── runtime/       # TS, unchanged (source of types & themes)
├── docs/
│   └── ratatui-migration/
└── Cargo.toml         # workspace root
```

Add a workspace `Cargo.toml`:

```toml
[workspace]
members = ["apps/tui-rs"]
resolver = "2"
```

## Local development

```sh
# One-time:
rustup default stable
cargo install cargo-watch cargo-bloat cargo-audit

# Hot-reload during dev:
cargo watch -x 'run -p opensessions-sidebar'
# or:
cargo watch -s 'cargo run --release -p opensessions-sidebar'

# Inspect binary size:
cargo bloat --release --crates -p opensessions-sidebar
cargo bloat --release -p opensessions-sidebar -n 30   # top 30 functions

# Audit deps:
cargo audit
cargo tree --duplicates
```

## Versioning

Match the npm package version. Tag `v0.X.Y` triggers the GH workflow which
publishes both the npm package (postinstall pulls binaries) and the GH release
artifacts.

## Compatibility

Server stays TS, so `opensessions-sidebar` (Rust) talks to `apps/server` (TS,
Bun). Once both are released together, no version mismatch risk in normal
install.

If the user has bun-launched `apps/server` from one branch and `tui-rs` from
another, the schema-version handshake (Phase 0) catches mismatches and exits
cleanly with an actionable error.

## Cross-mux compatibility

Rust client detects mux at startup (tmux/zellij/none). The server already
supports both. No new build artifact needed — same binary handles all muxes.

## What we don't ship

- No `node_modules` for the Rust client (it has none).
- No bun runtime in the Rust binary itself (only used to launch the server,
  which the user already needs to have installed for the TS server).
- No prebuilt server binary — server is small and bun-launched fast enough.

## Pre-built binary install via Homebrew (future)

Phase 8+: publish a Homebrew tap so `brew install opensessions/tap/opensessions`
works without npm. The npm path stays primary.
