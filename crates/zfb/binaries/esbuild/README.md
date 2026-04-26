# `crates/zfb/binaries/esbuild/` — esbuild binary slot

This directory reserves placement for the **esbuild standalone CLI binary**
that the `zfb-islands` crate invokes as a subprocess (see
`crates/zfb-islands/README.md` for the version pin).

## Slot layout

The release-tarball layout looks like this:

```text
zfb/                      # release tarball root
├── zfb                   # the user-facing CLI executable
└── binaries/
    ├── tailwindcss-v4    # bundled Tailwind v4 CLI (zfb-css)
    └── esbuild/          # this directory
        └── esbuild       # bundled esbuild CLI (zfb-islands)
```

The `esbuild` binary file itself is **never** committed to this repo.
`.gitignore` excludes the actual binary path; this directory is preserved
in git via `.gitkeep`.

## Why a subdirectory rather than a single file slot

`crates/zfb/binaries/tailwindcss-v4` is a single-file slot because Tailwind
ships exactly one CLI binary per platform. esbuild also ships a single CLI
binary per platform — a subdirectory is used here so that platform-specific
release tooling (a future epic) has room to drop a sidecar (e.g. a checksum
manifest or a `LICENSE.md`) without polluting the parent `binaries/`
namespace. The runtime path the subprocess code resolves to is
`crates/zfb/binaries/esbuild` — that path is interpreted as the *binary
file itself*, not as a directory; see the override env var
`ZFB_ESBUILD_BIN`.

> If the subdirectory shape proves to be unnecessary at release-engineering
> time, this slot may be flattened back to a single file path. The
> subprocess code reads the path verbatim, so the call site is unaffected.

## Why this slot is reserved now

Sub 2 of [issue #6](https://github.com/Takazudo/zudo-front-builder/issues/6)
implements the subprocess wrapper. The wrapper needs a deterministic
default path for the binary so that user code can construct an
`EsbuildSubprocessConfig::default()` without environment fiddling. The
binary itself will be downloaded and verified by a future
release-engineering epic — this sub-task only **reserves the slot**.

## Runtime resolution

The `EsbuildSubprocessConfig::default()` constructor resolves the binary
path as follows:

1. If the `ZFB_ESBUILD_BIN` env var is set, use that path verbatim.
2. Otherwise, default to `crates/zfb/binaries/esbuild/esbuild` (i.e. the
   `esbuild` executable file inside this slot directory; relative to the
   current working directory at engine construction time).

A clear error message is returned if the binary is not present at the
resolved path.
