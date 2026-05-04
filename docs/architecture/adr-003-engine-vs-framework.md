# ADR-003: Engine vs framework boundary

- **Status:** Accepted
- **Date:** 2026-04-27
- **Owners:** Sub 50 (Engine Primitives — engine-vs-framework boundary)
- **Related:** ADR-001 (JS runtime selection — superseded by ADR-005;
  the `deno_core` implementation it gated has been replaced with the
  embedded V8 host — see ADR-005 and ADR-007), ADR-002 (framework adapter contract
  for Preact / React), ADR-004 (content bridge contract — the Rust↔JS
  surface for `getCollection` / `getEntry`), Epic #42 (Engine
  Primitives)

## Context

zfb has so far been described as "a Rust-native static site generator
for TSX". That phrasing is accurate but it hides a load-bearing
distinction: zfb is the **engine** (router, renderer, content pipeline,
metadata plumbing) — and a *framework* like a future zudo-doc-v2 is what
authors actually point users at. The framework owns sidebar generation,
search index building, blog routing conventions, theme controls,
versioning UI, i18n routing, layout components — all the opinionated
shape that turns "an engine that emits HTML" into "a documentation
site you scaffold in one command."

That split matters now because the Engine Primitives epic
(Subs 45–50) is closing the v0 → v1 boundary. After Sub 50 ships, third
parties can build a framework on top of zfb without forking zfb itself.
For that to be a real promise, the engine has to commit to a small,
named set of primitives — and stay disciplined about **what stays out**
of zfb proper.

This ADR fixes the boundary. It enumerates the six primitives the
engine commits to, lists the framework concerns that are explicitly
**not** zfb's job, and pins the head / asset-injection contract that
frameworks rely on for `<head>` rendering.

## Decision (one sentence)

**zfb ships six engine primitives — frontmatter, content collections,
build-time content query bridge, `paths()`, MDX directive registry, and
non-HTML page emission — plus a `PageMeta` head/asset contract.
Everything else (sidebars, search, theming, i18n routing, versioning,
blog conventions) is framework territory and stays outside the engine.**

## Engine primitive contract

These six primitives are the v1 engine surface. Each is a stable
contract that frameworks can build on without reaching past it.

### 1. Frontmatter (unified)

`.md`, `.mdx`, and `.tsx` sources all declare frontmatter through the
same on-disk shape, normalized to one `serde_json::Value` on the Rust
side via `zfb-content::extract_frontmatter`. YAML for markdown; a
statically-extractable `export const frontmatter = {...}` literal for
TSX. The literal-only restriction on TSX is intentional — it keeps the
extractor AST-only (no module evaluation). See
[Frontmatter](/concepts/frontmatter) for the full contract.

### 2. Content collections

Directories of `.md` / `.mdx` files declared in `zfb.config` are
walked, parsed, and indexed at build time by `zfb-content::collection`.
Each entry exposes `slug`, `data` (parsed frontmatter), and `Content`
(a compiled JSX module) — addressed by a stable
`mdx://<collection>/<slug>` specifier. See
[Content Collections](/concepts/content-collections).

### 3. Build-time content query bridge

A deterministic Rust → JS snapshot
(`zfb-content::build_snapshot` / `ContentSnapshot`) is built before any
TSX module runs and embedded on `globalThis.__zfb.content`. Pages call
`getCollection` / `getEntry` from `zfb/content` and get plain in-memory
data, synchronously. The snapshot is sorted by
`(collection_name, slug)` and is byte-stable across runs. The full
contract — Rust types, JS-side shape, d.ts surface — is fixed by
[ADR-004](/architecture/adr-004-content-bridge).

### 4. `paths()` — dynamic and catchall route enumeration

Dynamic (`pages/blog/[slug].tsx`) and catchall
(`pages/docs/[...slug].tsx`) pages export a synchronous `paths()`
function returning `Array<{ params, props }>`. The router consumes
`params` to produce concrete URLs and threads `props` to the page
component as the `props` prop. See [Dynamic Routes](/concepts/dynamic-routes).
The static-route side lives in [Routing](/concepts/routing).

### 5. MDX directive registry

Frameworks register their own MDX directives — container
(`:::callout`), leaf (`::youtube`), text (`:badge`) — and map them to
JSX components without writing Rust. The registry lives in
`zfb-content::plugins::directives::DirectiveRegistry`; the pipeline
exposes it through `Pipeline::with_defaults()`. Unknown directives emit
a `Vec<DirectiveDiagnostic>` sink rather than failing the build. JSX
attribute support is **string-literal-only** in v1 — raw-expression
attributes are rejected. See
[Custom Directives](/concepts/custom-directives).

### 6. Non-HTML page emission

`pages/sitemap.xml.tsx`, `pages/llms.txt.tsx`, `pages/feed.rss.tsx` —
non-HTML page outputs are first-class. The output extension is picked
by the precedence rule **frontmatter `extension` > filename convention
> `html` default**, jointly enforced by `zfb-content::tsx_frontmatter`
(frontmatter side) and `zfb-router::route::Route` (filename side).
Per-page `Content-Type` metadata is threaded the same way; the dev
server consults it when setting response headers. Stale outputs are
cleaned up across builds when `extension` changes. See
[Non-HTML Pages](/concepts/non-html-pages).

## Framework concerns kept OUT of zfb

These are explicitly not engine primitives. zfb provides the
substrate; framework code (in user space, or in a downstream package
like a future zudo-doc-v2) wires the substrate into a finished site.

- **Sidebar / navigation generation.** Walking content collections to
  build a sidebar tree, ordering by `sidebar_position`, grouping by
  category, generating breadcrumbs.
- **Search index.** Pagefind / Lunr / custom indexer integration.
  Framework decision; engine just emits HTML and metadata.
- **Blog conventions.** Tag pages, archive pages, RSS feed, post
  pagination. All buildable on top of `getCollection` + `paths()` +
  non-HTML page emission, but the engine doesn't ship the convention.
- **i18n routing.** `/ja/...` mirroring `/...`, locale-aware
  `getCollection`, locale negotiation. Frameworks compose this from
  collections + dynamic routes.
- **Theme controls.** Light / dark / dim toggles, color tokens, a
  design-token CSS layer. Framework owns the theme system; the engine
  just runs the CSS pipeline (Tailwind v4 / PostCSS) it's pointed at.
- **Versioning UI.** Multi-version docs, version dropdown, "current"
  vs "stable" routing. Built on top of collections + routing in user
  space.
- **Layout components.** Header, footer, sidebar, table of contents,
  card grids, content shell. Frameworks publish these as ordinary
  components; the engine never imposes a default.
- **Site-level chrome.** Skip links, focus management, keyboard
  shortcuts, scroll-to-top. Framework concern.
- **Auth, server actions, RSC, streaming HTML.** Out of scope
  permanently in v1 — see [ADR-002](/architecture/adr-002-framework-adapters)
  for the React-side decision to stay on `renderToString` only.

The principle: if the answer to *"would two frameworks built on zfb
do this differently?"* is yes, it belongs in the framework.

## Head / asset-injection contract (mandatory)

This is the load-bearing piece that lets frameworks render the `<head>`
without reaching into the engine.

The carrier is `crates/zfb-render/src/meta.rs::PageMeta`. Its current
fields (Sub 49):

```rust
pub struct PageMeta {
    pub title: Option<String>,
    pub description: Option<String>,
    pub layout: Option<String>,
    /// All other fields preserved as raw JSON for the layout to
    /// consume freely.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}
```

The contract:

- `title`, `description`, `layout` are first-class typed fields.
- **Everything else** the page declares in `export const meta = {...}`
  flows through `extra` as `serde_json::Map<String, Value>`. Frameworks
  use `extra` to stash any structured head data they want — `openGraph`,
  `twitter`, canonical URL, alternate links, robots directives, custom
  meta tags — without engine changes.
- Layouts (rendered by the framework, not the engine) read `meta` (the
  whole `PageMeta`) and decide how to project it into `<head>`. The
  engine wraps the page output with the resolved layout and stops
  there.

**No Rust changes are required in this epic to support this pattern**
— the contract is the deliverable. `PageMeta` already flattens unknown
keys; `ResolvedMeta` already threads them to the layout call site. The
ADR pins this as a stable promise frameworks can rely on.

In scope for the contract (frameworks may use `extra` for these
freely):

- Canonical URL (`canonical`).
- `<link rel="alternate">` for i18n locale switching.
- Open Graph tags (`openGraph: { title, description, image, url, type }`).
- Twitter card tags (`twitter: { card, site, creator, image }`).
- `robots`, `themeColor`, viewport overrides, custom JSON-LD.

How a framework consumes it: the page's `meta` export is parsed by
`parse_meta` and resolved to a layout module path by `resolve_meta`.
The layout component receives the typed `PageMeta` (typically as
`props.meta`) and renders the `<head>` from there:

```tsx
// framework-owned layout, not part of zfb
export default function DefaultLayout({ meta, children }) {
  const og = meta.openGraph ?? {};
  return (
    <html>
      <head>
        <title>{meta.title}</title>
        {meta.description && <meta name="description" content={meta.description} />}
        {og.image && <meta property="og:image" content={og.image} />}
        {/* …everything else the framework wants to project from meta.extra */}
      </head>
      <body>{children}</body>
    </html>
  );
}
```

The engine never inspects the contents of `extra`. That's the whole
point — extending the head surface is a framework / page concern, not
an engine release.

## Consequences

### Positive

- **Stable v1 surface.** Six primitives, named and documented. A
  framework author has one list to read.
- **Framework competition is possible.** Anyone can ship a zudo-doc-v2,
  a docs-only minimal kit, an i18n-first preset, a blog kit — without
  forking zfb. The engine doesn't pick winners.
- **Engine evolution stays bounded.** Adding capabilities post-v1
  requires either (a) extending one of the six primitives in a way
  that's clearly engine-shaped, or (b) a new ADR. Sidebar generation
  cannot drift into the engine just because someone asks.
- **Head surface is open-ended without API churn.** `PageMeta.extra`
  absorbs new head concerns as JSON. No engine release needed to add
  a new OG variant.

### Negative

- **Framework gap on day one.** zfb v1 ships an engine; the docs site
  for zfb itself uses the existing zudo-doc Astro setup, not a
  zfb-native framework. Users who want a finished site experience
  before zudo-doc-v2 lands have to assemble layouts and conventions
  themselves.
- **The boundary is a documentation concern, not a compiler one.**
  Nothing prevents a framework from leaking engine-shaped logic, or
  the engine from drifting toward framework concerns. This ADR plus
  the sidebar of "what's in" / "what's out" is the enforcement
  mechanism.
- **Two surfaces to learn.** Engine primitives vs framework
  conventions. We accept the cognitive cost in exchange for the
  competition / extensibility benefit.

## Alternatives considered

### One monolithic "zfb" that includes a default framework

Bundle a default theme, a default sidebar generator, a default search,
a default blog scaffold. Rejected because it forces an opinionated
shape on every user and makes "swap framework" mean "fork zfb." The
engine stays narrow precisely so the framework layer can be plural.

### No separation — let users compose primitives ad hoc forever

Don't promise a framework layer at all; expose primitives and let each
project assemble its own conventions every time. Rejected because the
common case (someone wants a docs site) is real and large; without a
named framework destination users repeatedly reinvent the same wheel.
A documented split says "yes a framework is coming, here's the
contract it builds on" without bloating the engine.

### Hide the boundary

Treat zfb as "just a static site generator" and ignore the
engine/framework distinction in docs. Rejected because the distinction
shows up the moment a non-trivial user asks "where do I put sidebar
generation?" — and "in your code, not zfb" needs to be a documented
position, not a runtime surprise.

### Couple the head contract to a concrete schema (e.g. typed `openGraph`)

Type `PageMeta.openGraph`, `PageMeta.twitter`, etc., as concrete Rust
structs. Rejected because the head surface evolves faster than the
engine should release. `serde_json::Map` via `extra` is the right
shape for an open-ended pass-through; frameworks can build their own
typed wrappers in TypeScript without engine churn.

## References

- `crates/zfb-render/src/meta.rs` — `PageMeta`, `ResolvedMeta`,
  `parse_meta`, `resolve_meta`.
- `crates/zfb-content/src/frontmatter.rs` — unified frontmatter
  extractor.
- `crates/zfb-content/src/tsx_frontmatter.rs` — TSX
  `export const frontmatter` static extractor.
- `crates/zfb-content/src/content_bridge.rs` — `ContentSnapshot`,
  `build_snapshot`.
- `crates/zfb-content/src/plugins/directives.rs` —
  `DirectiveRegistry`, `DirectiveDiagnostic`.
- `crates/zfb-content/src/pipeline.rs` — `Pipeline::with_defaults`.
- `crates/zfb-router/src/route.rs` — `Route::output_filename`,
  filename-convention extension parsing.
- ADR-001 — `/architecture/adr-001-js-runtime`
- ADR-002 — `/architecture/adr-002-framework-adapters`
- ADR-004 — `/architecture/adr-004-content-bridge`
- Tracking issue: #50 (Engine Primitives — engine-vs-framework boundary)
