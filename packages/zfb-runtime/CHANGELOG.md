# Changelog

## 0.1.0-next.4

Version bump for lockstep release. No API changes in `zfb-runtime` itself. Note: the content-snapshot flow fix (#442) touched `packages/zfb/src/content.ts`, which affects how the CLI calls `setContentSnapshot` — but the `zfb-runtime` API surface is unchanged.

## 0.1.0-next.1

Initial public prerelease on npm.

- JavaScript runtime for zfb static sites: Hono-backed page router, content snapshots, client-side hydration.
- Subpath exports: `snapshot`, `client-router`.
