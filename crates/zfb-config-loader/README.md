# zfb-config-loader

`zfb.config.ts` evaluator shared by `zfb` and `zfb-server`.

This crate exists because `zfb` depends on `zfb-server`, so the TypeScript
config evaluator cannot live in either crate without creating a cycle. The
loader is config-shape-agnostic: it returns the evaluated default export as
JSON and leaves deserializing into a concrete config struct to the caller.

## Evaluation model

- The config (local imports and the `zfb/config` / `@takazudo/zfb/config`
  helper included) is bundled into one ESM file by the supervised native
  compiler worker (`zfb-compiler-worker`). The host supplies the worker
  through `LoadOptions::compiler_worker`: the zfb CLI passes
  `WorkerLauncher::zfb_cli()`, an embedder a launcher for its own executable
  whose `main` calls `zfb_compiler_worker::run_if_requested()` first. Without
  a launcher a `.ts` load fails with an actionable error.
- Default builds enable `direct-v8`. The config is bundled for the neutral
  platform and evaluated in-process with the direct V8 host, creating a fresh
  host per call. No runtime Node process is required. A `node:*` or bare Node
  builtin import is rejected before evaluation, so `zfb.config.ts` stays a
  self-contained data config.
- Slim builds (`--no-default-features`) compile out the V8 evaluator, bundle
  for the Node platform, and evaluate with a `node` subprocess, where builtins
  stay available. This path requires `node` on `PATH`.

## Public API

- **`load_from_ts_file(ts_path, project_root, opts)`** - bundle, evaluate, and
  return a `LoadedTsConfig`.
- **`LoadedTsConfig`** - contains `config: serde_json::Value` plus
  `resolved_plugins`, one `file://` URL per `config.plugins[]` entry when
  plugin resolution is enabled.
- **`LoadOptions`** - the compiler worker launcher, an optional node override,
  plugin resolution, and a test-only canned JSON path.
- **`WorkerLauncher`** - re-exported from `zfb-compiler-worker`.
- **`resolve_plugin_path_to_file_url`** and the `node_resolve` exports -
  plugin/bare-specifier resolution helpers used by the loader and tests.
- **`output_bounded` / `output_bounded_with`** - subprocess helpers that bound
  output waits, kill stuck children, and retry `ETXTBSY` spawn races.

## Usage

```rust,ignore
use zfb_config_loader::{load_from_ts_file, LoadOptions, WorkerLauncher};

let opts = LoadOptions {
    compiler_worker: Some(WorkerLauncher::zfb_cli()?),
    ..Default::default()
};
let loaded = load_from_ts_file(
    project_root.join("zfb.config.ts").as_path(),
    project_root,
    &opts,
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
bounded subprocess behavior, the loader envelope parsing paths, and (in
`tests/native_config.rs`) real config bundles through a test-only worker host,
`config-loader-test-worker`.
