# Building zfb locally

This document covers the toolchain and one-time setup needed to build, test, and develop the `zfb` workspace locally. For the project mission and architecture, see [README.md](./README.md).

## Required toolchains

| Tool   | Version            | Notes                                             |
| ------ | ------------------ | ------------------------------------------------- |
| Rust   | stable             | `rustup install stable && rustup default stable`  |
| Node   | ≥ 22 (LTS)         | The fetch script and Node-side glue use ESM + `fetch`; `engine-strict=true` is set, and the docs site's transitive `chevrotain@12` declares `engines.node >=22`. |
| pnpm   | as pinned          | The repo declares `packageManager` in `package.json`; use Corepack (`corepack enable`) or install the matching pnpm version directly. |

CI uses Node 22 and Rust stable on `ubuntu-latest`.

## First build is slow

The first `cargo build --workspace` on a clean machine takes **15–30 minutes**. The dominant cost is compiling the V8 JavaScript engine (pulled in via `deno_core` by the `zfb-render` crate). This is a one-time cost: subsequent incremental builds are fast because Cargo only recompiles changed crates.

To make the wait productive, start with:

```sh
cargo build --workspace --tests --no-deps
```

`--no-deps` skips recompiling external dependencies (useful when you only changed workspace crate code). `--tests` also compiles test harnesses so the first `cargo test` run is faster.

See [CONTRIBUTING.md](./CONTRIBUTING.md) for tips on speeding up local Rust compilation with `sccache`.

## Initial setup

```sh
# 1. Install JS deps
pnpm install --frozen-lockfile

# 2. Fetch the pinned tailwindcss v4 standalone CLI binary into
#    crates/zfb/binaries/tailwindcss-v4 (idempotent — fast no-op on re-run)
pnpm fetch:tailwind

# 3. Build the Rust workspace (first run: 15-30 min; subsequent runs: fast)
cargo build --workspace
```

## The Tailwind binary

The CSS pipeline shells out to the Tailwind v4 standalone CLI as a subprocess. The binary file is **not** committed to git — it is materialized on demand.

There are two supported ways to provide it:

1. **Default — `pnpm fetch:tailwind`**: downloads the pinned asset from the upstream GitHub release, verifies SHA-256 against `sha256sums.txt`, and places it at `crates/zfb/binaries/tailwindcss-v4` (or `tailwindcss-v4.exe` on Windows). Supported platforms: `darwin-x64`, `darwin-arm64`, `linux-x64`, `linux-arm64`, `win32-x64`. For musl-libc Linux, use the override below.
2. **Override — `ZFB_TAILWIND_BIN`**: set this env var to an absolute path of any tailwindcss v4 standalone binary you already have on disk. The engine uses it directly and `pnpm fetch:tailwind` becomes a no-op. This is the right choice for CI images that already bundle tailwind, or for musl-libc systems.

See [`crates/zfb-css/README.md`](./crates/zfb-css/README.md) ("Getting the binary") for the full contract and the [`crates/zfb/binaries/README.md`](./crates/zfb/binaries/README.md) for the runtime resolution layout.

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
