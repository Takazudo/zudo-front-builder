# Contributing

Thanks for your interest in `zudo-front-builder`. The full build pipeline is shipping — embedded V8 host, islands bundler, CSS pipeline, content collections, client router, and dev-server are all wired. The toolchain and pre-commit pipeline below are active.

## Toolchain

- **Rust**: stable channel, pinned via `rust-toolchain.toml` at the repo root. With `rustup` installed, the correct toolchain is selected automatically.
- **Node / pnpm**: **Node 20 or later and pnpm 10 or later are required.** pnpm is pinned via [Corepack](https://nodejs.org/api/corepack.html) (the `packageManager` field in `package.json`). Run `corepack enable` once and pnpm will resolve to the pinned version automatically. The repo sets `engine-strict=true` in `.npmrc`, so `pnpm install` will hard-error if your Node or pnpm version is below the minimum — install the correct version before running install.

## First build expectation

The first `cargo build --workspace` on a clean machine takes **15–30 minutes**. The bottleneck is V8 — the JavaScript engine pulled in by the `zfb-render` crate via `deno_core`. This is unavoidable on a cold cache but is a one-time cost.

To minimise the wait on your first checkout:

```sh
# Compile workspace crates + test harnesses, skip recompiling
# third-party deps that haven't changed.
cargo build --workspace --tests --no-deps
```

After this, incremental rebuilds and `cargo test` runs are fast.

### Cargo feature gate — `embed_v8`

The V8 engine is gated behind a **default-on** cargo feature called `embed_v8` on the `zfb-render` crate. Crates that do not need JavaScript rendering (e.g. `zfb-content`, utility crates) do not depend on `zfb-render` and therefore never compile V8 unless they explicitly ask for it. This means:

- `cargo test -p zfb-content` compiles in seconds — no V8.
- `cargo build --workspace` compiles everything including V8 exactly once and caches the result.
- If you want to build or test workspace members that don't touch rendering, use `-p <crate>` to avoid pulling in the V8 dependency graph unnecessarily.

### Speeding up local builds with sccache

[sccache](https://github.com/mozilla/sccache) is a compiler cache that wraps `rustc` and stores compilation artefacts in a local (or remote S3/GCS) cache. For zfb development its biggest win is the V8 layer: once V8 is in the sccache store, switching branches or cleaning `target/` no longer triggers a V8 recompile.

```sh
# Install
cargo install sccache

# Enable for the current shell session
export RUSTC_WRAPPER=sccache

# Optional: point at a larger on-disk cache (default is ~/.cache/sccache)
export SCCACHE_DIR=/path/to/large/drive/.sccache
export SCCACHE_CACHE_SIZE=20G
```

Add the exports to your shell profile to persist them.

### Faster test runs with cargo-nextest

If the workspace test suite becomes slow, [cargo-nextest](https://nexte.st/) is a drop-in replacement for `cargo test` that runs tests in parallel and streams output efficiently:

```sh
cargo install cargo-nextest
cargo nextest run --workspace
```

## Workspace layout

This is a Cargo workspace. Crates live under `crates/`. The main bin crate is `crates/zfb/`.

Common commands:

```sh
cargo build --workspace
cargo run -p zfb
```

## Workflow basics

- Branch off `main` (or the relevant base branch for an in-flight epic).
- Keep commits focused; conventional commit-style messages are appreciated but not strictly enforced.
- `lefthook` runs the pre-commit pipeline (rustfmt, clippy, JS/CSS formatters). For ad-hoc checks before opening a PR, `cargo fmt` and `cargo clippy --workspace` are still useful.
- Open a PR against `main` (or the relevant epic base branch). CI runs the same checks as the pre-commit pipeline.

## Formatting

JavaScript, TypeScript, JSON, and YAML are formatted with **Prettier**. Markdown and MDX use the dedicated `@takazudo/mdx-formatter` step.

Run formatters across the repo:

```sh
pnpm format         # autoformat all (TS/JS/JSON/YAML + MD/MDX)
pnpm format:check   # check formatting (CI-friendly, exits non-zero on diffs)
```

Targeted variants:

- `pnpm format:ts` / `pnpm format:check:ts` — Prettier over JS/TS/JSON/YAML
- `pnpm format:mdx` / `pnpm format:check:mdx` — Markdown/MDX

The pre-commit hook (lefthook) runs Prettier on staged matching files and re-stages the fixes.

### Why Prettier (and not Oxfmt) for now

[Oxfmt](https://oxc.rs/docs/guide/usage/formatter.html) is the formatter from the Oxc project and is the long-term direction the JS ecosystem is moving toward (30x faster than Prettier, 100% Prettier JS/TS conformance, supports JSON and YAML). At evaluation time (2026-04-26) it is published as `oxfmt` v0.46.0 and is officially in **Beta** ([announcement](https://oxc.rs/blog/2026-02-24-oxfmt-beta)) — sub-1.0 with no formal stable release yet, comparable to where Oxlint sat before its v1.0 stable announcement.

To keep the foundation conservative we ship Prettier today and revisit the swap once Oxfmt cuts a 1.0 / "stable" release. Tracker: [oxc-project/oxc milestone 19](https://github.com/oxc-project/oxc/milestone/19).

## CI secrets

The Cloudflare Pages deploy workflow (wired up in a later sub-task) expects the following GitHub Actions secrets to be present on the repository:

- **`CLOUDFLARE_API_TOKEN`** (required) — API token with `Pages:Edit` permission, used by `wrangler pages deploy`.
- **`CLOUDFLARE_ACCOUNT_ID`** (required) — the Cloudflare account ID that owns the Pages project.
- **`IFTTT_PROD_NOTIFY`** (optional) — IFTTT webhook key used to push a notification when a production deploy lands. Omit to skip notifications.

## External tool version pins

zfb shells out to a small set of third-party tools (esbuild for the islands bundler, wrangler/workerd for Cloudflare Pages preview, Tailwind v4 for the CSS engine). Every one of those tools is **exact-pinned** so that the same source tree produces byte-identical output regardless of when or where it is built — this matters for asset-hash stability and for keeping the SSR pipeline from drifting under our feet when upstream cuts a patch release.

The pin lives in two places that **must move together**:

1. The npm-side declaration in [`package.json`](./package.json) (no `^`/`~` for these entries).
2. The Rust-side constant that the subprocess wrapper checks against at startup.

| Tool        | Rust constant                                                                                       | Where                                          | npm-side pin                          |
| ----------- | --------------------------------------------------------------------------------------------------- | ---------------------------------------------- | ------------------------------------- |
| esbuild     | `EXPECTED_ESBUILD_VERSION`, `EXPECTED_ESBUILD_SHA256`                                               | `crates/zfb-islands/src/esbuild.rs`            | (binary; populated by release engineering into `crates/zfb/binaries/esbuild/esbuild`) |
| wrangler    | `EXPECTED_WRANGLER_VERSION`                                                                         | `crates/zfb/src/commands/preview.rs`           | `wrangler` in `package.json`          |
| workerd     | `EXPECTED_WORKERD_VERSION` (informational; pinned transitively via wrangler → `pnpm-lock.yaml`)     | `crates/zfb/src/commands/preview.rs`           | (transitive; resolved in `pnpm-lock.yaml`) |

### Bumping wrangler / workerd

1. Pick the new versions you want (typically a coordinated wrangler + workerd set; see the [wrangler changelog](https://github.com/cloudflare/workers-sdk/releases?q=wrangler) for matched sets).
2. Edit the `wrangler` entry in `package.json` to the new exact version.
3. Edit `EXPECTED_WRANGLER_VERSION` and `EXPECTED_WORKERD_VERSION` in `crates/zfb/src/commands/preview.rs` to match.
4. Run `pnpm install` to refresh `pnpm-lock.yaml`. Confirm the resolved `workerd` version in the lockfile matches the constant you just set.
5. Run `cargo test -p zfb` to make sure the version-gate tests still pass.
6. Commit `package.json`, `pnpm-lock.yaml`, and the constants change in one commit so the pin moves atomically.

If you ever need to bypass the wrangler version gate (e.g. while a bump is mid-flight on a feature branch), set `ZFB_SKIP_WRANGLER_VERSION_CHECK=1` for the duration of the `zfb preview` invocation. Do not check this in.

### Bumping esbuild

esbuild is shipped as a Go-built standalone CLI binary that release engineering downloads into `crates/zfb/binaries/esbuild/esbuild` at release-tarball assembly time — the binary is **not** committed to this repo. To bump:

1. Pick the new esbuild version (the latest stable 0.x at release-cut time; see <https://github.com/evanw/esbuild/releases>).
2. Edit `EXPECTED_ESBUILD_VERSION` in `crates/zfb-islands/src/esbuild.rs`.
3. Compute the SHA-256 of the platform-specific binary you intend to ship (e.g. `sha256sum esbuild` on Linux, `shasum -a 256 esbuild` on macOS) and edit `EXPECTED_ESBUILD_SHA256` to that lowercase hex digest. Leave the constant as the empty string (`""`) only if you are explicitly handing the SHA-pin step off to the next release-engineering pass — the version gate still runs in that case, but the checksum gate is skipped (with a clear log line).
4. Update the `## esbuild Version` table in `crates/zfb-islands/README.md`.
5. Run `cargo test -p zfb-islands` — the unit tests use the mock-subprocess code path and do not require the real binary.
6. Drop the new binary into `crates/zfb/binaries/esbuild/esbuild` locally to test the end-to-end gate, but do not commit it (`.gitignore` already excludes the path).

The verification gate is implemented in `ensure_binary_verified` in `crates/zfb-islands/src/esbuild.rs` and runs once per binary path per process: it spawns `esbuild --version`, asserts the reported version equals `EXPECTED_ESBUILD_VERSION`, and (when populated) hashes the binary and asserts the SHA-256 equals `EXPECTED_ESBUILD_SHA256`. A mismatch on either gate aborts with a clear, actionable error pointing back at this section.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](./LICENSE).
