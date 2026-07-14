#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";

const REQUIRED_PACKED_FILES = [
  "package/dist/index.js",
  "package/dist/index.d.ts",
  "package/dist/browser.js",
  "package/dist/wasm/zfb_md_wasm_glue.zfb-resource.mjs",
  "package/dist/wasm/zfb_md_wasm_glue.zfb-resource.d.mts",
  "package/dist/wasm/zfb_md_wasm_bg.wasm",
];

const REQUIRED_WASM_FILES = new Set(
  REQUIRED_PACKED_FILES.filter((path) => path.startsWith("package/dist/wasm/")),
);
const REQUIRED_RUNTIME_RESOURCE_FILES = new Set(
  [...REQUIRED_WASM_FILES].filter(
    (path) => path.endsWith(".zfb-resource.mjs") || path.endsWith(".wasm"),
  ),
);
const ALLOWED_WASM_DECLARATION_FILES = new Set(["package/dist/wasm/zfb_md_wasm_bg.wasm.d.ts"]);

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

  const actualWasmFiles = paths.filter(
    (path) => path.startsWith("package/dist/wasm/") && !path.endsWith("/"),
  );
  const unexpected = actualWasmFiles.filter(
    (path) => !REQUIRED_WASM_FILES.has(path) && !ALLOWED_WASM_DECLARATION_FILES.has(path),
  );
  if (unexpected.length > 0) {
    throw new Error(
      `packed @takazudo/zfb-md-wasm has unexpected wasm resources: ${unexpected.join(", ")}`,
    );
  }

  const runtimeResources = actualWasmFiles.filter(
    (path) => path.endsWith(".zfb-resource.mjs") || path.endsWith(".wasm"),
  );
  if (
    runtimeResources.length !== REQUIRED_RUNTIME_RESOURCE_FILES.size ||
    runtimeResources.some((path) => !REQUIRED_RUNTIME_RESOURCE_FILES.has(path))
  ) {
    throw new Error(
      `packed @takazudo/zfb-md-wasm has duplicate runtime resources: ${runtimeResources.join(", ")}`,
    );
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
