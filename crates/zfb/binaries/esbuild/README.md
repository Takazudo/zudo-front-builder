# `crates/zfb/binaries/esbuild/` — esbuild workspace staging directory

This directory is the workspace staging location for the **esbuild standalone
CLI binary** that `zfb-islands` invokes as a subprocess (see
`crates/zfb-islands/README.md` for the version pin). The production `zfb`
binary embeds the staged esbuild file from `$OUT_DIR/vendor/bin/` via
`include_dir!` and extracts it to a tempdir at runtime.

## Staging layout

The workspace staging layout is:

```text
crates/zfb/binaries/
├── tailwindcss-v4        # staged Tailwind v4 CLI (zfb-css)
└── esbuild/              # this directory
    └── esbuild           # staged esbuild CLI (zfb-islands)
```

The `esbuild` binary file itself is **never** committed to this repo.
`.gitignore` excludes the actual binary path; this directory is preserved
in git via `.gitkeep`.

## Why a subdirectory rather than a single file

`crates/zfb/binaries/tailwindcss-v4` is a single-file staging path because
Tailwind ships exactly one CLI binary per platform. esbuild also ships a
single CLI binary per platform, but this directory keeps the workspace path
unambiguous: the staged executable is `crates/zfb/binaries/esbuild/esbuild`
(or `esbuild.exe` on Windows), while `crates/zfb/binaries/esbuild/` is only
the parent directory.

Set `ZFB_ESBUILD_BIN` or call `EsbuildSubprocessConfig::with_binary_path` to
use a different executable path.

## Build and embedding flow

`crates/zfb/build.rs` downloads the pinned esbuild package for the current
platform, extracts the CLI binary, verifies its SHA-256, and stages it at this
workspace path. The same build script then copies the executable to
`$OUT_DIR/vendor/bin/esbuild` so `include_dir!` embeds it in the compiled
`zfb` executable. Runtime callers in the `zfb` crate prefer that embedded
copy and keep the extraction tempdir alive while the subprocess runs.

## Runtime resolution

The `zfb` command path resolves esbuild as follows:

1. An explicit `esbuild_binary` / `EsbuildSubprocessConfig::with_binary_path`
   value wins.
2. If `ZFB_ESBUILD_BIN` is set, use that path verbatim.
3. Otherwise, extract the embedded `bin/esbuild` file to a tempdir and use
   that path.
4. If the embedded tier is unavailable in a direct workspace flow, fall back
   to `crates/zfb/binaries/esbuild/esbuild` relative to the current working
   directory at config construction time.

A clear error message is returned if the binary is not present at the
resolved path.
