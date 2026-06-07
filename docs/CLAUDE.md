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

```
docs/
├── pages/               # File-based routing (zfb page modules)
│   └── lib/             # Shared page utilities (nav, locale merge, doc props)
├── plugins/             # zfb integration plugins (copy-public, search-index, llms-txt, etc.)
├── public/              # Static assets copied flat to dist/ (favicons, img/)
└── src/
    ├── components/      # Preact components
    ├── config/          # settings.ts + i18n config
    ├── content/
    │   ├── docs/        # EN MDX content
    │   └── docs-ja/     # Japanese MDX content (mirrors docs/ structure)
    ├── hooks/           # zfb lifecycle hooks
    ├── styles/          # global.css (design tokens + Tailwind config)
    └── utils/           # Shared utilities (base URL, docs helpers, etc.)
```

## Content Conventions

### Frontmatter

- Required: `title` (string)
- Optional: `description`, `sidebar_position` (number), `sidebar_label`, `category`, `tags`
- Sidebar order is driven by `sidebar_position`
- Sidebar category label comes from the `index.mdx` frontmatter in each directory, or from a `_category_.json` file when there is no `index.mdx`
- `_category_.json` files are NOT disposable: the zfb content-collection loader skips them as data files (emitting a benign build warning), but the docs app's nav layer (`loadCategoryMeta` via `resolveNavSource`) still reads them for category `label`, `position`, `description`, `sortOrder`, and `noPage`. Deleting them can regress sidebar/category nav (e.g. `api`, `architecture`, generated Claude sections).

### Links

- Use **relative `.mdx` paths** for cross-doc links: `[label](../other-dir/page.mdx)` — zfb's `resolveMarkdownLinks` converts these to root-relative route URLs at build time (the only fully reliable form).
- For links to pages that don't exist in the current locale collection (e.g., JA docs pointing to EN-only sections that `resolveMarkdownLinks` can't resolve), use **root-relative absolute paths** that the base rewriter prepends `base` to: `/ja/docs/markdown-features/` (keeps JA locale shell) or `/docs/recipes/admonitions` (EN). Do NOT use bare relative paths like `../markdown-features/`: zfb leaves them unrewritten, and under Cloudflare Pages' trailing-slash redirect (`/x` → `/x/`) the browser resolves them from the wrong base, producing 404s.

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

## Package CSS generation (`@takazudo/zudo-doc` safelist)

`@takazudo/zudo-doc` ships **no precompiled CSS** — its header/sidebar/doclayout/toc/footer
components carry their styles as static Tailwind class literals (`class: "sticky top-0 z-50 …"`)
baked into the compiled dist JS. As the **consumer**, this site is responsible for generating
those utilities. The monorepo covers them with `@source "packages/zudo-doc/src"`; a consumer
can't scan the monorepo source, and the bare `@source "node_modules/@takazudo/zudo-doc/dist/**"`
directive is unreliable under Tailwind v4 (it drops plain utilities like `sticky`/`top-0`/`z-50`
intermittently, and far more on a non-git CI build — see zudolab/zudo-doc#1971, #883).

So instead of scanning the dist, `scripts/gen-zudo-doc-safelist.mjs` **extracts a complete
safelist from it at build time**: it reads every `class: "…"` literal in the package dist and
writes `src/styles/_zudo-doc-safelist.css` (a list of `@source inline("…")` lines), which
`global.css` `@import`s. The generator runs automatically before every `zfb build`/`zfb dev`
(chained with `&&` in the `build`/`dev` scripts in `package.json`), so the safelist is **complete
and stays correct across dependency bumps** — it derives from the installed dist, not a frozen
hand list. The
generated file is committed (with a "DO NOT EDIT" header) so the tree shows what was extracted;
a rebuild leaves it unchanged (deterministic, sorted). Do **not** re-add a bare node_modules
`@source` to `global.css` — it reintroduces the dropped-utility bug.
