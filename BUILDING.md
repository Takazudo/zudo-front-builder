# Building zfb locally

This document covers the toolchain and one-time setup needed to build, test, and develop the `zfb` workspace locally. For the project mission and architecture, see [README.md](./README.md).

## Required toolchains

| Tool   | Version            | Notes                                             |
| ------ | ------------------ | ------------------------------------------------- |
| Rust   | stable             | `rustup install stable && rustup default stable`  |
| Node   | ≥ 20 (LTS)         | The fetch script and Node-side glue use ESM + `fetch`. |
| pnpm   | as pinned          | The repo declares `packageManager` in `package.json`; use Corepack (`corepack enable`) or install the matching pnpm version directly. |

CI uses Node 20 and Rust stable on `ubuntu-latest`.

## Initial setup

```sh
# 1. Install JS deps
pnpm install --frozen-lockfile

# 2. Fetch the pinned tailwindcss v4 standalone CLI binary into
#    crates/zfb/binaries/tailwindcss-v4 (idempotent — fast no-op on re-run)
pnpm fetch:tailwind

# 3. Build the Rust workspace
cargo build --workspace
```

## The Tailwind binary

The CSS pipeline shells out to the Tailwind v4 standalone CLI as a subprocess. The binary file is **not** committed to git — it is materialized on demand.

There are two supported ways to provide it:

1. **Default — `pnpm fetch:tailwind`**: downloads the pinned asset from the upstream GitHub release, verifies SHA-256 against `sha256sums.txt`, and places it at `crates/zfb/binaries/tailwindcss-v4` (or `tailwindcss-v4.exe` on Windows). Supported platforms: `darwin-x64`, `darwin-arm64`, `linux-x64`, `linux-arm64`, `win32-x64`. For musl-libc Linux, use the override below.
2. **Override — `ZFB_TAILWIND_BIN`**: set this env var to an absolute path of any tailwindcss v4 standalone binary you already have on disk. The engine uses it directly and `pnpm fetch:tailwind` becomes a no-op. This is the right choice for CI images that already bundle tailwind, or for musl-libc systems.

See [`crates/zfb-css/README.md`](./crates/zfb-css/README.md) ("Getting the binary") for the full contract and the [`crates/zfb/binaries/README.md`](./crates/zfb/binaries/README.md) for the runtime resolution layout.

## Running tests

```sh
# Default — fast, mock-only, no external binary required
cargo test --workspace

# Heavyweight — runs the previously-`#[ignore]`-gated tests that exercise
# the real Tailwind subprocess. Requires the binary to be available.
ZFB_TAILWIND_BIN="$(pwd)/crates/zfb/binaries/tailwindcss-v4" \
  cargo test -p zfb-css --tests -- --ignored
```

The `ZFB_TAILWIND_BIN` export is needed because `cargo test -p <crate>` runs with the package directory as CWD, while the engine's default relative path is resolved from the workspace root.

## Format / lint

```sh
pnpm format:check         # check (TS/MD/MDX, runs in CI)
pnpm format               # apply
```

## Useful scripts

```sh
pnpm docs:dev             # Astro dev server for the docs site
pnpm docs:build           # static build into docs/dist/
pnpm fetch:tailwind       # (re-)materialize the Tailwind v4 binary
```
