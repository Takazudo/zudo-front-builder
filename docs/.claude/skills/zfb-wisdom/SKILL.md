---
name: zfb-wisdom
description: >-
  Search and reference documentation from the zfb (zudo-front-builder) project.
  Use when answering questions about zfb features, CLI usage, configuration,
  content collections, routing, build pipeline, or architecture decisions.
user-invocable: true
argument-hint: "[-u|--update] [topic keyword, e.g., 'routing', 'build-pipeline', 'content-collections']"
---

# zfb Documentation Reference

Look up documentation from the zfb (zudo-front-builder) project.
Documentation base path: `docs/src/content/docs` (relative to repo root)

## Mode Detection

Parse the argument string for flags:

- If args start with `-u` or `--update`: enter **Update mode** (see below)
- Otherwise: enter **Lookup mode** (default)

Strip the flag from the remaining argument to get the topic keyword.

## Lookup Mode (default)

1. Find the relevant article(s) from the `docs/` directory based on the topic
2. Read ONLY the specific article(s) you need — do NOT load all articles at once
3. For architecture questions, also check `architecture/` for ADR files
4. Apply the information from the article when answering the user's question
5. Mention the source article path so the user can find it for further reading

## Update Mode (`-u` / `--update`)

The user has new information and wants to add or update documentation in this repo.

### Workflow

1. **Understand the new info**: Ask the user what they learned or want to
   document. The topic keyword (if provided) hints at the subject area.
2. **Find existing docs**: Search the `docs/` directory for articles related to
   the topic. Read them to understand what is already covered.
3. **Decide create vs update**: If an existing article covers the topic, update
   it. Otherwise, create a new `.mdx` file in the appropriate subdirectory.
4. **Write the content**: Follow the doc-authoring rules in `docs/CLAUDE.md`:
   - Required frontmatter: `title` (string). Always set `sidebar_position`.
     Optional: `description`, `sidebar_label`, `tags`, etc.
   - Do NOT use `# h1` in content — the frontmatter `title` renders as h1.
     Start with `## h2` headings.
   - Use available MDX components (`<Note>`, `<Tip>`, `<Info>`, `<Warning>`,
     `<Danger>`) where appropriate.
5. **Update Japanese docs**: Create or update the corresponding file under
   `docs-ja/` mirroring the English directory structure. Keep code blocks and
   diagrams identical — only translate surrounding prose.
6. **Format**: Run `pnpm format:md` inside `docs/` to format the changed files.
7. **Verify**: Run `pnpm build` inside `docs/` to confirm the site builds correctly.

## Documentation Structure

The documentation is organized in MDX files under `docs/`:

```
- api/
- architecture/
- changelog/
- claude-skills/
- concepts/
- dogfood/
- getting-started/
- guides/
```

Architecture Decision Records (ADRs) are in `architecture/`.

Browse the `docs/` directory to discover available articles. Each `.mdx` file
has YAML frontmatter with `title` and `description` fields that help identify
the right article to read.

## Japanese Documentation

Japanese translations are available under `docs-ja/`. When the user is working
in Japanese or asks for Japanese content, prefer articles from `docs-ja/`.
