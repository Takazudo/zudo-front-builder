# pnpm-workspace-consumer fixture

Regression fixture for issue #122 / #117. A consumer project's `pages/`
imports a workspace package by its scoped name. The package's source
`.tsx` file carries `"use client"` and lives under
`node_modules/@scope/pkg/src/index.tsx` — exactly the shape pnpm
materialises for a workspace dependency.

The scanner must:

1. Accept the bare specifier `@takazudo/zfb-blog-islands` (no
   short-circuit on bare specifiers).
2. Walk up from the importer's directory, find
   `node_modules/@takazudo/zfb-blog-islands/`, read `package.json` and
   honour its `source` field.
3. Reach `src/index.tsx`, detect `"use client"`, and yield one island
   for each exported component.

Without the fix from #122, scanning this fixture returns an empty set —
which downstream means production builds ship `data-zfb-island` markers
in HTML but no `dist/assets/islands.js`, so every interactive island is
dead.
