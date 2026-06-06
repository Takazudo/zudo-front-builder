# zfb

Frontend build orchestrator — the `zfb` CLI binary and its supporting library crate.

## Subcommands

### `zfb new`

Scaffold a new project from a template.

```sh
zfb new my-site
zfb new my-site --template basic-blog
```

The `basic-blog` template (the only template in v0) is baked into the binary at
compile time from `crates/zfb/templates/basic-blog/`.

### `zfb dev`

Run the local development server.

```sh
zfb dev
zfb dev --port 8080
zfb dev --host 0.0.0.0
```

`--port` and `--host` layer over config: CLI > `zfb.config.json` > built-in
default (`localhost:3000`). Neither has a clap default value so the command
body can distinguish "user passed a value" from "user accepted the default".

### `zfb build`

Build the project for production.

```sh
zfb build
zfb build --outdir dist
```

Default `--outdir` is `dist`.

### `zfb preview`

Preview a previously built project.

```sh
zfb preview
zfb preview --port 4321 --outdir dist
```

Default port falls back to `zfb.config.json` then to `4321`. Default `--outdir`
is `dist`. Pass `--host 0.0.0.0` to expose the preview to other LAN devices.

### `zfb check`

Typecheck the project and validate content collection schemas.

```sh
zfb check
zfb check --skip-tsc
```

Two failure modes:

1. TypeScript errors — `tsc --noEmit` subprocess. Any error tsc would flag in
   normal CI is flagged here.
2. Content collection schema violations — every entry's frontmatter is validated
   against the JSON Schema in `zfb.config.json`'s `collections[].schema` (when
   present).

`--skip-tsc` skips the tsc subprocess; schema validation still runs. Useful in
CI lanes that have no TypeScript dependency installed or for schema-only checks.

## Architecture

```
main.rs
  Cli::parse()                    ← clap (crate::cli)
  match Command variant
    ├── commands::new::run()
    ├── commands::dev::run()
    ├── commands::build::run()
    ├── commands::preview::run()
    └── commands::check::run()
  Err → report_error() → stderr + exit(1)
```

Each `commands/<name>.rs` module exposes one `async fn run(args: &FooArgs) -> anyhow::Result<()>`. Errors propagate with `?` and are rendered once at the top level by `report_error`, avoiding double-printing.

## Configuration

Config is loaded at the start of every command via `Config::load_from_dir()`.
Resolution order:

1. `zfb.config.ts` — bundled by pinned esbuild, evaluated in-process by V8 (default build) or by a node subprocess (slim build).
2. `zfb.config.json` — parsed directly by serde.
3. `Config::default()` — built-in defaults (see below).

### Key `Config` fields and defaults

| Field | Default | Notes |
|---|---|---|
| `out_dir` | `"dist"` | Production output directory |
| `public_dir` | `"public"` | Static assets directory |
| `host` | — | Dev/preview host; CLI flag takes precedence |
| `port` | — | Dev/preview port; CLI flag takes precedence |
| `framework` | `Preact` | `Preact` or `React` |
| `collections` | `[]` | Content collection definitions |
| `output` | `Auto` | `Static`, `Hybrid`, or `Auto` |
| `base` | — | Base path prefix for all URLs |
| `strip_md_ext` | — | Strip `.md`/`.mdx` from generated links |
| `trailing_slash` | — | Add/remove trailing slashes |
| `emit_routes_manifest` | — | Write a JSON routes manifest to dist |
| `plugin_hook_timeout_secs` | — | Max time per plugin hook |

`OutputMode::Auto` selects `Static` or `Hybrid` based on what the project uses.

### `embed_v8` feature flag

The default build enables the `embed_v8` cargo feature, which compiles in
`deno_core` / V8 for in-process config evaluation and SSR. Without this
feature the binary is lighter ("slim build") but falls back to a node
subprocess for config loading and fails loudly if any code path requires SSR.
The two code paths gated behind the feature are `ssr_adapter` and
`v8_host_adapter`.

## Public library surface

The library crate (`lib.rs`) exposes the following items to adapter authors
and integration tests:

```rust,ignore
use zfb::{
    // CLI parsing
    cli::{Cli, Command, NewArgs, DevArgs, BuildArgs, PreviewArgs, CheckArgs},
    // Command dispatch
    commands,
    // Configuration
    config::Config,
    // Dynamic-route planning (for adapter authors)
    DeferredDynamicRoute, PendingDynamicRoute,
    // Top-level error renderer
    report_error,
};
```

## Tests

```sh
cargo test -p zfb
```

Integration tests live in `crates/zfb/tests/` and cover build lifecycle
(`build_cleans_outdir`, `build_terminates`), the `check` command, content
snapshot behaviour (`content_snapshot_no_deferred`), CSS module components,
framework package resolution, and version stamping.
