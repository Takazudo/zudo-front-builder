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
import { matchesExpected } from "./browser-adapter.mjs";
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
  if (
    control.get("missing-rule-detection")?.mutations?.removedStylesheetDetected !== true ||
    control.get("missing-rule-detection")?.mutations?.wrongSelectorDetected !== true ||
    control.get("variant-inactive-state")?.mutations?.wrongMediaDetected !== true ||
    control.get("writing-mode-axis")?.verticalDifferences !== true
  )
    throw Error("Pilot mutation or writing-mode evidence absent");
  for (const id of ["specificity-and-authored-cascade", "nested-token-scope"]) {
    const pairs = control.get(id)?.observations;
    if (
      !Array.isArray(pairs) ||
      pairs.length !== 1 ||
      !pairs[0]?.wind?.pass ||
      !pairs[0]?.reference?.pass ||
      !pairs[0]?.wind?.verified ||
      !pairs[0]?.reference?.verified ||
      [pairs[0].wind, pairs[0].reference].some(
        (item) =>
          !matchesExpected(item.observation, item.expected) ||
          item.servedSha256 !== item.stylesheetSha256 ||
          item.expected !== (id === "nested-token-scope" ? "31px" : "inline"),
      )
    )
      throw Error(`Pilot control observation missing: ${id}`);
  }
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
  const nativeCompiler = await reference.compile("@theme { --*: initial; } @tailwind utilities;");
  const expectedNativeReferenceCss = `@layer theme, base, components, utilities;\n@layer base {\n${reference.preflightCss}\n}\n@layer utilities {\n${nativeCompiler.build(["block"])}\n}\n${nativeAuthored}`;
  const nativeWindReport = await readJson(resolve(output, "wind-native/native-reset/report.json"));
  if (
    native?.observations?.length !== 12 ||
    native.observations.some(
      (item) =>
        !item.pass ||
        !item.verified ||
        !matchesExpected(item.observation, item.expected) ||
        item.stylesheetSha256 !==
          (item.engine === "wind" ? sha256(nativeCss) : sha256(nativeReferenceCss)) ||
        item.servedSha256 !== item.stylesheetSha256,
    ) ||
    native.windCssSha256 !== sha256(nativeCss) ||
    native.referenceCssSha256 !== sha256(nativeReferenceCss) ||
    nativeReferenceCss !== expectedNativeReferenceCss ||
    nativeWindReport.inputMode !== "compiler" ||
    native.fixtureTreeDigest !== (await treeDigest(fromRoot("tests/wind-compatibility/native"))) ||
    native.windProcessStatus !== 0
  )
    throw Error("Pilot native control artifact claim invalid");
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
