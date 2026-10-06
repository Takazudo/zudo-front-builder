import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import {
  artifactIdentity,
  classify,
  compareExtractionSets,
  outcomes,
  validatePilot,
} from "./differential-core.mjs";
import {
  diagnosticCheck,
  expectedWindConfig,
  referenceTheme,
  structuralChecks,
  treeDigest,
} from "./differential-runner.mjs";
import { documentFor, matchesExpected } from "./browser-adapter.mjs";
import { scanOriginal } from "./oxide-scanner.mjs";
import { expectedExtractionStructure, parseCssStructure } from "./structure.mjs";
import { digest, fromRoot, readJson, sha256 } from "./reference.mjs";

const pilotAdapters = [
  "differential-runner.mjs",
  "browser-adapter.mjs",
  "differential-core.mjs",
  "structure.mjs",
  "oxide-scanner.mjs",
  "reference.mjs",
  "reference-module-graph.mjs",
];
const exact = (actual, expected, label) => {
  if (digest(actual) !== digest(expected)) throw Error(`Pilot ${label} differs from current input`);
};

export function signedPilotReportId(report) {
  const { reportId: _ignored, ...body } = report;
  return `sha256:${digest(body)}`;
}

export function validatePilotEnvelope(
  report,
  profile,
  manifest,
  observations,
  extractionManifest,
  expectedIdentity,
) {
  if (report.reportId !== signedPilotReportId(report))
    throw Error("Pilot reportId changed or body edited");
  if (
    report.schemaVersion !== 1 ||
    report.kind !== "wind-differential-pilot" ||
    report.admission !== "pilot-only" ||
    report.complete !== true ||
    report.exitCode !== 0 ||
    Object.hasOwn(report, "infrastructure") ||
    Object.hasOwn(report, "browserAdmission")
  )
    throw Error("Pilot report incomplete or invalid admission");
  exact(report.identity, expectedIdentity, "identity");
  const required = validatePilot(profile, manifest, observations);
  exact(report.pilotCaseIds, manifest.caseIds, "case ID list");
  if (
    !Array.isArray(report.cases) ||
    report.cases.length !== manifest.caseIds.length ||
    !Array.isArray(report.controls) ||
    report.controls.length !== profile.requiredControls.length ||
    !Array.isArray(report.extraction) ||
    report.extraction.length !== extractionManifest.caseIds.length
  )
    throw Error("Pilot case/control/extraction row count changed");
  exact(
    report.cases.map((row) => row.caseId),
    manifest.caseIds,
    "case rows",
  );
  exact(
    report.controls.map((row) => row.controlId),
    profile.requiredControls,
    "control rows",
  );
  exact(
    report.extraction.map((row) => row.caseId),
    extractionManifest.caseIds,
    "extraction rows",
  );
  for (const row of report.cases) {
    const policy = required.get(row.caseId);
    if (
      row.disposition !== policy.disposition ||
      row.outcome !==
        {
          "equivalent-shared": outcomes.shared,
          "equivalent-mapped": outcomes.mapped,
          "intentional-difference": outcomes.difference,
          "implementation-gap": outcomes.unsupported,
        }[policy.disposition] ||
      !row.checks ||
      digest(Object.keys(row.checks)) !==
        digest(["identity", "structure", "observations", "diagnostics"]) ||
      Object.values(row.checks).some((value) => value !== true) ||
      row.structure?.pass !== true ||
      !Array.isArray(row.observations) ||
      row.observations.length !== observations[row.caseId].probes.length
    )
      throw Error(`Pilot supported case downgraded or evidence missing: ${row.caseId}`);
  }
  for (const control of report.controls) {
    if (control.outcome !== outcomes.shared)
      throw Error(`Pilot control missing or failed: ${control.controlId}`);
    if (
      Object.hasOwn(control, "observations") &&
      (!Array.isArray(control.observations) || !control.observations.length)
    )
      throw Error(`Pilot control observations empty: ${control.controlId}`);
  }
  for (const row of report.extraction)
    if (row.outcome !== outcomes.shared)
      throw Error(`Pilot extraction missing or failed: ${row.caseId}`);
  validatePilotAssessmentEnvelope(
    report,
    profile,
    manifest,
    observations,
    extractionManifest,
    expectedIdentity,
  );
  return true;
}

// An assessment can retain a complete observed mismatch. The same identity,
// membership and transport envelope is required as for passing admission.
export function validatePilotAssessmentEnvelope(
  report,
  profile,
  manifest,
  observations,
  extractionManifest,
  expectedIdentity,
) {
  if (
    report?.reportId !== signedPilotReportId(report) ||
    report.schemaVersion !== 1 ||
    report.kind !== "wind-differential-pilot" ||
    report.admission !== "pilot-only" ||
    ![0, 1].includes(report.exitCode) ||
    Object.hasOwn(report, "infrastructure") ||
    Object.hasOwn(report, "browserAdmission")
  )
    throw Error("Pilot assessment missing, edited or infrastructure failed");
  exact(report.identity, expectedIdentity, "identity");
  const required = validatePilot(profile, manifest, observations);
  exact(report.pilotCaseIds, manifest.caseIds, "assessment case list");
  exact(
    report.cases?.map((row) => row.caseId),
    manifest.caseIds,
    "assessment case rows",
  );
  exact(
    report.controls?.map((row) => row.controlId),
    profile.requiredControls,
    "assessment control rows",
  );
  exact(
    report.extraction?.map((row) => row.caseId),
    extractionManifest.caseIds,
    "assessment extraction rows",
  );
  const completeRows =
    report.cases.every(
      (row) =>
        row.disposition === required.get(row.caseId).disposition &&
        ![outcomes.missing, outcomes.infrastructure].includes(row.outcome) &&
        digest(Object.keys(row.checks ?? {})) ===
          digest(["identity", "structure", "observations", "diagnostics"]) &&
        row.observations?.length === observations[row.caseId].probes.length,
    ) &&
    report.controls.every(
      (row) => ![outcomes.missing, outcomes.infrastructure].includes(row.outcome),
    ) &&
    report.extraction.every(
      (row) => ![outcomes.missing, outcomes.infrastructure].includes(row.outcome),
    );
  if (!completeRows || (report.complete === true) !== (report.exitCode === 0))
    throw Error("Pilot assessment truncated or inconsistent");
  return true;
}

function validatedObservation(item, engine, name, cssHash, htmlHash, authoredHash) {
  if (
    !item ||
    item.engine !== engine ||
    item.probe !== name ||
    item.verified !== true ||
    item.pass !== true ||
    item.stylesheetSha256 !== cssHash ||
    item.servedSha256 !== cssHash ||
    item.settings?.documentSha256 !== htmlHash ||
    item.settings?.authoredCssSha256 !== authoredHash ||
    !matchesExpected(item.observation, item.expected)
  )
    throw Error(`Pilot browser observation invalid: ${name}`);
}

function exactControlObservation(item, side, css, candidate, probe, requirePass = true) {
  const settings = item?.settings;
  const box = item?.observation?.box;
  return (
    item?.engine === side &&
    item.probe === probe.name &&
    item.verified === true &&
    (requirePass
      ? item.pass === true
      : item.pass === matchesExpected(item.observation, probe[side])) &&
    item.expected === probe[side] &&
    item.stylesheetSha256 === sha256(css) &&
    item.servedSha256 === sha256(css) &&
    settings?.documentSha256 === sha256(probe.documentHtml) &&
    settings?.servedDocumentSha256 === sha256(documentFor(candidate, probe)) &&
    settings?.authoredCssSha256 === sha256(probe.authoredCss ?? "") &&
    settings?.scopeVars === (probe.scopeVars ?? "") &&
    settings?.selector === (probe.selector ?? "#target") &&
    settings?.viewportWidth === (probe.viewportWidth ?? 800) &&
    settings?.viewportHeight === 700 &&
    settings?.writingMode === (probe.writingMode ?? "horizontal-tb") &&
    settings?.direction === (probe.direction ?? "ltr") &&
    settings?.hover === Boolean(probe.hover) &&
    settings?.keyboardFocus === Boolean(probe.keyboardFocus) &&
    settings?.pseudo === (probe.pseudo ?? null) &&
    settings?.relativeTo === (probe.relativeTo ?? "#box") &&
    typeof item.observation?.value === "string" &&
    item.observation.value.length > 0 &&
    Array.isArray(item.observation.cssTree) &&
    Array.isArray(item.observation.cssRules) &&
    box &&
    ["x", "y", "width", "height", "parentX", "parentY", "parentWidth", "parentHeight"].every(
      (key) => Number.isFinite(box[key]),
    ) &&
    (!requirePass || matchesExpected(item.observation, probe[side]))
  );
}

export function validatePilotControlPairs(
  pairs,
  probes,
  windCss,
  referenceCss,
  candidate,
  requirePass = true,
) {
  if (!Array.isArray(pairs) || pairs.length !== probes.length)
    throw Error("Pilot control probe count changed");
  for (let index = 0; index < probes.length; index++) {
    const pair = pairs[index];
    if (
      !exactControlObservation(
        pair?.wind,
        "wind",
        windCss,
        candidate,
        probes[index],
        requirePass,
      ) ||
      !exactControlObservation(
        pair?.reference,
        "reference",
        referenceCss,
        candidate,
        probes[index],
        requirePass,
      )
    )
      throw Error(
        `Pilot control probe identity/artifact/expectation changed: ${probes[index].name}`,
      );
  }
  return true;
}

export function validatePilotNativeObservations(
  items,
  probes,
  windCss,
  referenceCss,
  requirePass = true,
) {
  if (!Array.isArray(items) || items.length !== probes.length * 2)
    throw Error("Pilot native control probe count changed");
  for (let index = 0; index < probes.length; index++) {
    if (
      !exactControlObservation(
        items[2 * index],
        "wind",
        windCss,
        "block native",
        probes[index],
        requirePass,
      ) ||
      !exactControlObservation(
        items[2 * index + 1],
        "reference",
        referenceCss,
        "block native",
        probes[index],
        requirePass,
      )
    )
      throw Error(
        `Pilot native control probe identity/artifact/expectation changed: ${probes[index].name}`,
      );
  }
  return true;
}

export async function currentPilotIdentity({
  profile,
  manifest,
  windBuild,
  reference,
  scanner,
  browserEnvironment,
}) {
  const pilotRoot = fromRoot("tests/wind-compatibility/pilot");
  const extractionRoot = fromRoot("tests/wind-compatibility/extraction");
  const adapterHashes = await Promise.all(
    pilotAdapters.map(async (name) =>
      sha256(await readFile(fromRoot(`scripts/wind-compatibility/${name}`))),
    ),
  );
  return {
    windBuild,
    windSpecVersion: 1,
    windSpecRevision: 12,
    catalogDigest: await treeDigest(fromRoot("crates/zudo-wind/src/catalog")),
    reference: reference.identity,
    scanner: scanner.identity,
    profileDigest: sha256(await readFile(fromRoot("tests/wind-compatibility/profile.json"))),
    fixtureManifestDigest: sha256(await readFile(resolve(pilotRoot, "manifest.json"))),
    pilotFixtureTreeDigest: await treeDigest(pilotRoot),
    extractionFixtureTreeDigest: await treeDigest(extractionRoot),
    nativeFixtureTreeDigest: await treeDigest(fromRoot("tests/wind-compatibility/native")),
    sourceInputDigest: digest(
      manifest.caseIds.map((id) => [
        id,
        profile.requiredCases.find((row) => row.id === id).candidate,
      ]),
    ),
    observationsDigest: sha256(await readFile(resolve(pilotRoot, "observations.json"))),
    assertionDigest: sha256(await readFile(resolve(pilotRoot, "observations.json"))),
    extractionManifestDigest: sha256(await readFile(resolve(extractionRoot, "manifest.json"))),
    adapterDigest: digest(adapterHashes),
    lockfileDigest: sha256(await readFile(fromRoot("pnpm-lock.yaml"))),
    browserEnvironment,
    resetMode: { shared: "none", native: "minimal-v1", referenceNative: "pinned-preflight-css" },
    caseIds: manifest.caseIds,
    windProcess: { status: 0, stderr: "", stdout: "" },
  };
}

export function validatePilotCorruptionControls(controls, blockCss, blockProbe, blockHtml) {
  const byId = new Map(controls.map((row) => [row.controlId, row]));
  const missing = byId.get("missing-rule-detection")?.mutations;
  const wrong = byId.get("wrong-value-detection")?.mutations;
  const wrongValueCss = blockCss.replace(/display:\s*block/, "display: none");
  const wrongSelectorCss = blockCss.replace(/\.block\s*\{/, ".wrong { ");
  const negative = (item, css) =>
    item?.engine === "wind" &&
    item.probe === blockProbe.name &&
    item.verified === true &&
    item.pass === false &&
    item.stylesheetSha256 === sha256(css) &&
    item.servedSha256 === sha256(css) &&
    item.settings?.documentSha256 === sha256(blockHtml) &&
    item.settings?.authoredCssSha256 === sha256(blockProbe.authoredCss ?? "") &&
    item.settings?.selector === (blockProbe.selector ?? "#target") &&
    item.settings?.viewportWidth === (blockProbe.viewportWidth ?? 800) &&
    item.settings?.viewportHeight === 700 &&
    item.settings?.writingMode === (blockProbe.writingMode ?? "horizontal-tb") &&
    item.settings?.direction === (blockProbe.direction ?? "ltr") &&
    item.settings?.hover === Boolean(blockProbe.hover) &&
    item.settings?.keyboardFocus === Boolean(blockProbe.keyboardFocus) &&
    item.settings?.pseudo === (blockProbe.pseudo ?? null) &&
    item.settings?.relativeTo === (blockProbe.relativeTo ?? "#box") &&
    typeof item.observation?.value === "string" &&
    item.observation.value.length > 0 &&
    Array.isArray(item.observation.cssTree) &&
    Array.isArray(item.observation.cssRules) &&
    item.observation.box &&
    ["x", "y", "width", "height", "parentX", "parentY", "parentWidth", "parentHeight"].every(
      (key) => Number.isFinite(item.observation.box[key]),
    ) &&
    digest(item.expected) === digest(blockProbe.wind) &&
    !matchesExpected(item.observation, blockProbe.wind);
  if (
    !missing?.removedStylesheetDetected ||
    !missing?.wrongSelectorDetected ||
    !wrong?.wrongValueDetected ||
    wrongValueCss === blockCss ||
    wrongSelectorCss === blockCss ||
    !negative(missing.removedStylesheet, "") ||
    !negative(missing.wrongSelector, wrongSelectorCss) ||
    !negative(wrong.wrongValue, wrongValueCss)
  )
    throw Error("Pilot wrong-value/missing-rule browser controls absent or forged");
  return true;
}

export async function validatePilotArtifacts(
  report,
  reportPath,
  profile,
  manifest,
  observations,
  extractionManifest,
  reference,
  scanner,
) {
  const output = resolve(reportPath, "..");
  const pilotRoot = fromRoot("tests/wind-compatibility/pilot");
  for (const row of report.cases) {
    const policy = profile.requiredCases.find((item) => item.id === row.caseId);
    const directory = resolve(pilotRoot, row.caseId);
    const fixture = await readFile(resolve(directory, "case.json"));
    expectedWindConfig(policy, JSON.parse(fixture), profile);
    const html = await readFile(resolve(directory, "index.html"), "utf8");
    const windCss = await readFile(
      resolve(output, "wind-compiler", row.caseId, "wind.css"),
      "utf8",
    );
    const windReport = await readJson(resolve(output, "wind-compiler", row.caseId, "report.json"));
    const referenceCss = await readFile(
      resolve(output, "reference-compiler", row.caseId, "reference.css"),
      "utf8",
    );
    const compiler = await reference.compile(referenceTheme(profile, policy));
    if (referenceCss !== compiler.build([policy.candidate]))
      throw Error(`Pilot reference CSS drift: ${row.caseId}`);
    const diagnosticsPass = diagnosticCheck(policy, windReport);
    const structure = structuralChecks(policy, windCss, referenceCss, windReport, diagnosticsPass);
    const checks = {
      identity: windReport.inputMode === "compiler" && windReport.specCaseId === row.caseId,
      structure: structure.pass,
      observations: true,
      diagnostics: diagnosticsPass,
    };
    if (
      row.outcome !== classify(policy, checks) ||
      digest(row.structure) !== digest(structure) ||
      digest(row.reviewedDifferenceIds) !== digest(structure.observedDifferenceIds) ||
      row.configDigest !== digest(profile.configurations[policy.configuration]) ||
      row.sourceInputDigest !== digest([policy.candidate]) ||
      row.fixtureDigest !== sha256(fixture) ||
      row.fixtureTreeDigest !== (await treeDigest(directory)) ||
      digest(row.wind) !== digest(artifactIdentity(windCss, windReport)) ||
      row.reference?.cssSha256 !== sha256(referenceCss)
    )
      throw Error(`Pilot case artifact claim differs from current inputs: ${row.caseId}`);
    row.observations.forEach((pair, index) => {
      const probe = observations[row.caseId].probes[index];
      if (
        digest(pair?.wind?.expected) !== digest(probe.wind) ||
        digest(pair?.reference?.expected) !== digest(probe.reference)
      )
        throw Error(`Pilot observation expected state changed: ${row.caseId}/${probe.name}`);
      validatedObservation(
        pair.wind,
        "wind",
        probe.name,
        sha256(windCss),
        sha256(html),
        sha256(probe.authoredCss ?? ""),
      );
      validatedObservation(
        pair.reference,
        "reference",
        probe.name,
        sha256(referenceCss),
        sha256(html),
        sha256(probe.authoredCss ?? ""),
      );
    });
  }
  const control = new Map(report.controls.map((row) => [row.controlId, row]));
  validatePilotCorruptionControls(
    report.controls,
    await readFile(resolve(output, "wind-compiler/block/wind.css"), "utf8"),
    observations.block.probes[0],
    await readFile(fromRoot("tests/wind-compatibility/pilot/block/index.html"), "utf8"),
  );
  if (
    control.get("missing-rule-detection")?.mutations?.removedStylesheetDetected !== true ||
    control.get("missing-rule-detection")?.mutations?.wrongSelectorDetected !== true ||
    control.get("variant-inactive-state")?.mutations?.wrongMediaDetected !== true ||
    control.get("writing-mode-axis")?.verticalDifferences !== true
  )
    throw Error("Pilot mutation or writing-mode evidence absent");
  const blockHtml = await readFile(
    fromRoot("tests/wind-compatibility/pilot/block/index.html"),
    "utf8",
  );
  const blockWindCss = await readFile(resolve(output, "wind-compiler/block/wind.css"), "utf8");
  const blockReferenceCss = await readFile(
    resolve(output, "reference-compiler/block/reference.css"),
    "utf8",
  );
  const cascadeProbe = {
    ...observations.block.probes[0],
    name: "authored-cascade",
    authoredCss: ".block { display: inline; }",
    wind: "inline",
    reference: "inline",
    documentHtml: blockHtml,
  };
  validatePilotControlPairs(
    control.get("specificity-and-authored-cascade")?.observations,
    [cascadeProbe],
    blockWindCss,
    blockReferenceCss,
    "block",
  );
  const spacingHtml = await readFile(
    fromRoot("tests/wind-compatibility/pilot/named-spacing/index.html"),
    "utf8",
  );
  const spacingWindCss = await readFile(
    resolve(output, "wind-compiler/named-spacing/wind.css"),
    "utf8",
  );
  const spacingReferenceCss = await readFile(
    resolve(output, "reference-compiler/named-spacing/reference.css"),
    "utf8",
  );
  const scopeProbe = {
    name: "nested-token-scope",
    property: "padding-top",
    scopeVars: "--zw-spacing-hsp-sm:31px;--spacing-hsp-sm:31px;",
    wind: "31px",
    reference: "31px",
    documentHtml: spacingHtml,
  };
  validatePilotControlPairs(
    control.get("nested-token-scope")?.observations,
    [scopeProbe],
    spacingWindCss,
    spacingReferenceCss,
    "p-hsp-sm",
  );
  const native = control.get("native-reset-controls");
  const nativeCss = await readFile(resolve(output, "wind-native/native-reset/wind.css"));
  const nativeReferenceCss = await readFile(
    resolve(output, "reference-native/reference.css"),
    "utf8",
  );
  const nativeAuthored = await readFile(
    fromRoot("tests/wind-compatibility/native/native-reset/authored.css"),
    "utf8",
  );
  const nativeHtml = await readFile(
    fromRoot("tests/wind-compatibility/native/native-reset/index.html"),
    "utf8",
  );
  const nativeProbes = [
    {
      name: "native-reset-box-sizing",
      property: "box-sizing",
      wind: "border-box",
      reference: "border-box",
    },
    { name: "native-authored-cascade", property: "display", wind: "inline", reference: "inline" },
    {
      name: "native-heading",
      selector: "#heading",
      property: "font-size",
      wind: "32px",
      reference: "16px",
    },
    {
      name: "native-list",
      selector: "#list",
      property: "list-style-type",
      wind: "disc",
      reference: "none",
    },
    {
      name: "native-border",
      selector: "#border",
      property: "border-top-style",
      wind: "inset",
      reference: "solid",
    },
    {
      name: "native-form",
      selector: "#form",
      property: "border-top-style",
      wind: "outset",
      reference: "solid",
    },
  ].map((probe) => ({ ...probe, documentHtml: nativeHtml }));
  const nativeCompiler = await reference.compile("@theme { --*: initial; } @tailwind utilities;");
  const expectedNativeReferenceCss = `@layer theme, base, components, utilities;\n@layer base {\n${reference.preflightCss}\n}\n@layer utilities {\n${nativeCompiler.build(["block"])}\n}\n${nativeAuthored}`;
  const nativeWindReport = await readJson(resolve(output, "wind-native/native-reset/report.json"));
  if (
    !native ||
    native.windCssSha256 !== sha256(nativeCss) ||
    native.referenceCssSha256 !== sha256(nativeReferenceCss) ||
    nativeReferenceCss !== expectedNativeReferenceCss ||
    nativeWindReport.inputMode !== "compiler" ||
    native.fixtureTreeDigest !== (await treeDigest(fromRoot("tests/wind-compatibility/native"))) ||
    native.windProcessStatus !== 0
  )
    throw Error("Pilot native control artifact claim invalid");
  validatePilotNativeObservations(native.observations, nativeProbes, nativeCss, nativeReferenceCss);
  for (const row of report.extraction) {
    const directory = fromRoot(`tests/wind-compatibility/extraction/${row.caseId}`);
    const definition = await readJson(resolve(directory, "case.json"));
    const assertions = await readJson(resolve(directory, "assertions.json"));
    const source = await readFile(resolve(directory, definition.sourceFile), "utf8");
    const extension = definition.sourceFile.split(".").at(-1);
    const referenceCandidates = scanOriginal(scanner.Scanner, source, extension);
    const compiler = await reference.compile("@theme { --*: initial; } @tailwind utilities;");
    const referenceCss = compiler.build(referenceCandidates);
    const windCss = await readFile(
      resolve(output, "wind-extraction", row.caseId, "wind.css"),
      "utf8",
    );
    const windReport = await readJson(
      resolve(output, "wind-extraction", row.caseId, "report.json"),
    );
    const windStructure = parseCssStructure(windCss);
    const referenceStructure = parseCssStructure(referenceCss);
    const ruleNames = (tree) =>
      tree
        .filter((node) => node.kind === "rule" && node.head.startsWith("."))
        .map((node) => node.head.slice(1).replaceAll("\\:", ":"));
    if (
      !compareExtractionSets(
        {
          windCandidates: windReport.extractedCandidates,
          referenceCandidates,
          windRules: windReport.rules,
          referenceRules: ruleNames(referenceStructure),
        },
        assertions,
      ) ||
      digest(windStructure) !==
        digest(expectedExtractionStructure(assertions.expectedWindRules, "wind")) ||
      digest(referenceStructure) !==
        digest(expectedExtractionStructure(assertions.expectedReferenceRules, "reference")) ||
      digest(row.expected) !== digest(definition.expectedCandidates) ||
      digest(row.assertions) !== digest(assertions) ||
      digest(row.windCandidates) !== digest(windReport.extractedCandidates) ||
      digest(row.referenceCandidates) !== digest(referenceCandidates) ||
      digest(row.windStructure) !== digest(windStructure) ||
      digest(row.referenceStructure) !== digest(referenceStructure) ||
      row.sourceInputDigest !== sha256(source) ||
      row.fixtureTreeDigest !== (await treeDigest(directory)) ||
      row.windCssSha256 !== sha256(windCss) ||
      row.referenceCssSha256 !== sha256(referenceCss) ||
      row.windReportSha256 !== sha256(JSON.stringify(windReport)) ||
      row.windProcessStatus !== 0 ||
      windReport.inputMode !== "extract" ||
      windReport.sourceBytes !== Buffer.byteLength(source)
    )
      throw Error(`Pilot extraction artifact claim invalid: ${row.caseId}`);
  }
  return true;
}

export async function validatePilotAssessmentCases(
  report,
  reportPath,
  profile,
  observations,
  reference,
) {
  const output = resolve(reportPath, "..");
  const pilotRoot = fromRoot("tests/wind-compatibility/pilot");
  for (const row of report.cases) {
    const policy = profile.requiredCases.find((item) => item.id === row.caseId);
    const directory = resolve(pilotRoot, row.caseId);
    const fixture = await readFile(resolve(directory, "case.json"));
    expectedWindConfig(policy, JSON.parse(fixture), profile);
    const html = await readFile(resolve(directory, "index.html"), "utf8");
    const windCss = await readFile(
      resolve(output, "wind-compiler", row.caseId, "wind.css"),
      "utf8",
    );
    const windReport = await readJson(resolve(output, "wind-compiler", row.caseId, "report.json"));
    const referenceCss = await readFile(
      resolve(output, "reference-compiler", row.caseId, "reference.css"),
      "utf8",
    );
    const compiler = await reference.compile(referenceTheme(profile, policy));
    if (
      compiler.build([policy.candidate]) !== referenceCss ||
      row.wind?.cssSha256 !== sha256(windCss) ||
      row.wind?.reportSha256 !== sha256(JSON.stringify(windReport)) ||
      row.reference?.cssSha256 !== sha256(referenceCss) ||
      row.fixtureDigest !== sha256(fixture) ||
      row.fixtureTreeDigest !== (await treeDigest(directory)) ||
      row.configDigest !== digest(profile.configurations[policy.configuration]) ||
      windReport.inputMode !== "compiler" ||
      windReport.specCaseId !== row.caseId
    )
      throw Error(`Pilot assessment raw case changed: ${row.caseId}`);
    for (let index = 0; index < row.observations.length; index++) {
      const pair = row.observations[index],
        probe = observations[row.caseId].probes[index];
      for (const [side, css] of [
        ["wind", windCss],
        ["reference", referenceCss],
      ]) {
        const item = pair?.[side];
        if (
          !item ||
          item.engine !== side ||
          item.probe !== probe.name ||
          item.verified !== true ||
          item.stylesheetSha256 !== sha256(css) ||
          item.servedSha256 !== sha256(css) ||
          item.settings?.documentSha256 !== sha256(html) ||
          item.settings?.authoredCssSha256 !== sha256(probe.authoredCss ?? "") ||
          digest(item.expected) !== digest(probe[side]) ||
          item.pass !== matchesExpected(item.observation, item.expected)
        )
          throw Error(
            `Pilot assessment browser evidence invalid: ${row.caseId}/${probe.name}/${side}`,
          );
      }
    }
  }
  return true;
}

export async function validatePilotAssessmentArtifacts(
  report,
  reportPath,
  profile,
  observations,
  reference,
  scanner,
) {
  await validatePilotAssessmentCases(report, reportPath, profile, observations, reference);
  const output = resolve(reportPath, "..");
  for (const control of report.controls)
    for (const pair of (control.observations ?? []).flat())
      for (const item of pair?.wind || pair?.reference ? [pair.wind, pair.reference] : [pair]) {
        if (
          !item ||
          item.verified !== true ||
          item.stylesheetSha256 !== item.servedSha256 ||
          item.pass !== matchesExpected(item.observation, item.expected)
        )
          throw Error(`Pilot assessment control transport invalid: ${control.controlId}`);
      }
  const controls = new Map(report.controls.map((row) => [row.controlId, row]));
  const blockHtml = await readFile(
    fromRoot("tests/wind-compatibility/pilot/block/index.html"),
    "utf8",
  );
  const blockWindCss = await readFile(resolve(output, "wind-compiler/block/wind.css"), "utf8");
  const blockReferenceCss = await readFile(
    resolve(output, "reference-compiler/block/reference.css"),
    "utf8",
  );
  validatePilotControlPairs(
    controls.get("specificity-and-authored-cascade")?.observations,
    [
      {
        ...observations.block.probes[0],
        name: "authored-cascade",
        authoredCss: ".block { display: inline; }",
        wind: "inline",
        reference: "inline",
        documentHtml: blockHtml,
      },
    ],
    blockWindCss,
    blockReferenceCss,
    "block",
    false,
  );
  const spacingHtml = await readFile(
    fromRoot("tests/wind-compatibility/pilot/named-spacing/index.html"),
    "utf8",
  );
  const spacingWindCss = await readFile(
    resolve(output, "wind-compiler/named-spacing/wind.css"),
    "utf8",
  );
  const spacingReferenceCss = await readFile(
    resolve(output, "reference-compiler/named-spacing/reference.css"),
    "utf8",
  );
  validatePilotControlPairs(
    controls.get("nested-token-scope")?.observations,
    [
      {
        name: "nested-token-scope",
        property: "padding-top",
        scopeVars: "--zw-spacing-hsp-sm:31px;--spacing-hsp-sm:31px;",
        wind: "31px",
        reference: "31px",
        documentHtml: spacingHtml,
      },
    ],
    spacingWindCss,
    spacingReferenceCss,
    "p-hsp-sm",
    false,
  );
  const native = controls.get("native-reset-controls");
  const nativeCss = await readFile(resolve(output, "wind-native/native-reset/wind.css"), "utf8");
  const nativeReferenceCss = await readFile(
    resolve(output, "reference-native/reference.css"),
    "utf8",
  );
  const nativeAuthored = await readFile(
    fromRoot("tests/wind-compatibility/native/native-reset/authored.css"),
    "utf8",
  );
  const nativeHtml = await readFile(
    fromRoot("tests/wind-compatibility/native/native-reset/index.html"),
    "utf8",
  );
  const nativeCompiler = await reference.compile("@theme { --*: initial; } @tailwind utilities;");
  const expectedNativeReferenceCss = `@layer theme, base, components, utilities;\n@layer base {\n${reference.preflightCss}\n}\n@layer utilities {\n${nativeCompiler.build(["block"])}\n}\n${nativeAuthored}`;
  if (
    !native ||
    native.windCssSha256 !== sha256(nativeCss) ||
    native.referenceCssSha256 !== sha256(nativeReferenceCss) ||
    nativeReferenceCss !== expectedNativeReferenceCss ||
    native.fixtureTreeDigest !== (await treeDigest(fromRoot("tests/wind-compatibility/native")))
  )
    throw Error("Pilot assessment native raw evidence changed");
  const nativeProbes = [
    {
      name: "native-reset-box-sizing",
      property: "box-sizing",
      wind: "border-box",
      reference: "border-box",
    },
    { name: "native-authored-cascade", property: "display", wind: "inline", reference: "inline" },
    {
      name: "native-heading",
      selector: "#heading",
      property: "font-size",
      wind: "32px",
      reference: "16px",
    },
    {
      name: "native-list",
      selector: "#list",
      property: "list-style-type",
      wind: "disc",
      reference: "none",
    },
    {
      name: "native-border",
      selector: "#border",
      property: "border-top-style",
      wind: "inset",
      reference: "solid",
    },
    {
      name: "native-form",
      selector: "#form",
      property: "border-top-style",
      wind: "outset",
      reference: "solid",
    },
  ].map((probe) => ({ ...probe, documentHtml: nativeHtml }));
  validatePilotNativeObservations(
    native.observations,
    nativeProbes,
    nativeCss,
    nativeReferenceCss,
    false,
  );
  for (const row of report.extraction) {
    const directory = fromRoot(`tests/wind-compatibility/extraction/${row.caseId}`);
    const definition = await readJson(resolve(directory, "case.json"));
    const source = await readFile(resolve(directory, definition.sourceFile), "utf8");
    const candidates = scanOriginal(
      scanner.Scanner,
      source,
      definition.sourceFile.split(".").at(-1),
    );
    const compiler = await reference.compile("@theme { --*: initial; } @tailwind utilities;");
    const referenceCss = compiler.build(candidates);
    const windCss = await readFile(
      resolve(output, "wind-extraction", row.caseId, "wind.css"),
      "utf8",
    );
    const windReport = await readJson(
      resolve(output, "wind-extraction", row.caseId, "report.json"),
    );
    if (
      row.sourceInputDigest !== sha256(source) ||
      row.fixtureTreeDigest !== (await treeDigest(directory)) ||
      digest(row.referenceCandidates) !== digest(candidates) ||
      row.referenceCssSha256 !== sha256(referenceCss) ||
      row.windCssSha256 !== sha256(windCss) ||
      row.windReportSha256 !== sha256(JSON.stringify(windReport)) ||
      windReport.inputMode !== "extract"
    )
      throw Error(`Pilot assessment extraction raw evidence changed: ${row.caseId}`);
  }
  return true;
}
