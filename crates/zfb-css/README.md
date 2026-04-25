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
> and re-run the release-engineering binary-fetch step (future epic) to
> materialize the matching binary.

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
