# zfb-config-loader

`zfb.config.ts` evaluator shared by `zfb` and `zfb-server`.

This crate exists because `zfb` depends on `zfb-server`, so the TypeScript
config evaluator cannot live in either crate without creating a cycle. The
loader is config-shape-agnostic: it returns the evaluated default export as
JSON and leaves deserializing into a concrete config struct to the caller.

## Evaluation model

- Default builds enable `embed_v8`. The config is bundled with the pinned
  esbuild binary and evaluated in-process with the embedded V8 host from
  `zfb-render`. No runtime Node process is required.
- Slim builds (`--no-default-features`) compile out the V8 evaluator and fall
  back to a `node` subprocess. This path requires `node` on `PATH`.
- esbuild uses `--platform=neutral`; `node:*` imports in `zfb.config.ts`
  become bundle-time errors instead of hidden runtime dependencies.

## Public API

- **`load_from_ts_file(ts_path, project_root, opts)`** - bundle, evaluate, and
  return a `LoadedTsConfig`.
- **`LoadedTsConfig`** - contains `config: serde_json::Value` plus
  `resolved_plugins`, one `file://` URL per `config.plugins[]` entry when
  plugin resolution is enabled.
- **`LoadOptions`** - optional overrides for esbuild, node, embedded-esbuild
  extraction, plugin resolution, and a test-only canned JSON path.
- **`EmbeddedEsbuildGetter`** - callback used by the `zfb` binary to expose
  its embedded esbuild snapshot as a resolver tier.
- **`resolve_plugin_path_to_file_url`** and the `node_resolve` exports -
  plugin/bare-specifier resolution helpers used by the loader and tests.
- **`output_bounded` / `output_bounded_with`** - subprocess helpers that bound
  output waits, kill stuck children, and retry `ETXTBSY` spawn races.

## Usage

```rust,ignore
use zfb_config_loader::{load_from_ts_file, LoadOptions};

let loaded = load_from_ts_file(
    project_root.join("zfb.config.ts").as_path(),
    project_root,
    &LoadOptions::default(),
)
.await?;

let value = loaded.config;
let plugin_urls = loaded.resolved_plugins;
```

Set `LoadOptions { resolve_plugins: false, ..Default::default() }` for embed
callers that only need scalar config fields and should not fail when CLI-only
plugin packages are absent from the deployed app.

## Tests

```sh
cargo test -p zfb-config-loader
```

The suite covers node-style bare specifier resolution, plugin URL conversion,
bounded subprocess behavior, and the loader envelope parsing paths.
