# `zfb-css`

CSS build pipeline for zfb. It compiles utility candidates with the in-process
[zudo-wind](../zudo-wind) engine, bundles authored CSS imports, processes CSS
Modules, and emits hashed stylesheet assets.

The top-level entry point is `CssPipeline`. It combines engine output with
scoped CSS Modules, computes a content hash, and returns the stylesheet and its
metadata to the build pipeline. Production builds use the emitter variant so
asset hashing and companion files are handled by the production asset pipeline.

## Architecture

### Utility CSS engines

[`src/engine.rs`](src/engine.rs) defines the `CssEngine` interface for the
utility CSS stage. `WindEngine` adapts zudo-wind's compiler to this interface;
it receives the extracted candidates and `WindConfig`, compiles in-process,
and returns diagnostics and provenance with the generated CSS. It does not
launch a separate CSS executable.

`AuthoredCssEngine` supplies authored CSS when project configuration sets
`wind: false`. Authored stylesheets and CSS Modules continue through the rest
of the pipeline when utility generation is disabled.

The `zfb-css` crate also resolves authored stylesheet imports, tracks their
companion assets, and processes `*.module.css` files through `lightningcss`.
The build and islands pipelines own source discovery and JavaScript rewriting.

### `CssEngine` interface

```rust
pub trait CssEngine {
    fn produce_utility_css(&self, sources: &[PathBuf]) -> Result<CssEngineOutput>;
}
```

This interface keeps utility generation separate from CSS Modules
compilation, stylesheet concatenation, hashing, and asset emission.

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

[`src/pipeline.rs`](src/pipeline.rs) defines `CssPipeline<E: CssEngine>`, the
top-level entry point. `CssPipeline::build` runs four stages in order:

1. **Engine stage:** calls `engine.produce_utility_css(sources)` to obtain
   authored CSS and generated utility CSS from the configured engine.
2. **CSS Modules stage:** compiles all `*.module.css` files via
   `CssModulesProcessor::process`.
3. **Concatenation:** appends module CSS to the engine output with a newline
   separator, preserving the deterministic input order.
4. **Hash + emit:** hashes the combined bytes with SHA-256, takes the first 8
   hex characters, and writes
   `{output_root}/assets/styles-{hash}.css` atomically (write to temp,
   then rename).

The `CssPipeline::build_emitter` variant skips the global asset disk write;
it is used by `ProductionAssetPipeline`, which owns asset hashing and the
final URL rewrite.

`link_href(base_url, asset_path)` derives the public URL the renderer injects
as `<link href="...">` without re-hashing.

### Engine composition

`WindEngine` calls zudo-wind's compiler with the configured candidate set and
configuration, then combines the compiler's ordered output parts with authored
CSS. `AuthoredCssEngine` returns authored CSS without compiling utilities.
Both use the same downstream modules, hashing, and emission stages.
