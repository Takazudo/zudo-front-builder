# Phase 2 Smoke Report — pnpm pack verification for all 3 TS packages

**Date:** 2026-05-21
**Branch:** npm-release-prep/pack-smoke
**Verified by:** subagent (issue #336)

---

## Summary

All three packages tarballed cleanly. `publishConfig` overrides applied correctly (main/types/exports all point at `./dist/...` in extracted package.json). All subpath imports resolved from a throwaway `npm install`. Adapter bin printed usage without errors.

**Phase 2 sign-off: PASS**

---

## Per-package: tarball details

### @takazudo/zfb

- **Tarball:** `takazudo-zfb-0.1.0-next.0.tgz`
- **Size:** 54,037 bytes
- **File count:** 44 (including package.json)
- **Extracted package.json:**
  - `main`: `./dist/index.js` ✓
  - `types`: `./dist/index.d.ts` ✓
  - `exports["."].default`: `./dist/index.js` ✓
  - All 7 subpath exports point at `./dist/*.js` ✓
  - No `./src/` references ✓

**File list:**

```
CHANGELOG.md
LICENSE
README.md
dist/config.d.ts  dist/config.d.ts.map  dist/config.js  dist/config.js.map
dist/content.d.ts  dist/content.d.ts.map  dist/content.js  dist/content.js.map
dist/frontmatter.d.ts  dist/frontmatter.d.ts.map  dist/frontmatter.js  dist/frontmatter.js.map
dist/index.d.ts  dist/index.d.ts.map  dist/index.js  dist/index.js.map
dist/island.d.ts  dist/island.d.ts.map  dist/island.js  dist/island.js.map
dist/jsx-types.d.ts  dist/jsx-types.d.ts.map  dist/jsx-types.js  dist/jsx-types.js.map
dist/paginate.d.ts  dist/paginate.d.ts.map  dist/paginate.js  dist/paginate.js.map
dist/plugins.d.ts  dist/plugins.d.ts.map  dist/plugins.js  dist/plugins.js.map
dist/runtime.d.ts  dist/runtime.d.ts.map  dist/runtime.js  dist/runtime.js.map
dist/types.d.ts  dist/types.d.ts.map  dist/types.js  dist/types.js.map
package.json
```

**Checks:**

- dist/ present with all subpath-export entries ✓
- README.md, CHANGELOG.md, LICENSE present ✓
- No src/ in tarball ✓
- No __tests__/, no *.test.ts, no tsconfig.json ✓

---

### @takazudo/zfb-runtime

- **Tarball:** `takazudo-zfb-runtime-0.1.0-next.0.tgz`
- **Size:** 55,522 bytes
- **File count:** 56 (including package.json)
- **Extracted package.json:**
  - `main`: `./dist/index.js` ✓
  - `types`: `./dist/index.d.ts` ✓
  - `exports["."].default`: `./dist/index.js` ✓
  - `exports["./snapshot"].default`: `./dist/snapshot.js` ✓
  - `exports["./client-router"].default`: `./dist/client-router/index.js` ✓
  - No `./src/` references ✓
  - `peerDependencies["@takazudo/zfb"]`: `0.1.0-next.0` (workspace:* correctly rewritten) ✓

**File list (selected):**

```
CHANGELOG.md  LICENSE  README.md
dist/client-router.d.ts  dist/client-router.js  (+ maps)
dist/client-router/cssesc.{d.ts,js}  (+ maps)
dist/client-router/events.{d.ts,js}  (+ maps)
dist/client-router/index.{d.ts,js}  (+ maps)
dist/client-router/prefetch.{d.ts,js}  (+ maps)
dist/client-router/router.{d.ts,js}  (+ maps)
dist/client-router/swap-functions.{d.ts,js}  (+ maps)
dist/client-router/types.{d.ts,js}  (+ maps)
dist/framework.{d.ts,js}  (+ maps)
dist/index.{d.ts,js}  (+ maps)
dist/router.{d.ts,js}  (+ maps)
dist/snapshot.{d.ts,js}  (+ maps)
dist/view-transitions.{d.ts,js}  (+ maps)
package.json
```

**Checks:**

- dist/ present with all subpath-export entries (including dist/client-router/index.js) ✓
- README.md, CHANGELOG.md, LICENSE present ✓
- No src/ in tarball ✓
- No __tests__/, no *.test.ts, no tsconfig.json ✓

---

### @takazudo/zfb-adapter-cloudflare

- **Tarball:** `takazudo-zfb-adapter-cloudflare-0.1.0-next.0.tgz`
- **Size:** 10,714 bytes
- **File count:** 15 (including package.json)
- **Extracted package.json:**
  - `main`: `./dist/index.js` ✓
  - `types`: `./dist/index.d.ts` ✓
  - `exports["."].default`: `./dist/index.js` ✓
  - `exports["./build"].default`: `./dist/build.js` ✓
  - `bin["zfb-adapter-cloudflare"]`: `./bin/cli.mjs` ✓
  - No `./src/` references in exports ✓

**File list:**

```
CHANGELOG.md  LICENSE  README.md
bin/cli.mjs
dist/build.d.ts  dist/build.d.ts.map  dist/build.js  dist/build.js.map
dist/index.d.ts  dist/index.d.ts.map  dist/index.js  dist/index.js.map
dist/worker-wrapper.mjs
package.json
src/worker-wrapper.mjs
```

**Checks:**

- dist/ present with all subpath-export entries ✓
- bin/cli.mjs present ✓
- src/worker-wrapper.mjs present (Option C — canonical source for cli.mjs import) ✓
- dist/worker-wrapper.mjs present (copied by build script) ✓
- README.md, CHANGELOG.md, LICENSE present ✓
- No other src/* files in tarball ✓
- No __tests__/, no *.test.ts, no tsconfig.json ✓

---

## Subpath import smoke test

**Install:** `npm install <zfb.tgz> <zfb-runtime.tgz> <zfb-adapter-cloudflare.tgz>` — 4 packages added, 0 vulnerabilities.

**index.mjs imports tested:**

| Import | Result |
|--------|--------|
| `@takazudo/zfb` | PASS |
| `@takazudo/zfb/runtime` | PASS |
| `@takazudo/zfb/content` | PASS |
| `@takazudo/zfb/paginate` | PASS |
| `@takazudo/zfb/config` | PASS |
| `@takazudo/zfb/plugins` | PASS |
| `@takazudo/zfb/frontmatter` | PASS |
| `@takazudo/zfb-runtime` | PASS |
| `@takazudo/zfb-runtime/snapshot` | PASS |
| `@takazudo/zfb-runtime/client-router` | PASS |
| `@takazudo/zfb-adapter-cloudflare` | PASS |
| `@takazudo/zfb-adapter-cloudflare/build` | PASS |

**`node index.mjs` output:** `all imports resolved` — no ERR_MODULE_NOT_FOUND, no ERR_PACKAGE_PATH_NOT_EXPORTED.

---

## Adapter bin smoke test

```
$ ./node_modules/.bin/zfb-adapter-cloudflare --help
Usage:
  zfb-adapter-cloudflare bundle <input> --outdir <dir>

Wrap an ESM bundle (the output of zfb-build's bundler) into a
Cloudflare Pages `_worker.js` placed under <dir>.

Options:
  --outdir <dir>    Output directory. Required.
  -h, --help        Show this help.
```

**Result: PASS** — bin shim resolved, usage printed, no ERR_MODULE_NOT_FOUND.

---

## Phase 2 sign-off

**PASS**

All three packages (`@takazudo/zfb`, `@takazudo/zfb-runtime`, `@takazudo/zfb-adapter-cloudflare`) tarball cleanly: `publishConfig` overrides applied correctly in every extracted `package.json`, all expected files are present, no source/test/config artifacts leaked, all 12 declared subpath imports resolve from a throwaway `npm install`, and the `zfb-adapter-cloudflare` bin shim works correctly. Phase 3 (changelog, versioning, publish-dry-run) is unblocked.
