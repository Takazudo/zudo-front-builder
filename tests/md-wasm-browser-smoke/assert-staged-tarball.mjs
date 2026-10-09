/**
 * Proves the fixture consumes the exact archive the wasm-md job packed. This
 * deliberately runs before `zfb build`: a source package path or hand-copied
 * glue/wasm files cannot satisfy both the tarball assertion and the installed
 * published manifest checks below.
 */

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, realpathSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  assertPackedContents,
  packedPaths,
} from "../../crates/zfb-md-wasm/npm/scripts/assert-packed.mjs";

const testRoot = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(testRoot, "..", "..");
const archive = resolve(repoRoot, "wasm-md-artifact", "zfb-md-wasm.tgz");
const installedPackage = resolve(testRoot, "node_modules", "@takazudo", "zfb-md-wasm");
const fixturePackage = resolve(
  testRoot,
  "fixture-site",
  "node_modules",
  "@takazudo",
  "zfb-md-wasm",
);

if (!existsSync(archive)) {
  throw new Error(`packed archive is missing: ${archive}`);
}
assertPackedContents(packedPaths(archive));

const installedManifestPath = resolve(installedPackage, "package.json");
if (!existsSync(installedManifestPath)) {
  throw new Error(`fixture did not install the packed package: ${installedManifestPath}`);
}
const installedManifest = JSON.parse(readFileSync(installedManifestPath, "utf8"));
if (installedManifest.name !== "@takazudo/zfb-md-wasm") {
  throw new Error(`unexpected staged package: ${installedManifest.name}`);
}
const installedArtifactManifestUrl = import.meta
  .resolve("@takazudo/zfb-md-wasm/shipped-artifacts.json");
if (!installedArtifactManifestUrl.startsWith("file:")) {
  throw new Error(
    `shipped artifact manifest resolved to a non-file URL: ${installedArtifactManifestUrl}`,
  );
}
const installedArtifactManifestPath = fileURLToPath(installedArtifactManifestUrl);
const expectedInstalledArtifactManifestPath = resolve(
  installedPackage,
  "dist",
  "shipped-artifacts.json",
);
if (
  realpathSync(installedArtifactManifestPath) !==
  realpathSync(expectedInstalledArtifactManifestPath)
) {
  throw new Error("shipped-artifacts.json export does not resolve inside the staged package");
}
const installedArtifactManifestBytes = readFileSync(installedArtifactManifestPath);
const packedArtifactManifestBytes = execFileSync("tar", [
  "-xOf",
  archive,
  "package/dist/shipped-artifacts.json",
]);
if (!installedArtifactManifestBytes.equals(packedArtifactManifestBytes)) {
  throw new Error(
    "installed shipped-artifacts.json differs from the manifest in the packed tarball",
  );
}
const shippedArtifactManifest = JSON.parse(installedArtifactManifestBytes.toString("utf8"));
if (
  shippedArtifactManifest.schemaVersion !== 1 ||
  shippedArtifactManifest.name !== installedManifest.name ||
  shippedArtifactManifest.version !== installedManifest.version ||
  !Array.isArray(shippedArtifactManifest.artifacts) ||
  shippedArtifactManifest.artifacts.length !== 4
) {
  throw new Error("installed shipped-artifacts.json has an invalid package identity or schema");
}
if (
  installedManifest.main !== "./dist/index.js" ||
  installedManifest.exports?.["."]?.browser !== "./dist/browser.js"
) {
  throw new Error("staged package does not carry the published direct/browser root entry contract");
}
for (const relativePath of [
  "dist/browser.js",
  "dist/wasm/zfb_md_wasm_glue.zfb-resource.mjs",
  "dist/wasm/zfb_md_wasm_bg.wasm",
]) {
  if (!existsSync(resolve(installedPackage, relativePath))) {
    throw new Error(`installed packed package is missing ${relativePath}`);
  }
}

const sourcePackage = resolve(repoRoot, "crates", "zfb-md-wasm", "npm");
if (realpathSync(installedPackage) === realpathSync(sourcePackage)) {
  throw new Error("browser fixture resolved the source package instead of the packed tarball");
}
if (!existsSync(fixturePackage)) {
  throw new Error(`fixture project node_modules does not expose packed md-wasm: ${fixturePackage}`);
}
if (realpathSync(fixturePackage) !== realpathSync(installedPackage)) {
  throw new Error(
    "fixture project node_modules does not resolve md-wasm from the asserted lane package",
  );
}
for (const relativePath of [
  "@takazudo/zfb/package.json",
  "@takazudo/zfb-runtime/package.json",
  "hono/package.json",
]) {
  const frameworkPackage = resolve(testRoot, "fixture-site", "node_modules", relativePath);
  if (!existsSync(frameworkPackage)) {
    throw new Error(
      `fixture project node_modules is missing required zfb framework plumbing: ${relativePath}`,
    );
  }
}
const firstPartySource = resolve(testRoot, "fixture-site", "components", "md-wasm-highlighter.tsx");
if (!readFileSync(firstPartySource, "utf8").includes('import("@takazudo/zfb-md-wasm")')) {
  throw new Error("fixture is missing the first-party packed md-wasm root importer");
}

console.log(
  "md-wasm browser fixture and exported manifest are staged from the asserted packed tarball",
);
