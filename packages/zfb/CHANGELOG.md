# @takazudo/zfb

## 0.0.0

Initial pre-release. Package is workspace-internal pending the first
public npm publish (see release-day checklist in the repo root).

Public surface at this point:

- `<Island when="load|idle|visible">` — JSX wrapper that marks a region
  for client-side hydration. JSX-runtime-agnostic (works under preact or
  react).
- `scheduleHydrate(target, when, fire)` — the runtime branching helper
  used by the hydration runtime, re-exported from `@takazudo/zfb/runtime`.
- `When`, `WHEN_VALUES`, `DEFAULT_WHEN`, `isWhen`, `resolveWhen` — type
  and runtime utilities pinning the spelling of the three hydration modes.
- `getCollection(name)`, `parseFrontmatter(raw)` — content collection
  helpers exported from `@takazudo/zfb/content`. `parseFrontmatter` is
  public so consumers can reuse the v0 frontmatter parser when writing
  custom content loaders.
- `defaultComponents` plus the eleven named per-element overrides
  (`ContentParagraph`, `ContentLink`, …) for MDX rendering.
- `paginate(items, opts)`, `PaginatedPage<T>`, `PaginateRoute<T>` —
  pagination helpers exported from `@takazudo/zfb/paginate`.
- `defineConfig(config)` — config helper exported from
  `@takazudo/zfb/config` for the recommended `zfb.config.ts` form.

See <https://takazudomodular.com/pj/zudo-front-builder/> for the full
documentation.
