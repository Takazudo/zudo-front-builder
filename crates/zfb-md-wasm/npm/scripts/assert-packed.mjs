#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";

const REQUIRED_PACKED_FILES = [
  "package/dist/index.js",
  "package/dist/index.d.ts",
  "package/dist/mdast.js",
  "package/dist/mdast.d.ts",
  "package/dist/browser.js",
  "package/dist/wasm/zfb_md_wasm_glue.zfb-resource.mjs",
  "package/dist/wasm/zfb_md_wasm_glue.zfb-resource.d.mts",
  "package/dist/wasm/zfb_md_wasm_bg.wasm",
  // The highlight-only artifact (zfb#1849, epic zfb#1845) -- served by the
  // `./highlight` export subpath, built with the `pipeline` Cargo feature
  // off.
  "package/dist/highlight.js",
  "package/dist/highlight.d.ts",
  "package/dist/highlight-browser.js",
  "package/dist/wasm-highlight/zfb_md_wasm_highlight_glue.zfb-resource.mjs",
  "package/dist/wasm-highlight/zfb_md_wasm_highlight_glue.zfb-resource.d.mts",
  "package/dist/wasm-highlight/zfb_md_wasm_highlight_bg.wasm",
];

// Two independently closed-set wasm resource directories -- `dist/wasm/`
// (default artifact) and `dist/wasm-highlight/` (highlight-only artifact).
// Each gets its own required/allowed/duplicate-runtime-resource check below
// so a resource leaking into (or missing from) either directory fails
// closed, exactly as the single-directory check did before #1849.
const WASM_RESOURCE_SETS = [
  {
    prefix: "package/dist/wasm/",
    allowedDeclarationFiles: new Set(["package/dist/wasm/zfb_md_wasm_bg.wasm.d.ts"]),
  },
  {
    prefix: "package/dist/wasm-highlight/",
    allowedDeclarationFiles: new Set([
      "package/dist/wasm-highlight/zfb_md_wasm_highlight_bg.wasm.d.ts",
    ]),
  },
];

for (const set of WASM_RESOURCE_SETS) {
  set.requiredFiles = new Set(REQUIRED_PACKED_FILES.filter((path) => path.startsWith(set.prefix)));
  set.requiredRuntimeResourceFiles = new Set(
    [...set.requiredFiles].filter(
      (path) => path.endsWith(".zfb-resource.mjs") || path.endsWith(".wasm"),
    ),
  );
}

export function packedPaths(archivePath) {
  return execFileSync("tar", ["-tzf", archivePath], { encoding: "utf8" })
    .split("\n")
    .filter(Boolean);
}

/**
 * Fail closed on a missing entry or any second runtime resource. This checks
 * the actual packed tarball, not merely the source tree or dist directory.
 */
export function assertPackedContents(paths) {
  const files = new Set(paths);
  const missing = REQUIRED_PACKED_FILES.filter((path) => !files.has(path));
  if (missing.length > 0) {
    throw new Error(`packed @takazudo/zfb-md-wasm is missing: ${missing.join(", ")}`);
  }

  for (const set of WASM_RESOURCE_SETS) {
    const actualFiles = paths.filter((path) => path.startsWith(set.prefix) && !path.endsWith("/"));
    const unexpected = actualFiles.filter(
      (path) => !set.requiredFiles.has(path) && !set.allowedDeclarationFiles.has(path),
    );
    if (unexpected.length > 0) {
      throw new Error(
        `packed @takazudo/zfb-md-wasm has unexpected resources under ${set.prefix}: ${unexpected.join(", ")}`,
      );
    }

    const runtimeResources = actualFiles.filter(
      (path) => path.endsWith(".zfb-resource.mjs") || path.endsWith(".wasm"),
    );
    if (
      runtimeResources.length !== set.requiredRuntimeResourceFiles.size ||
      runtimeResources.some((path) => !set.requiredRuntimeResourceFiles.has(path))
    ) {
      throw new Error(
        `packed @takazudo/zfb-md-wasm has duplicate runtime resources under ${set.prefix}: ${runtimeResources.join(", ")}`,
      );
    }
  }
}

function isMain() {
  const argument = process.argv[1];
  return argument !== undefined && import.meta.url === pathToFileURL(argument).href;
}

if (isMain()) {
  const archivePath = process.argv[2];
  if (!archivePath) {
    throw new Error("usage: node scripts/assert-packed.mjs <package.tgz>");
  }
  assertPackedContents(packedPaths(archivePath));
  console.log("packed @takazudo/zfb-md-wasm resource layout is valid");
}
