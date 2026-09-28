# `crates/zfb/binaries/`

Workspace staging path for the standalone esbuild executable used by
`zfb-islands`. The binary is not committed. During a `zfb` crate build,
`crates/zfb/build.rs` downloads and verifies the host-platform executable,
stages it here for local reuse, and copies it into `$OUT_DIR/vendor/bin/` so
`include_dir!` embeds it in the compiled `zfb` binary.

An installed `zfb` binary does not need an adjacent `binaries/` directory. It
extracts the embedded esbuild file to a temporary directory when it runs the
bundler. Direct workspace development can use the staged path as a fallback.

## Expected staged file

| Path | Purpose | Owner |
| --- | --- | --- |
| `esbuild/esbuild` | esbuild standalone executable | `zfb-islands` |

See [`esbuild/README.md`](./esbuild/README.md) for the staging and resolution
details.

## Embedded npm packages

The build script also snapshots runtime packages into the compiled binary;
they are package trees, not helper binaries:

| Package | Source |
| --- | --- |
| `@takazudo/zfb`, `@takazudo/zfb-runtime` | This workspace's `packages/<name>/src/` tree |
| `hono` | The published tree in `node_modules/.pnpm/hono@<version>/node_modules/hono/` |

`pnpm-lock.yaml` pins Hono, and `HONO_VERSION` in `crates/zfb/build.rs` is
checked against that installed tree. Run `pnpm install --frozen-lockfile`
before building so the snapshot can be assembled.

The owned JSX runtime is vendored from the first-party package, and zudo-wind
is compiled into zfb as Rust code. Neither requires a separately staged
framework runtime or CSS CLI.
