# `zfb-islands`

Islands runtime pipeline for `zfb`. Scans for `"use client"` components,
bundles them via the esbuild CLI subprocess into a shared entry bundle
plus code-split chunks, and emits the hydration markup + runtime glue.

The crate is fully built out. See `src/lib.rs` rustdoc for the
architecture overview: the `ClientBundler` trait,
`EsbuildSubprocessBundler`, the scanner, hydration rewrite, manifest,
and code-splitting. The `NativeRustBundler` placeholder lives in
`src/future_rust_native.rs`.

## esbuild Version

This crate invokes the **esbuild standalone CLI** as a subprocess.

| Field                 | Value                                                        |
| --------------------- | ------------------------------------------------------------ |
| Pinned version        | **`0.25.12`**                                                |
| Major line            | esbuild 0.25.x                                               |
| Distribution          | Standalone CLI binary (Go-built; no Node.js required)        |
| Version source        | `EXPECTED_ESBUILD_VERSION` in `crates/zfb-toolchain-pins`    |
| Workspace fallback    | `crates/zfb/binaries/esbuild/esbuild` (or `.exe` on Windows) |
| Embedded runtime name | `bin/esbuild` inside the `include_dir!` vendor snapshot       |
| Upstream              | <https://github.com/evanw/esbuild/releases>                  |

> The pin **must be reviewed and refreshed before each `zfb` release**.
> Bump `0.25.12` to whatever the latest stable esbuild 0.x is at
> release-cut time. Update `EXPECTED_ESBUILD_VERSION` in
> `crates/zfb-toolchain-pins/src/lib.rs`, `EXPECTED_ESBUILD_SHA256` in
> `crates/zfb-islands/src/esbuild.rs`, and the esbuild SHA-256 table in
> `crates/zfb/build.rs` in the same commit.

### Why a pinned version

- **Reproducible builds.** A fixed esbuild version means the same source
  tree produces byte-identical bundles regardless of when or where it is
  built — the SHA-256 hash that drives `islands-{hash}.js` depends on this.
- **Embedded binary.** The compiled `zfb` executable embeds the exact binary
  `zfb-islands` expects and extracts it to a tempdir at runtime — no `npx`,
  no `node_modules`, no network calls while building a user site.
- **Subprocess contract.** The CLI flags this crate calls for the shared
  bundle path (`--bundle`, `--format=esm`, `--splitting=true`,
  `--tree-shaking=true`, `--entry-names=islands`,
  `--chunk-names=islands-chunk-[hash]`, `--outdir`) are the 0.x surface
  area; locking the version prevents flag drift from breaking the wrapper.
  `--minify` and `--sourcemap=linked` are applied per `BundleConfig`.

### Why a subprocess wrapper (and not a Rust-native bundler)

esbuild ships only as a Go-built CLI today. There is no production-ready
drop-in Rust-native bundler for our needs (swc-bundler is close but not
turnkey for our entry-point pattern). Until one exists (or until we build
one), invoking the official CLI as a subprocess is the lowest-risk way to
get correct, up-to-date bundling output.

To keep us free to swap implementations later, the subprocess call lives
behind an internal `ClientBundler` trait. The esbuild binary is one
implementation; a hypothetical Rust-native crate would be another, with no
changes required at call sites. See `src/future_rust_native.rs` for the
`NativeRustBundler` stub.

### Why the workspace fallback lives at `crates/zfb/binaries/esbuild/esbuild`

The `zfb` CLI owns both the Cargo build script that downloads helper binaries
and the `include_dir!` snapshot that embeds them. Placing the workspace
fallback under `crates/zfb/` keeps download, checksum, embedding, and runtime
extraction details colocated with the crate that owns the runtime contract.
See `crates/zfb/binaries/README.md` for the staging-path details.

The binary file itself is **not** committed to git — `.gitignore` excludes
it. `crates/zfb/build.rs` downloads and SHA-verifies the esbuild binary
from the npm registry at `cargo build` / `cargo install` time, embeds it via
`$OUT_DIR/vendor/bin/`, and leaves the workspace copy as the
direct-development fallback.
