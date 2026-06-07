#!/usr/bin/env node
// Vendors @takazudo/zudo-doc's compiled dist into a Tailwind-scannable location
// so the CONSUMER generates the package's component utility classes. The WHY
// (context that lives outside the codebase):
//
//   @takazudo/zudo-doc ships NO precompiled CSS (`files: ["dist","README.md"]`).
//   Its components carry their styles as Tailwind class literals baked into the
//   compiled dist JS — inline on `class`/`className`, but also stashed in module
//   consts, ternary/`&&` branches, and backtick templates. The CONSUMER is
//   responsible for generating those utilities (in the zudo-doc monorepo they
//   are covered by `@source "packages/zudo-doc/src"`; a consumer cannot scan the
//   monorepo source).
//
//   Tailwind v4 deliberately EXCLUDES node_modules from class scanning — even a
//   bare `@source "node_modules/@takazudo/zudo-doc/dist/**"` emits zero package
//   utilities (proven: `.sticky` etc. are absent from the built CSS). So we copy
//   the dist to a non-node_modules path that Tailwind scans normally, and
//   `global.css` points `@source` at the COPY. Tailwind's own scanner then
//   extracts every class candidate from the dist — robust to every literal form
//   (const, ternary, backtick), which a hand-rolled token extractor was not.
//
//   The copy (docs/.zudo-doc-tw/) is a gitignored build artifact: regenerated on
//   every build/dev so it tracks dependency bumps automatically. Workaround for
//   the package shipping no CSS + Tailwind v4 node_modules scanning
//   (zudolab/zudo-doc#1971; #883).
//
// Run automatically before `zfb build`/`zfb dev` (chained with `&&` in the
// `build`/`dev` scripts — see package.json) so the copy is always fresh before
// Tailwind scans it. Chained directly rather than via a pre/post lifecycle hook
// so it runs regardless of the pnpm `enable-pre-post-scripts` setting.

import { cpSync, rmSync, existsSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const docsRoot = join(__dirname, "..");
const srcDist = join(docsRoot, "node_modules", "@takazudo", "zudo-doc", "dist");
// Must match the `@source` path in src/styles/global.css and the .gitignore entry.
const destDir = join(docsRoot, ".zudo-doc-tw");

if (!existsSync(srcDist)) {
  console.error(
    `vendor-zudo-doc-dist: package dist not found at ${srcDist} — is @takazudo/zudo-doc installed?`,
  );
  process.exit(1);
}

// Clean-copy so a downgraded/removed file in the package never lingers in the
// scanned copy and resurrects a stale class.
rmSync(destDir, { recursive: true, force: true });
cpSync(srcDist, destDir, { recursive: true });

console.log(
  `vendor-zudo-doc-dist: vendored @takazudo/zudo-doc/dist -> ${destDir.replace(docsRoot + "/", "")} (Tailwind-scanned for package classes)`,
);
