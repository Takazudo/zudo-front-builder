// Shared, dependency-free implementation of `emitWorker`.
//
// Kept as a plain `.mjs` module (no TypeScript) so it can be imported from
// both the typed TypeScript surface (`src/build.ts`) and the dependency-free
// CLI binary (`bin/cli.mjs`) without needing a TypeScript loader.
//
// Do NOT duplicate this logic elsewhere. The two consumers always stay in
// sync by importing from here.
//
// invariant: no runtime npm deps — see SECURITY-DEPS.md

import { copyFile, mkdir, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";

// `.assetsignore` tells the Workers Static Assets uploader (and Pages'
// asset server) to skip these two files, so they are only ever reachable
// through the Worker's module graph, not served as public static assets.
const ASSETS_IGNORE_CONTENT = "_worker.js\n_zfb_inner.mjs\n";

/**
 * Emit a Cloudflare Workers Static Assets (Pages-compatible) `_worker.js`
 * that wraps the zfb input bundle.
 *
 * Output shape (three files in `outdir`):
 *
 *   _worker.js       — Worker entry point (`main` in wrangler.toml, or the
 *                      Pages advanced-mode convention)
 *   _zfb_inner.mjs   — the input bundle, copied verbatim
 *   .assetsignore    — excludes the two files above from the asset upload
 *
 * @param {{ inputBundlePath: string; outdir: string; workerWrapperSource: string }} input
 * @returns {Promise<{ workerPath: string; innerBundlePath: string; assetsIgnorePath: string }>}
 */
export async function emitWorker({ inputBundlePath, outdir, workerWrapperSource }) {
  const outdirAbs = resolve(outdir);
  const inputAbs = resolve(inputBundlePath);

  await mkdir(outdirAbs, { recursive: true });
  const innerBundlePath = join(outdirAbs, "_zfb_inner.mjs");
  await copyFile(inputAbs, innerBundlePath);

  const workerPath = join(outdirAbs, "_worker.js");
  await writeFile(workerPath, workerWrapperSource, "utf8");

  const assetsIgnorePath = join(outdirAbs, ".assetsignore");
  await writeFile(assetsIgnorePath, ASSETS_IGNORE_CONTENT, "utf8");

  return { workerPath, innerBundlePath, assetsIgnorePath };
}
