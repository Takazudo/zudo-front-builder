# Docs

Documentation site built with [zudo-doc](https://github.com/zudolab/zudo-doc) — a zfb-based documentation framework with MDX, Tailwind CSS v4, and Preact islands.

## Tech Stack

- **zfb** — static site engine (Rust binary + JS plugin host)
- **MDX** — content format
- **Tailwind CSS v4** — compiled by the Tailwind v4 CLI embedded in the `zfb` binary; there is no per-project `tailwindcss` / `@tailwindcss/vite` install (see `src/content/docs/concepts/styling.mdx`)
- **Preact** — for interactive islands only
- **`@takazudo/zfb-md-wasm`** — code highlighting (compiled into the zfb pipeline; no separate Shiki dependency)

## Setup

Two scripts in `docs/scripts/` each install a Claude Code skill that symlinks this docs tree (`src/content/docs` + `docs-ja`) into the user-scope skills dir (`~/.claude/skills/`) for AI lookup access. They produce **distinct** skills:

- **zfb-wisdom skill** — run `bash docs/scripts/setup-zfb-wisdom.sh` once. Fixed skill name `zfb-wisdom` (generated Claude-resource page: `src/content/docs/claude-skills/zfb-wisdom/index.mdx`).
- **{project}-wisdom skill** — run `pnpm --filter docs setup:doc-skill` (wired to `bash scripts/setup-doc-skill.sh`). Interactive; default skill name is `<package.json name>-wisdom` (currently `docs-wisdom`).

## Commands

All commands run from the **repo root** with the `--filter docs` workspace flag, or directly inside `docs/`:

- `pnpm docs:dev` — starts the dev loop, which is **two processes** run in parallel (`run-p dev:zfb dev:history`): `zfb dev` on port **4321**, and `doc-history-server` on port **4322** (feeds the `docHistory` feature; see below).
- `pnpm docs:build` — static HTML export to `docs/dist/`
- `pnpm docs:check` — zfb type checking (`tsc --noEmit` over `zfb.config.ts`, collection schemas, and `src/`). Note: `pages/` is excluded in `tsconfig.json`, so page modules are NOT type-checked here — they are checked when `zfb build` bundles them.
- `pnpm --filter docs check:html` — validates emitted HTML in `dist/` against `docs/.htmlvalidate.json` (`html-validate`); no root-level alias exists for this one.
- `pnpm docs:preview` — serve the built `docs/dist/` locally

## Key Directories

This site is a **minimal v4 scaffold host**: one config file drives everything, and the package owns chrome, routing, translations, color schemes, and the directive vocabulary. The host supplies only data — config, content, styles, and the few files below.

```
docs/
├── zfb.config.ts        # The ONE config file — defineConfig(zudoDoc({ ... }))
├── pages/                # Three route stubs the host must carry (see below)
│   ├── index.tsx                     # "/" — re-exports @takazudo/zudo-doc/routes/index
│   ├── docs/[[...slug]].tsx          # "/docs/**" — self-contained stub, see file header
│   └── [locale]/docs/[[...slug]].tsx # "/{locale}/docs/**" — locale counterpart
├── public/               # Static assets copied flat to dist/ (favicons, img/)
└── src/
    ├── config/
    │   └── docs-schema.ts  # 10-line wrapper over the package's default schema,
    │                       # adding the one host-specific frontmatter key: `tier`
    ├── content/
    │   ├── docs/        # EN MDX content
    │   └── docs-ja/     # Japanese MDX content (mirrors docs/ structure)
    └── styles/          # global.css (Tailwind config + package CSS imports + token overrides)
```

There is no `src/config/settings.ts`, `settings-types.ts`, `i18n.ts`, `color-schemes.ts`,
`tag-vocabulary.ts`, or `zfb-shim.d.ts` — `zudoDoc({ ... })` in `zfb.config.ts` replaces all of
them. `docs-schema.ts` is the only survivor under `src/config/`, and only because this site adds
one frontmatter key the package doesn't know about.

**Why three route stubs exist, not zero:** `zudoDoc()`'s routes plugin injects `/docs/[[...slug]]`,
`/[locale]/docs/[[...slug]]`, `/[locale]`, `/404`, `/sitemap.xml`, and `/robots.txt` package-side —
but injected **dynamic** routes 404 under `zfb dev` (a pre-existing zfb dev-mode gap, distinct from
the static-route injection gap). The two `[[...slug]]` stubs under `pages/` work around it by
reconstructing the route from the sanctioned package entrypoints (`virtual:zudo-doc-route-context`,
`@takazudo/zudo-doc/route-context`, `@takazudo/zudo-doc/chrome`) — see each file's own header
comment for the full contract. `pages/index.tsx` is a one-line re-export because the static home
route doesn't hit the same dynamic-route bug. There is no `pages/[locale].tsx` — the package serves
`/[locale]` itself without a host stub.

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
- **metaTags** — `<head>` metadata; shallow-merged wholesale (a supplied nested object replaces the package default, not patches it — all keys must be given even when only a couple differ)
- **logo** — `/img/logo.svg`, reproducing the pre-v4 home hero mask (v4's own default is an auto-generated mark)
- **locales** — the `ja` locale, mapped to `src/content/docs-ja`
- **cjkFriendly** — Applies `remark-cjk-friendly` for better CJK line-breaking
- **defaultLocaleOnlyPrefixes** — sections built only in EN (see i18n above)
- **sitemap** — Generates `sitemap.xml`
- **llmsTxt** — Generates `llms.txt` for LLM consumption
- **claudeResources** — Auto-generated docs for Claude Code resources (skills, claude-md); `{ claudeDir: ".claude" }` (reads from `docs/.claude/`)
- **docMetainfo** — Shows document metadata (word count, reading time) below the title
- **docHistory** — Shows document edit history on each page; fed by the `doc-history-server` dev process (port 4322) / `doc-history-out` build artifact
- **sidebarResizer** — Draggable sidebar width
- **sidebarToggle** — Show/hide desktop sidebar button
- **imageEnlarge** — Click-to-enlarge for content images
- **dynamicPageTransition** — Animated page transitions
- **footer** — footer links + copyright, host-owned chrome content
- **headerNav** — the top-nav category links (Getting Started, Install, Concepts, …)
- **headerRightItems** — the right-aligned header controls (GitHub link, theme toggle, search, language switcher)
- **buildDocsSchema** — the host's `docs-schema.ts` wrapper, adding the `tier` frontmatter enum
- **adapter** — `@takazudo/zfb-adapter-cloudflare`

The host **does** own its identity strings and top-level chrome content (logo, footer, header nav)
via the keys above — what it does NOT own is translations, color schemes, the directive
vocabulary, or the chrome *rendering* (how header/sidebar/TOC/footer are laid out and styled),
which all come from the package default. Eject via `npx zudo-doc eject <component>` before
hand-customizing any of the latter.
