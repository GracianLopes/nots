# Development

## Prerequisites

- Rust toolchain (`rustup`) with `cargo`
- Node.js >= 22 and pnpm 10 (`corepack enable` or standalone)
- Tauri 2 system dependencies (Linux: webkit2gtk-4.1, libsoup-3.0,
  javascriptcoregtk-4.1, gtk+-3.0, alsa/pulse)

## Setup

```bash
pnpm install
cargo build -p notsai-tauri
```

## Run (dev)

```bash
pnpm tauri dev
```

## Verification gates

Every change must pass, in order:

```bash
pnpm lint && pnpm typecheck
pnpm test
cargo fmt --all --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

## Build (release)

```bash
pnpm tauri build
```