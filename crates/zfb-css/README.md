# `zfb-css`

CSS pipeline for `zfb`. Wraps the Tailwind CSS subprocess and (later) merges
CSS Modules output into a single hashed global asset.

The crate is fully built out. See `src/lib.rs` rustdoc for the
architecture overview: `CssPipeline` (top-level entry point), the
`CssEngine` trait, `TailwindSubprocessEngine`, CSS Modules processing
via `lightningcss` (`modules` + `scanner`), the authored CSS engine,
the production emitter, and the native-engine placeholder.

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
> `4.2.0` to whatever the latest stable Tailwind v4.x is at release-cut time.
> Update the SHA-256 constants in `crates/zfb/build.rs` and the version
> constant in `scripts/fetch-tailwind.mjs` in the same commit — both must
> stay in lockstep with the version line above.

## Getting the binary

The binary file is **not** committed to git. There are two supported ways to
provide it for builds and tests that exercise the Tailwind subprocess path.

### Option 1 (default): `cargo build` / `cargo install`

`crates/zfb/build.rs` is the authoritative binary-population path.
When you run `cargo build` (or `cargo install zfb`), the build script
detects your platform, downloads the pinned `tailwindcss` v4 standalone
binary from the [tailwindlabs GitHub release](https://github.com/tailwindlabs/tailwindcss/releases),
verifies its SHA-256, and stages it at `crates/zfb/binaries/tailwindcss-v4`
(or `tailwindcss-v4.exe` on Windows). Re-runs are a fast no-op when the
on-disk binary already matches the pinned checksum.

The script `scripts/fetch-tailwind.mjs` (invoked via `pnpm fetch:tailwind`)
performs the same download for developer convenience — useful when you
want the binary available before running a full `cargo build`.

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

The binary file itself is **not** committed to git — `.gitignore` excludes it.
`crates/zfb/build.rs` downloads and SHA-verifies it at `cargo build` /
`cargo install` time; `pnpm fetch:tailwind` provides the same binary for
local development without a full build.

---

## Architecture

### Engine layer: `CssEngine` trait

[`src/engine.rs`](src/engine.rs) defines the `CssEngine` trait:

```rust
pub trait CssEngine {
    fn produce_utility_css(&self, sources: &[PathBuf]) -> Result<String>;
}
```

This is the single swap point between the current subprocess wrapper and any
future Rust-native implementation. Everything downstream — CSS Modules
compilation, concatenation, hashing, asset emission — is engine-agnostic.

Three implementations ship in this crate:

| Type | Module | Purpose |
| ---- | ------ | ------- |
| `TailwindSubprocessEngine` | [`src/engine.rs`](src/engine.rs) | Default: shells out to the `tailwindcss` v4 CLI binary |
| `AuthoredCssEngine` | [`src/authored_engine.rs`](src/authored_engine.rs) | `tailwind.enabled = false` path: returns authored CSS verbatim |
| `NativeRustEngine` | [`src/native_engine.rs`](src/native_engine.rs) | Placeholder: every method returns a "not yet implemented" error |

### `TailwindSubprocessEngine` and the synthesised entry CSS

`TailwindSubprocessEngine` does not pass the user's `styles/global.css`
directly to the Tailwind binary. Instead it **synthesises** a wrapper entry
CSS file and passes that to `tailwindcss -i <tmp>`. The exact ordering of
sections in that synthesised file is a contract:

1. `@import "tailwindcss";` — emitted only when the user's `input_css` does
   not already contain an active (non-commented-out) Tailwind import. If the
   user file already has `@import "tailwindcss";` the synthesiser suppresses
   the duplicate (Tailwind v4 errors on a doubled import). CSS block comments
   and line comments are stripped before this check.
2. `@source "<glob>";` directives for every entry in
   `TailwindSubprocessConfig::content_globs` (the user project). User-project
   globs are emitted **before** framework globs so per-project overrides win
   in cascade order.
3. `@source "<glob>";` directives for every entry in
   `TailwindSubprocessConfig::framework_package_globs` (e.g. shared
   `packages/zudo-doc-v2/**` that must survive Tailwind's tree-shake).
4. The contents of `input_css`, when provided (the user's authored
   `styles/global.css`).
5. The inline `theme_block`, when provided.

`build_synthesised_entry_css` (public API) builds this string and is also
stashed on `TailwindSubprocessEngine::last_entry_css` so tests can inspect
it without spawning the binary.

The entry temp file is written to `working_dir` (not a subdirectory) so that
any relative `@import "./x.css";` inside the user's input CSS resolves
against the correct sibling files. Stale entry temp files left by an
abnormally-terminated run are swept on the next invocation (see issue #821).

### `AuthoredCssEngine` and the `tailwind.enabled = false` path

When a project opts out of Tailwind entirely, the `zfb` build command
supplies an `AuthoredCssEngine` instead of `TailwindSubprocessEngine`.
`AuthoredCssEngine` implements `CssEngine::produce_utility_css` by returning
a pre-supplied authored CSS string verbatim — no subprocess, no synthesised
entry, no Tailwind import:

```rust
let engine = AuthoredCssEngine::new("body { margin: 0; }");
// or:
let engine = AuthoredCssEngine::default(); // returns empty string
```

The rest of the pipeline — CSS Modules compilation, concatenation, hashing,
asset emission via `CssPipeline` — is unchanged. The authored-CSS path is
handled in the `zfb` crate's build command; `zfb-css` only provides the
engine and the shared `is_tailwind_import_line` predicate (also used by the
import stripper in the `tailwind.enabled = false` build path).

### CSS Modules: `[hash]_[local]` scoping and the class-map contract

[`src/modules.rs`](src/modules.rs) compiles each `*.module.css` file via
`lightningcss`'s CSS Modules support. The default class-name pattern is
`[hash]_[local]`, where `[hash]` is derived from the **project-relative**
file path (not the absolute path — see issue #825). Using the relative path
ensures identical scoped class names across machines and checkout paths, while
still keeping modules in different directories distinct even when they share a
basename.

When `CssModulesConfig::project_root` is set to the project root,
`src/card.module.css` always hashes to the same `[hash]` prefix regardless of
where the project is checked out. When `project_root` is
`None` or the module path is outside the root, the absolute path is used as a
fallback (stable within a build, but not across relocations).

The compiled CSS output for all modules is concatenated in input order,
separated by blank lines, and returned alongside a class-name map.

#### `.classes.json` disk contract

When `CssPipelineConfig::class_map_dir` is set to `Some(dir)`, the pipeline
writes one JSON file per processed `.module.css` into `dir`:

```
{dir}/<sha8>__<basename>.classes.json
```

where `<sha8>` is the first 8 hex characters of the SHA-256 of the
project-relative module path (the same normalised string fed to lightningcss
for the `[hash]` prefix), and `<basename>` is the module file's filename
(e.g. `card.module.css`). The double underscore separates hash from name so
files are unambiguous even when two modules share a basename.

Each file is a flat, alphabetically sorted JSON object mapping original class
name to scoped class name:

```json
{ "btn": "abc12345_btn", "btn-primary": "abc12345_btn-primary" }
```

The `zfb-bundler` esbuild plugin intercepts `import styles from
"./foo.module.css"` and replaces it with a virtual ESM module that re-exports
this map as the default export. Using a static map (not a live `Proxy`)
allows tree-shaking, minification, and identical SSR rendering.

When `class_map_dir` is `None`, no JSON is written to disk; the in-memory
class maps are still returned via `CssPipelineOutput::class_maps`.

### Global stylesheet ordering contract

[`src/pipeline.rs`](src/pipeline.rs) defines `CssPipeline<E: CssEngine>`,
the top-level entry point. `CssPipeline::build` runs four stages in order:

1. **Engine stage:** calls `engine.produce_utility_css(sources)` to obtain
   the Tailwind (or authored) CSS string.
2. **CSS Modules stage:** compiles all `*.module.css` files via
   `CssModulesProcessor::process`.
3. **Concatenation:** combines the two halves as
   `tailwind_output + "\n" + modules_css`. The separator ensures that a class
   appended to the Tailwind half is distinguishable from one prepended to the
   modules half when hashing.
4. **Hash + emit:** hashes the combined bytes with SHA-256, takes the first 8
   hex characters, and writes
   `{output_root}/assets/styles-{hash}.css` atomically (write to temp,
   then rename).

The `CssPipeline::build_emitter` variant skips the global asset disk write;
it is used by `ProductionAssetPipeline`, which owns asset hashing and the
final URL rewrite.

`link_href(base_url, asset_path)` derives the public URL the renderer injects
as `<link href="...">` without re-hashing.

### Engine swap story

`NativeRustEngine` exists as a build-time guard: if `CssEngine` gains a new
method without both engines implementing it, the build fails here rather than
silently at a call site. The swap to a future Rust-native implementation
requires only a one-line change at the `CssPipeline` constructor; no changes
are needed in the CSS Modules, hashing, or emission code.
