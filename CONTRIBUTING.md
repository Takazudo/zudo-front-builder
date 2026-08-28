# Contributing

Thanks for your interest in `zudo-front-builder`. The full build pipeline is shipping — embedded V8 host, islands bundler, CSS pipeline, content collections, client router, and dev-server are all wired. The toolchain and pre-commit pipeline below are active.

## Toolchain

- **Rust**: stable channel, pinned via `rust-toolchain.toml` at the repo root. With `rustup` installed, the correct toolchain is selected automatically.
- **Node / pnpm**: **Node 22.16.0 or later and pnpm 11 or later are required.** pnpm is pinned via [Corepack](https://nodejs.org/api/corepack.html) (the `packageManager` field in `package.json`). Run `corepack enable` once and pnpm will resolve to the pinned version automatically. The repo sets `engineStrict: true` in `pnpm-workspace.yaml`, so `pnpm install` will hard-error if your Node or pnpm version is below the minimum — install the correct version before running install. Node 22.16.0 is the effective floor on the 22.x line (`html-validate` requires `^20.19.0 || ^22.16.0 || >=24.0.0`); earlier 22.x versions will fail `pnpm install`.

## First build expectation

The first `cargo build --workspace` on a clean machine takes **15–30 minutes**. The bottleneck is V8 — the JavaScript engine pulled in by the `zfb-render` crate via `deno_core`. This is unavoidable on a cold cache but is a one-time cost.

To minimise the wait on your first checkout:

```sh
pnpm install --frozen-lockfile
cargo build --workspace
```

This is executable as written and is the supported first build. It also runs `crates/zfb/build.rs`, which downloads, SHA-256-verifies, and stages the pinned host-platform `esbuild` and `tailwindcss` binaries. After this, incremental rebuilds and test runs are fast.

If you want to compile test harnesses during the warm-up build, use Cargo's all-targets mode:

```sh
cargo build --workspace --all-targets
```

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

[cargo-nextest](https://nexte.st/) is the adopted CI runner for Rust tests. It runs tests in parallel, applies the repo's retry/timeout policy from [`.config/nextest.toml`](./.config/nextest.toml), and emits JUnit telemetry.

```sh
cargo install cargo-nextest
cargo nextest run --workspace
```

nextest does not run doctests, so run the doctest lane separately when you need local parity with CI:

```sh
cargo test --workspace --doc
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
- `lefthook` runs the pre-commit pipeline: Prettier over JS/TS/JSON/YAML (no Rust) and `@takazudo/mdx-formatter` over MD/MDX. Rust formatting is not enforced automatically — run `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` manually before opening a PR.
- Open a PR against `main` (or the relevant epic base branch). CI is a strict superset of the pre-commit pipeline. The main PR gate includes `cargo fmt --all --check`, `pnpm typecheck:workspace`, `pnpm test:workspace`, `pnpm format:check`, `cargo build --workspace --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, the env-gated binary integration tests, `cargo nextest run --workspace --profile ci`, `cargo test --workspace --doc`, and actionlint.

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

The Cloudflare Workers deploy workflows expect the following GitHub Actions secrets to be present on the repository:

- **`CLOUDFLARE_API_TOKEN`** (required) — API token with Workers-scoped permissions, used by `wrangler deploy`.
- **`CLOUDFLARE_ACCOUNT_ID`** (required) — the Cloudflare account ID that owns the `zfb-docs` Worker.
- **`IFTTT_PROD_NOTIFY`** (optional) — IFTTT webhook key used to push a notification when a production deploy lands. Omit to skip notifications.

## External tool version pins

zfb shells out to a small set of third-party tools (esbuild for the islands bundler, wrangler/workerd for Cloudflare Workers preview, Tailwind v4 for the CSS engine). Every one of those tools is **exact-pinned** so that the same source tree produces byte-identical output regardless of when or where it is built — this matters for asset-hash stability and for keeping the SSR pipeline from drifting under our feet when upstream cuts a patch release.

The pin sources are:

| Tool        | Version source                                                                                  | Checksum / lock source                                                                                                      | Package-side pin                                      |
| ----------- | ----------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------- |
| esbuild     | `EXPECTED_ESBUILD_VERSION` in `crates/zfb-toolchain-pins/src/lib.rs`                            | `EXPECTED_ESBUILD_SHA256` 5-platform `cfg` block in `crates/zfb-islands/src/esbuild.rs`; matching `ESBUILD_SHA256_*` constants in `crates/zfb/build.rs` | No root `package.json` pin; `build.rs` downloads the matching `@esbuild/<platform>` tarball |
| tailwindcss | `TAILWIND_VERSION` in `crates/zfb/build.rs`; mirrored by `TAILWIND_VERSION` in `scripts/fetch-tailwind.mjs` | `TAILWIND_SHA256_*` constants in `crates/zfb/build.rs`                                                                       | No root `package.json` pin; `build.rs` downloads the standalone GitHub release asset |
| wrangler    | `EXPECTED_WRANGLER_VERSION` in `crates/zfb-toolchain-pins/src/lib.rs`                           | `pnpm-lock.yaml`                                                                                                            | Exact `wrangler` devDependency in root `package.json` |
| workerd     | `EXPECTED_WORKERD_VERSION` in `crates/zfb-toolchain-pins/src/lib.rs`                            | `pnpm-lock.yaml` transitive dependency from wrangler                                                                         | Transitive only                                       |

### Bumping wrangler / workerd

1. Pick the new versions you want (typically a coordinated wrangler + workerd set; see the [wrangler changelog](https://github.com/cloudflare/workers-sdk/releases?q=wrangler) for matched sets).
2. Edit the `wrangler` entry in `package.json` to the new exact version.
3. Edit `EXPECTED_WRANGLER_VERSION` and `EXPECTED_WORKERD_VERSION` in `crates/zfb-toolchain-pins/src/lib.rs` to match.
4. Run `pnpm install` to refresh `pnpm-lock.yaml`. Confirm the resolved `workerd` version in the lockfile matches the constant you just set.
5. Run `cargo test -p zfb` to make sure the version-gate tests still pass.
6. Commit `package.json`, `pnpm-lock.yaml`, and the constants change in one commit so the pin moves atomically.

Note: the exact pin above governs **this repo's own** reproducibility (CI, e2e, lockfile). The consumer-facing `zfb preview` gate treats `EXPECTED_WRANGLER_VERSION` as the **minimum supported version** (issue 2379): an older wrangler aborts with upgrade guidance, an equal one passes silently, and a newer one proceeds with an info line (or a warning on an untested major) — so a consumer's routine wrangler bump never breaks `pnpm preview`.

If you ever need to bypass the wrangler version gate (e.g. while a bump is mid-flight on a feature branch), set `ZFB_SKIP_WRANGLER_VERSION_CHECK=1` for the duration of the `zfb preview` invocation. Do not check this in.

### Bumping esbuild

esbuild is shipped as a Go-built standalone CLI binary. The binary is **not** committed to this repo; `crates/zfb/build.rs` downloads the platform-specific `@esbuild/*` tarball during Cargo builds, verifies the extracted binary, and stages it under `crates/zfb/binaries/esbuild/`. To bump:

1. Pick the new esbuild version (the latest stable 0.x at release-cut time; see <https://github.com/evanw/esbuild/releases>).
2. Edit `EXPECTED_ESBUILD_VERSION` in `crates/zfb-toolchain-pins/src/lib.rs`.
3. Compute the SHA-256 of the extracted binary in each supported `@esbuild/*` package (`linux-x64`, `linux-arm64`, `darwin-arm64`, `darwin-x64`, `win32-x64`) and update the corresponding entries in the `EXPECTED_ESBUILD_SHA256` 5-platform `cfg` block in `crates/zfb-islands/src/esbuild.rs`.
4. Update the matching `ESBUILD_SHA256_LINUX_X64`, `ESBUILD_SHA256_LINUX_ARM64`, `ESBUILD_SHA256_MACOS_ARM64`, `ESBUILD_SHA256_MACOS_X64`, and `ESBUILD_SHA256_WIN_X64` constants in `crates/zfb/build.rs` in the same commit.
5. Update the `## esbuild Version` table in `crates/zfb-islands/README.md`.
6. Run `cargo build --workspace --all-targets` so the host-platform binary is downloaded, SHA-256-verified, and staged by `build.rs`.
7. Run `cargo test -p zfb-islands` — the unit tests use the mock-subprocess code path and do not require the real binary.

The verification gate is implemented in `ensure_binary_verified` in `crates/zfb-islands/src/esbuild.rs` and runs once per binary path per process: it spawns `esbuild --version`, asserts the reported version equals `EXPECTED_ESBUILD_VERSION`, and (when populated) hashes the binary and asserts the SHA-256 equals `EXPECTED_ESBUILD_SHA256`. A mismatch on either gate aborts with a clear, actionable error pointing back at this section.

### Bumping Tailwind

Tailwind v4 is shipped as the upstream standalone CLI binary. The binary is **not** committed to this repo; `crates/zfb/build.rs` downloads the platform-specific GitHub release asset during Cargo builds, verifies it, and stages it as `crates/zfb/binaries/tailwindcss-v4` (or `.exe` on Windows). To bump:

1. Pick the new Tailwind v4 version from <https://github.com/tailwindlabs/tailwindcss/releases>.
2. Edit `TAILWIND_VERSION` in `crates/zfb/build.rs`.
3. Edit the mirrored `TAILWIND_VERSION` in `scripts/fetch-tailwind.mjs` so the optional Tailwind-only prefetch helper stays aligned.
4. Update the `TAILWIND_SHA256_LINUX_X64`, `TAILWIND_SHA256_LINUX_ARM64`, `TAILWIND_SHA256_MACOS_ARM64`, `TAILWIND_SHA256_MACOS_X64`, and `TAILWIND_SHA256_WIN_X64` constants in `crates/zfb/build.rs` from the release's `sha256sums.txt`.
5. Update any Tailwind version tables in `crates/zfb-css/README.md`.
6. Run `cargo build --workspace --all-targets` so the host-platform binary is downloaded, SHA-256-verified, and staged by `build.rs`.

## Supply chain

Runtime deps on publishable packages are supply-chain liabilities for downstream users. See [SECURITY-DEPS.md](./SECURITY-DEPS.md) for the full policy, the current runtime-dep audit, and the checklist to follow before adding a new runtime dependency.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](./LICENSE).
