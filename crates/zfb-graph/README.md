# zfb-graph

Page dependency graph for the zfb framework. Tracks which pages depend on
which sources (content collections, layouts, components, styles, data
modules) and answers: "given this file changed, which pages need to be
re-rendered?"

This crate is part of Epic 6 (build infrastructure) and is consumed by
`zfb-build`. It is a pure in-memory data structure with no IO.

## Public API

### Types

- `PageId` — newtype wrapping a `PathBuf`, identifying a page by its source
  `.tsx` path. Lines up with `zfb_router::Route::source_path` semantically
  without taking a hard dependency on the router crate.
- `DepKind` — informational tag for an edge: `Module`, `Content`, `Style`,
  `Data`, `Asset`, `Other`. The graph itself does not branch on kind — every
  recorded edge invalidates its owning page — but the build orchestrator can
  use the tag to decide which sub-pipeline (CSS, islands, etc.) to re-run.
- `PageDeps` — input record: `{ page, deps: Vec<(PathBuf, DepKind)> }`.
- `DependencyGraph` — the graph itself.
- `DirtySet` — return shape of `dirty_pages`. Either `All` (sentinel meaning
  "rebuild every page") or `Specific(BTreeSet<PageId>)`.

### Construction

```rust
let g = DependencyGraph::from_pages([
    PageDeps::new(PageId::new("/proj/pages/a.tsx"), vec![
        (PathBuf::from("/proj/layouts/main.tsx"), DepKind::Module),
    ]),
]);
```

Or build empty and `upsert` page-by-page as the resolver finishes each one.

### Methods

- `dirty_pages(&path) -> DirtySet` — minimal set of pages whose output is
  affected by changing `path`. Read-only. Returns `DirtySet::All` for files
  registered as global, an empty `DirtySet::Specific` for unknown paths.
- `add_node(path) -> bool` — register a brand-new file (e.g., a fresh
  markdown entry). Returns `true` if newly added. The graph cannot infer
  consumers; the caller should re-resolve affected pages and feed the new
  edges back via `upsert`.
- `remove_node(&path) -> BTreeSet<PageId>` — drop a file. Returns the set of
  former consumers; these pages need rebuilding. If `path` was a page itself
  the page is deleted from the graph.
- `upsert(record)` — install or replace the dep set for a single page.
  Idempotent and stale-edge-safe: removes prior reverse edges from this page
  before installing the new ones.
- `mark_global(path) / unmark_global(&path) / is_global(&path)` — manage the
  set of files whose change forces `DirtySet::All`.
- `pages() / page_count() / deps_of(&page) / consumers_of(&path) / snapshot()`
  — diagnostics + introspection helpers.

## Coarse-rebuild policy

Some files affect every page (`zfb.config.ts`, top-level `_app.tsx`, etc.).
Rather than enumerating per-page edges for these, register them via
`mark_global` and the next `dirty_pages` call returns `DirtySet::All` — a
sentinel that the orchestrator treats as "rebuild everything". This keeps
the graph small (no synthetic edges from every page to the config file) and
makes the policy explicit at registration time.

The graph does not auto-register any path as global. The caller decides.

## Self-edge

Every page is implicitly its own dependency: changing
`/proj/pages/index.tsx` dirties `/proj/pages/index.tsx`. `upsert` adds this
self-edge automatically so callers do not need to remember.

## Deletion semantics

A deleted file invalidates all of its former consumers — they imported a
thing that no longer exists. `remove_node` returns exactly that consumer
set so the orchestrator can rebuild those pages and surface the missing-
import error per-page.

## Tests

Unit tests in `src/lib.rs`. Integration test in `tests/dirty_pages.rs`
builds a small fixture graph and exercises edit / add / delete / global
flows.

```sh
cargo test -p zfb-graph
```
