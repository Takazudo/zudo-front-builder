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

/**
 * Emit a Cloudflare Pages `_worker.js` that wraps the zfb input bundle.
 *
 * Output shape (two files in `outdir`):
 *
 *   _worker.js       — entry imported by Cloudflare Pages advanced mode
 *   _zfb_inner.mjs   — the input bundle, copied verbatim
 *
 * @param {{ inputBundlePath: string; outdir: string; workerWrapperSource: string }} input
 * @returns {Promise<{ workerPath: string; innerBundlePath: string }>}
 */
export async function emitWorker({ inputBundlePath, outdir, workerWrapperSource }) {
  const outdirAbs = resolve(outdir);
  const inputAbs = resolve(inputBundlePath);

  await mkdir(outdirAbs, { recursive: true });
  const innerBundlePath = join(outdirAbs, "_zfb_inner.mjs");
  await copyFile(inputAbs, innerBundlePath);

  const workerPath = join(outdirAbs, "_worker.js");
  await writeFile(workerPath, workerWrapperSource, "utf8");

  return { workerPath, innerBundlePath };
}
