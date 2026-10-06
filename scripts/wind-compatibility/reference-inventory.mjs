import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { extractSurface } from "./inventory.mjs";
import { loadReference } from "./differential-runner.mjs";
import { tarFiles } from "./upstream.mjs";
import { digest, fromRoot, outsideCheckout, readJson, sha256 } from "./reference.mjs";

const sourceNames = ["utilities.ts", "variants.ts", "utilities.test.ts", "variants.test.ts",
  "compat/legacy-utilities.ts", "compat/legacy-utilities.test.ts"];
const diff = (before, after) => ({
  added: after.filter((item) => !before.some((old) => digest(old) === digest(item))),
  removed: before.filter((item) => !after.some((next) => digest(next) === digest(item))),
});

async function sourceSurface(assessment, cache, sourceCommit) {
  if (!/^[0-9a-f]{40}$/.test(sourceCommit)) throw Error("Reference source tag commit absent");
  const sourceCapture = assessment.captures.find((row) =>
    row.url === `https://codeload.github.com/tailwindlabs/tailwindcss/tar.gz/${sourceCommit}`);
  if (!sourceCapture) throw Error("Reference source archive capture absent");
  const sourceBytes = await readFile(await outsideCheckout(resolve(cache, sourceCapture.cacheFile)));
  if (sha256(sourceBytes) !== sourceCapture.sha256)
    throw Error("Reference source archive changed");
  const files = tarFiles(sourceBytes, `tailwindcss-${sourceCommit}/`);
  const sourceFiles = Object.fromEntries(sourceNames.map((name) => {
    const bytes = files.get(`packages/tailwindcss/src/${name}`);
    if (!bytes) throw Error(`Reference source registration/test file missing: ${name}`);
    return [name, bytes.toString("utf8")];
  }));
  return { surface: extractSurface(sourceFiles), sha256: sourceCapture.sha256 };
}

async function runtime(reference) {
  if (typeof reference.loadDesignSystem !== "function")
    throw Error("Reference compiler lacks reviewable runtime registration API");
  const design = await reference.loadDesignSystem("@theme { --*: initial; }");
  return {
    staticUtilities: [...design.utilities.keys("static")].sort(),
    functionalUtilities: [...design.utilities.keys("functional")].sort(),
    variants: [...design.variants.entries()].map(([name, value]) => ({ name, kind: value.kind }))
      .sort((a, b) => a.name.localeCompare(b.name)),
  };
}

export async function candidateInventory({ candidate, accepted, assessment, cache }) {
  const reference = await loadReference(cache, candidate);
  const observed = await runtime(reference);
  const previous = accepted ? await loadReference(cache, accepted) : null;
  const baseline = previous ? await runtime(previous) :
    await readJson(fromRoot("tests/wind-compatibility/runtime-keysets.v1.json"));
  const sourceCommit = assessment.sections.source.tagCommit;
  const { surface, sha256: sourceArchiveSha256 } = await sourceSurface(assessment, cache, sourceCommit);
  const baselineSurface = accepted
    ? (await sourceSurface(assessment, cache, assessment.sections.source.previousTagCommit)).surface
    : await readJson(fromRoot("tests/wind-compatibility/upstream-surface.v1.json"));
  return {
    schemaVersion: 1, candidateVersion: candidate.version,
    artifactSha256: candidate.artifactSha256,
    sourceTagCommit: sourceCommit,
    sourceArchiveSha256,
    runtime: observed,
    runtimeChanges: Object.fromEntries(Object.keys(observed).map((key) =>
      [key, diff(baseline[key], observed[key])])),
    sourceSurfaceDigest: `sha256:${digest(surface)}`,
    sourceChanges: Object.fromEntries(
      ["staticUtilities", "functionalUtilities", "staticVariants", "functionalVariants",
        "compoundVariants", "finiteLoopSites", "dynamicSourceSites", "testsInspected"]
        .filter((key) => Array.isArray(surface[key]) && Array.isArray(baselineSurface[key]))
        .map((key) => [key, diff(baselineSurface[key], surface[key])])),
    sourceMetadataChanges: Object.fromEntries(
      ["functionalConstraints", "dynamicRegistrationSites", "testExamples"]
        .filter((key) => digest(surface[key]) !== digest(baselineSurface[key]))
        .map((key) => [key, { acceptedDigest: `sha256:${digest(baselineSurface[key])}`,
          candidateDigest: `sha256:${digest(surface[key])}` }])),
    extractionLimits: surface.extractionLimits ?? surface.limits ?? null,
    tagToNpmArtifactLink: "unverified",
  };
}
