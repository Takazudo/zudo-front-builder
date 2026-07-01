// Structural guard for issue #1298.
//
// The client-facing root barrel (`src/index.ts`) must stay bundleable for
// `--platform=browser`. Its one poison hop was a *runtime value* re-export of
// `createPageRouter` from `./router.js` — that file imports `hono`, so the
// re-export dragged the Hono server router into any island that imported the
// bare `@takazudo/zfb-runtime`, and esbuild then had to resolve the
// transitively-buried `hono` (which pnpm does not hoist) → `zfb build` failed.
//
// The fix moved the factory to the server-only `@takazudo/zfb-runtime/server`
// subpath (`src/server.ts`). `export type { ... } from "./router.js"` is fine
// — esbuild strips type-only re-exports and no runtime edge survives. A bare
// `export { ... } from "./router.js"` is NOT fine — it is a runtime edge.
//
// This is a Level-1 structural test: it reads the source text rather than
// bundling, so it always runs (no esbuild needed) and fails loudly the moment
// someone reintroduces the runtime re-export. The behavioral proof that the
// exclusion actually holds under esbuild lives in the Rust Level-3 test
// `crates/zfb-islands/tests/server_router_excluded_from_client_bundle.rs`.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const indexSource = readFileSync(new URL("../index.ts", import.meta.url), "utf8");

// A runtime value re-export: `export { ... } from "./router.js"` — but NOT
// `export type { ... }`. The negative lookahead after `export` rejects the
// `type` keyword so type-only re-exports are allowed through.
const RUNTIME_REEXPORT_FROM_ROUTER =
  /export\s+(?!type\b)\{[^}]*\}\s*from\s*["']\.\/router(?:\.js)?["']/;

// A type-only re-export: `export type { ... } from "./router.js"`.
const TYPE_REEXPORT_FROM_ROUTER = /export\s+type\s*\{[^}]*\}\s*from\s*["']\.\/router(?:\.js)?["']/;

describe("root barrel (src/index.ts) — client-safe (issue #1298)", () => {
  it("does NOT runtime-re-export any value from ./router.js", () => {
    expect(indexSource).not.toMatch(RUNTIME_REEXPORT_FROM_ROUTER);
  });

  it("does NOT mention createPageRouter as a runtime export", () => {
    // Belt-and-suspenders: even a differently-shaped runtime re-export of the
    // factory (e.g. an aliased `export { createPageRouter as x }`) is banned.
    // `createPageRouter` may still appear inside a doc comment referencing the
    // /server subpath, so scope the check to `export {`-form statements only.
    const runtimeExportOfFactory =
      /export\s+(?!type\b)\{[^}]*\bcreatePageRouter\b[^}]*\}\s*from\s*["']\.\/router(?:\.js)?["']/;
    expect(indexSource).not.toMatch(runtimeExportOfFactory);
  });

  it("still type-re-exports the page-router types from ./router.js", () => {
    // The types carry no runtime, so keeping them on the root barrel is the
    // whole point — consumers still `import type { PageRouter } from
    // "@takazudo/zfb-runtime"`. This asserts the fix did not over-reach and
    // strip the type surface too.
    expect(indexSource).toMatch(TYPE_REEXPORT_FROM_ROUTER);
  });
});
