# Changelog

> **Newer releases:** see https://takazudomodular.com/pj/zudo-front-builder/docs/changelog/ for v0.1.0-next.5 and later. Entries below are historical (kept for npm readers).

## Unreleased

### Performance

- **paths() memo** — `createPageRouter` now evaluates each dynamic route's `paths()` export exactly once per router instance. Previously, both the `GET /__paths__/<route>` build-pipeline call and every subsequent page render request invoked `paths()` independently, producing O(N²) work for an N-page dynamic route. The memo is shared between the `/__paths__` handler and the per-page render handler; rejections are not cached so a transient paths() failure retries on the next request. Production SSR isolates evaluate `paths()` once per isolate, matching the build-time-enumerator contract; dev mode is safe because each file-save creates a fresh router instance with a clean memo. (#974)

## 0.1.0-next.4

Version bump for lockstep release. No API changes in `zfb-runtime` itself. Note: the content-snapshot flow fix (#442) touched `packages/zfb/src/content.ts`, which affects how the CLI calls `setContentSnapshot` — but the `zfb-runtime` API surface is unchanged.

## 0.1.0-next.1

Initial public prerelease on npm.

- JavaScript runtime for zfb static sites: Hono-backed page router, content snapshots, client-side hydration.
- Subpath exports: `snapshot`, `client-router`.
