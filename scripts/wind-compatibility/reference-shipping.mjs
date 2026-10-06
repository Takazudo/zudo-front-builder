import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { digest, fromRoot, outsideCheckout, readJson, sha256 } from "./reference.mjs";
import { treeDigest } from "./differential-runner.mjs";
import { testedPaths } from "./reference-identity.mjs";

export const shippingManifestPath = "tests/wind-compatibility/shipping/manifest.json";
const requiredMechanisms = ["source-add", "source-edit", "source-remove", "source-rename",
  "token-change", "token-removal", "css-build", "css-dev", "css-ssr",
  "missing-css-detection", "component-state"];

export function validateShippingManifest(manifest) {
  if (manifest.schemaVersion !== 1 || manifest.kind !== "wind-shipping-manifest" ||
      !Number.isSafeInteger(manifest.revision) || manifest.revision < 1 ||
      !Array.isArray(manifest.required) || !manifest.required.length ||
      new Set(manifest.required.map((item) => item.id)).size !== manifest.required.length ||
      manifest.required.some((item) => !item.id || !item.mode || !item.engine ||
        !item.configuration || !item.fixture || !item.mechanism ||
        !item.expectedObservation || typeof item.screenshotRequired !== "boolean") ||
      !manifest.fixtureRoot || !Array.isArray(manifest.configFiles) ||
      !manifest.configFiles.length ||
      requiredMechanisms.some((name) => !manifest.required.some((item) => item.mechanism === name)))
    throw Error("Shipping manifest incomplete or duplicate");
  return true;
}

export async function validateShippingEvidence(report, comparison, output, toolchain) {
  let manifest;
  try { manifest = await readJson(fromRoot(shippingManifestPath)); }
  catch { throw Error("Checked shipping manifest missing; acceptance unavailable"); }
  validateShippingManifest(manifest);
  const tracked = new Set(testedPaths());
  if (!manifest.fixtureRoot.startsWith("tests/wind-compatibility/shipping/") ||
      manifest.fixtureRoot.includes("..") ||
      !tracked.has(shippingManifestPath) ||
      manifest.configFiles.some((path) => !tracked.has(path)) ||
      new Set(manifest.configFiles).size !== manifest.configFiles.length ||
      manifest.required.some((row) => !row.fixture.startsWith(manifest.fixtureRoot + "/") ||
        row.fixture.includes("..") || !tracked.has(row.fixture)))
    throw Error("Shipping fixture/config paths must be checked tracked inputs");
  const fixtureTreeDigest = await treeDigest(fromRoot(manifest.fixtureRoot));
  const configDigest = digest(await Promise.all(manifest.configFiles.map(async (path) =>
    [path, sha256(await readFile(fromRoot(path)))])));
  if (report?.schemaVersion !== 1 || report.kind !== "wind-shipping-consumer-evidence" ||
      report.reportId !== `sha256:${digest(Object.fromEntries(
        Object.entries(report).filter(([key]) => key !== "reportId")))}` ||
      report.manifestDigest !== sha256(await readFile(fromRoot(shippingManifestPath))) ||
      report.testedSourceSha !== comparison.testedSourceSha ||
      report.testedInputsDigest !== comparison.testedInputs.digest ||
      report.profileDigest !== comparison.profile.digest ||
      digest(report.windBuild) !== digest(comparison.candidate.windBuild) ||
      digest(report.toolchain) !== digest(toolchain) ||
      report.fixtureTreeDigest !== fixtureTreeDigest ||
      report.configDigest !== configDigest ||
      report.windBuild?.buildExitCode !== 0 ||
      !report.windBuild?.binarySha256 || !report.windBuild?.sourceDigest ||
      !report.windBuild?.lockDigest ||
      report.windBuild.gitSha !== comparison.testedSourceSha ||
      !Array.isArray(report.results) || report.results.length !== manifest.required.length)
    throw Error("Shipping evidence identity, build, or membership invalid");
  const expected = manifest.required.map(({ id, mode, engine, configuration, fixture, mechanism }) =>
    ({ id, mode, engine, configuration, fixture, mechanism }));
  const actual = report.results.map(({ id, mode, engine, configuration, fixture, mechanism }) =>
    ({ id, mode, engine, configuration, fixture, mechanism }));
  if (digest(actual) !== digest(expected)) throw Error("Shipping case/mode/engine membership mismatch");
  output = await outsideCheckout(output);
  for (let index = 0; index < report.results.length; index++) {
    const result = report.results[index], requirement = manifest.required[index];
    if (result.outcome !== "passed") throw Error(`Shipping case failed or skipped: ${result.id}`);
    for (const key of ["generatedCss", "servedCss", "observation", "screenshot"]) {
      const item = result.raw?.[key];
      if (key === "screenshot" && item === null && !requirement.screenshotRequired) continue;
      if (!item || !item.path || !/^[0-9a-f]{64}$/.test(item.sha256 ?? ""))
        throw Error(`Missing shipping raw ${key}: ${result.id}`);
      const path = await outsideCheckout(resolve(output, item.path));
      if (!path.startsWith(`${output}/`) || sha256(await readFile(path)) !== item.sha256)
        throw Error(`Shipping raw ${key} changed: ${result.id}`);
    }
    if (result.raw.generatedCss.sha256 !== result.raw.servedCss.sha256)
      throw Error(`Shipping generated/served CSS differs: ${result.id}`);
    const observation = await readJson(resolve(output, result.raw.observation.path));
    if (observation?.cssResponse?.status !== 200 ||
        observation?.cssResponse?.contentType !== "text/css; charset=utf-8" ||
        observation?.cssResponse?.sha256 !== result.raw.servedCss.sha256 ||
        digest(observation?.observed) !== digest(requirement.expectedObservation) ||
        observation?.caseId !== result.id || observation?.engine !== result.engine)
      throw Error(`Shipping observation or HTTP response invalid: ${result.id}`);
  }
  return { manifestDigest: report.manifestDigest, evidenceDigest: `sha256:${digest(report)}` };
}
