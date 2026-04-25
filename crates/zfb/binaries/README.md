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

See `crates/zfb-css/README.md` for the pinned Tailwind version and rationale.

## Why no binaries are committed

Binaries are large, platform-specific, and license-bound. They are **never**
committed to this repository. Instead:

- `.gitignore` excludes the actual binary file paths (e.g. `tailwindcss-v4`).
- The slot directory is preserved in git via `.gitkeep`.
- Release engineering (a separate, future epic) is responsible for downloading
  the correct platform-specific binary, verifying its signature/checksum, and
  placing it into this directory before the release tarball is assembled.

This sub-task (Sub 4 of [issue #5](https://github.com/Takazudo/zudo-front-builder/issues/5))
only **reserves the slot** — it does not implement the download or
release-tarball assembly logic.

## Runtime contract (sketch)

At runtime, sibling crates (e.g. `zfb-css`) resolve the binary path roughly as:

```text
<dir-of-zfb-executable>/binaries/<binary-name>
```

Resolution helpers will live in the `zfb` crate when the release-engineering
epic lands. Until then, downstream crates should treat the path as
implementation-defined and access it only through a `zfb`-provided API.
