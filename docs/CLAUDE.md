# Docs

Documentation site built with [zudo-doc](https://github.com/zudolab/zudo-doc) — an Astro-based documentation framework with MDX, Tailwind CSS v4, and Preact islands.

## Tech Stack

- **Astro** — static site generator with Content Collections
- **MDX** — content format via `@astrojs/mdx`
- **Tailwind CSS v4** — via `@tailwindcss/vite` (not `@astrojs/tailwind`)
- **Preact** — for interactive islands only (with compat mode for React API)
- **Shiki** — built-in code highlighting

## Setup

- **zfb-wisdom Claude Code skill** — run `bash docs/scripts/setup-zfb-wisdom.sh` once to give AI agents lookup access to this documentation tree (see `src/content/docs/claude-skills/zfb-wisdom.mdx`)

## Commands

- `pnpm dev` — Astro dev server (port 4321)
- `pnpm build` — static HTML export to `dist/`
- `pnpm check` — Astro type checking

## Key Directories

```
src/
├── components/          # Astro + Preact components
│   └── admonitions/     # Note, Tip, Info, Warning, Danger
├── config/              # Settings, color schemes
├── content/
│   └── docs/            # MDX content
│   └── docs-ja/         # Japanese MDX content (mirrors docs/)
├── layouts/             # Astro layouts
├── pages/               # File-based routing
└── styles/
    └── global.css       # Design tokens & Tailwind config
```

## Content Conventions

### Frontmatter

- Required: `title` (string)
- Optional: `description`, `sidebar_position` (number), `category`
- Sidebar order is driven by `sidebar_position`

### Admonitions

Available in all MDX files without imports: `<Note>`, `<Tip>`, `<Info>`, `<Warning>`, `<Danger>`
Each accepts an optional `title` prop.

### Headings

Do NOT use h1 (`#`) in doc content — the page title from frontmatter is rendered as h1. Start content headings from h2 (`##`).

## Components

- Default to **Astro components** (`.astro`) — zero JS, server-rendered
- Use **Preact islands** (`client:load`) only when client-side interactivity is needed

## i18n

- English (default): `/docs/...` — content in `src/content/docs/`
- Japanese: `/ja/docs/...` — content in `src/content/docs-ja/`
- Japanese docs should mirror the English directory structure

## Active Settings Flags

The following flags are set in `src/config/settings.ts` and are currently enabled:

- **mermaid** — Renders Mermaid diagrams in MDX content (`true`)
- **sitemap** — Generates `sitemap.xml` via `@astrojs/sitemap` (`true`)
- **docMetainfo** — Shows document metadata (word count, reading time, etc.) below the title (`true`)
- **cjkFriendly** — Applies `remark-cjk-friendly` for better CJK line-breaking (`true`)
- **llmsTxt** — Generates `llms.txt` for LLM consumption (`true`)
- **docHistory** — Shows document edit history on each page (`true`)
- **sidebarResizer** — Draggable sidebar width (`true`)
- **sidebarToggle** — Show/hide desktop sidebar button (`true`)
- **claudeResources** — Auto-generated docs for Claude Code resources (skills, commands); value is `{ claudeDir: ".claude" }` (object, not a plain boolean)
