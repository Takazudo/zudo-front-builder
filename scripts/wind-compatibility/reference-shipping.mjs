import { readFile, realpath } from "node:fs/promises";
import { resolve } from "node:path";
import { digest, fromRoot, outsideCheckout, readJson, sha256 } from "./reference.mjs";
import { treeDigest } from "./differential-runner.mjs";
import { testedPaths } from "./reference-identity.mjs";
import { verifyProductionBuild } from "./production-build.mjs";
import { requiredMatrixMember } from "./browser-adapter.mjs";
import { verifyDistProof } from "../../tests/wind-real-build/dist-proof.mjs";

export const shippingManifestPath = "tests/wind-compatibility/shipping/manifest.json";
const requiredMechanisms = [
  "source-add",
  "source-edit",
  "source-remove",
  "source-rename",
  "token-change",
  "token-removal",
  "css-build",
  "css-dev",
  "css-ssr",
  "missing-css-detection",
  "component-state",
];

export function validateShippingManifest(manifest) {
  if (
    manifest.schemaVersion !== 1 ||
    manifest.kind !== "wind-shipping-manifest" ||
    !Number.isSafeInteger(manifest.revision) ||
    manifest.revision < 1 ||
    !Array.isArray(manifest.required) ||
    !manifest.required.length ||
    new Set(manifest.required.map((item) => item.id)).size !== manifest.required.length ||
    manifest.required.some(
      (item) =>
        !item.id ||
        !item.mode ||
        !item.engine ||
        !item.configuration ||
        !item.fixture ||
        !item.mechanism ||
        !item.expectedObservation ||
        typeof item.screenshotRequired !== "boolean",
    ) ||
    !manifest.fixtureRoot ||
    !Array.isArray(manifest.configFiles) ||
    !manifest.configFiles.length ||
    requiredMechanisms.some((name) => !manifest.required.some((item) => item.mechanism === name))
  )
    throw Error("Shipping manifest incomplete or duplicate");
  return true;
}

export async function validateShippingEvidence(report, comparison, output, toolchain) {
  let manifest;
  try {
    manifest = await readJson(fromRoot(shippingManifestPath));
  } catch {
    throw Error("Checked shipping manifest missing; acceptance unavailable");
  }
  validateShippingManifest(manifest);
  const tracked = new Set(testedPaths());
  if (
    !manifest.fixtureRoot.startsWith("tests/wind-compatibility/shipping/") ||
    manifest.fixtureRoot.includes("..") ||
    !tracked.has(shippingManifestPath) ||
    manifest.configFiles.some((path) => !tracked.has(path)) ||
    new Set(manifest.configFiles).size !== manifest.configFiles.length ||
    manifest.required.some(
      (row) =>
        !row.fixture.startsWith(manifest.fixtureRoot + "/") ||
        row.fixture.includes("..") ||
        !tracked.has(row.fixture),
    )
  )
    throw Error("Shipping fixture/config paths must be checked tracked inputs");
  const fixtureTreeDigest = await treeDigest(fromRoot(manifest.fixtureRoot));
  const configDigest = digest(
    await Promise.all(
      manifest.configFiles.map(async (path) => [path, sha256(await readFile(fromRoot(path)))]),
    ),
  );
  if (
    report?.schemaVersion !== 1 ||
    report.kind !== "wind-shipping-consumer-evidence" ||
    report.reportId !==
      `sha256:${digest(
        Object.fromEntries(Object.entries(report).filter(([key]) => key !== "reportId")),
      )}` ||
    report.manifestDigest !== sha256(await readFile(fromRoot(shippingManifestPath))) ||
    report.testedSourceSha !== comparison.testedSourceSha ||
    report.testedInputsDigest !== comparison.testedInputs.digest ||
    report.profileDigest !== comparison.profile.digest ||
    digest(report.windBuild) !== digest(comparison.candidate.windBuild) ||
    digest(report.toolchain) !== digest(toolchain) ||
    report.fixtureTreeDigest !== fixtureTreeDigest ||
    report.configDigest !== configDigest ||
    report.windBuild?.buildExitCode !== 0 ||
    !report.windBuild?.binarySha256 ||
    !report.windBuild?.sourceDigest ||
    !report.windBuild?.lockDigest ||
    report.windBuild.gitSha !== comparison.testedSourceSha ||
    digest(report.reference) !== digest(comparison.candidate.reference) ||
    !report.referenceCss ||
    !report.referenceScreenshot ||
    !report.sourcePlan ||
    !report.productionBuild ||
    !report.distProof ||
    !Array.isArray(report.results) ||
    report.results.length !== manifest.required.length
  )
    throw Error("Shipping evidence identity, build, or membership invalid");
  await verifyProductionBuild(report.productionBuild, comparison);
  await verifyDistProof(report.distProof, report.distProof.distPath);
  if (digest(report.distProof.productionBuild) !== digest(report.productionBuild))
    throw Error("Shipping dist proof does not match production executable");
  const { chromium } = await import("@playwright/test");
  const profile = await readJson(fromRoot("tests/wind-compatibility/profile.json"));
  if (
    report.browserEnvironment?.name !== "chromium" ||
    report.browserEnvironment.launchExecutableKind !== "chromium-full-explicit" ||
    report.browserEnvironment.styleDelivery !== "real zfb or fixture HTTP response" ||
    report.browserEnvironment.requiredMatrixMember !== true ||
    !requiredMatrixMember(profile, report.browserEnvironment) ||
    !report.browserEnvironment.executable ||
    (await realpath(report.browserEnvironment.executable)) !==
      (await realpath(chromium.executablePath())) ||
    sha256(await readFile(report.browserEnvironment.executable)) !==
      report.browserEnvironment.executableSha256
  )
    throw Error("Shipping browser executable or profile membership invalid");
  output = await outsideCheckout(output);
  const referenceCssPath = await outsideCheckout(resolve(output, report.referenceCss.path));
  if (
    !referenceCssPath.startsWith(`${output}/`) ||
    !/^[0-9a-f]{64}$/.test(report.referenceCss.sha256 ?? "") ||
    !(await readFile(referenceCssPath)).length ||
    sha256(await readFile(referenceCssPath)) !== report.referenceCss.sha256
  )
    throw Error("Independent reference CSS missing or changed");
  const referenceScreenshotPath = await outsideCheckout(
    resolve(output, report.referenceScreenshot.path),
  );
  const referenceScreenshot = await readFile(referenceScreenshotPath);
  if (
    !referenceScreenshotPath.startsWith(`${output}/`) ||
    !/^[0-9a-f]{64}$/.test(report.referenceScreenshot.sha256 ?? "") ||
    sha256(referenceScreenshot) !== report.referenceScreenshot.sha256 ||
    !referenceScreenshot.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]))
  )
    throw Error("Independent reference screenshot missing or changed");
  const sourcePlanPath = await outsideCheckout(resolve(output, report.sourcePlan.path));
  const sourcePlan = await readFile(sourcePlanPath);
  if (
    !sourcePlanPath.startsWith(`${output}/`) ||
    !/^[0-9a-f]{64}$/.test(report.sourcePlan.sha256 ?? "") ||
    sha256(sourcePlan) !== report.sourcePlan.sha256 ||
    !sourcePlan.toString().includes("src") ||
    !sourcePlan.toString().includes("@fixture/shipping-ui") ||
    !sourcePlan.toString().includes("wind.json")
  )
    throw Error("Shipping source/package plan missing or changed");
  const expected = manifest.required.map(
    ({ id, mode, engine, configuration, fixture, mechanism }) => ({
      id,
      mode,
      engine,
      configuration,
      fixture,
      mechanism,
    }),
  );
  const actual = report.results.map(({ id, mode, engine, configuration, fixture, mechanism }) => ({
    id,
    mode,
    engine,
    configuration,
    fixture,
    mechanism,
  }));
  if (digest(actual) !== digest(expected))
    throw Error("Shipping case/mode/engine membership mismatch");
  for (let index = 0; index < report.results.length; index++) {
    const result = report.results[index],
      requirement = manifest.required[index];
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
    const css = await readFile(resolve(output, result.raw.generatedCss.path), "utf8");
    if (!css.length) throw Error(`Shipping CSS empty: ${result.id}`);
    if (Array.isArray(requirement.expectedObservation.present)) {
      if (
        requirement.expectedObservation.present.some((needle) => !css.includes(needle)) ||
        requirement.expectedObservation.absent.some((needle) => css.includes(needle))
      )
        throw Error(`Shipping CSS selector/token facts changed: ${result.id}`);
    }
    if (requirement.screenshotRequired) {
      const expectedViewport =
        result.id === "build-composition"
          ? { width: 1120, height: 800 }
          : { width: 1280, height: 720 };
      if (
        digest(result.browserSettings) !==
        digest({
          viewport: expectedViewport,
          colorScheme: "light",
          reducedMotion: "no-preference",
          deviceScaleFactor: 1,
        })
      )
        throw Error(`Shipping screenshot settings changed: ${result.id}`);
      const screenshot = await readFile(resolve(output, result.raw.screenshot.path));
      if (!screenshot.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10])))
        throw Error(`Shipping screenshot is not PNG: ${result.id}`);
      if (result.id === "css-initial" && sha256(screenshot) !== report.referenceScreenshot.sha256)
        throw Error("Independent reference utility pixels differ");
    }
    const observation = await readJson(resolve(output, result.raw.observation.path));
    if (
      observation?.cssResponse?.status !== 200 ||
      observation?.cssResponse?.contentType !== "text/css; charset=utf-8" ||
      observation?.cssResponse?.sha256 !== result.raw.servedCss.sha256 ||
      digest(observation?.observed) !== digest(requirement.expectedObservation) ||
      observation?.caseId !== result.id ||
      observation?.engine !== result.engine
    )
      throw Error(`Shipping observation or HTTP response invalid: ${result.id}`);
  }
  return { manifestDigest: report.manifestDigest, evidenceDigest: `sha256:${digest(report)}` };
}
