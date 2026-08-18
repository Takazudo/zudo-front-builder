// Structural guard for issue #2437 — the client-router component/activation
// split.
//
// The root barrel (`src/index.ts`) must import `ClientRouter` from the pure
// `client-router-component.ts` module — never from the activation shim
// `client-router.ts`, which runs `init()` as a module-scope side effect —
// and `client-router-component.ts` must never import
// `client-router/router.js`. This is a Level-1 structural test: it reads
// source text rather than executing, so it always runs (default "node"
// environment) and fails loudly the moment either invariant regresses.
// Cloned from the `root-barrel-no-server-router.test.ts` pattern (#1298).
// The runtime behavioral proof (no listeners registered, no history writes)
// lives in `client-router-split.test.ts` (happy-dom).

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const indexSource = readFileSync(new URL("../index.ts", import.meta.url), "utf8");
const componentSource = readFileSync(
  new URL("../client-router-component.ts", import.meta.url),
  "utf8",
);

describe("root barrel (src/index.ts) — ClientRouter source (#2437)", () => {
  it("re-exports ClientRouter from the pure component module", () => {
    expect(indexSource).toMatch(
      /export\s*\{\s*ClientRouter\b[^}]*\}\s*from\s*["']\.\/client-router-component\.js["']/,
    );
  });

  it("does NOT re-export ClientRouter from the activation shim (./client-router.js)", () => {
    // Bounded so it does not false-match "./client-router-component.js" or
    // "./client-router/router.js" — only the exact shim path.
    const reexportFromShim =
      /export\s+(?!type\b)\{[^}]*\bClientRouter\b[^}]*\}\s*from\s*["']\.\/client-router(?:\.js)?["']/;
    expect(indexSource).not.toMatch(reexportFromShim);
  });
});

describe("client-router-component.ts — no router.js import (#2437)", () => {
  it("does not import ./client-router/router.js in any form", () => {
    expect(componentSource).not.toMatch(/from\s*["']\.\/client-router\/router(?:\.js)?["']/);
  });
});
