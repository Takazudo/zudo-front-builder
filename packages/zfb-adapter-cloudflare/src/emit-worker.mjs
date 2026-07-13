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

import { copyFile, lstat, mkdir, readFile, writeFile } from "node:fs/promises";
import { basename, dirname, join, resolve } from "node:path";

// `.assetsignore` tells the Workers Static Assets uploader (and Pages'
// asset server) to skip these two files, so they are only ever reachable
// through the Worker's module graph, not served as public static assets.
const ASSETS_IGNORE_BASE_ENTRIES = ["_worker.js", "_zfb_inner.mjs"];
const RESERVED_OUTPUT_NAMES = new Set([...ASSETS_IGNORE_BASE_ENTRIES, ".assetsignore"]);

function resolveAssets(inputBundlePath, assetPaths) {
  const inputDir = dirname(inputBundlePath);
  const names = new Set();

  return assetPaths.map((assetPath) => {
    const sourcePath = resolve(inputDir, assetPath);
    const outputName = basename(sourcePath);
    if (!outputName || outputName === "." || outputName === "..") {
      throw new Error("asset path has no valid basename: " + assetPath);
    }
    if (RESERVED_OUTPUT_NAMES.has(outputName)) {
      throw new Error("asset basename collides with generated adapter output: " + outputName);
    }
    if (names.has(outputName)) {
      throw new Error("asset basename collision: " + outputName);
    }
    names.add(outputName);
    return { sourcePath, outputName };
  });
}

async function pathExists(path) {
  try {
    await lstat(path);
    return true;
  } catch (error) {
    if (error && typeof error === "object" && error.code === "ENOENT") {
      return false;
    }
    throw error;
  }
}

async function readExistingAssetsIgnore(path) {
  try {
    return await readFile(path, "utf8");
  } catch (error) {
    if (error && typeof error === "object" && error.code === "ENOENT") {
      return "";
    }
    throw error;
  }
}

function mergeAssetsIgnore(existing, requiredEntries) {
  const listed = new Set(existing.split(/\r?\n/));
  const missing = requiredEntries.filter((entry) => !listed.has(entry));
  if (missing.length === 0) {
    return existing;
  }

  const prefix = existing.length > 0 && !existing.endsWith("\n") ? existing + "\n" : existing;
  return prefix + missing.map((entry) => entry + "\n").join("");
}

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
 * @param {{ inputBundlePath: string; outdir: string; assets?: readonly string[]; workerWrapperSource: string }} input
 * @returns {Promise<{ workerPath: string; innerBundlePath: string; assetsIgnorePath: string }>}
 */
export async function emitWorker({ inputBundlePath, outdir, assets = [], workerWrapperSource }) {
  const outdirAbs = resolve(outdir);
  const inputAbs = resolve(inputBundlePath);
  const resolvedAssets = resolveAssets(inputAbs, assets);

  await mkdir(outdirAbs, { recursive: true });
  for (const asset of resolvedAssets) {
    const sourceInfo = await lstat(asset.sourcePath);
    if (!sourceInfo.isFile()) {
      throw new Error("asset is not a file: " + asset.sourcePath);
    }
    const destination = join(outdirAbs, asset.outputName);
    if (await pathExists(destination)) {
      throw new Error(
        "asset basename collision: " + asset.outputName + " would overwrite " + destination,
      );
    }
  }

  const innerBundlePath = join(outdirAbs, "_zfb_inner.mjs");
  await copyFile(inputAbs, innerBundlePath);

  const workerPath = join(outdirAbs, "_worker.js");
  await writeFile(workerPath, workerWrapperSource, "utf8");

  const assetsIgnorePath = join(outdirAbs, ".assetsignore");
  const existingAssetsIgnore = await readExistingAssetsIgnore(assetsIgnorePath);
  for (const asset of resolvedAssets) {
    await copyFile(asset.sourcePath, join(outdirAbs, asset.outputName));
  }
  const assetsIgnore = mergeAssetsIgnore(existingAssetsIgnore, [
    ...ASSETS_IGNORE_BASE_ENTRIES,
    ...resolvedAssets.map((asset) => asset.outputName),
  ]);
  await writeFile(assetsIgnorePath, assetsIgnore, "utf8");

  return { workerPath, innerBundlePath, assetsIgnorePath };
}
