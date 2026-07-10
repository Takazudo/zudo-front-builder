# `crates/zfb/binaries/` — Workspace staging paths for embedded helper binaries

This directory records the workspace staging paths for third-party helper
binaries that `zfb` invokes as subprocesses. The binaries themselves are not
committed. During a `zfb` crate build, `crates/zfb/build.rs` downloads and
verifies the platform-specific files, stages them here for local reuse, then
copies them into `$OUT_DIR/vendor/bin/` so `include_dir!` embeds them into the
compiled `zfb` executable.

An installed `zfb` binary does not rely on an executable-adjacent `binaries/`
directory. At runtime, `zfb` extracts embedded helper binaries to a fresh
tempdir via `crates/zfb/src/render_pipeline.rs::embedded_binary`.

## Why this lives here

The `zfb` crate owns the Cargo build script and the `include_dir!` snapshot
that embeds helper binaries. Keeping the staging paths inside `crates/zfb/`
keeps the download, checksum, embedding, and runtime extraction contract
colocated with the user-facing CLI crate.

Sibling crates still expose workspace-relative fallback paths for direct
in-repo development and focused package tests. The production `zfb` command
path wires in the embedded extraction tier before those fallbacks are needed.

## Expected staged files

| Path                              | Purpose                                                       | Owner crate   |
| --------------------------------- | ------------------------------------------------------------- | ------------- |
| `tailwindcss-v4`                  | Tailwind CSS v4 standalone CLI binary (subprocess invocation) | `zfb-css`     |
| `esbuild/esbuild`                 | esbuild standalone CLI binary (subprocess invocation)         | `zfb-islands` |

See `crates/zfb-css/README.md` for the pinned Tailwind version and rationale.
See `crates/zfb-islands/README.md` for the pinned esbuild version and rationale,
and `crates/zfb/binaries/esbuild/README.md` for the esbuild subdirectory
rationale.

## Embedded npm packages

In addition to the binaries above, `crates/zfb/build.rs` snapshots three
groups of npm packages directly **into the compiled `zfb` binary** via
`include_dir!`. They are not staged under `crates/zfb/binaries/`; they ride
inside the executable bytes themselves and are extracted to a tempdir at
`zfb build` / `zfb dev` time so esbuild can resolve framework imports without
a consumer-side `node_modules/`.

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
- The staging directories are preserved in git via `.gitkeep`.
- **The Cargo build script (`crates/zfb/build.rs`) is the authoritative
  population path.** When you run `cargo build` or `cargo install --path
  crates/zfb`, the build script detects the current platform, downloads the
  pinned binary (esbuild from the npm registry, tailwindcss from GitHub
  releases), verifies its SHA-256 against pinned constants, stages the binary
  here atomically, then copies it to `$OUT_DIR/vendor/bin/` for embedding.
  Re-runs are no-ops when the on-disk binary already matches the pinned
  checksum.
- `scripts/fetch-tailwind.mjs` at the repo root is kept as a **developer
  convenience** (superseded by the build script) — it can still be run via
  `pnpm fetch:tailwind` in environments that already have Node.js available.
- The `ZFB_ESBUILD_BIN` and `ZFB_TAILWIND_BIN` env vars are the documented
  escape hatches for consumers and CI environments that supply their own
  binaries or have no network access. When either is set, the build script
  skips the corresponding download step entirely.

## Runtime resolution

Production callers should use the resolver wiring in the `zfb` crate. The
effective precedence is:

```text
explicit path on the config
-> ZFB_ESBUILD_BIN / ZFB_TAILWIND_BIN
-> embedded binary under $OUT_DIR/vendor/bin/ extracted to a tempdir
-> workspace staging path under crates/zfb/binaries/
```

The last tier exists for direct workspace development. It is not the installed
distribution model.
