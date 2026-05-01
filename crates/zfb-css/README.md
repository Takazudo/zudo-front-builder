# `zfb-css`

CSS pipeline for `zfb`. Wraps the Tailwind CSS subprocess and (later) merges
CSS Modules output into a single hashed global asset.

> This README currently documents only the **Tailwind v4 version pin** for Sub
> 4 of [issue #5](https://github.com/Takazudo/zudo-front-builder/issues/5).
> Crate-level documentation (architecture, `CssEngine` trait, public API) is
> being added by Sub 1 in parallel and will merge with this file.

## Tailwind CSS Version

This crate invokes the **Tailwind CSS v4 standalone CLI** as a subprocess.

| Field                 | Value                                                              |
| --------------------- | ------------------------------------------------------------------ |
| Pinned version        | **`4.2.0`**                                                        |
| Major line            | Tailwind CSS v4.x                                                  |
| Distribution          | Standalone CLI binary (no Node.js required at runtime)             |
| Expected binary path  | `crates/zfb/binaries/tailwindcss-v4` (in the release tarball)      |
| Upstream              | <https://github.com/tailwindlabs/tailwindcss/releases>             |

> The pin **must be reviewed and refreshed before each `zfb` release**. Bump
> `4.2.0` to whatever the latest stable Tailwind v4.x is at release-cut time,
> and re-run `pnpm fetch:tailwind` to materialize the matching binary.
> The version constant in `scripts/fetch-tailwind.mjs` must move in lockstep
> with the version line above — both are the source of truth until we add a
> shared workspace pin file.

## Getting the binary

The binary file is **not** committed to git. There are two supported ways to
provide it for builds and tests that exercise the Tailwind subprocess path.

### Option 1 (default): `pnpm fetch:tailwind`

From the workspace root:

```sh
pnpm fetch:tailwind
```

The script (`scripts/fetch-tailwind.mjs`) detects your platform/arch, downloads
the matching asset from the [pinned tailwindlabs/tailwindcss release](https://github.com/tailwindlabs/tailwindcss/releases),
verifies its SHA-256 against the release's `sha256sums.txt`, places it at
`crates/zfb/binaries/tailwindcss-v4` (or `tailwindcss-v4.exe` on Windows), and
makes it executable. Re-runs are a fast no-op when the on-disk binary already
matches the pinned checksum.

Supported platforms: `darwin-x64`, `darwin-arm64`, `linux-x64`, `linux-arm64`,
`win32-x64`. On musl-libc Linux distros (Alpine and friends), use the override
below to point at a manually-fetched musl asset.

### Option 2 (override): `ZFB_TAILWIND_BIN`

If you already have a tailwindcss v4 standalone binary on disk — for example
because your CI image bundles it, or because you maintain a system-wide copy —
set the env var to its absolute path and the engine will use it directly:

```sh
export ZFB_TAILWIND_BIN=/usr/local/bin/tailwindcss
cargo test -p zfb-css -- --ignored
```

When `ZFB_TAILWIND_BIN` is set, `pnpm fetch:tailwind` is a no-op (it trusts
the override). The engine's path resolution is implemented at
[`crates/zfb-css/src/engine.rs`](src/engine.rs).

### Running the heavyweight tests locally

Tests that need the real binary are gated with `#[ignore]` and run via:

```sh
ZFB_TAILWIND_BIN="$(pwd)/crates/zfb/binaries/tailwindcss-v4" \
  cargo test -p zfb-css --tests -- --ignored
```

The `ZFB_TAILWIND_BIN` export is needed because `cargo test -p <crate>` runs
with the package directory as CWD; the engine's default relative path
(`crates/zfb/binaries/tailwindcss-v4`) is resolved from the workspace root.

### Why a pinned version

- **Reproducible builds.** A fixed Tailwind version means the same source tree
  produces byte-identical CSS regardless of when or where it is built.
- **Bundled binary.** The release tarball ships the exact binary `zfb-css`
  expects — no `npx`, no `node_modules`, no network calls at user build time.
- **Subprocess contract.** The CLI flags this crate calls (`--input`,
  `--output`, `--watch`, content-scan globs) are the v4 surface area; locking
  the version prevents flag drift from breaking the wrapper.

### Why a subprocess wrapper (and not a Rust-native engine)

Tailwind v4 ships only as a JavaScript/Node-built CLI today. There is no
production-ready Rust-native Tailwind engine. Until one exists (or until we
build one), invoking the official CLI as a subprocess is the lowest-risk way
to get correct, up-to-date Tailwind output.

To keep us free to swap implementations later, the subprocess call lives
behind an internal `CssEngine` trait. The `tailwindcss-v4` binary is one
implementation; a hypothetical Rust-native crate would be another, with no
changes required at call sites.

### Why the binary lives at `crates/zfb/binaries/tailwindcss-v4`

The `zfb` CLI is the single user-facing executable; bundled tooling needs to
sit in a path the `zfb` runtime can locate relative to its own executable.
Placing the binary inside `crates/zfb/` keeps the release-tarball layout
colocated with the crate that owns the runtime contract. See
`crates/zfb/binaries/README.md` for the slot-level details.

The binary file itself is **not** committed to git — `.gitignore` excludes it,
and release engineering (a future, separate epic) is responsible for
populating the slot before tarball assembly.
