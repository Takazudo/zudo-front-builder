# Contributing to `zfb` (SDK package)

This file collects notes specific to the `zfb` npm package — the public
SDK that ships `Island`, the hydration runtime, and the v0 content/paginate
helpers. Repo-wide conventions live in the top-level `CONTRIBUTING.md`.

## Quick reference

```sh
pnpm --filter zfb test         # vitest, all suites
pnpm --filter zfb test:watch   # vitest in watch mode
pnpm --filter zfb typecheck    # tsc --noEmit, no emit
```

The package extends `../../tsconfig.base.json`. Override compiler options
locally only when the SDK genuinely needs them (e.g. `verbatimModuleSyntax`,
`exactOptionalPropertyTypes`, JSX-related toggles).

## Publishing notes — consumers outside the workspace

The package ships a **dual src / dist layout**. In the workspace, `exports`
points at raw `./src/*.ts` files so workspace siblings type-check and run
without a build step. On publish, `publishConfig` repoints every entry to
the compiled `./dist/` artifacts (`tsc` emitting `.js` + `.d.ts`).

**Development (workspace):**

```jsonc
"exports": {
  ".":            { "types": "./src/index.ts",      "default": "./src/index.ts" },
  "./runtime":    { "types": "./src/runtime.ts",    "default": "./src/runtime.ts" },
  "./content":    { "types": "./src/content.ts",    "default": "./src/content.ts" },
  "./paginate":   { "types": "./src/paginate.ts",   "default": "./src/paginate.ts" },
  "./config":     { "types": "./src/config.ts",     "default": "./src/config.ts" },
  "./plugins":    { "types": "./src/plugins.ts",    "default": "./src/plugins.ts" },
  "./frontmatter":{ "types": "./src/frontmatter.ts","default": "./src/frontmatter.ts" },
  "./slugify":    { "types": "./src/slugify.ts",    "default": "./src/slugify.ts" },
  "./package.json": "./package.json",
}
```

**Published (npm registry — `publishConfig` overrides the above):**

```jsonc
"publishConfig": {
  "main":  "./dist/index.js",
  "types": "./dist/index.d.ts",
  "exports": {
    ".":            { "types": "./dist/index.d.ts",      "default": "./dist/index.js" },
    "./runtime":    { "types": "./dist/runtime.d.ts",    "default": "./dist/runtime.js" },
    "./content":    { "types": "./dist/content.d.ts",    "default": "./dist/content.js" },
    "./paginate":   { "types": "./dist/paginate.d.ts",   "default": "./dist/paginate.js" },
    "./config":     { "types": "./dist/config.d.ts",     "default": "./dist/config.js" },
    "./plugins":    { "types": "./dist/plugins.d.ts",    "default": "./dist/plugins.js" },
    "./frontmatter":{ "types": "./dist/frontmatter.d.ts","default": "./dist/frontmatter.js" },
    "./slugify":    { "types": "./dist/slugify.d.ts",    "default": "./dist/slugify.js" },
    "./package.json": "./package.json",
  }
}
```

The `build` script (`tsc`) and the `prepublishOnly` hook (`pnpm build && pnpm test`)
are already wired in `package.json`. The `files` field ships `dist/`, `bin/`,
`README.md`, `CHANGELOG.md`, and `LICENSE` — raw `src/` is not included in the
published tarball.

`private: true` has been dropped; the package publishes to the public registry
under `publishConfig.access: "public"`.

## Bridge contract — `globalThis.__zfb.content`

`zfb/content` exposes a `Content` field on every `CollectionEntry`. At call
time, that component consults a small namespaced bridge installed by the
Rust-side `zfb-render` `Renderer` before each page module is evaluated:

```ts
declare global {
  var __zfb: {
    content: {
      // Returns a renderable component for the entry, or undefined when
      // the bridge isn't present (JS-only environments / unit tests).
      get(specifier: string): ((props: { components?: Record<string, unknown> }) => unknown) | undefined;
    };
  };
}
```

The bridge is keyed by `entry.module_specifier`, which the JS stub
constructs as `mdx://<collection>/<slug>` (no hash). The Rust-side
`zfb_content::collection::Entry::module_specifier` adds a `#<hash>` suffix
for cache addressing — the bridge resolver must accept either form.

**Fallback path.** When `globalThis.__zfb?.content?.get(specifier)` returns
`undefined` — or when the `__zfb` namespace is absent entirely (typical of
unit tests, dev sandboxes, and any non-renderer evaluation context) —
`Content` returns a JSX element wrapping the raw markdown body in
`<pre data-zfb-content-fallback>` with a leading `[zfb fallback render]`
marker line. The marker survives unstyled environments and doubles as a
grep target for "production renderer didn't run" diagnostics.

**Why a bridge.** Keeps `packages/zfb` runtime-agnostic: the SDK never
imports preact or react, and never has to know which JSX runtime the user
chose. The Rust renderer owns module evaluation, so it owns the namespace
that hands compiled-MDX components back to user code at the call site.

The contract is mirrored in JSDoc on `CollectionEntry.Content`
(`packages/zfb/src/content.ts`) and cross-referenced from
`crates/zfb-render/src/loader.rs` so the two halves stay in sync. When
either half changes, update both.

## Surface stability

The public surface is whatever `package.json` `exports` lists today. Adding
a new helper means:

1. Implement it in `src/<name>.ts` (kebab-case file).
2. Add a vitest suite under `src/__tests__/<name>.test.ts`.
3. Add the subpath to `exports` *and* to this file's reference table.
4. Mention the new entry point in the docs site (`docs/api/`).

Removing or renaming an existing helper is a breaking change and needs an
ADR — even before v1.0, the workspace's other crates depend on the names
through `import "zfb"` strings.

### Export reference table

| Subpath | Source file | Description |
|---------|-------------|-------------|
| `.` | `src/index.ts` | Re-exports all public symbols |
| `./runtime` | `src/runtime.ts` | Islands runtime (`mountIslands`, `mountNewIslands`) |
| `./content` | `src/content.ts` | Content collections (`getCollection`, `CollectionEntry`) |
| `./paginate` | `src/paginate.ts` | Pagination helper |
| `./config` | `src/config.ts` | Project config types |
| `./plugins` | `src/plugins.ts` | Plugin lifecycle types (`ZfbPlugin`, build/dev hooks) |
| `./frontmatter` | `src/frontmatter.ts` | Frontmatter schema helpers |
| `./slugify` | `src/slugify.ts` | Heading-slug parity helper (`slugify`, `SlugAllocator`) |
| `./package.json` | `package.json` | Package metadata for tooling |
