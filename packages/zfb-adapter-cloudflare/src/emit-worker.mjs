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
// generated JavaScript files. Every copied Wasm basename is excluded too, so
// it is reachable only through the Worker's module graph and never served as
// a public static asset.
//
// Wrangler reads `<assets root>/.assetsignore` with the gitignore-style
// `ignore` (node-ignore) matcher, where the last matching rule wins. The
// generated exclusions are therefore appended as one final block of
// root-anchored literal rules, after every user rule, so a user negation such
// as `!_worker.js` cannot re-include a generated file.
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
    if (UNREPRESENTABLE_ASSETS_IGNORE_CHAR.test(outputName)) {
      throw new Error(
        "asset basename cannot be excluded literally in .assetsignore " +
          "(it contains CR, LF, or '?'); rename the asset: " +
          JSON.stringify(outputName),
      );
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

// The adapter never emits source maps (they are not published and would leak
// local source paths), so a trailing external `sourceMappingURL` reference in
// the copied bundle would dangle and make source-map-aware tools (Vite Workers
// integration) fail with ENOENT. Inline `data:` maps are self-contained and kept.
const TRAILING_EXTERNAL_SOURCE_MAP =
  /(?:\r?\n)?[ \t]*\/\/[#@][ \t]*sourceMappingURL=(?!data:)\S*\s*$/;

export function stripDanglingSourceMapReference(source) {
  const stripped = source.replace(TRAILING_EXTERNAL_SOURCE_MAP, "");
  if (stripped === source) {
    return source;
  }
  return stripped.endsWith("\n") || stripped.length === 0 ? stripped : stripped + "\n";
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

// CR/LF would split a line-oriented rule. node-ignore 5.x (Wrangler's
// matcher) turns even an escaped `\?` into a single-character wildcard, so a
// literal `?` cannot be excluded without also excluding neighboring files.
const UNREPRESENTABLE_ASSETS_IGNORE_CHAR = /[\r\n?]/;

// Root-anchored literal rule for one output basename. The leading `/` anchors
// the rule to the assets root and also neutralizes a leading `#` or `!`.
// `\`, `*`, `[` and `]` are backslash-escaped. Trailing whitespace is
// stripped by both Git and node-ignore unless quoted: a trailing space is
// written `\ `, and a trailing tab `[<tab>]`, because node-ignore rewrites an
// escaped `\<tab>` into a space. Interior spaces and tabs are already literal.
function literalAssetsIgnoreRule(name) {
  let trailingStart = name.length;
  while (
    trailingStart > 0 &&
    (name[trailingStart - 1] === " " || name[trailingStart - 1] === "\t")
  ) {
    trailingStart -= 1;
  }
  let rule = "/";
  for (let index = 0; index < name.length; index += 1) {
    const char = name[index];
    if (char === "\\" || char === "*" || char === "[" || char === "]") {
      rule += "\\" + char;
    } else if (index >= trailingStart && char === " ") {
      rule += "\\ ";
    } else if (index >= trailingStart && char === "\t") {
      rule += "[\t]";
    } else {
      rule += char;
    }
  }
  return rule;
}

// Preserve every existing line and its order, then append the generated block
// last so it wins. Earlier generated blocks are kept: assets copied by an
// earlier emission may still be in the output directory. The block is skipped
// only when the identical complete block already ends the file on a line
// boundary, so repeated emission does not grow the file.
function mergeAssetsIgnore(existing, requiredNames) {
  const block = requiredNames.map((name) => literalAssetsIgnoreRule(name) + "\n").join("");
  if (existing === block || existing.endsWith("\n" + block)) {
    return existing;
  }
  const prefix = existing.length > 0 && !existing.endsWith("\n") ? existing + "\n" : existing;
  return prefix + block;
}

/**
 * Emit a Cloudflare Workers Static Assets `_worker.js` that wraps the zfb
 * input bundle. Cloudflare Pages advanced mode is unverified.
 *
 * Output shape (two generated JavaScript files, `.assetsignore`, and zero or
 * more copied Wasm assets in `outdir`):
 *
 *   _worker.js       — Worker entry point (`main` in wrangler.toml)
 *   _zfb_inner.mjs   — the input bundle, copied with any trailing external
 *                      `sourceMappingURL` comment removed (no map is emitted)
 *   <asset>.wasm     — each bundle-relative Wasm input, copied by basename
 *   .assetsignore    — existing rules, then a final block of root-anchored
 *                      literal exclusions for every generated JavaScript and
 *                      Wasm basename
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
  const inputSource = await readFile(inputAbs, "utf8");
  await writeFile(innerBundlePath, stripDanglingSourceMapReference(inputSource), "utf8");

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
