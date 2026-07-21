# zfb

Frontend build orchestrator - the `zfb` CLI binary and its supporting
library crate.

## Subcommands

### `zfb new`

Scaffold a new project from a template.

```sh
zfb new my-site
zfb new my-site --template basic-blog
zfb new my-site --template node-free
```

v0 ships two templates, both baked into the binary at compile time from
`crates/zfb/templates/<name>/`:

| Template | Role |
| --- | --- |
| `basic-blog` | Default starter with a `package.json` and normal Node/pnpm workflow |
| `node-free` | Starter with no `package.json` / no install step, for environments with no Node or pnpm on `PATH` |

### `zfb dev`

Run the local development server.

```sh
zfb dev
zfb dev --port 8080
zfb dev --host 0.0.0.0
zfb dev --host
```

`--port` and `--host` layer over config: CLI > `zfb.config.*` > built-in
default (`localhost:3000`). Both fields stay optional in clap so the command
body can distinguish "user passed a value" from "use the next fallback".
Bare `--host` is a Vite-style shortcut for `0.0.0.0`.

Dev-render environment switches:

| Env var | Effect |
| --- | --- |
| `ZFB_DEV_EAGER=1` | Disable lazy dev rendering and render affected routes eagerly on every file change |
| `ZFB_LAZY_DEV_RENDER=0|1` | Precise lazy/eager override; wins over `ZFB_DEV_EAGER` |
| `ZFB_DEV_BOOT_LAZY=1` | Serve a valid prebuilt `dist/` immediately and render each route on first request; falls back to the eager boot render when no servable `dist/` exists |
| `ZFB_DEV_BOOT_LAZY=cold` | Seedless variant of the above: render each route on first request without requiring a prebuilt `dist/` at all — every route serves the dev 404 page (with livereload) until its own first request |
| `ZFB_DEV_DEFER_BUNDLE=0` | Opt out of boot-lazy bundle deferral (either variant); build the renderer before bind |

### `zfb build`

Build the project for production.

```sh
zfb build
zfb build --outdir dist
zfb build --minify-html
zfb build --no-minify-html
```

Output-directory precedence is CLI `--outdir` > `outDir` in `zfb.config.*` >
the built-in `dist` default. `--minify-html` / `--no-minify-html` are an
explicit tri-state (`BuildMinifyHtml`) layered over `minifyHtml` config.

### `zfb preview`

Preview a previously built project.

```sh
zfb preview
zfb preview --port 4321 --outdir dist
zfb preview --host
```

Port and host fall back to `zfb.config.*`, then to `4321` and `localhost`.
Output-directory precedence matches build: CLI `--outdir` > config `outDir` >
`dist`. Bare `--host` is the same LAN shortcut as `zfb dev --host`: `0.0.0.0`.

### `zfb check`

Typecheck the project and validate content collection schemas.

```sh
zfb check
zfb check --skip-tsc
```

Two failure modes:

1. TypeScript errors - `tsc --noEmit` subprocess. Any error tsc would flag in
   normal CI is flagged here.
2. Content collection schema violations - every entry's frontmatter is
   validated against the JSON Schema in `collections[].schema` when present.

`--skip-tsc` skips the tsc subprocess; schema validation still runs.

## Architecture

```text
main.rs
  Cli::parse()                    <- clap (crate::cli)
  match Command variant
    |-- commands::new::run()
    |-- commands::dev::run()
    |-- commands::build::run()
    |-- commands::preview::run()
    `-- commands::check::run()
  Err -> report_error() -> stderr + exit(1)
```

Each `commands/<name>.rs` module exposes one `async fn run(args: &FooArgs) ->
anyhow::Result<()>`. Errors propagate with `?` and are rendered once at the
top level by `report_error`, avoiding double-printing.

## Configuration

Config is loaded at the start of every command via `Config::load_from_dir()`.
Resolution order:

1. `zfb.config.ts` - bundled by pinned esbuild and evaluated by
   `zfb-config-loader` (embedded V8 in default builds, node subprocess in
   slim builds).
2. `zfb.config.json` - parsed directly by serde.
3. `Config::default()` - built-in defaults.

TypeScript wins over JSON when both files are present.

### Key `Config` fields and defaults

| Rust field | Config key | Default | Notes |
| --- | --- | --- | --- |
| `out_dir` | `outDir` | `"dist"` | Build/preview output and dev's prebuilt seed; build/preview CLI flag takes precedence |
| `public_dir` | `publicDir` | `"public"` | Static assets directory |
| `host` | `host` | `None` | Dev/preview bind host; CLI flag takes precedence |
| `port` | `port` | `None` | Dev/preview port; CLI flag takes precedence |
| `allowed_hosts` | `allowedHosts` | `[]` | DNS-rebinding allowlist for non-loopback binds |
| `framework` | `framework` | `Preact` | `Preact` or `React` |
| `collections` | `collections` | `[]` | Content collection definitions |
| `tailwind` | `tailwind` | enabled | `enabled: false` opts out of Tailwind |
| `prefetch` | `prefetch` | `None` | `disabled: true` disables runtime prefetch wiring |
| `minify_html` | `minifyHtml` | `false` | Can be overridden per build by CLI flags |
| `bundle` | `bundle` | `None` | `exclude` globs for files kept out of the esbuild graph |
| `plugins` | `plugins` | `[]` | User plugin entries, with resolved module URLs on the TS path |
| `adapter` | `adapter` | `None` | Deploy-target adapter package; `None` / `"none"` means static |
| `strip_md_ext` | `stripMdExt` | `false` | Rewrite authored `.md` / `.mdx` links to route URLs |
| `code_highlight` | `codeHighlight` | `None` | Syntect theme / theme-directory options; `mode: "class"` emits re-themeable 18-role classes (`hi-*` → `--zfb-hi-*`) instead of inline colours (`classPrefix`, `roleClasses`, `defaultStylesheet`) |
| `resolve_markdown_links` | `resolveMarkdownLinks` | `None` | Markdown link resolver and broken-link policy |
| `base` | `base` | `None` | URL prefix for emitted asset/page URLs |
| `trailing_slash` | `trailingSlash` | `false` | Append `/` to extensionless rewritten hrefs |
| `markdown` | `markdown` | `None` | GFM, TOC, external-link, CJK, and feature plugin options |
| `site` | `site` | `None` | Absolute canonical site origin emitted to runtime globals |
| `emit_routes_manifest` | `emitRoutesManifest` | `None` (emit) | `false` skips `<outDir>/__zfb/routes.json` |
| `extra_watch_paths` | `extraWatchPaths` | `[]` | Absolute external directories watched by `zfb dev` |
| `output` | `output` | `Auto` | `Static`, `Hybrid`, or detection-driven `Auto` |
| `plugin_hook_timeout_secs` | `pluginHookTimeoutSecs` | `None` | Overrides env/built-in plugin hook timeout |
| `copy_public_with_base` | `copyPublicWithBase` | `true` | Copy `public/` under the base path in production output |
| `presets` | `presets` | `[]` | Partial config presets merged before validation |

`OutputMode::Auto` selects the V8/SSR topology from detected
`prerender = false` routes, while `Static` and `Hybrid` are explicit
precondition choices.

### `embed_v8` feature flag

The default build enables `embed_v8`, which propagates to `zfb-render`,
`zfb-build`, `zfb-server`, and `zfb-config-loader`. It compiles in
`deno_core` / V8 for in-process TS-config evaluation and SSR dispatch.

`cargo build --no-default-features -p zfb` produces the slim build. In that
mode, TS config evaluation falls back to a `node` subprocess and SSR-required
paths fail loudly. The V8-bearing adapter modules gated in this crate are
`lazy_render_adapter`, `ssr_adapter`, and `v8_host_adapter`.

## Public library surface

The library crate (`lib.rs`) exposes the CLI parser, command modules,
configuration types, diagnostics, render-planning helpers, and the top-level
error renderer:

```rust,ignore
use zfb::{
    bounded_join,
    cli::{
        BuildArgs, BuildMinifyHtml, CheckArgs, Cli, Command, DevArgs, NewArgs,
        PreviewArgs,
    },
    commands,
    config::{
        BundleConfig, CodeHighlightConfig, CollectionDef, Config, Framework,
        JsonSchema, MarkdownConfig, OutputMode, PluginConfig, PrefetchConfig,
        ResolveMarkdownLinksConfig, TailwindConfig,
    },
    diagnostics,
    render_pipeline,
    DeferredDynamicRoute, PendingDynamicRoute,
    report_error,
};
```

`render_pipeline` also defines the dynamic-route expansion and SSR dispatch
planning types used by adapter-facing code.

## Tests

```sh
cargo test -p zfb
```

Integration tests live in `crates/zfb/tests/` and cover build lifecycle,
`check`, content snapshots, CSS module components, framework package
resolution, dev-server behavior, and version stamping.
