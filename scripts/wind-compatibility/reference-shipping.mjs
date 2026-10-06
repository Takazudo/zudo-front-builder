import { readFile, realpath } from "node:fs/promises";
import { resolve } from "node:path";
import { digest, fromRoot, outsideCheckout, readJson, sha256 } from "./reference.mjs";
import { loadReference, treeDigest } from "./differential-runner.mjs";
import { testedPaths } from "./reference-identity.mjs";
import { verifyProductionBuild } from "./production-build.mjs";
import { requiredMatrixMember } from "./browser-adapter.mjs";
import { verifyDistProof } from "../../tests/wind-real-build/dist-proof.mjs";
import { parseCssStructure } from "./structure.mjs";

export const shippingManifestPath = "tests/wind-compatibility/shipping/manifest.json";
export function validShippingCssResponse(response) {
  return (
    response?.status === 200 &&
    (response.contentType === "text/css" || response.contentType === "text/css; charset=utf-8")
  );
}
export function assertReplayedReferenceCss(replayed, retained) {
  if (!replayed.length || !replayed.equals(retained))
    throw Error("Independent reference CSS cannot be reproduced from SRI artifact");
}

function hexColorToRgb(value) {
  if (!/^#[0-9a-f]{3}(?:[0-9a-f]{3})?$/i.test(value ?? ""))
    throw Error(`Shipping token is not a hex RGB color: ${value}`);
  const hex = value.slice(1);
  const pairs = hex.length === 3 ? [...hex].map((digit) => digit + digit) : hex.match(/../g);
  return `rgb(${pairs.map((pair) => Number.parseInt(pair, 16)).join(", ")})`;
}

export function assertShippingTokenColor(css, token, expectedColor) {
  const layers = parseCssStructure(css).filter(
    (node) => node.kind === "rule" && node.head === "@layer zw-tokens",
  );
  const roots = layers.flatMap((layer) =>
    layer.children.filter((node) => node.kind === "rule" && node.head === ":root"),
  );
  const declarations = roots.flatMap((root) =>
    root.children.filter(
      (node) => node.kind === "declaration" && node.name === `--zw-color-${token}`,
    ),
  );
  if (layers.length !== 1 || roots.length !== 1 || declarations.length !== 1)
    throw Error(`Shipping token declaration missing or duplicate: ${token}`);
  if (hexColorToRgb(declarations[0].value) !== expectedColor)
    throw Error(`Shipping token color differs from contract: ${token}`);
}

function assertShippingTokenRemovedFromConfig(config, token) {
  const colors = config?.wind?.tokens?.colors;
  if (
    !colors ||
    typeof colors !== "object" ||
    Array.isArray(colors) ||
    Object.hasOwn(colors, token)
  )
    throw Error(`Shipping removed token still present or config invalid: ${token}`);
}
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
    !report.referenceCache ||
    !report.referenceScreenshot ||
    !report.sourcePlan ||
    !report.productionBuild ||
    !report.distProof ||
    !report.nodeFreeCss ||
    !Array.isArray(report.results) ||
    report.results.length !== manifest.required.length
  )
    throw Error("Shipping evidence identity, build, or membership invalid");
  await verifyProductionBuild(report.productionBuild, comparison);
  await verifyDistProof(report.distProof, report.distProof.distPath);
  if (digest(report.distProof.productionBuild) !== digest(report.productionBuild))
    throw Error("Shipping dist proof does not match production executable");
  if (
    digest(report.nodeFreeDevEnvironment) !==
    digest({
      PATH: "",
      NODE_PATH: null,
      ZFB_ESBUILD_BIN: report.distProof.execution?.buildEnvironment?.ZFB_ESBUILD_BIN,
    })
  )
    throw Error("Node-free dev environment unverified");
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
  if (
    digest(report.nodeFreeCss.args) !==
      digest([
        "css",
        "--input",
        "entry.css",
        "--output",
        "node-free.css",
        "--project-root",
        "<project>",
      ]) ||
    digest(report.nodeFreeCss.environment) !== digest({ PATH: "", NODE_PATH: null }) ||
    report.nodeFreeCss.exitCode !== 0
  )
    throw Error("Node-free CSS command or environment invalid");
  for (const key of ["rawCss", "stdout", "stderr"]) {
    const item = report.nodeFreeCss[key];
    if (!item?.path || !/^[0-9a-f]{64}$/.test(item.sha256 ?? ""))
      throw Error(`Node-free CSS ${key} missing`);
    const path = await outsideCheckout(resolve(output, item.path));
    if (!path.startsWith(`${output}/`) || sha256(await readFile(path)) !== item.sha256)
      throw Error(`Node-free CSS ${key} changed`);
  }
  const initial = report.results.find((row) => row.id === "css-initial");
  if (!initial || report.nodeFreeCss.rawCss.sha256 !== initial.raw.generatedCss?.sha256)
    throw Error("Node-free CSS differs from public CLI CSS");
  const referenceCssPath = await outsideCheckout(resolve(output, report.referenceCss.path));
  if (
    !referenceCssPath.startsWith(`${output}/`) ||
    !/^[0-9a-f]{64}$/.test(report.referenceCss.sha256 ?? "") ||
    !(await readFile(referenceCssPath)).length ||
    sha256(await readFile(referenceCssPath)) !== report.referenceCss.sha256
  )
    throw Error("Independent reference CSS missing or changed");
  const reference = await loadReference(report.referenceCache, comparison.candidate.reference);
  if (digest(reference.identity) !== digest(report.reference))
    throw Error("Independent reference artifact changed");
  const referenceCompiler = await reference.compile(
    "@theme { --*: initial; --color-brand: #336699; --color-transient: #883344; } @tailwind utilities;",
  );
  const replayedReferenceCss = Buffer.from(referenceCompiler.build(["bg-brand", "text-transient"]));
  assertReplayedReferenceCss(replayedReferenceCss, await readFile(referenceCssPath));
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
    !sourcePlan.toString().includes("manifest wind-shipping-fixture")
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
    await validateShippingRawCase(report.results[index], manifest.required[index], output, report);
  }
  return { manifestDigest: report.manifestDigest, evidenceDigest: `sha256:${digest(report)}` };
}

export async function validateShippingRawCase(result, requirement, output, report) {
  output = await outsideCheckout(output);
  if (result.outcome !== "passed") throw Error(`Shipping case failed or skipped: ${result.id}`);
  for (const key of ["generatedCss", "servedCss", "observation", "screenshot"]) {
    const item = result.raw?.[key];
    if (key === "screenshot" && item === null && !requirement.screenshotRequired) continue;
    if (key === "generatedCss" && requirement.mode.startsWith("production-dev") && item === null)
      continue;
    if (!item || !item.path || !/^[0-9a-f]{64}$/.test(item.sha256 ?? ""))
      throw Error(`Missing shipping raw ${key}: ${result.id}`);
    const path = await outsideCheckout(resolve(output, item.path));
    if (!path.startsWith(`${output}/`) || sha256(await readFile(path)) !== item.sha256)
      throw Error(`Shipping raw ${key} changed: ${result.id}`);
  }
  if (requirement.mode.startsWith("production-dev") && result.raw.generatedCss !== null)
    throw Error(`Dev mode must report served-only CSS: ${result.id}`);
  if (result.raw.generatedCss && result.raw.generatedCss.sha256 !== result.raw.servedCss.sha256)
    throw Error(`Shipping generated/served CSS differs: ${result.id}`);
  const css = await readFile(resolve(output, result.raw.servedCss.path), "utf8");
  if (!css.length) throw Error(`Shipping CSS empty: ${result.id}`);
  if (requirement.mode === "production-dev-warm") {
    const contract = requirement.expectedObservation.steps;
    if (
      !Array.isArray(result.steps) ||
      result.steps.length !== contract.length ||
      !result.cleanFinalCss?.path ||
      !/^[0-9a-f]{64}$/.test(result.cleanFinalCss.sha256 ?? "")
    )
      throw Error("Dev transition raw evidence incomplete");
    const cleanPath = await outsideCheckout(resolve(output, result.cleanFinalCss.path));
    if (
      !cleanPath.startsWith(`${output}/`) ||
      sha256(await readFile(cleanPath)) !== result.cleanFinalCss.sha256 ||
      result.cleanFinalCss.sha256 !== result.raw.servedCss.sha256
    )
      throw Error("Dev clean/warm same-mode CSS differs");
    let previousInputDigest;
    const mutationPaths = [
      "components/shipping.tsx",
      "components/added.tsx",
      "components/added.tsx",
      "components/added.tsx",
      "components/shipping-renamed.tsx",
      "zfb.config.json",
      "zfb.config.json",
    ];
    for (let stepIndex = 0; stepIndex < contract.length; stepIndex++) {
      const step = result.steps[stepIndex],
        expectedStep = contract[stepIndex];
      if (
        digest({
          id: step.id,
          present: step.present,
          absent: step.absent,
          ...(expectedStep.backgroundColor ? { backgroundColor: step.backgroundColor } : {}),
        }) !== digest(expectedStep) ||
        step.mutation?.path !== mutationPaths[stepIndex] ||
        (stepIndex === 3
          ? step.mutation.sha256 !== null
          : !/^[0-9a-f]{64}$/.test(step.mutation.sha256 ?? "")) ||
        (stepIndex === 4 && step.mutation.oldPathAbsent !== true) ||
        !/^[0-9a-f]{64}$/.test(step.inputDigest ?? "") ||
        step.inputDigest === previousInputDigest ||
        !validShippingCssResponse(step.cssResponse) ||
        !step.rawCss?.path ||
        !/^[0-9a-f]{64}$/.test(step.rawCss.sha256 ?? "") ||
        !step.rawConfig?.path ||
        !/^[0-9a-f]{64}$/.test(step.rawConfig.sha256 ?? "")
      )
        throw Error(`Dev transition identity invalid: ${step?.id}`);
      const stepPath = await outsideCheckout(resolve(output, step.rawCss.path));
      const stepCss = await readFile(stepPath);
      const configPath = await outsideCheckout(resolve(output, step.rawConfig.path));
      if (
        !configPath.startsWith(`${output}/`) ||
        sha256(await readFile(configPath)) !== step.rawConfig.sha256
      )
        throw Error(`Dev config input changed: ${step.id}`);
      if (stepIndex === 3) {
        if (step.rawInput !== null) throw Error("Removed dev source retained as an input");
      } else {
        if (!step.rawInput?.path || step.rawInput.sha256 !== step.mutation.sha256)
          throw Error(`Dev mutation input missing: ${step.id}`);
        const inputPath = await outsideCheckout(resolve(output, step.rawInput.path));
        if (
          !inputPath.startsWith(`${output}/`) ||
          sha256(await readFile(inputPath)) !== step.mutation.sha256
        )
          throw Error(`Dev mutation input changed: ${step.id}`);
      }
      if (
        !stepPath.startsWith(`${output}/`) ||
        !stepCss.length ||
        sha256(stepCss) !== step.rawCss.sha256 ||
        step.cssResponse.sha256 !== step.rawCss.sha256 ||
        expectedStep.present.some((needle) => !stepCss.toString().includes(needle)) ||
        expectedStep.absent.some((needle) => stepCss.toString().includes(needle))
      )
        throw Error(`Dev transition CSS changed: ${step.id}`);
      if (expectedStep.backgroundColor) {
        if (
          hexColorToRgb((await readJson(configPath)).wind?.tokens?.colors?.shipping) !==
          expectedStep.backgroundColor
        )
          throw Error(`Dev token config differs from contract: ${step.id}`);
        assertShippingTokenColor(stepCss.toString(), "shipping", expectedStep.backgroundColor);
      }
      if (step.id === "token-removal")
        assertShippingTokenRemovedFromConfig(await readJson(configPath), "transient");
      previousInputDigest = step.inputDigest;
    }
    if (result.steps.at(-1).rawCss.sha256 !== result.raw.servedCss.sha256)
      throw Error("Dev final CSS does not match retained transition");
  }
  if (requirement.mode === "production-dev-ssr") {
    const dev = report.results.find((row) => row.id === "dev-stylesheet");
    if (result.raw.servedCss.sha256 !== dev?.raw?.servedCss?.sha256)
      throw Error("Request-time route CSS differs from live dev stylesheet");
  }
  if (Array.isArray(requirement.expectedObservation.present)) {
    if (
      requirement.expectedObservation.present.some((needle) => !css.includes(needle)) ||
      requirement.expectedObservation.absent.some((needle) => css.includes(needle))
    )
      throw Error(`Shipping CSS selector/token facts changed: ${result.id}`);
  }
  if (result.id === "token-change" || result.id === "token-removal") {
    const rawConfig = result.raw.config;
    if (!rawConfig?.path || !/^[0-9a-f]{64}$/.test(rawConfig.sha256 ?? ""))
      throw Error(`Shipping ${result.id} config evidence missing`);
    const configPath = await outsideCheckout(resolve(output, rawConfig.path));
    if (
      !configPath.startsWith(`${output}/`) ||
      sha256(await readFile(configPath)) !== rawConfig.sha256
    )
      throw Error(`Shipping ${result.id} config evidence changed`);
    const config = await readJson(configPath);
    if (result.id === "token-change") {
      if (
        hexColorToRgb(config.wind?.tokens?.colors?.brand) !==
        requirement.expectedObservation.backgroundColor
      )
        throw Error("Shipping token-change config differs from contract");
      assertShippingTokenColor(css, "brand", requirement.expectedObservation.backgroundColor);
    } else assertShippingTokenRemovedFromConfig(config, "transient");
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
    !validShippingCssResponse(observation?.cssResponse) ||
    observation?.cssResponse?.sha256 !== result.raw.servedCss.sha256 ||
    (requirement.mode === "production-dev-warm" &&
      observation.cssResponse.contentType !== result.steps.at(-1).cssResponse.contentType) ||
    digest(observation?.observed) !== digest(requirement.expectedObservation) ||
    observation?.caseId !== result.id ||
    observation?.engine !== result.engine
  )
    throw Error(`Shipping observation or HTTP response invalid: ${result.id}`);
}
