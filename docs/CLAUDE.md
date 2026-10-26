# Docs

Documentation site built with [zudo-doc](https://github.com/zudolab/zudo-doc) — a zfb-based documentation framework with MDX, zudo-wind utility CSS, and zudo-react islands.

The host runs on the released owned-engine stack: exact published `@takazudo/zfb*` 5.1.4 and
`@takazudo/zudo-doc` / `zudo-doc-history-server` 8.1.1 (#3329). Verify
versions from the docs command context (`pnpm --dir docs exec zfb --version`,
`pnpm --dir docs why <pkg>`), not from hoisted root copies.

The docs member requires Node 24 (`engines.node` is `>=24 <25`; zudo-doc 8 requires `>=24`), and the repo develops on Node 24.

## Tech Stack

- **zfb** — static site engine (Rust binary + JS plugin host)
- **MDX** — content format
- **zudo-wind** — utility CSS compiled in-process by `zfb`. `zudoDoc()` registers the package's `wind.json` manifest and var-backed defaults; the host's top-level `wind` key in `zfb.config.ts` adds the `md` (768px) breakpoint the playgrounds use, excludes Wind preview samples and the workshop's generated starter code from source discovery, and reserves the workshop's authored class names. `src/styles/global.css` holds only package CSS imports, authored CSS and `:root` overrides — no Tailwind directives. Gate with `pnpm exec zfb wind audit --project-root . --plan build --fail-on error`
- **zudo-react** — host islands (the playgrounds) use setup-time signals, `Show`/`For`, and scope `onActivate`/`effect`/`onCleanup`; there is no Preact rendering or compatibility path in the host
- **`@takazudo/zfb-md-wasm`** — code highlighting (compiled into the zfb pipeline; no separate Shiki dependency)

## Dependency audit register

Keep these direct docs declarations when auditing dependencies:

- **zudo-doc peer feature packages:** `diff`, `katex`, `zod`, and `@takazudo/zudo-doc-history-server`. `@takazudo/zudo-doc` declares these as peers; the docs site installs them deliberately to satisfy the package contract and opt into its features.
- **No `@takazudo/zdtp`.** It is an optional zudo-doc peer used only by the design token panel. `designTokenPanel` stays at the package default `false`, so since zudo-doc 8.1.1 the preset shadows `@takazudo/zudo-doc/zdtp-loader` with a stub and nothing imports zdtp. Add it back only together with `designTokenPanel: true`.
- **No direct `preact` / `preact-render-to-string`.** zudo-doc core rendering does not need Preact. Never re-add Preact to host code or tests (component tests render with `@takazudo/zfb/zudo-react/server`).
- **No separate script supervisor:** `dev` and `dev:network` use `run-parallel`, a bin shipped by `@takazudo/zudo-doc` (already a direct dependency, since 5.14.0), so no separate process-supervisor package belongs here. Any future replacement must forward signals to the whole descendant tree, propagate the real exit status, and reap both children on Ctrl-C — `run-parallel` was verified for all three in the PR for issue #2826. JSON cannot carry this rationale as a comment.

This register is a keep-list, not a request to add dependencies. Re-check the installed zudo-doc package and the generated `docs/.zfb-build/bundle.mjs` before changing these declarations.

## tsconfig.json inlines zudo-doc's base config

`docs/tsconfig.json` copies the `compilerOptions` and `files` of
`@takazudo/zudo-doc@7.1.0/tsconfig.base.json` instead of extending it (#3556).
The copy was introduced because 6.1.1 set
`baseUrl: "."`, which TypeScript 7 rejects (TS5102) even when inherited through `extends`.
Upstream 6.2.0 removed `baseUrl`; the compiler options now match except for the host
`types: ["node"]` addition (TypeScript 6+ no longer auto-loads `@types/*`). The 7.1.0 base is byte-identical to 6.2.0, so no option changed in the bump.
It keeps the base's `jsxImportSource: "@takazudo/zfb/zudo-react"` and has no
`paths`: the old `react` → `preact/compat` aliases are gone, and nothing imports `@/`. When zudo-doc
is bumped, diff its new `tsconfig.base.json` against this file and carry over any change, then
update its entry in the root `ZUDO_DEPS_PINS.md` (provenance for this copy, the adapted route stubs
and the identity fixture's generated styleguide files).

## Setup

Two scripts in `docs/scripts/` each install a Claude Code skill that symlinks this docs tree (`src/content/docs` + `docs-ja`) into the user-scope skills dir (`~/.claude/skills/`) for AI lookup access. They produce **distinct** skills:

- **zfb-wisdom skill** — run `bash docs/scripts/setup-zfb-wisdom.sh` once. Fixed skill name `zfb-wisdom` (generated Claude-resource page: `src/content/docs/claude-skills/zfb-wisdom/index.mdx`).
- **{project}-wisdom skill** — run `pnpm --filter docs setup:doc-skill` (wired to `bash scripts/setup-doc-skill.sh`). Interactive; default skill name is `<package.json name>-wisdom` (currently `docs-wisdom`).

## Commands

All commands run from the **repo root** with the `--filter docs` workspace flag, or directly inside `docs/`:

- `pnpm docs:dev` — starts the dev loop, which is **two processes** run in parallel (`run-parallel dev:zfb dev:history`, the supervisor bin shipped by `@takazudo/zudo-doc`): `zfb dev` on port **4321**, and `doc-history-server` on port **4322** (feeds the `docHistory` feature; see below). The supervisor accepts only literal script names — no flags, globs, or forwarded arguments — so `pnpm docs:dev -- --host` fails loudly instead of silently ignoring the flag; use `pnpm --filter docs dev:network` for `--host 0.0.0.0`. If either child exits non-zero, the other is torn down and the real exit status is propagated. Ctrl-C or SIGINT/SIGTERM aimed at the supervisor alone signals the entire descendant tree; a second signal escalates to SIGKILL. The `ERROR: "dev:zfb" exited with 130.` line on Ctrl-C is expected.
- `pnpm docs:build` — static HTML export to `docs/dist/`
- `pnpm docs:check` — zfb type checking (`tsc --noEmit` over `zfb.config.ts`, collection schemas, `src/`, and `pages/`).
- `pnpm --filter docs check:html` — validates emitted HTML in `dist/` against `docs/.htmlvalidate.json` (`html-validate`); no root-level alias exists for this one.
- `pnpm docs:preview` — serve the built `docs/dist/` locally

## Key Directories

This site is a **minimal v4 scaffold host**: one config file drives everything, and the package owns chrome, routing, translations, color schemes, and the directive vocabulary. The host supplies only data — config, content, styles, and the few files below.

```
docs/
├── zfb.config.ts        # The ONE config file — defineConfig(zudoDoc({ ... })), incl. `wind`
├── pages/                # Four host-owned route files (see below)
│   ├── index.tsx                     # "/" — re-exports @takazudo/zudo-doc/routes/index
│   ├── [locale].tsx                  # "/{locale}" — locale home: EN-only compact links + JA tags
│   ├── docs/[[...slug]].tsx          # "/docs/**" — self-contained stub, see file header
│   └── [locale]/docs/[[...slug]].tsx # "/{locale}/docs/**" — locale counterpart (JA fallback dirs)
├── public/               # Static assets copied flat to dist/ (favicons, img/, _redirects)
└── src/
    ├── chrome-bindings.tsx   # chromeBindingsModule: MDX playground islands, home extras, nav chrome
    ├── navigation-chrome.tsx # Header/Sidebar factories fed with the editorial navigation data
    ├── components/           # Host islands (playgrounds, design workshop, Wind preview enhancer)
    ├── config/
    │   ├── docs-schema.ts        # wrapper over the package schema adding the `tier` key
    │   ├── navigation-groups.mjs # shared editorial membership (sidebar + overview cards)
    │   ├── navigation-scope.mjs  # local sidebar views for Wind pages
    │   └── route-migrations.mjs  # compatibility redirects/anchors
    ├── content/
    │   ├── docs/        # EN MDX content
    │   └── docs-ja/     # Japanese MDX content (mirrors docs/ structure)
    └── styles/          # global.css (package CSS imports + authored CSS + :root overrides)
```

There is no `src/config/settings.ts`, `settings-types.ts`, `i18n.ts`, `color-schemes.ts`,
`tag-vocabulary.ts`, or `zfb-shim.d.ts` — `zudoDoc({ ... })` in `zfb.config.ts` replaces all of
them.

**Why the host keeps route files:** `zudoDoc()`'s routes plugin injects `/docs/[[...slug]]`,
`/[locale]/docs/[[...slug]]`, `/[locale]`, `/404`, `/sitemap.xml`, and `/robots.txt` package-side;
a host `pages/` route shadows the package one. The two `[[...slug]]` stubs keep explicit host
ownership of the doc routes, rebuilt from the sanctioned package entrypoints
(`virtual:zudo-doc-route-context`, `@takazudo/zudo-doc/route-context`, `@takazudo/zudo-doc/chrome`).
Each statically imports `../src/chrome-bindings` (the island scanner cannot follow the virtual
bindings module to on-disk client components) and merges the package `DocHistoryBoundary` over the
bindings so the history button hydrates. The locale stub threads each locale's content dir and the
EN fallback (`isFallback`, EN `docsDir` for doc history). `pages/[locale].tsx` replaces the
package locale home so the compact site links keep EN-only sections while tag counts stay
locale-filtered. `pages/index.tsx` is a one-line re-export of the package home.

**Sidebar navigation (06R, #4099):** `src/config/navigation-groups.mjs` is the one editorial
membership source for both the sidebar and the bilingual overview cards (keep them in sync with
`node docs/scripts/sync-navigation-indexes.mjs --check`). `navigation-chrome.tsx` hands the
package's native `buildSidebarNavigation` the complete Wind editorial forest
(`buildWindEditorialForest` in `navigation-scope.mjs`: Wind → Learn / Utility reference → groups →
family pages, plus the Design system playground cross-link occurrence) and a `SidebarDefinition`
whose `initialPath` selects the page's configured local view. Editorial parents come from the
membership data, never URL segments (`utilities/background` is a URL sibling of
`utilities/surfaces-borders`). Broaden / Show only this branch / Restore, persistence and
rendering are zudo-doc's; the host adds no scope state, storage handling or migration.
`initialPathMode: "node"` (zudo-doc 6.1) makes the configured view the linked scope root itself:
its overview row appears exactly once, current on its own page, above its members, and Broaden
moves to its real editorial parent (Background: Surfaces & borders → Utility reference →
zudo-wind). The Wind index, compatibility and Coming from Tailwind pages start at the Wind root,
the highest scope, so the native scope toolbar is hidden there; Coming from Tailwind keeps that
view without being one of its members. zudo-doc 6.1 persists only
explicit expand/collapse choices (`zd-sidebar-scope`, version 2): automatic path opening is never
stored, a deliberate collapse survives hash changes, filtering and same-page reload, the next page
reveals its own path, Restore clears the selection and choices but keeps the filter, and older
unversioned expansion is discarded once. `sidebar_initial_expansion: collapsed` on both Wind
index pages keeps those views compact. Architecture and Build with zfb get the same treatment:
`buildEditorialForest` gives each a dedicated single-root forest of real pages (the section
landing page → its group landing pages → their articles), so every group row is a linked parent
row with a separate disclosure and no repeated link to its own index. Explicit `type: "category"`
items cannot do this: they are synthetic no-page nodes. Architecture pages start at the four group
rows (`initialPathMode: "children"`) and Broaden reaches the real Architecture page, which is a
node-mode root view of its own; `sidebar_initial_expansion: collapsed` on both Architecture indexes
reveals only the current group. Build with zfb keeps its landing row above its six groups (node
mode); Troubleshooting intentionally appears in two groups, and cross-links such as Markdown
Features and zudo-wind stay leaves. Deploy & Embed uses the same machinery with no groups: its
real landing row (node mode, `initialPath: [0]`) above `DEPLOY_SLUGS` as four leaves; that root is
the highest scope, so the native toolbar stays hidden there. Section
ownership is `canonicalNavSection`: Architecture owns every `architecture/*` route even when Build
or Deploy lists it; practical guides are classified by membership without moving their URLs.
URL-based breadcrumbs and pagination remain native. The chrome factory caches each Header/Sidebar
pair by locale and scope id, and each scope id maps to exactly one forest and baseline.
`tests/docs-navigation-structure/verify.mjs` asserts the Wind contract on the served host;
`scripts/__tests__/docs-navigation-structure.test.mjs` checks all four against the installed
zudo-doc resolver, and `tests/docs-wind-preview/page-backed-navigation.chromium.spec.mjs` checks it on the
built site.

**Extension seam:** to customize a package-owned component (chrome, a specific route, etc.), run
`npx zudo-doc eject <component>` — this copies the package's implementation into the host tree so
it can be modified, rather than requiring the host to reconstruct package internals by hand.

## Content Conventions

### Frontmatter

- Required: `title` (string)
- Optional: `description`, `sidebar_position` (number), `sidebar_label`, `category`, `tags`
- Sidebar order is driven by `sidebar_position`
- Category metadata lives **only** in the directory's `index.mdx` frontmatter. Every top-level docs directory has one, in both locales — add one when you create a new category, or it falls back to a title-cased directory name (`api` → "Api"). The keys `buildNavTree` reads are `sidebar_label` (else `title`), `sidebar_position`, `description`, `category_sort_order` (`"asc"` default — `changelog` sets `desc`), and `category_no_page`
- **Do not add `_category_.json` sidecars.** They are not read under zfb: zudo-doc's `loadCategoryMeta` reads them with `node:fs`, which the SSG runtime stubs with a throwing proxy, so the read fails closed to an empty map with no build error (zfb#2196). They also each emitted a data-file skip warning (zfb#1032). All 17 were migrated to frontmatter and deleted — upstream retired the sidecar in favour of frontmatter, which `build-tree.js` reads first at every site
- **Careful with `category_no_page: true`** on a directory named in `headerNav` (`docs/zfb.config.ts`): it suppresses the category's `href` and its page is never emitted, but the header still links it from every page — a site-wide 404 that `--strict-broken` does **not** catch

### Changelog lanes

The changelog has five package lanes in this stable order: `zfb`, `zfb-runtime`,
`zfb-adapter-cloudflare`, `create-zfb`, and `zfb-md-wasm`. The `zfb` lane owns
`@takazudo/zfb`, `@takazudo/zfb-slugify`, the Rust engine/CLI, and the five platform carrier packages; each other lane
owns only its named package. The complete shared lockstep history through **v2.10.0** stays in
`src/content/docs/changelog/zfb/` and must not be duplicated into another lane.

Release pages use the path convention
`src/content/docs/changelog/<lane>/v<version>.mdx`, which emits
`/docs/changelog/<lane>/v<version>/`. For a new release, calculate that lane's
`sidebar_position` as **max existing release position + 1**, as if the target page were absent,
using `scripts/next-changelog-sidebar-position.mjs`; do not reuse a position from another lane.
Each lane index owns its own descending release order, and its previous/next pagination is disabled.
The oldest release in every populated lane must stop its pager at the lane boundary with
`pagination_next: null`; no pager may cross from one lane into another.

In `docs/zfb.config.ts`, `Changelog` is the parent header item with `categoryMatch: "changelog"`
and the five lane paths as children. The child items intentionally omit `categoryMatch`: nav-scope
matches only the first slug segment, so the parent owns the `changelog` scope and lane paths stay
under it.

### Links

- Use **relative `.mdx` paths** for cross-doc links: `[label](../other-dir/page.mdx)` — zfb's `resolveMarkdownLinks` converts these to root-relative route URLs at build time (the only fully reliable form).
- For links to pages that don't exist in the current locale collection (e.g., JA docs pointing to EN-only sections that `resolveMarkdownLinks` can't resolve), use **root-relative absolute paths** that the base rewriter prepends `base` to: `/ja/docs/markdown-features/` (keeps JA locale shell) or `/docs/recipes/admonitions` (EN). Do NOT use bare relative paths like `../markdown-features/`: zfb leaves them unrewritten, and under Cloudflare Workers Static Assets' trailing-slash redirect (`/x` → `/x/`) the browser resolves them from the wrong base, producing 404s.

### Admonitions

Available in all MDX files without imports: `<Note>`, `<Tip>`, `<Info>`, `<Warning>`, `<Danger>`, `<Caution>`, `<Details>`
Each accepts an optional `title` prop.

### Headings

Do NOT use h1 (`#`) in doc content — the page title from frontmatter is rendered as h1. Start content headings from h2 (`##`).

**Heading IDs are always hierarchical.** v4 removed the `headingIdStrategy` option entirely —
`zudoDoc()` throws if the key is present — and hardcodes ancestor-prefixed IDs (e.g. an h3 under an
h2 gets an id combining both), unlike this site's pre-v4 flat (github-slugger) IDs. This was an
**accepted, deliberate consequence** of adopting v4 (see epic #1953's Wave-2 decision record): 22%
of anchors (h3+) changed shape, so external deep links into those headings may 404. Do not try to
force flat IDs back via a post-`zudoDoc()` `markdown.features.headingIds` override — it desyncs the
renderer from the package's own TOC/sidebar allocator (which hardcodes the hierarchical algorithm
with no strategy parameter), breaking in-page TOC links on every h3+ heading. This was tried and
rejected; it is strictly worse than the anchor churn it avoids.

**No gate validates anchors — and `slugify` strips ASCII punctuation only.** Two facts that combine
badly, especially in `docs-ja/`:

- `pnpm --filter docs check` is `tsc --noEmit` plus collection-schema validation. It does **not**
  resolve `#anchor` targets, so a cross-reference that points at nothing passes the gate silently.
- The slug allocator strips **ASCII** punctuation only. CJK punctuation — `、`, `（`, `）`, `。` —
  survives into the slug verbatim. A naturally-written heading like `## デフォルトエクスポート（props）について`
  yields a slug containing the full-width parens, so a hand-written link that assumes they were
  stripped resolves to nothing, uncaught.

**Prefer punctuation-free headings when they are link targets**, and verify a CJK anchor mechanically
rather than eyeballing the slug — import the allocator the site itself uses:

```
docs/node_modules/@takazudo/zudo-doc/dist/extract-headings/index.js
```

(Found in zfb#2360, during the JA half of epic #2351: that agent verified its three new CJK
cross-references this way and wrote both new headings punctuation-free so the slugs stayed
predictable. An actual anchor-validation gate would be a larger, separate decision.)

## i18n

- English (default): `/docs/...` — content in `src/content/docs/`
- Japanese: `/ja/docs/...` — content in `src/content/docs-ja/`
- Pages missing from `docs-ja/` are served with EN content (locale-first merge with EN fallback)
- `defaultLocaleOnlyPrefixes` in `zfb.config.ts` lists sections built only in EN (e.g., `/docs/claude-md/`, `/docs/claude-skills/`)

## Active Settings Flags

The following keys are set in `zfb.config.ts`'s `zudoDoc({ ... })` call:

- **siteName / siteDescription / siteUrl / githubUrl** — site identity, used in metadata and chrome
- **base** — `/` for normal deployments. `scripts/build-wind-docs-base.mjs` temporarily sets a literal prefix and `copyPublicWithBase: false` for actual subpath browser verification, then restores this config. The test server mounts the untouched output at that prefix so missing asset/link rewrites fail. The pinned config evaluator has no `process` global, so do not read environment variables directly here.
- **metaTags** — `<head>` metadata; shallow-merged wholesale (a supplied nested object replaces the package default, not patches it — all keys must be given even when only a couple differ)
- **logo** — `/img/logo.svg`, reproducing the pre-v4 home hero mask (v4's own default is an auto-generated mark)
- **locales** — the `ja` locale, mapped to `src/content/docs-ja`
- **cjkFriendly** — Enables the CJK-friendly Markdown pipeline for better CJK line-breaking
- **defaultLocaleOnlyPrefixes** — sections built only in EN (see i18n above)
- **sitemap** — Generates `sitemap.xml`
- **llmsTxt** — Generates `llms.txt` for LLM consumption
- **claudeResources** — Auto-generated docs for Claude Code resources (skills, claude-md); `{ claudeDir: ".claude" }` (reads from `docs/.claude/`)
- **docMetainfo** — Shows document metadata (word count, reading time) below the title
- **docHistory** — Shows document edit history on each page; fed by the `doc-history-server` dev process (port 4322) / `doc-history-out` build artifact
- **sidebarResizer** — Draggable sidebar width
- **sidebarToggle** — Show/hide desktop sidebar button
- **tocToggle** — Show/hide desktop table-of-contents button
- **imageEnlarge** — Click-to-enlarge for content images
- **dynamicPageTransition** — Animated page transitions
- **footer** — footer links + copyright, host-owned chrome content
- **headerNav** — the top-nav category links (Getting Started, Build with zfb, zudo-wind, zudo-react, Deploy & Embed, Reference, Recipes; secondary architecture/guides and contributor material)
- **headerRightItems** — the right-aligned header controls (GitHub link, theme toggle, search, language switcher)
- **buildDocsSchema** — the host's `docs-schema.ts` wrapper, adding the `tier` frontmatter enum
- **adapter** — `@takazudo/zfb-adapter-cloudflare`

The host **does** own its identity strings and top-level chrome content (logo, footer, header nav)
via the keys above — what it does NOT own is translations, color schemes, the directive
vocabulary, or the chrome *rendering* (how header/sidebar/TOC/footer are laid out and styled),
which all come from the package default. Eject via `npx zudo-doc eject <component>` before
hand-customizing any of the latter.
