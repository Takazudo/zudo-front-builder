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

The package is **published source-first**: every entry in `package.json`
`exports` points at a raw `./src/*.ts` file rather than a built artifact.
There is no `build` script, no `dist/` directory, and `prepublishOnly`
does not run a compiler.

```jsonc
"exports": {
  ".":         { "types": "./src/index.ts",   "default": "./src/index.ts" },
  "./runtime": { "types": "./src/runtime.ts", "default": "./src/runtime.ts" },
  "./content": { "types": "./src/content.ts", "default": "./src/content.ts" },
  "./paginate":{ "types": "./src/paginate.ts","default": "./src/paginate.ts" },
  "./config":  { "types": "./src/config.ts",  "default": "./src/config.ts"  },
}
```

That choice is intentional — the package's only first-party consumers today
are workspace siblings (the `examples/basic-blog` dogfood site, and the
zfb runtime itself once it lands), and the zfb dev pipeline strips TS at
load time. So the workspace pays no cost for raw `.ts` consumption.

If/when `zfb` is published to a public registry for non-workspace consumers,
a few things must change first — capture them here as a checklist:

- [ ] Add a `build` script (likely `tsc -p tsconfig.build.json` emitting
      to `./dist/`).
- [ ] Repoint each `exports` entry to its `./dist/` counterpart (keep
      `"types"` on the matching `.d.ts`).
- [ ] Drop `private: true` from `package.json` (or scope the registry
      accordingly).
- [ ] Decide on a `prepublishOnly` that runs the build + the test suite.
- [ ] Bump the version to a real semver.

Until that work happens, keep the source-first stance: `exports` should
keep pointing at `./src/*.ts` so workspace siblings type-check and run
without an extra build step in the dev loop.

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
