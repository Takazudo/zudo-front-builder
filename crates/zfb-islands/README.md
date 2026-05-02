# `zfb-islands`

Islands runtime pipeline for `zfb`. Scans for `"use client"` components,
bundles them via the esbuild CLI subprocess into a single hashed JS asset,
and emits the hydration markup + runtime glue.

> This README currently documents the **esbuild version pin** for Sub 2 of
> [issue #6](https://github.com/Takazudo/zudo-front-builder/issues/6).
> Crate-level documentation (architecture, `ClientBundler` trait, public
> API) lives in `src/lib.rs` rustdoc and will be expanded by Subs 1 and 3.

## esbuild Version

This crate invokes the **esbuild standalone CLI** as a subprocess.

| Field                | Value                                                          |
| -------------------- | -------------------------------------------------------------- |
| Pinned version       | **`0.25.12`**                                                  |
| Major line           | esbuild 0.25.x                                                 |
| Distribution         | Standalone CLI binary (Go-built; no Node.js required)          |
| Expected binary path | `crates/zfb/binaries/esbuild` (in the release tarball)         |
| Upstream             | <https://github.com/evanw/esbuild/releases>                    |

> The pin **must be reviewed and refreshed before each `zfb` release**.
> Bump `0.25.12` to whatever the latest stable esbuild 0.x is at
> release-cut time, and re-run the release-engineering binary-fetch step
> (future epic) to materialize the matching binary.

### Why a pinned version

- **Reproducible builds.** A fixed esbuild version means the same source
  tree produces byte-identical bundles regardless of when or where it is
  built — the SHA-256 hash that drives `islands-{hash}.js` depends on this.
- **Bundled binary.** The release tarball ships the exact binary
  `zfb-islands` expects — no `npx`, no `node_modules`, no network calls at
  user build time.
- **Subprocess contract.** The CLI flags this crate calls (`--bundle`,
  `--format=esm`, `--splitting=false`, `--minify`, `--tree-shaking=true`,
  `--sourcemap=linked`, `--outfile`) are the 0.x surface area; locking the
  version prevents flag drift from breaking the wrapper.

### Why a subprocess wrapper (and not a Rust-native bundler)

esbuild ships only as a Go-built CLI today. There is no production-ready
drop-in Rust-native bundler for our needs (swc-bundler is close but not
turnkey for our entry-point pattern). Until one exists (or until we build
one), invoking the official CLI as a subprocess is the lowest-risk way to
get correct, up-to-date bundling output.

To keep us free to swap implementations later, the subprocess call lives
behind an internal `ClientBundler` trait. The esbuild binary is one
implementation; a hypothetical Rust-native crate would be another, with no
changes required at call sites. See `src/native_bundler.rs` for the stub.

### Why the binary lives at `crates/zfb/binaries/esbuild`

The `zfb` CLI is the single user-facing executable; bundled tooling needs
to sit in a path the `zfb` runtime can locate relative to its own
executable. Placing the binary inside `crates/zfb/` keeps the
release-tarball layout colocated with the crate that owns the runtime
contract. See `crates/zfb/binaries/README.md` for the slot-level details
(this slot mirrors the Tailwind v4 slot reserved by Epic 4 / Sub 4).

The binary file itself is **not** committed to git — `.gitignore` excludes
it, and release engineering (a future, separate epic) is responsible for
populating the slot before tarball assembly.
