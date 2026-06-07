# Docs

Documentation site built with [zudo-doc](https://github.com/zudolab/zudo-doc) — a zfb-based documentation framework with MDX, Tailwind CSS v4, and Preact islands.

## Tech Stack

- **zfb** — static site engine (Rust binary + JS plugin host)
- **MDX** — content format
- **Tailwind CSS v4** — via `@tailwindcss/vite`
- **Preact** — for interactive islands only
- **Shiki** — code highlighting

## Setup

- **zfb-wisdom Claude Code skill** — run `bash docs/scripts/setup-zfb-wisdom.sh` once to give AI agents lookup access to this documentation tree (see `src/content/docs/claude-skills/zfb-wisdom.mdx`)

## Commands

All commands run from the **repo root** with the `--filter docs` workspace flag, or directly inside `docs/`:

- `pnpm docs:dev` — zfb dev server (port 4321)
- `pnpm docs:build` — static HTML export to `docs/dist/`
- `pnpm docs:check` — zfb type checking (runs tsc --noEmit on collection schemas + pages)
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
- Sidebar category label comes from the `index.mdx` frontmatter in each directory (`_category_.json` files are ignored and emit a benign build warning)

### Links

- Use **relative `.mdx` paths** for cross-doc links: `[label](../other-dir/page.mdx)` — zfb's `resolveMarkdownLinks` converts these to route URLs at build time.
- For links to pages that don't exist in the current locale collection (e.g., JA docs pointing to EN-only sections), use bare relative paths without `.mdx` (e.g., `../markdown-features/`) or absolute paths (e.g., `/docs/recipes/admonitions`); zfb only rewrites `.md`/`.mdx` relative links.

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
