#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { statSync } from "node:fs";
import { pathToFileURL } from "node:url";

export const MAX_PACKED_BYTES = 3_900_000;

const ENTRY_FILES = [
  "index.js",
  "index.d.ts",
  "browser.js",
  "browser.d.ts",
  "mdast.js",
  "mdast.d.ts",
  "highlight.js",
  "highlight.d.ts",
  "highlight-browser.js",
  "highlight-browser.d.ts",
  "render.js",
  "render.d.ts",
  "render-browser.js",
  "render-browser.d.ts",
  "parse.js",
  "parse.d.ts",
  "parse-browser.js",
  "parse-browser.d.ts",
].map((name) => `package/dist/${name}`);

export const WASM_RESOURCE_SETS = [
  { dirName: "wasm", stem: "zfb_md_wasm" },
  { dirName: "wasm-highlight", stem: "zfb_md_wasm_highlight" },
  { dirName: "wasm-render", stem: "zfb_md_wasm_render" },
  { dirName: "wasm-parse", stem: "zfb_md_wasm_parse" },
].map(({ dirName, stem }) => {
  const prefix = `package/dist/${dirName}/`;
  return {
    dirName,
    stem,
    prefix,
    requiredFiles: new Set([
      `${prefix}${stem}_glue.zfb-resource.mjs`,
      `${prefix}${stem}_glue.zfb-resource.d.mts`,
      `${prefix}${stem}_bg.wasm`,
      `${prefix}${stem}_bg.wasm.d.ts`,
    ]),
  };
});

export const REQUIRED_PACKED_FILES = [
  ...ENTRY_FILES,
  ...WASM_RESOURCE_SETS.flatMap((set) => [...set.requiredFiles]),
];

export function packedPaths(archivePath) {
  return execFileSync("tar", ["-tzf", archivePath], { encoding: "utf8" })
    .split("\n")
    .filter(Boolean);
}

/** Fail closed on missing, extra, duplicated, or cross-artifact resources. */
export function assertPackedContents(paths) {
  const files = new Set(paths);
  const missing = REQUIRED_PACKED_FILES.filter((path) => !files.has(path));
  if (missing.length > 0) {
    throw new Error(`packed @takazudo/zfb-md-wasm is missing: ${missing.join(", ")}`);
  }

  for (const set of WASM_RESOURCE_SETS) {
    const actualFiles = paths.filter((path) => path.startsWith(set.prefix) && !path.endsWith("/"));
    const unexpected = actualFiles.filter((path) => !set.requiredFiles.has(path));
    if (unexpected.length > 0) {
      throw new Error(
        `packed @takazudo/zfb-md-wasm has unexpected resources under ${set.prefix}: ${unexpected.join(", ")}`,
      );
    }
    if (actualFiles.length !== set.requiredFiles.size) {
      throw new Error(
        `packed @takazudo/zfb-md-wasm resource set is not closed under ${set.prefix}`,
      );
    }
  }

  const approvedResources = new Set(WASM_RESOURCE_SETS.flatMap((set) => [...set.requiredFiles]));
  const strayResources = paths.filter(
    (path) =>
      path.startsWith("package/dist/") &&
      (path.endsWith(".zfb-resource.mjs") || path.endsWith(".wasm")) &&
      !approvedResources.has(path),
  );
  if (strayResources.length > 0) {
    throw new Error(
      `packed @takazudo/zfb-md-wasm has unapproved runtime resources: ${strayResources.join(", ")}`,
    );
  }
}

export function assertPackedArchive(archivePath) {
  const packedBytes = statSync(archivePath).size;
  if (packedBytes > MAX_PACKED_BYTES) {
    throw new Error(
      `packed @takazudo/zfb-md-wasm archive is ${packedBytes} bytes; ceiling is ${MAX_PACKED_BYTES}`,
    );
  }
  assertPackedContents(packedPaths(archivePath));
  return packedBytes;
}

const argument = process.argv[1];
if (argument !== undefined && import.meta.url === pathToFileURL(argument).href) {
  const archivePath = process.argv[2];
  if (!archivePath) {
    throw new Error("usage: node scripts/assert-packed.mjs <package.tgz>");
  }
  const packedBytes = assertPackedArchive(archivePath);
  console.log(`packed @takazudo/zfb-md-wasm resource layout is valid (${packedBytes} bytes)`);
}
