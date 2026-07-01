# Docs

Documentation site built with [zudo-doc](https://github.com/zudolab/zudo-doc) — a zfb-based documentation framework with MDX, Tailwind CSS v4, and Preact islands.

## Tech Stack

- **zfb** — static site engine (Rust binary + JS plugin host)
- **MDX** — content format
- **Tailwind CSS v4** — via `@tailwindcss/vite`
- **Preact** — for interactive islands only
- **Shiki** — code highlighting

## Setup

Two scripts in `docs/scripts/` each install a Claude Code skill that symlinks this docs tree (`src/content/docs` + `docs-ja`) into the user-scope skills dir (`~/.claude/skills/`) for AI lookup access. They produce **distinct** skills:

- **zfb-wisdom skill** — run `bash docs/scripts/setup-zfb-wisdom.sh` once. Fixed skill name `zfb-wisdom` (see `src/content/docs/claude-skills/zfb-wisdom.mdx`).
- **{project}-wisdom skill** — run `pnpm --filter docs setup:doc-skill` (wired to `bash scripts/setup-doc-skill.sh`). Interactive; default skill name is `<package.json name>-wisdom` (currently `docs-wisdom`).

## Commands

All commands run from the **repo root** with the `--filter docs` workspace flag, or directly inside `docs/`:

- `pnpm docs:dev` — zfb dev server (port 4321)
- `pnpm docs:build` — static HTML export to `docs/dist/`
- `pnpm docs:check` — zfb type checking (`tsc --noEmit` over `zfb.config.ts`, collection schemas, and `src/`). Note: `pages/` is excluded in `tsconfig.json`, so page modules are NOT type-checked here — they are checked when `zfb build` bundles them.
- `pnpm docs:preview` — serve the built `docs/dist/` locally

## Key Directories

This site uses **zudo-doc v2 package-owned routes** (`settings.packageOwnedRoutes: true`).
`@takazudo/zudo-doc/plugins/routes` injects and owns every route — `/docs/[[...slug]]`,
`/[locale]`, `/[locale]/docs/**`, `/404`, `/sitemap.xml`, `/robots.txt`, tags, versions — and
renders all page chrome (header/sidebar/TOC/footer) from package defaults, reconstructed from a
`virtual:zudo-doc-route-context` payload that `zudoDocPreset()` builds out of the host's
`settings` + `translations` + `colorSchemes` (see `zfb.config.ts`). The host owns only **data**
(config + content + CSS + assets) plus the one route the package does not inject: `/`.

```
docs/
├── pages/               # Host owns only the root home + island registration
│   ├── index.tsx        # "/" — re-exports @takazudo/zudo-doc/routes/index (package default home)
│   └── _register-islands.ts  # registers the DocHistory client island for package-owned routes
├── public/              # Static assets copied flat to dist/ (favicons, img/)
└── src/
    ├── config/          # settings, i18n (+ translations), docs-schema, color-schemes, tag-vocabulary
    ├── content/
    │   ├── docs/        # EN MDX content
    │   └── docs-ja/     # Japanese MDX content (mirrors docs/ structure)
    └── styles/          # global.css (design tokens + Tailwind config + package CSS imports)
```

The former v1 host-owned route/chrome layer (`pages/lib/**`, `pages/docs/**`, `pages/[locale]/**`,
`src/components/**`, `src/utils/**`, `src/hooks/**`) was **deleted** when this site adopted v2
package-owned routes — the package now provides all of it. Do not reintroduce those stubs; extend
via zudo-doc's `hostBindings`/preset seams instead.

## Content Conventions

### Frontmatter

- Required: `title` (string)
- Optional: `description`, `sidebar_position` (number), `sidebar_label`, `category`, `tags`
- Sidebar order is driven by `sidebar_position`
- Sidebar category label comes from the `index.mdx` frontmatter in each directory, or from a `_category_.json` file when there is no `index.mdx`
- `_category_.json` files are NOT disposable: the zfb content-collection loader skips them as data files (emitting a benign build warning — silenced when the collection's `include`/`exclude` globs already filter the file out, see zfb#1032), but the docs app's nav layer (`loadCategoryMeta` via `resolveNavSource`) still reads them for category `label`, `position`, `description`, `sortOrder`, and `noPage`. Deleting them can regress sidebar/category nav (e.g. `api`, `architecture`, generated Claude sections).

### Links

- Use **relative `.mdx` paths** for cross-doc links: `[label](../other-dir/page.mdx)` — zfb's `resolveMarkdownLinks` converts these to root-relative route URLs at build time (the only fully reliable form).
- For links to pages that don't exist in the current locale collection (e.g., JA docs pointing to EN-only sections that `resolveMarkdownLinks` can't resolve), use **root-relative absolute paths** that the base rewriter prepends `base` to: `/ja/docs/markdown-features/` (keeps JA locale shell) or `/docs/recipes/admonitions` (EN). Do NOT use bare relative paths like `../markdown-features/`: zfb leaves them unrewritten, and under Cloudflare Workers Static Assets' trailing-slash redirect (`/x` → `/x/`) the browser resolves them from the wrong base, producing 404s.

### Admonitions

Available in all MDX files without imports: `<Note>`, `<Tip>`, `<Info>`, `<Warning>`, `<Danger>`, `<Caution>`, `<Details>`
Each accepts an optional `title` prop.

### Headings

Do NOT use h1 (`#`) in doc content — the page title from frontmatter is rendered as h1. Start content headings from h2 (`##`).

## i18n

- English (default): `/docs/...` — content in `src/content/docs/`
- Japanese: `/ja/docs/...` — content in `src/content/docs-ja/`
- Pages missing from `docs-ja/` are served with EN content (locale-first merge with EN fallback)
- `defaultLocaleOnlyPrefixes` in `settings.ts` lists sections built only in EN (e.g., `/docs/claude-md/`, `/docs/claude-skills/`)

## Active Settings Flags

The following flags are set in `src/config/settings.ts` and are currently enabled:

- **mermaid** — Renders Mermaid diagrams in MDX content
- **sitemap** — Generates `sitemap.xml`
- **docMetainfo** — Shows document metadata (word count, reading time) below the title
- **cjkFriendly** — Applies `remark-cjk-friendly` for better CJK line-breaking
- **llmsTxt** — Generates `llms.txt` for LLM consumption
- **docHistory** — Shows document edit history on each page
- **sidebarResizer** — Draggable sidebar width
- **sidebarToggle** — Show/hide desktop sidebar button
- **claudeResources** — Auto-generated docs for Claude Code resources (skills, claude-md); value is `{ claudeDir: ".claude" }` (reads from `docs/.claude/`)
- **headingIdStrategy** — Set to `"flat"` (github-slugger flat IDs) to preserve existing `#anchor` deep links from the pre-migration site

## Package CSS generation (`@takazudo/zudo-doc`)

`@takazudo/zudo-doc` 0.2.0 ships **`safelist.css`** (zudolab/zudo-doc#1996), which lists every
Tailwind class the package emits as `@source` inline values. Importing it in `global.css` is all
that is needed for Tailwind v4 to generate the package utilities:

```css
@import "@takazudo/zudo-doc/safelist.css";
```

**History note:** before 0.2.0, the package shipped no precompiled CSS and Tailwind v4 deliberately
excluded `node_modules` from scanning, so this repo used a vendor-copy workaround
(`scripts/vendor-zudo-doc-dist.mjs` + `@source "./.zudo-doc-tw/**"`) to make the package classes
available. That machinery was deleted when the safelist mechanism landed (zudolab/zudo-doc#1996).
