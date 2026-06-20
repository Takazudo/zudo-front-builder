# zfb-build

The dev-loop orchestrator for the zudo-front-builder framework.

`zfb-build` ties the four moving parts of the dev pipeline together:

1. [`zfb-watcher`](../zfb-watcher) for filesystem change events.
2. [`zfb-graph`](../zfb-graph) for "given this file changed, which pages
   need to be re-rendered?" queries.
3. The page renderer (Epic 3 — `zfb-render` / `zfb-content` /
   `zfb-router`).
4. The CSS pipeline (Epic 4 — `zfb-css`) and the islands bundler
   (Epic 5 — `zfb-islands`).

The orchestrator deliberately does **not** depend on those last three
crates. They're injected as callback functions on a `BuildContext` so:

- the orchestrator's surface stays free of heavyweight transitive
  dependencies (the SWC pipeline in `zfb-render`, and the
  esbuild npm subprocess wrappers in `zfb-css` /
  `zfb-islands`), and
- tests can plug in fakes that count invocations without spawning
  Tailwind / esbuild subprocesses.

Wiring concrete renderers / engines / bundlers happens in the bin crate
(Epic 7's `zfb dev` command).

## Public API

`lib.rs` re-exports items across several module groups:

- **Orchestrator** — `BuildOrchestrator<P>`, `OrchestratorConfig`
  (six fields: `project_root`, `watch_roots`, `extra_watch_paths`,
  `policy`, `debounce`, `external_invalidation`), `DiscoveryHook`, `DiscoveryOutcome`.
- **Pipeline (dev / prod)** — `AssetPipeline` trait, `DevAssetPipeline`,
  `ProductionAssetPipeline`, `apply_prod_asset_pipeline`,
  `BuildContext` (legacy transition shim — see below),
  `DevBuildContext`, `ProdBuildContext`, `BuildOutcome`, and the
  runner types `CssRunner`, `IslandsRunner`, `PageRenderer`,
  `RendererReloader`.
- **Bundler** — `bundle`, `BundleManifest`, `BundleMode`,
  `BundlerInput`, `BundlerOutput`, and related content-collection
  types.
- **Adapter** — `AdapterRunner`, `DefaultAdapterRunner`,
  `run_adapter_bundle`, `AdapterChoice`, and supporting types.
- **Plugin host** — `PluginHost`, `PluginSpec`, `BuildHookContext`,
  `DevRegisterContext`, and the plugin-registry types.
- **Renderer** — `start`, `render_all`, `render_one`, `reload`,
  `shutdown`, `Backend`, and supporting SSR types.
- **Atomic** — `atomic_write`, `atomic_write_string`,
  `validate_output_path`.

```rust,ignore
use std::sync::{Arc, Mutex};
use zfb_build::{
    AssetPipeline, BuildContext, BuildOrchestrator, DevAssetPipeline,
    OrchestratorConfig, RenderedPage,
};
use zfb_graph::DependencyGraph;

let graph = Arc::new(Mutex::new(DependencyGraph::new()));
let pipeline = DevAssetPipeline::new();
let orch = BuildOrchestrator::new(
    OrchestratorConfig::new("/path/to/proj", vec!["pages".into(), "content".into()]),
    graph,
    pipeline,
);

// `BuildContext` is a legacy transition shim still accepted by `run()`.
// New dev callers should construct `DevBuildContext` (which carries the
// same fields) and convert via `DevBuildContext::into_build_context()`.
// Production callers should use `ProdBuildContext` (dist_root +
// render_pages only) instead.
let ctx = BuildContext {
    dist_root: "/path/to/proj/dist".into(),
    render_pages: Arc::new(|pages| {
        // call into zfb-render here
        Ok(vec![/* RenderedPage { … } */])
    }),
    run_css: None,
    run_islands: None,
    reload_renderer: None,
};

orch.run(ctx, None, |outcome| {
    println!("rebuilt: {} page(s)", outcome.pages_rendered);
}).await?;
```

## The `AssetPipeline` trait

```rust,ignore
pub trait AssetPipeline: Send + Sync {
    fn apply(&self, plan: &RebuildPlan, ctx: &BuildContext) -> Result<BuildOutcome>;
}
```

The shape is intentionally minimal:

- **One method** — every implementation gets a fully-formed
  `RebuildPlan` (the orchestrator has already folded watcher events
  through the granularity policy and the dependency graph).
- **No async** — pipelines do CPU + IO work but don't need to await.
  The orchestrator's `run` loop is the async boundary; pipelines run
  synchronously inside one tick. This keeps tests and embedded
  use-cases simple.
- **`Send + Sync`** — the orchestrator may be wrapped in an `Arc` and
  shared with the dev preview server, so the pipeline must be safe to
  call from multiple threads.

Why a trait when there's only one impl today? Production / SSR / edge
builds will need different behaviour:

| Build      | Differences from dev                                      |
| ---------- | --------------------------------------------------------- |
| Production | Minify, fail-fast, hashed asset URLs in HTML.             |
| SSR        | Skip writing HTML to disk; emit into a runtime bundle.    |
| Edge       | Skip dist/ entirely; emit workerd-shaped artefacts in RAM. |

Locking the orchestrator to a concrete struct now would force a
refactor when production-build lands. Locking to a trait costs one
virtual call per rebuild tick and keeps the door open. The trait also
guards the contract: if a future PR adds a method to `AssetPipeline`,
the dev impl breaks here, not in production.

## Granularity policy

`zfb-build` resolves the open question from issue #7 ("per-file vs.
coarse rebuild granularity") with the following rules. They live in
[`policy.rs`](src/policy.rs); the orchestrator
([`orchestrator.rs`](src/orchestrator.rs)) folds them through the
dependency graph.

| Change                                         | Pages re-rendered                              | CSS rerun? | Islands rerun? |
| ---------------------------------------------- | ---------------------------------------------- | ---------- | -------------- |
| `zfb.config.ts` (or any `mark_global` path)    | All                                            | Yes        | Yes            |
| `pages/foo.tsx`                                | Just `foo` (graph self-edge)                   | No         | No             |
| `content/foo.md`                               | Pages that import the content collection entry | No         | No             |
| `styles/main.css`                              | Pages with a recorded edge to it (usually 0)   | Yes        | No             |
| `components/Header.tsx` (used by N pages)      | Those N pages (graph reverse-edges)            | No         | Yes (\*)       |
| `data/site.json`                               | Pages with a recorded edge to it               | No         | No             |
| `public/logo.svg`                              | None                                           | No         | No             |
| Anything else (e.g. `package.json`)            | Whatever the graph says, usually nothing       | No         | No             |

(\*) A change to a `.tsx`/`.ts`/`.jsx`/`.js` file under any of the
configured **islands roots** re-bundles islands. Default islands roots
are `components/` and `src/`; override with
`GranularityPolicy::with_islands_roots`.

We **do not** parse the file inside `zfb-build` to check for the
`"use client"` directive. The islands sub-pipeline (Epic 5) re-runs its
scanner on every islands-root change. Its scanner is fast, and the
bundler re-emits the same hashed asset bytes if the islands set turns
out to be unchanged — so the cost of a "false positive" rerun is one
scanner pass plus a no-op bundle, which is well under the noise floor
of a typical save-rebuild cycle.

### When in doubt, rebuild more

The policy errs on the side of "rebuild a bit too much" rather than
"miss a change". A misclassified path produces a slow-but-correct
build; an over-aggressively-narrowed path produces a stale page. We
always pick the former. Concrete consequences:

- The classifier falls back to file-extension heuristics if no known
  root matches.
- `Unclassified` paths still consult the graph (they may be tracked as
  explicit deps by a power-user resolver), so an explicit edge always
  wins over a missing pattern match.

## Atomic dist write

Every file the pipeline emits is written to `<final>.tmp-<pid>-<seq>` in
the same directory and then `rename`d into place. `rename` is atomic on
the same filesystem, so a reader (the dev preview server, an external
fs-notify consumer, …) never observes a half-written file.

The temp file lives next to the destination — never in `/tmp` — so the
`rename` stays on the same filesystem. Cross-FS `rename` would silently
degrade to a copy + delete, breaking the atomicity guarantee.

The default `DevAssetPipeline` also keeps a tiny in-memory byte cache
(`HashMap<dist_path, bytes>`) so unchanged HTML is *not* re-written.
This keeps the dev preview server's WebSocket reload signal accurate:
`BuildOutcome::pages_written` lists only the pages whose bytes
actually moved.

## Graph persistence between runs

Issue #7 listed warm-start graph caching as an open question.

**Status: implemented.** `zfb_graph::persist` serialises the
`DependencyGraph` to `<project>/.zfb/graph.bin` via bincode and reads
it back on the next `zfb dev` cold start. The wiring lives in
`crates/zfb/src/commands/dev.rs`.

**Validity rule — ManifestDigest.** Each saved snapshot carries a
`ManifestDigest`: a SHA-256 over (1) the full byte content of every
config-like file (e.g. `zfb.config.ts`) and (2) a sorted listing of
every watched source file as `(relative_path, mtime_nanos, len_bytes)`.
On startup, a fresh digest is computed from the current project layout
and compared against the on-disk digest byte-for-byte. Any mismatch —
a changed config, an added/removed/modified source file — discards the
cache and falls through to a full resolver scan. A wire-format version
header provides an additional guard against stale files from older
`zfb` installs.

## Tests

```sh
cargo test -p zfb-build
```

- `src/{atomic,plan,policy,orchestrator}.rs` and
  `src/pipeline/{mod,dev,prod,orchestrator}.rs` carry focused unit
  tests for each module (`pipeline` is now a directory).
- `tests/integration_dev_loop.rs` covers the four acceptance scenarios:
  - Touching a `.md` file triggers a single-page rebuild.
  - Editing a global CSS file triggers a CSS-only rebuild (no page
    re-render unless a page recorded an edge).
  - Editing a `"use client"` component re-bundles islands without a
    full re-render.
  - The real watcher → orchestrator → pipeline path also works
    end-to-end (one async test using `notify`).
