# zfb-types

Canonical shared data types used across multiple zfb crates.

This crate is a low-dependency leaf that holds types shared by several workspace crates, preventing code duplication and circular dependencies. Every crate that needs these types can already depend on it without risk of cycles.

## Public API

- **`Segment`** — a single segment of a route template, parsed from a path component. Four variants:
  - `Static(String)` — literal segment, must match exactly.
  - `Dynamic(String)` — single-segment dynamic param from `[slug].tsx`.
  - `Catchall(String)` — rest param from `[...slug].tsx`; matches one or more trailing segments.
  - `OptionalCatchall(String)` — rest param from `[[...slug]].tsx`; matches zero or more trailing segments (the zero-case serves the bare directory URL).

  `Segment::template()` renders the canonical `:name` / `:name{.+}` Hono path-pattern form so the route key produced by `Route::template()` and the key registered by the bundler's `bracket_to_hono` are bit-identical — enabling the worker's `pagesByRoute` map to be looked up without a separate translation step.

- **`dev_mount_prefix(base: Option<&str>) -> Option<String>`** — normalises a `base` config value into the URL-path mount prefix for the dev server. Returns `None` for empty, root-only (`"/"`), or absolute-URL bases (CDN origins); returns `Some("/foo")` — leading slash, no trailing slash — for sub-path bases. Shared so `zfb-server` (mounting) and the build pipeline (href rewriting) always agree on the canonical form.

- **Asset URL constants** — stable public URLs and filenames for the production asset graph, shared between the renderer (head injection) and the islands / CSS emitters (asset registration):

  | Constant | Value |
  | --- | --- |
  | `STABLE_CSS_URL` | `"/assets/styles.css"` |
  | `STABLE_ISLANDS_URL` | `"/assets/islands.js"` |
  | `STABLE_ASSETS_URL_PREFIX` | `"/assets/"` |
  | `DIST_ASSETS_DIR` | `"assets"` |
  | `STABLE_CSS_FILENAME` | `"styles.css"` |
  | `STABLE_ISLANDS_FILENAME` | `"islands.js"` |

  Production builds let `ProductionAssetPipeline` rewrite the stable URLs to hashed forms (`/assets/styles-<hash>.css`, etc.). Dev builds keep the stable URLs as-is. The constants live here rather than in `zfb-build` because the renderer cannot depend on `zfb-build` (that would be a cycle), and `zfb-build` already depends on `zfb-render`.

## Tests

```sh
cargo test -p zfb-types
```

- `src/asset_urls.rs` — stable URL / filename pairing stays in sync; both URLs share the assets prefix; `DIST_ASSETS_DIR` matches `STABLE_ASSETS_URL_PREFIX`.
- `src/base_prefix.rs` — `dev_mount_prefix` covers `None`, empty string, root slash, multiple trailing slashes, path with/without trailing slash, absolute URL, missing leading slash.
