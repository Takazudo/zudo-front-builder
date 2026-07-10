# Building zfb locally

> **Contributors only.** This document is for contributors building zfb from source. End users who just want to install and use the CLI should follow the [installation guide](https://takazudomodular.com/pj/zudo-front-builder/docs/getting-started/installation) instead.

This document covers the toolchain and one-time setup needed to build, test, and develop the `zfb` workspace locally. For the project mission and architecture, see [README.md](./README.md).

## Required toolchains

| Tool   | Version            | Notes                                             |
| ------ | ------------------ | ------------------------------------------------- |
| Rust   | stable             | `rustup install stable && rustup default stable`  |
| Node   | ≥ 22.12.0          | The fetch script and Node-side glue use ESM + `fetch`; `engineStrict: true` is set in `pnpm-workspace.yaml`. Node 22.12.0 is the effective floor (vite 7 / `@tailwindcss/vite` requires `^20.19.0 || >=22.12.0`); earlier 22.x versions will fail `pnpm install`. |
| pnpm   | as pinned          | The repo declares `packageManager` in `package.json`; use Corepack (`corepack enable`) or install the matching pnpm version directly. |

CI uses Node 22 and Rust stable on `ubuntu-latest`.

## First build is slow

The first `cargo build --workspace` on a clean machine takes **15–30 minutes**. The dominant cost is compiling the V8 JavaScript engine (pulled in via `deno_core` by the `zfb-render` crate). This is a one-time cost: subsequent incremental builds are fast because Cargo only recompiles changed crates.

After installing JS deps, start with the normal workspace build:

```sh
pnpm install --frozen-lockfile
cargo build --workspace
```

That command is intentionally plain: it builds every workspace crate for your host target, and the `crates/zfb/build.rs` build script automatically downloads, SHA-256-verifies, and stages the pinned `esbuild` and `tailwindcss` binaries before the `zfb` crate compiles. If you also want to precompile test harnesses after the first dependency install, use:

```sh
cargo build --workspace --all-targets
```

See [CONTRIBUTING.md](./CONTRIBUTING.md) for tips on speeding up local Rust compilation with `sccache`.

## Initial setup

```sh
# 1. Install JS deps
pnpm install --frozen-lockfile

# 2. Build the Rust workspace (first run: 15-30 min; subsequent runs: fast).
#    This also provisions the pinned esbuild and tailwindcss binaries.
cargo build --workspace
```

## Provisioned external binaries

The islands bundler shells out to `esbuild`, and the CSS pipeline shells out to the Tailwind v4 standalone CLI. These binary files are **not** committed to git; they are materialized on demand.

The default path is Cargo-driven. Any normal build that compiles `crates/zfb` runs [`crates/zfb/build.rs`](./crates/zfb/build.rs), which:

- downloads the pinned platform-specific `@esbuild/*` npm tarball, extracts the standalone `esbuild` binary, verifies it against the `ESBUILD_SHA256_*` constants, and stages it under `crates/zfb/binaries/esbuild/`;
- downloads the pinned Tailwind v4 standalone binary from the upstream GitHub release, verifies it against the `TAILWIND_SHA256_*` constants, and stages it as `crates/zfb/binaries/tailwindcss-v4` (or `.exe` on Windows);
- copies both verified binaries into the embedded vendor snapshot so installed `zfb` binaries can run without a workspace checkout.

Supported build platforms are `darwin-x64`, `darwin-arm64`, `linux-x64-gnu`, `linux-arm64-gnu`, and `win32-x64-msvc`. For unsupported targets such as musl-libc Linux, set both `ZFB_ESBUILD_BIN` and `ZFB_TAILWIND_BIN` to absolute paths for pre-verified binaries.

`pnpm fetch:tailwind` still exists as a Tailwind-only developer convenience, but it is not part of first-build setup. Plain `cargo build --workspace` provisions both binaries.

See [`crates/zfb-css/README.md`](./crates/zfb-css/README.md) ("Getting the binary") for the Tailwind runtime contract and the [`crates/zfb/binaries/README.md`](./crates/zfb/binaries/README.md) for the runtime resolution layout.

## Embedded npm packages

`crates/zfb/build.rs` snapshots a small set of npm packages into `$OUT_DIR/vendor/` at compile time, then `crates/zfb/src/render_pipeline.rs` embeds the snapshot into the binary via `include_dir!`. At `zfb build` / `zfb dev` time, when the consumer has no `node_modules/`, the snapshot is extracted to a tempdir and esbuild resolves bare imports against it. Two groups of packages live in the snapshot:

| Group | Packages | Source | Pin location |
| --- | --- | --- | --- |
| `@takazudo/*` (sub #198) | `@takazudo/zfb`, `@takazudo/zfb-runtime` | `packages/<name>/src/` (TypeScript source) | the workspace itself — versions follow `packages/<name>/package.json` |
| Framework runtimes (sub #209) | `preact`, `preact-render-to-string`, `hono` | `node_modules/.pnpm/<name>@<ver>*/node_modules/<name>/` (published trees) | `pnpm-lock.yaml`; mirrored as `*_VERSION` constants in `crates/zfb/build.rs` |

To bump a framework-runtime version:

1. Update the dependency in the relevant `package.json` (e.g. `packages/zfb-runtime/package.json` for `hono`, or in a standalone demo repo's `package.json` for `preact`/`preact-render-to-string`).
2. Run `pnpm install` so `pnpm-lock.yaml` re-resolves.
3. Update the corresponding `*_VERSION` constant in `crates/zfb/build.rs` to match the new lockfile entry.
4. Rebuild — the build script re-snapshots the new tree.

`pnpm install --frozen-lockfile` is a hard prerequisite for `cargo build` because the build script reads `node_modules/.pnpm/`. The smoke test `embedded_node_modules_extracts_runtime_layout` (in `crates/zfb/src/render_pipeline.rs`) and the integration test `framework_packages_no_pnpm` (in `crates/zfb/tests/`) both fail with an actionable error if the embedded snapshot drifts away from the pinned versions.

## Running tests

```sh
# Default — non-ignored workspace suite; Cargo provisions host binaries as needed
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
pnpm docs:dev             # zfb dev server for the docs site
pnpm docs:build           # static build into docs/dist/
pnpm fetch:tailwind       # optional Tailwind-only prefetch; cargo build is authoritative
```

## Release builds and cross-compilation

The per-platform binaries shipped on npm (`linux-x64-gnu`, `linux-arm64-gnu`, `darwin-x64`, `darwin-arm64`, `win32-x64-msvc`) are built by [`.github/workflows/release.yml`](./.github/workflows/release.yml) using cross-compilation targets. That workflow is the source of truth for the full release matrix; `cargo build --workspace` above only targets your host platform.
