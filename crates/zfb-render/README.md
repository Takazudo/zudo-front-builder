# zfb-render

TSX → JS compile pipeline (SWC), JS runtime host, and page render orchestrator for zfb.

`zfb-render` stitches together three moving parts:

1. [`swc_pipeline`](src/swc_pipeline.rs) — SWC parse + transform (TypeScript strip + JSX automatic-runtime transform) into ES module JavaScript.
2. [`loader`](src/loader.rs) — module resolver that compiles on demand, caches results, and funnels `.mdx` / `mdx://` specifiers through `zfb-content`'s MDX→JSX emitter before the SWC pass.
3. [`embedded_v8`](src/embedded_v8/mod.rs) — in-process V8 host (`deno_core 0.399`) that loads and evaluates the compiled bundle, drives `dispatch_fetch`, and surfaces V8 stack traces for source-map re-projection.

The orchestrator ([`render`](src/render.rs)) is intentionally thin: it owns a `ModuleLoader` and a `RenderHost` and stitches them together. Smart bits (path resolution and `paths()` evaluation) live in their own modules.

## Public API

`lib.rs` re-exports the items callers need:

- **Core** — `RenderError`, `Result`
- **SWC pipeline** — `SwcPipeline`, `CompileOptions`, `CompiledModule`, `JsxRuntime`
- **Loader** — `read_to_string`, `ResolverError`
- **Render orchestrator** — `Renderer<H>`, `RenderRequest`
- **Runtime host trait** — `RenderHost`, `ModuleHandle`
- **Embedded V8 host** (`embed_v8` feature, default-on) — `EmbeddedV8RenderHost`, `BundleModuleLoader`, `AliasHook`, `VirtualModuleHook`, `PluginRegistryHooks`, `HttpRequestLike`, `HttpResponseLike`
- **Config evaluator** (`embed_v8` feature) — `ThreadedConfigEvaluator`, `ConfigEvalError`

Additional public modules: [`adapters`](src/adapters/) (Preact / React JSX runtime adapters), [`paths`](src/paths.rs) (`paths()` runtime resolver), [`paths_extract`](src/paths_extract.rs) (static literal extractor), [`sourcemap`](src/sourcemap.rs) (V8 frame → original TSX line re-projection).

```rust,ignore
use zfb_render::{
    EmbeddedV8RenderHost, JsxRuntime, RenderRequest, Renderer,
};

// Host is created once per build; first-call cost is V8 snapshot warmup.
let host = EmbeddedV8RenderHost::new()?;
let mut renderer = Renderer::new(host, JsxRuntime::Preact);

let req = RenderRequest::new("pages/index.tsx", tsx_source);
let html: String = renderer.render(&req).await?;
```

## Architecture

### SWC pipeline (`swc_pipeline`)

`SwcPipeline::compile` takes a TSX source string and emits ES module JavaScript. The pass order is:

1. `resolver` — scope analysis.
2. `react` (automatic runtime) — JSX desugaring; `import_source` is `"preact"` or `"react"` depending on `JsxRuntime`.
3. `strip` — TypeScript type annotation removal.
4. `hygiene` + `fixer` — hygiene and parenthesisation cleanup.

`CompileOptions` carries `filename` (used in source maps and error messages), `jsx_runtime`, and `development` (off by default for SSR).

### Module loader (`loader`)

`ModuleLoader` wraps `SwcPipeline` with a compile cache (keyed by specifier string), extension probing (`.tsx → .ts → .jsx → .js → index.<ext>`), and MDX routing:

- Specifiers ending in `.mdx` or starting with `mdx://` are run through `zfb_content::mdx_jsx_emit::mdx_to_jsx_module_with_pipeline` first (mdast-phase + hast-phase plugins fire) and the resulting JSX is then fed to SWC.
- Bare specifiers (`preact`, `react`, `zfb`) are treated as runtime-provided; the loader does not attempt to resolve them from disk.

`Renderer` exposes a family of constructors (`new`, `with_strip_md_ext`, `with_strip_md_ext_and_gfm`, …) that thread markdown configuration knobs through to the loader, keeping dev rendering and the bundler in agreement.

### `RenderHost` trait (`render_host`)

```rust,ignore
#[async_trait(?Send)]
pub trait RenderHost {
    async fn execute_module(&mut self, name: &str, source: &str) -> Result<ModuleHandle>;
    async fn call_default(&mut self, handle: &ModuleHandle, props: JsonValue) -> Result<String>;
    async fn get_export(&mut self, handle: &ModuleHandle, name: &str) -> Result<JsonValue>;
}
```

The trait is `?Send` by design: the embedded V8 isolate is pinned to the thread that creates it. Tests use lightweight in-process fakes; integration tests against the real host live in `tests/embedded_v8_*.rs`.

### Embedded V8 host (`embedded_v8`, `embed_v8` feature)

`EmbeddedV8RenderHost` owns a `deno_core::JsRuntime` and a tokio current-thread runtime. At boot it installs:

1. **Web Platform polyfills** (`js/web_polyfills.js`) — `Request`, `Response`, `Headers`, `URL`, `URLSearchParams`, `TextEncoder`, `TextDecoder`, `atob`, `btoa`, `structuredClone`, a minimal `crypto.subtle`. A pure-JS polyfill is used instead of `deno_fetch` / `deno_web` to avoid their heavyweight compile surface (hyper / rustls / h2 / tower) — the SSG path never makes outgoing network requests.
2. **Browser Event globals** (`js/browser_event.js`) — `Event`, `CustomEvent`, `EventTarget`. Required so bundles that `class X extends Event` at the top level evaluate at all on the SSG path.
3. **Host bridge shim** (`js/globals_shim.js`) — installs `globalThis.__zfb.dispatch(url, method, headers, body)` (the Rust-to-JS call entrypoint), `__zfb.setBundle` (bundle registration), and a levelled console capture (`__zfb.drainConsoleLogs()`).

Per-dispatch flow: `dispatch_fetch` builds a small JS expression that calls `__zfb.dispatch(...)`, which constructs a JS `Request`, awaits `default.fetch(req)`, and returns `{ status, headers, body }`. The Rust side deserialises the result via `serde_v8`.

`node:fs`, `node:fs/promises`, `node:path`, `node:url`, and `node:buffer` resolve to throwing-proxy stubs so Workers-targeted user code that imports Node namespaces for code paths that only fire in production continues to *load* under SSG; only actual invocation fails.

### `Cargo.toml` feature gate

| Feature | Default | Effect |
| --- | --- | --- |
| `embed_v8` | yes | Pulls in `deno_core 0.399` + `tokio`; enables `EmbeddedV8RenderHost` and `ThreadedConfigEvaluator`. |
| *(no feature)* | — | Keeps only the SWC pipeline + loader + trait. Useful for CI images that only need the compile step. |

`deno_core` is pinned to `=0.399.0` and acts as a compatibility anchor tied to the workspace's `rust-toolchain.toml`. Bumping is a coordinated change — update the pin in lock-step with the toolchain.

### Why in-process V8 instead of a Node subprocess

The original render path spawned an external Node.js process per page. The in-process host eliminates the process-spawn overhead and the IPC serialisation round trip, keeps the isolate warm across all routes in a build, and removes the Node.js runtime from the deployment dependency graph entirely. V8 snapshot warmup is a one-time cost at `EmbeddedV8RenderHost::new()`.

## Tests

```sh
cargo test -p zfb-render
```

- `src/swc_pipeline.rs` — unit tests for TS stripping, Preact JSX transform, React JSX transform.
- `src/loader.rs` — unit tests for bare-specifier detection, MDX specifier detection, compile cache, `stripMdExt` href rewriting.
- `tests/render_smoke.rs` — round-trip smoke test: compile a TSX page, render it, assert the HTML.
- `tests/embedded_v8_smoke.rs` — V8 host lifecycle: module load, `call_default`, `get_export`, isolate drop on panic.
- `tests/embedded_v8_browser_event.rs` — `Event` / `CustomEvent` / `EventTarget` globals are present.
- `tests/embedded_v8_console_logs.rs` — `drainConsoleLogs` captures worker console output.
- `tests/embedded_v8_node_stubs.rs` — `node:*` imports load but throw on invocation.
- `tests/embedded_v8_real_bundle_smoke.rs` — workerd-shape bundle dispatch (`dispatch_fetch`).
- `tests/embedded_v8_plugin_resolver.rs` — `PluginRegistryHooks` / `VirtualModuleHook`.
- `tests/config_eval.rs` — `ThreadedConfigEvaluator` (`zfb.config.ts` evaluation in a blocking thread).
- `tests/mdx_loader.rs` — MDX→JSX path through the loader.
- `tests/integration_routing_rendering.rs` — routing fixtures scanned via `zfb-router`, rendered end-to-end.
- `tests/error_messages.rs` — resolve / compile error message shapes.
