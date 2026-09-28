# Building zfb locally

> **Contributors only.** This document is for contributors building zfb from source. End users who just want to install and use the CLI should follow the [installation guide](https://takazudomodular.com/pj/zudo-front-builder/docs/getting-started/installation) instead.

This document covers the toolchain and one-time setup needed to build, test, and develop the `zfb` workspace locally. For the project mission and architecture, see [README.md](./README.md).

## Required toolchains

| Tool   | Version            | Notes                                             |
| ------ | ------------------ | ------------------------------------------------- |
| Rust   | stable             | `rustup install stable && rustup default stable`  |
| Node   | ≥ 22.16.0          | The fetch script and Node-side glue use ESM + `fetch`; `engineStrict: true` is set in `pnpm-workspace.yaml`. Node 22.16.0 is the effective floor on the 22.x line (`html-validate` requires `^20.19.0 || ^22.16.0 || >=24.0.0`); earlier 22.x versions will fail `pnpm install`. |
| pnpm   | as pinned          | The repo declares `packageManager` in `package.json`; use Corepack (`corepack enable`) or install the matching pnpm version directly. |

CI uses Node 22 and Rust stable on `ubuntu-latest`.

## First build is slow

The first `cargo build --workspace` on a clean machine takes **15–30 minutes**. The dominant cost is compiling the V8 JavaScript engine (pulled in via `deno_core` by the `zfb-render` crate). This is a one-time cost: subsequent incremental builds are fast because Cargo only recompiles changed crates.

After installing JS deps, start with the normal workspace build:

```sh
pnpm install --frozen-lockfile
cargo build --workspace
```

That command is intentionally plain: it builds every workspace crate for your host target, and the `crates/zfb/build.rs` build script downloads, SHA-256-verifies, and stages the pinned esbuild binary before the `zfb` crate compiles. It also embeds Hono from the workspace pnpm store. If you also want to precompile test harnesses after the first dependency install, use:

```sh
cargo build --workspace --all-targets
```

See [CONTRIBUTING.md](./CONTRIBUTING.md) for tips on speeding up local Rust compilation with `sccache`.

## Initial setup

```sh
# 1. Install JS deps
pnpm install --frozen-lockfile

# 2. Build the Rust workspace (first run: 15-30 min; subsequent runs: fast).
#    This also provisions the pinned esbuild binary.
cargo build --workspace
```

## Provisioned external binaries

The islands bundler uses the standalone `esbuild` executable. The CSS pipeline compiles utility classes with zudo-wind in-process and does not need a separate CSS CLI binary.

The default esbuild path is Cargo-driven. Any normal build that compiles `crates/zfb` runs [`crates/zfb/build.rs`](./crates/zfb/build.rs), which:

- downloads the pinned platform-specific `@esbuild/*` npm tarball, extracts the standalone `esbuild` binary, verifies it against the `ESBUILD_SHA256_*` constants, and stages it under `crates/zfb/binaries/esbuild/`;
- copies the verified esbuild executable into the embedded vendor snapshot so installed `zfb` binaries can run without a workspace checkout.

Supported esbuild download platforms are `darwin-x64`, `darwin-arm64`, `linux-x64-gnu`, `linux-arm64-gnu`, and `win32-x64-msvc`, matched against Cargo's exact `TARGET` triple (not a substring match, so `x86_64-unknown-linux-musl` does not accidentally match the `-gnu` platform). For unsupported targets such as musl-libc Linux, set `ZFB_ESBUILD_BIN` to an absolute path to a pre-verified binary.

### `ZFB_ESBUILD_BIN` override contract

When `ZFB_ESBUILD_BIN` is set to a non-empty value, `build.rs`:

- requires the value to be an **absolute path** (relative paths are rejected with a clear error);
- requires the path to exist and be a regular file;
- stages that exact file into the embedded vendor snapshot in place of a download — **it skips SHA-256 pinning entirely**. Overrides are a documented trust boundary: the operator supplying the path is responsible for verifying it, the same way a locally-built or vendor-mirrored binary would be trusted. `build.rs` never downloads anything for a binary that has a valid override.

An empty-string value (e.g. an env var that is set but blank) is treated as unset, not as an override.

Cargo re-runs the build script when `ZFB_ESBUILD_BIN` changes (`cargo:rerun-if-env-changed`), and — once an override path is validated — when that file's contents change (`cargo:rerun-if-changed=<path>`), so editing an override binary in place and rebuilding picks it up without a manual `touch`.

See [`crates/zfb/binaries/README.md`](./crates/zfb/binaries/README.md) for the runtime resolution layout.

[`scripts/verify-vendor-override.sh`](./scripts/verify-vendor-override.sh) proves this contract end-to-end on a clean tree: it `git archive`s `HEAD` into a scratch directory (so no gitignored binary slot can be present), symlinks in `node_modules/` from the source checkout so the build script can read the embedded Hono package from `node_modules/.pnpm/`, then runs an offline, isolated-`$CARGO_TARGET_DIR` `cargo check -p zfb --no-default-features --offline` with `ZFB_ESBUILD_BIN` pointed at the source checkout's staged binary. It asserts zero downloads occur, the staged `$OUT_DIR/vendor/bin/` bytes hash-equal the override source, and a bogus override path fails with the direct path + var-naming error. Run it locally with `scripts/verify-vendor-override.sh` (defaults to `crates/zfb/binaries/esbuild/esbuild` as the override source — populate it first with a plain `cargo build --workspace`, which also warms `$CARGO_HOME`'s registry cache so the script's isolated `--offline` check can resolve crates without network access; or point `ZFB_VERIFY_ESBUILD_SRC` at a pre-staged binary elsewhere). Unix hosts only (macOS/Linux). It is not wired into CI or `b4push` — it's a manual confirmation tool, several minutes per run (a cold, isolated `cargo check`).

## Embedded npm packages

`crates/zfb/build.rs` snapshots a small set of npm packages into `$OUT_DIR/vendor/` at compile time, then `crates/zfb/src/render_pipeline.rs` embeds the snapshot into the binary via `include_dir!`. At `zfb build` / `zfb dev` time, when the consumer has no `node_modules/`, the snapshot is extracted to a tempdir and esbuild resolves bare imports against it. Two groups of packages live in the snapshot:

| Group | Packages | Source | Pin location |
| --- | --- | --- | --- |
| `@takazudo/*` (sub #198) | `@takazudo/zfb`, `@takazudo/zfb-runtime` | `packages/<name>/src/` (TypeScript source) | the workspace itself — versions follow `packages/<name>/package.json` |
| Router runtime | `hono` | `node_modules/.pnpm/hono@<ver>/node_modules/hono/` (published tree) | `pnpm-lock.yaml`; mirrored by `HONO_VERSION` in `crates/zfb/build.rs` |

To bump Hono:

1. Update the dependency in `packages/zfb-runtime/package.json`.
2. Run `pnpm install` so `pnpm-lock.yaml` re-resolves.
3. Update `HONO_VERSION` in `crates/zfb/build.rs` to match the new lockfile entry.
4. Rebuild — the build script re-snapshots the new tree.

`pnpm install --frozen-lockfile` is a hard prerequisite for `cargo build` because the build script reads Hono from `node_modules/.pnpm/`. The smoke test `embedded_node_modules_extracts_runtime_layout` (in `crates/zfb/src/render_pipeline.rs`) and the integration test `framework_packages_no_pnpm` (in `crates/zfb/tests/`) verify the embedded owned runtime and Hono layout.

## Running tests

```sh
# Default — non-ignored workspace suite; Cargo provisions host binaries as needed
cargo test --workspace

# Heavyweight — runs the previously-`#[ignore]`-gated real zudo-wind tests.
cargo test -p zudo-wind --tests -- --ignored
```

The zudo-wind engine runs in-process and does not require a staged CLI binary.

## Format / lint

```sh
pnpm format:check         # check (TS/MD/MDX, runs in CI)
pnpm format               # apply
```

## Useful scripts

```sh
pnpm docs:dev             # zfb dev server for the docs site
pnpm docs:build           # static build into docs/dist/
```

## Release builds and cross-compilation

The per-platform binaries shipped on npm (`linux-x64-gnu`, `linux-arm64-gnu`, `darwin-x64`, `darwin-arm64`, `win32-x64-msvc`) are built by [`.github/workflows/release.yml`](./.github/workflows/release.yml) using cross-compilation targets. That workflow is the source of truth for the full release matrix; `cargo build --workspace` above only targets your host platform.
