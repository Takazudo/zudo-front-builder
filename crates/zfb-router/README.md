# zfb-router

Scans a `pages/` directory and builds a sorted, validated file-based route tree following Next.js/Astro conventions.

## Public API

### Scanning

```rust,ignore
use zfb_router::{Router, scan_pages, RouterError};

// High-level: Router bundles the pages dir and the discovered routes
let router = Router::scan("src/pages")?;
let pages_dir = router.pages_dir();   // &Path
let routes    = router.routes();      // &[Route]

// Low-level: returns the same Vec<Route> directly
let routes = scan_pages("src/pages")?;
```

`scan_pages` / `Router::scan` walk the directory with `walkdir`, apply the
accepted-extension filter, build `Route` values, sort by specificity, and
detect ambiguities — all in one call.

### `Route` and related types

```rust,ignore
use zfb_router::{Route, RouteKind, Segment};

// RouteKind
RouteKind::Static    // plain segment, e.g. "about"
RouteKind::Dynamic   // [param], e.g. "[slug]"
RouteKind::Catchall  // [...param] or [[...param]] (optional)

// Key Route methods
route.template()              // "/blog/:slug", "/docs/:slug{.+}"
route.output_filename(None)   // "blog/[slug]/index.html"
route.source_path             // PathBuf of the page file
route.segments                // Vec<Segment>
route.kind                    // RouteKind
route.specificity             // u32 — higher = matched first
route.static_html             // bool — whether to render to static HTML
```

`Route::output_filename` respects a three-level precedence: frontmatter
`extension` override → filename convention → `"html"` default.
Error-page filenames (`404.tsx`, `500.tsx`) are special-cased to produce
`404.html` / `500.html` rather than `404/index.html`.

### File conventions

| File | Route template | Notes |
|---|---|---|
| `pages/index.tsx` | `/` | |
| `pages/about.tsx` | `/about` | |
| `pages/blog/[slug].tsx` | `/blog/:slug` | dynamic |
| `pages/docs/[...slug].tsx` | `/docs/:slug{.+}` | catchall |
| `pages/docs/[[...slug]].tsx` | `/docs/:slug{.+}` | optional catchall |
| `pages/_helpers.tsx` | — | ignored (`_`-prefix) |

Accepted page extensions: `tsx`, `mdx`, `md`, `html`. Files and directories
whose names begin with `_` are skipped.

### Specificity scoring

Routes are sorted highest-to-lowest specificity so the most precise match
wins in the build and dev server. The scoring formula applied per segment:

| Segment type | Weight |
|---|---|
| Static | 100 |
| Dynamic | 10 |
| Catchall | 1 |

Index pages receive a +1 bonus on top of their segment sum.

### Ambiguity detection

`RouterError::AmbiguousRoute` — two routes produce byte-identical templates
(e.g. `blog/[slug].tsx` and `blog/[id].tsx`).

`RouterError::AmbiguousShape` — two routes overlap under param-name-insensitive
comparison (issue #816), catching conflicts that differ only in parameter name.

Both errors carry the conflicting source paths for precise diagnostics.

### Sort parity

The sort order mirrors `zfb_build::bundler::route_sort_key` so the route list
presented to the dev server is identical to the one used during production
builds. Divergence between dev and prod routing is prevented at the source.

## Tests

```sh
cargo test -p zfb-router
```
