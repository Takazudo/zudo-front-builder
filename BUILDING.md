# Building zfb locally

> **Contributors only.** This document is for contributors building zfb from source. End users who just want to install and use the CLI should follow the [installation guide](https://takazudomodular.com/pj/zudo-front-builder/docs/getting-started/installation) instead.

This document covers the toolchain and one-time setup needed to build, test, and develop the `zfb` workspace locally. For the project mission and architecture, see [README.md](./README.md).

## Required toolchains

| Tool   | Version            | Notes                                             |
| ------ | ------------------ | ------------------------------------------------- |
| Rust   | stable             | Selected by [`rust-toolchain.toml`](./rust-toolchain.toml) (`channel = "stable"`, with `rustfmt` and `clippy`); `rustup` installs it on first use. Do not change the machine default to build zfb; see [Selecting a toolchain per command](#selecting-a-toolchain-per-command). |
| Node   | `^24.11.0 \|\| >=26.0.0` | The fetch script and Node-side glue use ESM + `fetch`; `engineStrict: true` is set in `pnpm-workspace.yaml`. The root `engines.node` is the Node 24+ part of the range Vite+ (`vp test`, the contributor test runner) supports; the `docs` member's `@takazudo/zudo-doc` and the published packages require Node 24 as well. Other versions fail `pnpm install`. |
| pnpm   | as pinned          | The repo declares `packageManager` in `package.json`; use Corepack (`corepack enable`) or install the matching pnpm version directly. |
| Python | >=3.11 as `python3` | Not used to build or run the CLI. The offline `tests/unit/*.sh` suite run by `pnpm b4push` and `health.yml` calls `python3` on [`scripts/capture-nextest-evidence.py`](./scripts/capture-nextest-evidence.py), which imports `tomllib` (3.11+). The [direct-V8 prototype](prototypes/direct-v8/README.md) scripts also need 3.11+ (`hashlib.file_digest`). |

Development and CI use **Node 24** (CI pins 24.14.0) and Rust stable on `ubuntu-latest`. The docs site depends on `@takazudo/zudo-doc`, which requires Node 24, so with `engineStrict: true` a full `pnpm install` (and so `pnpm b4push`) needs Node 24 locally too.

The published packages declare `engines.node >=24.0.0`; the CSS carrier checks load the native addon on the exact Node 24.0.0 floor.

### Selecting a toolchain per command

Keep toolchain choices scoped to the command or shell that needs them. Do not run
`rustup default`, `rustup override set`, global `npm`/`pnpm` installs or similar machine-wide
changes for zfb work; other checkouts and sessions on the same machine share those settings.

- Rust: `rust-toolchain.toml` already selects stable inside the checkout. To reproduce a
  recorded qualification toolchain, name it on the command: `cargo +1.99.1 test ...` or
  `RUSTUP_TOOLCHAIN=1.99.1 cargo test ...`.
- pnpm: the `packageManager` pin is applied per invocation by pnpm itself or Corepack.
- Python: pass the interpreter explicitly, for example
  `ZFB_DEV_PYTHON="$(command -v python3.11)"` and then `"$ZFB_DEV_PYTHON" scripts/...`. The
  system `python3` on macOS can be older than 3.11. The `tests/unit/*.sh` wrappers call bare
  `python3`, so for `pnpm b4push` put a 3.11+ interpreter first on that command's `PATH`
  (for example `PATH="/path/to/python3.11/bin:$PATH" pnpm b4push`).
- Build parallelism on a small machine: `CARGO_BUILD_JOBS=1 cargo ...` for that command only.

## First build is slow

The first `cargo build --workspace` on a clean machine takes **15–30 minutes**. The dominant cost is compiling the V8 JavaScript engine (the `v8` crate, pulled in through `zfb-js-host` by the `zfb-render` crate). This is a one-time cost: subsequent incremental builds are fast because Cargo only recompiles changed crates.

After installing JS deps, start with the normal workspace build:

```sh
pnpm install --frozen-lockfile
cargo build --workspace
```

That command is intentionally plain: it builds every workspace crate for your host target, and the `crates/zfb/build.rs` build script stages the embedded vendor snapshot, including Hono from the workspace pnpm store, before the `zfb` crate compiles. If you also want to precompile test harnesses after the first dependency install, use:

```sh
cargo build --workspace --all-targets
```

See [CONTRIBUTING.md](./CONTRIBUTING.md) for tips on speeding up local Rust compilation with `sccache`.

## Initial setup

```sh
# 1. Install JS deps
pnpm install --frozen-lockfile

# 2. Build the Rust workspace (first run: 15-30 min; subsequent runs: fast).
cargo build --workspace
```

## Bundling

zfb downloads, stages and runs no external bundler binary. Pages and the SSR bundle, islands, client scripts and module workers, TS-family plugins and `zfb.config.ts` are all bundled natively with Rolldown, which [`crates/zfb-bundler`](./crates/zfb-bundler) pins to a fixed upstream revision. `zfb --version` prints the embedded bundler identity on its `embedded bundler:` line. The CSS pipeline compiles utility classes with zudo-wind in-process and does not need a separate CSS CLI binary either.

The `ZFB_BUNDLER` and `ZFB_ROLLDOWN_PROTOTYPE` selectors, which once chose the esbuild backend, were removed: setting either one, to any value, fails before any command runs. `ZFB_ESBUILD_BIN` no longer exists.

## Embedded npm packages

`crates/zfb/build.rs` snapshots a small set of npm packages into `$OUT_DIR/vendor/` at compile time, then `crates/zfb/src/render_pipeline.rs` embeds the snapshot into the binary via `include_dir!`. At `zfb build` / `zfb dev` time, when the consumer has no `node_modules/`, the snapshot is extracted to a tempdir and the bundler resolves bare imports against it. Two groups of packages live in the snapshot:

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
pnpm format:check         # check (Oxfmt for JS/TS/JSON/YAML, mdx-formatter for MD/MDX; runs in CI)
pnpm format               # apply
```

## Useful scripts

```sh
pnpm docs:dev             # zfb dev server for the docs site
pnpm docs:build           # static build into docs/dist/
```

## Release builds and cross-compilation

The per-platform binaries shipped on npm (`linux-x64-gnu`, `linux-arm64-gnu`, `darwin-x64`, `darwin-arm64`, `win32-x64-msvc`) are built by [`.github/workflows/release.yml`](./.github/workflows/release.yml) using cross-compilation targets. That workflow is the source of truth for the full release matrix; `cargo build --workspace` above only targets your host platform.

## Direct V8 developer tooling

Since #4390 the default build uses the [direct V8 host](prototypes/direct-v8/CLI.md), which drives the pinned `v8` crate directly. The former deno_core host and its `embed_v8` rollback were removed in #4395. `release.yml` builds default features, so release binaries cut from this source use it too. There is no runtime config field or CLI switch.

```sh
cargo build --locked -p zfb --bin zfb
```

- The ICU 77 data is vendored at `crates/zfb-js-host/icu/icudtl.dat` and embedded at compile time; no environment variable or download is needed. `python3 prototypes/direct-v8/prepare-icu.py --verify` checks it against the pin.
- Run these builds through the heavy guard like any other V8 build.

The CI lane [`.github/workflows/direct-v8.yml`](./.github/workflows/direct-v8.yml) runs only when a PR carries the `direct-v8` or `native-default` label, or on manual dispatch. It records native rows on Linux x64, Linux arm64 and Windows x64 and is never a required check. Darwin rows need a Mac and are not run there.

The developer scripts in [`prototypes/direct-v8/`](prototypes/direct-v8/README.md) require Python >=3.11. On macOS, explicitly select an installed compatible interpreter, for example `ZFB_DEV_PYTHON="$(command -v python3.11)"`, and invoke the tools with `"$ZFB_DEV_PYTHON"`. Verify its version before running. This does not add a Python requirement to the production CLI.

## Constrained hosts (cloud sessions, small disks)

A full `cargo build --workspace` compiles V8 and needs tens of gigabytes of `target/`. A
cloud session observed with a 32 GB filesystem ran out of disk during a full Rust build, and
the cloud session of 2026-10-19 had about 30 GB writable and 15 GiB RAM. On such hosts:

- check `df -h .` and the existing `target/` size before any workspace build;
- prefer scoped commands (`cargo test -p <crate>`) for crates that do not depend on
  `zfb-render`, which do not compile V8;
- leave full builds and the full Rust suite to the hosted Linux CI (`health.yml`);
- a build that stops for lack of disk or memory ran zero tests: record it as not run.

The agent-facing rules for this are in [CLAUDE.md](./CLAUDE.md#codex-cloud-disk-capacity-and-broad-rust-verification).

## Native verification on macOS and Windows

CI runs the Rust suite on Linux on every PR; macOS runs weekly in `exam.yml`, and no CI job
runs the Rust suite on Windows. A Linux result, an emulated run or a WSL run does not stand in
for native macOS or Windows. Native runs follow [CLAUDE.md](./CLAUDE.md#moving-development-and-frozen-qualification):
check out one immutable SHA, keep `CARGO_BUILD_JOBS=1` and the heavy guard on the command,
run commands serially with zero retries, and record the toolchain, commands and first
results. Use [`scripts/capture-nextest-evidence.py`](./scripts/capture-nextest-evidence.py)
(nextest 0.9.140, Python 3.11+) to keep each command's original JUnit report. The current
native status and pending Mac prompts are in [research/development-queue.md](./research/development-queue.md).
