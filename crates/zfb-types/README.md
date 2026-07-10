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

- **Client-script filename helpers** — the canonical `*.client.{ts,tsx,js,jsx}` contract shared by the router and client-script bundler:
  - `CLIENT_SCRIPT_INFIX` is `".client."`.
  - `CLIENT_SCRIPT_EXTENSIONS` is `["ts", "tsx", "js", "jsx"]`.
  - `is_client_script_file(path)` checks the convention without touching the filesystem.
  - `client_script_entry_name(path)` returns the entry name, e.g. `search-widget` for `search-widget.client.ts`.

- **Shared helper functions** — small string/path helpers used by generated-code and path-normalisation call sites:
  - `json_string(s)` emits a safe JSON string literal, including escaping control characters and JS line/paragraph separators.
  - `escape_html(s)` escapes HTML text/attribute special characters.
  - `path_to_posix_string(path)` converts Windows separators to `/` for tools that require POSIX-style paths.
  - `normalize_path_lexical(path)` collapses `.` / `..` components without touching the filesystem.

- **Asset URL constants and helpers** — stable public URLs and filenames for the production asset graph, shared between the renderer (head injection), emitters (asset registration), and client-script pipeline:

  | Constant | Value |
  | --- | --- |
  | `STABLE_CSS_URL` | `"/assets/styles.css"` |
  | `STABLE_ISLANDS_URL` | `"/assets/islands.js"` |
  | `STABLE_ASSETS_URL_PREFIX` | `"/assets/"` |
  | `STABLE_CLIENT_SCRIPTS_URL_PREFIX` | `"/assets/client/"` |
  | `DIST_ASSETS_DIR` | `"assets"` |
  | `DIST_CLIENT_SCRIPTS_DIR` | `"client"` |
  | `STABLE_CSS_FILENAME` | `"styles.css"` |
  | `STABLE_ISLANDS_FILENAME` | `"islands.js"` |

  `stable_client_script_url(name)`, `stable_client_script_filename(name)`, and `stable_client_script_relative_path(name)` build the stable pre-hash URL / filename / `dist_root`-relative path for a named client-script entry.

  Production builds let `ProductionAssetPipeline` rewrite the stable URLs to hashed forms (`/assets/styles-<hash>.css`, etc.). Dev builds keep the stable URLs as-is. The constants live here rather than in `zfb-build` because the renderer cannot depend on `zfb-build` (that would be a cycle), and `zfb-build` already depends on `zfb-render`.

## Tests

```sh
cargo test -p zfb-types
```

- `src/asset_urls.rs` — stable URL / filename pairing stays in sync; both URLs share the assets prefix; `DIST_ASSETS_DIR` matches `STABLE_ASSETS_URL_PREFIX`.
- `src/base_prefix.rs` — `dev_mount_prefix` covers `None`, empty string, root slash, multiple trailing slashes, path with/without trailing slash, absolute URL, missing leading slash.
- `src/client_scripts.rs` — the `.client.*` predicate and entry-name derivation accept all supported extensions and reject bare or non-client files.
- `src/helpers.rs` — JSON/HTML escaping, platform-gated POSIX path conversion, and lexical path normalization.
