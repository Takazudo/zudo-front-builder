---
name: zudo-doc-translate
description: Translate zudo-doc documentation between English and Japanese with project-specific conventions.
triggers:
  - translate
  - 翻訳
  - i18n
  - en to ja
  - ja to en
  - translate docs
  - ドキュメント翻訳
---

# zudo-doc Translation Skill

Translate documentation between English and Japanese following project-specific conventions.

## i18n Structure

- English docs: `src/content/docs/` - routes at `/docs/...`
- Japanese docs: `src/content/docs-ja/` - routes at `/ja/docs/...`
- Directory structures generally mirror each other, except for sanctioned EN-only/fallback sections.
- Locale settings: `locales` in `src/config/settings.ts`.
- English is the default locale and is served without a locale prefix. Japanese uses the `/ja/` route prefix.
- Missing Japanese pages are resolved by the locale-first merge with English fallback; a missing `docs-ja` file is not automatically a bug.

## Sanctioned Mirror Exceptions

Do not create Japanese mirrors for these unless the user explicitly asks:

- `defaultLocaleOnlyPrefixes` in `src/config/settings.ts`: generated Claude-resource sections (`/docs/claude-md/`, `/docs/claude-skills/`, `/docs/claude-agents/`, `/docs/claude-commands/`) plus any explicitly listed EN-only parent sections.
- Changelog pages are EN-only. Do not create `src/content/docs-ja/changelog/**`; Japanese navigation should rely on the EN fallback for `/docs/changelog/`.

## Translation Rules

### Keep in English

- Component names: `<Note>`, `<Tip>`, `<Info>`, `<Warning>`, `<Danger>`, `<Tabs>`, `<TabItem>`, `<Details>`
- File paths: `src/content/docs/...`, `.claude/skills/...`, etc.
- CLI commands: `pnpm dev`, `pnpm build`, etc.
- Technical terms that are standard in English, such as component, props, frontmatter, and slug.
- Frontmatter field keys (`title`, `description`, `sidebar_position`, `category`).

### Code blocks

- Keep fenced code blocks structurally identical: do not alter commands, identifiers, paths, imports, string literals, output, indentation, or example behavior.
- Comments inside code blocks may be translated when they are prose comments and translating them does not change the example's behavior.

### Translate

- Frontmatter field values, such as the `title` value and the `description` value.
- The `title` prop of admonition components, such as `<Note title="注意">`.
- Prose content, headings, list items, and table cells, except as noted above.

### Table conventions

- In tables with a "Required" column, use **"Yes"** / **"No"** directly, not "はい" / "いいえ". Japanese conversational yes/no is unnatural in technical documentation.

### Internal links

- Use relative `.mdx` links when the target exists in both locale collections; zfb rewrites those at build time.
- For absolute route links to translated Japanese pages, use `/ja/docs/...`.
- For Japanese docs that intentionally link to an EN-only/fallback page, link to `/docs/...` and add the full-width suffix `（英語）` immediately after the link.
- Do not use bare relative route paths such as `../markdown-features/` for fallback pages. Use root-relative absolute paths so base rewriting is reliable.

## File Naming

- Japanese files usually use the same filenames as English, such as `writing-docs.mdx`.
- Only the parent directory differs: `docs/` vs `docs-ja/`, except for sanctioned EN-only/fallback sections.
- Example: `src/content/docs/guides/writing-docs.mdx` -> `src/content/docs-ja/guides/writing-docs.mdx`.

## Workflow

### En to Ja Translation

1. Read the English source file from `src/content/docs/`.
2. Check whether the target is covered by the sanctioned mirror exceptions.
3. If the corresponding Japanese file exists, read it first and update it from the English source rather than overwriting from scratch.
4. If it does not exist and is not a sanctioned exception, create the file at the equivalent path in `src/content/docs-ja/`.
5. Translate the content following the rules above.
6. Verify internal links point to the right target:
   - translated JA pages: relative `.mdx` links or `/ja/docs/...`
   - EN-only/fallback pages: `/docs/...` plus `（英語）`

### Ja to En Translation

1. Read the Japanese source file from `src/content/docs-ja/`.
2. Check if the corresponding English file already exists in `src/content/docs/`.
3. If it exists, read it first and update it from the Japanese source rather than overwriting from scratch.
4. If it does not exist, create the file at the equivalent path in `src/content/docs/`.
5. Translate the content following the rules above.
6. Verify internal links point to `/docs/...` or relative `.mdx` targets that resolve in the English collection.

### Post-Translation Checks

- Frontmatter keys are unchanged; only values are translated.
- Admonition component names remain in English.
- Code blocks preserve behavior; only prose comments may be translated.
- Internal links use the correct locale target and `（英語）` marker where needed.
- Missing mirrors are intentional only for the sanctioned exceptions above.
