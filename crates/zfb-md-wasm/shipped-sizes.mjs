import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const MANIFEST_PATH = resolve(dirname(fileURLToPath(import.meta.url)), "shipped-sizes.json");
const MEASURED_KEYS = ["finalWasm", "gzip9", "glue", "glueGzip9"];
const ARTIFACT_KEYS = ["root", "highlight", "render", "parse"];

function fail(message) {
  throw new Error(`Invalid shipped-sizes.json: ${message}`);
}

function requireObject(value, path) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    fail(`${path} must be an object`);
  }
}

function requireKey(object, key, path) {
  if (!Object.hasOwn(object, key)) fail(`missing key ${path}.${key}`);
  return object[key];
}

function requireNonNegativeInteger(value, path) {
  if (!Number.isSafeInteger(value) || value < 0) {
    fail(`${path} must be a non-negative safe integer`);
  }
}

function loadManifest() {
  let manifest;
  try {
    manifest = JSON.parse(readFileSync(MANIFEST_PATH, "utf8"));
  } catch (error) {
    throw new Error(`Unable to read shipped-sizes.json: ${error.message}`, { cause: error });
  }

  requireObject(manifest, "manifest");
  const measuredOnVersion = requireKey(manifest, "measuredOnVersion", "manifest");
  if (typeof measuredOnVersion !== "string" || measuredOnVersion.length === 0) {
    fail("measuredOnVersion must be a non-empty string");
  }

  const measured = requireKey(manifest, "measured", "manifest");
  requireObject(measured, "measured");
  for (const artifact of ARTIFACT_KEYS) {
    const values = requireKey(measured, artifact, `measured`);
    requireObject(values, `measured.${artifact}`);
    for (const key of MEASURED_KEYS) {
      const value = requireKey(values, key, `measured.${artifact}`);
      requireNonNegativeInteger(value, `measured.${artifact}.${key}`);
    }
  }

  const ceilings = requireKey(manifest, "ceilings", "manifest");
  requireObject(ceilings, "ceilings");
  for (const key of [...ARTIFACT_KEYS, "tarball"]) {
    const value = requireKey(ceilings, key, "ceilings");
    requireNonNegativeInteger(value, `ceilings.${key}`);
  }

  return { measuredOnVersion, measured, ceilings };
}

// measuredOnVersion is the version the artifacts were measured on. It is deliberately NOT
// asserted against crates/zfb-md-wasm/npm/package.json's version: a version bump that does
// not change the artifacts must not turn the docs guard red. Only a real artifact change
// moves measured (via the budgets script's --update-manifest), and only then does the docs
// version label move.
export const SHIPPED_SIZES = loadManifest();
