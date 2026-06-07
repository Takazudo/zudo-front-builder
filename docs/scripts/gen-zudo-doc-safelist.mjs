#!/usr/bin/env node
// Generates the complete Tailwind safelist for @takazudo/zudo-doc's component
// utility classes — the WHY for this whole mechanism (context that lives
// outside the codebase):
//
//   @takazudo/zudo-doc ships NO precompiled CSS (`files: ["dist","README.md"]`).
//   Its components carry their styles as STATIC Tailwind class literals baked
//   into the compiled dist JS, e.g. dist/header/header.js:
//     class: "sticky top-0 z-50 flex h-[3.5rem] items-center ..."
//   The CONSUMER (this site) is responsible for generating those utilities.
//   In the zudo-doc monorepo the classes are covered by
//   `@source "packages/zudo-doc/src"`; a consumer cannot scan the monorepo
//   source, so it must derive the classes from the published dist instead.
//
//   The bare `@source "node_modules/@takazudo/zudo-doc/dist/**"` directive is
//   NOT reliable under Tailwind v4 — its node_modules candidate scanning drops
//   plain utilities intermittently (e.g. `sticky`, `top-0`, `z-50`,
//   `antialiased`, `grid-cols-1`), and the masking effect of a `.git` checkout
//   means a non-git CI build drops far more. A hand-maintained `@source
//   inline(...)` list (the previous approach) froze at 46 entries and missed 83
//   of the package's classes — the #883 visual breakage.
//
//   This script extracts EVERY class token from the package dist on each build
//   and emits an `@source inline("…")` partial that global.css @imports, so the
//   safelist is complete and stays correct across dependency bumps (it derives
//   from the installed dist, not a frozen hand list).
//
// Run automatically before `zfb build` and `zfb dev` (chained with `&&` in the
// `build` and `dev` scripts — see package.json) so global.css always @imports
// an up-to-date safelist. Chained directly rather than via a pre/post lifecycle
// hook so it runs regardless of the pnpm `enable-pre-post-scripts` setting.

import { readFileSync, writeFileSync, readdirSync, statSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const docsRoot = join(__dirname, "..");
const distDir = join(docsRoot, "node_modules", "@takazudo", "zudo-doc", "dist");
const outFile = join(docsRoot, "src", "styles", "_zudo-doc-safelist.css");

/** Recursively collect every .js / .mjs file under the package dist. */
function collectFiles(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    const st = statSync(full);
    if (st.isDirectory()) {
      out.push(...collectFiles(full));
    } else if (name.endsWith(".js") || name.endsWith(".mjs")) {
      out.push(full);
    }
  }
  return out;
}

// Matches both compiled-JSX object literals (`class: "…"` / `className: "…"`)
// and template/raw-HTML attributes (`class="…"` / `className="…"`). The class
// payload is captured from the double-quoted value.
const CLASS_LITERAL_RE = /\bclass(?:Name)?\s*[:=]\s*"([^"]*)"/g;

function extractTokens(files) {
  const tokens = new Set();
  for (const file of files) {
    const src = readFileSync(file, "utf8");
    let m;
    while ((m = CLASS_LITERAL_RE.exec(src)) !== null) {
      for (const tok of m[1].split(/\s+/)) {
        const t = tok.trim();
        // Skip empties and any token carrying a template-interpolation marker
        // (`${…}`) or a brace — those are dynamic, not static utilities.
        if (!t || t.includes("${") || t.includes("{")) continue;
        tokens.add(t);
      }
    }
  }
  return [...tokens].sort();
}

const files = collectFiles(distDir);
const tokens = extractTokens(files);

const header = `/* AUTO-GENERATED — DO NOT EDIT.
 * Source: docs/scripts/gen-zudo-doc-safelist.mjs
 * Regenerated on every build/dev (chained in package.json \`build\`/\`predev\`)
 * from node_modules/@takazudo/zudo-doc/dist — see that script for the WHY.
 * ${tokens.length} class tokens extracted from the package dist. */
`;

const body = tokens.map((t) => `@source inline("${t}");`).join("\n");

writeFileSync(outFile, `${header}\n${body}\n`, "utf8");

console.log(
  `gen-zudo-doc-safelist: wrote ${tokens.length} tokens from ${files.length} dist files -> ${outFile.replace(docsRoot + "/", "")}`,
);
