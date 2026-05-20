# @takazudo/zfb-runtime

## 0.2.0-migration.0

BREAKING (semantic): `<ViewTransitions />` no longer injects a meta tag
or inline router script. Consumers must add
`@view-transition { navigation: auto; }` to their top-level stylesheet
to opt in to cross-document View Transitions. The previous injection
was incompatible with the spec and produced no visible transitions in
any browser; switching is a strict improvement. Mounts of
`<ViewTransitions />` continue to compile.

The `ViewTransitionsElement` type export is preserved. The function
signature is unchanged (`() => readonly ViewTransitionsElement[]`), so
existing host code that calls `ViewTransitions()` and spreads the
return into JSX continues to typecheck — it now spreads `[]`, which is
a no-op in JSX.

## 0.1.0-migration.0

Initial pre-release. Public surface:

- `createPageRouter(options) → PageRouter` — Hono-backed page router
  whose returned fetch handler is shape-compatible with the Cloudflare
  Workers `(request) => Promise<Response>` model.
- `PageDefinition`, `PageModule`, `PageHeading`, `PageRouter`,
  `FrameworkAdapter`, `ContentSnapshot`, `EntrySnapshot` — the public
  type surface consumed by zfb's build host.
- `@takazudo/zfb-runtime/snapshot` — `setContentSnapshot` /
  `getContentSnapshot` helpers, idempotent across rebuilds.
- `@takazudo/zfb-runtime/client-router` — client-side router used by the
  in-browser hydration runtime.

Package is workspace-internal pending the first public npm publish (see
release-day checklist in the repo root).
