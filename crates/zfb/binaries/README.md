# `crates/zfb/binaries/` — Bundled binary distribution slot

This directory reserves placement for third-party binaries that ship inside the
`zfb` release tarball. The binaries are invoked at runtime as subprocesses by
the `zfb` CLI (and by sibling crates that depend on `zfb`'s runtime layout).

## Why this lives here

The `zfb` crate is the user-facing CLI binary. When we package a release
tarball for distribution, this `binaries/` directory is included alongside the
`zfb` executable so that, at runtime, `zfb` can locate the bundled tools using
a path relative to its own executable. Keeping the slot inside `crates/zfb/`
(rather than at the workspace root) keeps the release-tarball layout colocated
with the crate that owns the runtime contract.

## Expected slots

| Path                              | Purpose                                                       | Owner crate     |
| --------------------------------- | ------------------------------------------------------------- | --------------- |
| `tailwindcss-v4`                  | Tailwind CSS v4 standalone CLI binary (subprocess invocation) | `zfb-css`       |
| `esbuild/esbuild`                 | esbuild standalone CLI binary (subprocess invocation)         | `zfb-islands`   |

See `crates/zfb-css/README.md` for the pinned Tailwind version and rationale.
See `crates/zfb-islands/README.md` for the pinned esbuild version and rationale,
and `crates/zfb/binaries/esbuild/README.md` for the slot-shape rationale
(the esbuild slot uses a subdirectory rather than a single file path).

## Embedded npm packages (no on-disk slot)

In addition to the binaries above, `crates/zfb/build.rs` snapshots three
groups of npm packages directly **into the compiled `zfb` binary** via
`include_dir!`. They have no `binaries/` slot — they ride inside the
executable bytes themselves and are extracted to a tempdir at `zfb build`
/ `zfb dev` time so esbuild can resolve framework imports without a
consumer-side `node_modules/`.

| Embedded package(s)                      | Sub  | Source                                                                  |
| ---------------------------------------- | ---- | ----------------------------------------------------------------------- |
| `@takazudo/zfb`, `@takazudo/zfb-runtime` | #198 | `packages/<name>/src/` (TypeScript source from this workspace)          |
| `preact`, `preact-render-to-string`      | #209 | `node_modules/.pnpm/<name>@<ver>*/node_modules/<name>/` (published)     |
| `hono`                                   | #209 | `node_modules/.pnpm/hono@<ver>/node_modules/hono/` (published)          |

The version pins for the framework runtimes (`preact`,
`preact-render-to-string`, `hono`) are constants near the top of
`crates/zfb/build.rs` (`PREACT_VERSION`, `PREACT_RTS_VERSION`,
`HONO_VERSION`); the source of truth is zfb's own `pnpm-lock.yaml`. See
the "Embedded npm packages" section in `BUILDING.md` for the bump
procedure.

## Why no binaries are committed

Binaries are large, platform-specific, and license-bound. They are **never**
committed to this repository. Instead:

- `.gitignore` excludes the actual binary file paths (e.g. `tailwindcss-v4`,
  `tailwindcss-v4.exe`).
- The slot directory is preserved in git via `.gitkeep`.
- **The Cargo build script (`crates/zfb/build.rs`) is the authoritative
  population path.** When you run `cargo build` or `cargo install --path
  crates/zfb`, the build script detects the current platform, downloads the
  pinned binary from the upstream release (esbuild from the npm registry,
  tailwindcss from GitHub releases), verifies its SHA-256 against pinned
  constants in `build.rs`, and stages the binary here atomically. Re-runs are
  no-ops when the on-disk binary already matches the pinned checksum.
- `scripts/fetch-tailwind.mjs` at the repo root is kept as a **developer
  convenience** (superseded by the build script) — it can still be run via
  `pnpm fetch:tailwind` in environments that already have Node.js available.
- The `ZFB_ESBUILD_BIN` and `ZFB_TAILWIND_BIN` env vars are the documented
  escape hatches for consumers and CI environments that supply their own
  binaries or have no network access. When either is set, the build script
  skips the corresponding download step entirely.

## Runtime contract (sketch)

At runtime, sibling crates (e.g. `zfb-css`) resolve the binary path roughly as:

```text
<dir-of-zfb-executable>/binaries/<binary-name>
```

Resolution helpers will live in the `zfb` crate when the release-engineering
epic lands. Until then, downstream crates should treat the path as
implementation-defined and access it only through a `zfb`-provided API.
