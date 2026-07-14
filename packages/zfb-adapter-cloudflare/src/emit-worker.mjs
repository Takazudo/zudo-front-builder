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

import { copyFile, lstat, mkdir, readFile, realpath, stat, writeFile } from "node:fs/promises";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";

// `.assetsignore` tells the Workers Static Assets uploader to skip the
// generated JavaScript files. Every copied Wasm basename is appended later,
// so it too is reachable only through the Worker's module graph and never
// served as a public static asset.
const ASSETS_IGNORE_BASE_ENTRIES = ["_worker.js", "_zfb_inner.mjs"];
const RESERVED_OUTPUT_NAMES = new Set([...ASSETS_IGNORE_BASE_ENTRIES, ".assetsignore"]);

function isPathWithin(parent, candidate) {
  const pathFromParent = relative(parent, candidate);
  return (
    pathFromParent === "" ||
    (!pathFromParent.startsWith(`..${sep}`) &&
      pathFromParent !== ".." &&
      !isAbsolute(pathFromParent))
  );
}

async function resolveAssets(inputBundlePath, assetPaths) {
  const inputDir = dirname(inputBundlePath);
  const canonicalInputDir = await realpath(inputDir);
  const names = new Set();
  const resolvedAssets = [];

  for (const assetPath of assetPaths) {
    if (typeof assetPath !== "string" || isAbsolute(assetPath)) {
      throw new Error("asset path must be bundle-relative: " + String(assetPath));
    }
    const sourcePath = resolve(inputDir, assetPath);
    if (!isPathWithin(inputDir, sourcePath)) {
      throw new Error("asset path escapes the input bundle directory: " + assetPath);
    }

    const sourceInfo = await stat(sourcePath);
    if (!sourceInfo.isFile()) {
      throw new Error("asset is not a file: " + sourcePath);
    }
    const canonicalSourcePath = await realpath(sourcePath);
    if (!isPathWithin(canonicalInputDir, canonicalSourcePath)) {
      throw new Error("asset path resolves outside the input bundle directory: " + assetPath);
    }

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
    resolvedAssets.push({ sourcePath: canonicalSourcePath, outputName });
  }

  return resolvedAssets;
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
 * Emit a Cloudflare Workers Static Assets `_worker.js` that wraps the zfb
 * input bundle. Cloudflare Pages advanced mode is unverified.
 *
 * Output shape (two generated JavaScript files, `.assetsignore`, and zero or
 * more copied Wasm assets in `outdir`):
 *
 *   _worker.js       — Worker entry point (`main` in wrangler.toml)
 *   _zfb_inner.mjs   — the input bundle, copied verbatim
 *   <asset>.wasm     — each bundle-relative Wasm input, copied by basename
 *   .assetsignore    — excludes every generated JavaScript and Wasm basename
 *                      from the asset upload
 *
 * @param {{ inputBundlePath: string; outdir: string; assets?: readonly string[]; workerWrapperSource: string }} input
 * @returns {Promise<{ workerPath: string; innerBundlePath: string; assetsIgnorePath: string }>}
 */
export async function emitWorker({ inputBundlePath, outdir, assets = [], workerWrapperSource }) {
  const outdirAbs = resolve(outdir);
  const inputAbs = resolve(inputBundlePath);
  const resolvedAssets = await resolveAssets(inputAbs, assets);

  await mkdir(outdirAbs, { recursive: true });
  for (const asset of resolvedAssets) {
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
