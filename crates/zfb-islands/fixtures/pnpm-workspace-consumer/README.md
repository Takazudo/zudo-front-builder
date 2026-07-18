# pnpm-workspace-consumer fixture

Regression fixture for issue #122 / #117. A consumer project's `pages/`
imports a workspace package by its scoped name. The package's source
`.tsx` file carries `"use client"`.

The on-disk layout mirrors what pnpm produces in a real consumer:

- `pages/` — pages that import the workspace package by its scoped
  bare specifier.
- `workspace/zfb-blog-islands/` — the workspace package itself
  (`package.json`, `src/index.tsx`). Lives outside `node_modules/` so
  the `node_modules/<pkg>` entry can be a symlink to it.
- `workspace/zfb-blog-shared/` — a SECOND workspace sibling
  (`package.json`, `src/index.ts`) that `zfb-blog-islands/src/index.tsx`
  itself imports by its bare package name (issue #1703, Guard (a)
  fixture extension). This is the shape Guard (a) detects: a bare
  package-name import reached from INSIDE an island, as opposed to the
  page-level import of `zfb-blog-islands` above (which is the ordinary,
  fully-supported "consume a workspace package as your islands source"
  pattern and does not trip the guard).
- `node_modules/@takazudo/zfb-blog-islands` and
  `node_modules/@takazudo/zfb-blog-shared` — created at test runtime as
  symlinks to their respective `../../workspace/<pkg>` directories.
  Checked-in symlinks are awkward across OSes and git settings, so the
  integration test wires the symlinks up itself before scanning.

The scanner must:

1. Accept the bare specifier `@takazudo/zfb-blog-islands` (no
   short-circuit on bare specifiers).
2. Walk up from the importer's directory, find
   `node_modules/@takazudo/zfb-blog-islands/`, recognise the symlink
   shape as a workspace package, read `package.json` and honour its
   `source` field.
3. Reach `src/index.tsx`, detect `"use client"`, and yield one island
   for each exported component.

Without the fix from #122, scanning this fixture returned an empty set
— which downstream means production builds shipped `data-zfb-island`
markers in HTML but no `dist/assets/islands.js`, so every interactive
island was dead.

The codex-review follow-up on PR #125 narrowed the bare-specifier
probe to **workspace** packages only (symlinks pointing outside
`node_modules/`); regular installed dependencies like `preact` are now
skipped silently to avoid descending into framework code on every
build.
