#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import {
  browserIdentity,
  observeIsolated,
  observePair,
  requiredMatrixMember,
} from "./browser-adapter.mjs";
import {
  checkMutations,
  completeCorpus,
  expectedOutcomes,
  finiteInventoryAccounting,
  observedUpstreamOutcome,
  validateCorpus,
} from "./corpus-core.mjs";
import {
  generatedCandidateLists,
  generatedCount,
  generatedSourceCount,
  generatedSourceStrings,
  generatedWidthCount,
  generatedWidths,
  seed,
  shrinkFailure,
  shrinkSourceFailure,
  shrinkWidthFailure,
} from "./corpus-seeds.mjs";
import { loadIndependentScanner, scanOriginal } from "./oxide-scanner.mjs";
import {
  currentPilotIdentity,
  validatePilotAssessmentArtifacts,
  validatePilotAssessmentEnvelope,
  validatePilotArtifacts,
  validatePilotEnvelope,
} from "./corpus-pilot.mjs";
import {
  compareCorpusStructure,
  expectedWindTree,
  canonical,
  validateStructureContracts,
} from "./corpus-structure.mjs";
import { supplementalProbePlan } from "./corpus-supplemental.mjs";
import { loadReference, treeDigest, verifyWindBuild } from "./differential-runner.mjs";
import { digest, fromRoot, outsideCheckout, readJson, sha256 } from "./reference.mjs";
import { parseCssStructure } from "./structure.mjs";
import { currentCorpusIdentity } from "./corpus-identity.mjs";
import { verifyUpstreamProvenance } from "./corpus-provenance.mjs";

const corpusRoot = fromRoot("tests/wind-compatibility/corpus");

function options(argv) {
  const result = { cache: "/tmp/zfb-wind-reference-cache", engine: "chromium" };
  for (let index = 0; index < argv.length; index += 2) {
    if (!argv[index]?.startsWith("--") || !argv[index + 1])
      throw Error("Expected --key value pairs");
    result[argv[index].slice(2)] = argv[index + 1];
  }
  for (const name of ["wind-binary", "wind-build-manifest", "output"])
    if (!result[name]) throw Error(`Missing --${name}`);
  if (!["chromium", "firefox", "webkit"].includes(result.engine))
    throw Error("Unknown browser engine");
  if (!result["pilot-report"])
    throw Error("Every corpus engine requires --pilot-report from the same Wind build and browser");
  if (result["assessment-mode"] !== undefined && result["assessment-mode"] !== "yes")
    throw Error("--assessment-mode must be yes when supplied");
  return result;
}

function theme(configuration, row) {
  const vars = Object.entries(configuration.referenceTheme ?? {})
    .map(([name, value]) => `${name}:${value};`)
    .join(" ");
  const dark =
    row.id === "dark-flex"
      ? '@custom-variant dark (&:where([data-theme="dark"], [data-theme="dark"] *));'
      : "";
  return `${dark}@theme { --*: initial; ${vars} } @tailwind utilities;`;
}

function upstreamTheme(definition) {
  const vars = [
    ...Object.entries(definition.spacing ?? {}).map(([name, value]) => [
      `--spacing-${name}`,
      value,
    ]),
    ...Object.entries(definition.colors ?? {}).map(([name, value]) => [`--color-${name}`, value]),
    ...(definition.spacingUnit ? [["--spacing", definition.spacingUnit]] : []),
  ]
    .map(([name, value]) => `${name}:${value};`)
    .join(" ");
  const dark = definition.dark
    ? `@custom-variant dark (&:where([${definition.dark.attribute}="${definition.dark.value}"], [${definition.dark.attribute}="${definition.dark.value}"] *));`
    : "";
  return `${dark}@theme { --*: initial; ${vars} } @tailwind utilities;`;
}

function ruleWrites(css, candidate) {
  const selector = "." + candidate.replace(/([^a-zA-Z0-9_-])/g, "\\$1");
  const found = [];
  function visit(nodes) {
    for (const node of nodes) {
      if (node.kind === "rule" && node.head === selector)
        found.push(
          Object.fromEntries(
            node.children.filter((x) => x.kind === "declaration").map((x) => [x.name, x.value]),
          ),
        );
      if (node.children) visit(node.children);
    }
  }
  visit(parseCssStructure(css));
  return found;
}

function referenceCandidateHeads(css) {
  return parseCssStructure(css)
    .filter((node) => node.kind === "rule" && node.head.startsWith("."))
    .map((node) => node.head);
}

function escapedCandidate(candidate) {
  return "." + candidate.replace(/([^a-zA-Z0-9_-])/g, "\\$1");
}

function wind(binary, fixtureRoot, output, mode = "compiler") {
  const run = spawnSync(
    binary,
    [fixtureRoot, output, "--mode", mode, "--manifest", resolve(fixtureRoot, "manifest.json")],
    { encoding: "utf8", timeout: 90_000 },
  );
  if (run.error || run.status !== 0)
    throw Error(`Wind fixture run failed: ${run.error?.message ?? run.stderr ?? run.status}`);
}

async function makeFixture(root, id, candidates, config) {
  const directory = resolve(root, id);
  await mkdir(directory, { recursive: true });
  await writeFile(
    resolve(directory, "case.json"),
    JSON.stringify(
      {
        caseId: id,
        reset: "none",
        explicitCandidates: candidates,
        ...(config.tokens?.spacing ? { spacing: config.tokens.spacing } : {}),
        ...(config.tokens?.colors ? { colors: config.tokens.colors } : {}),
        ...(config.tokens?.spacingUnit ? { spacingUnit: config.tokens.spacingUnit } : {}),
        ...(config.breakpoints && Object.keys(config.breakpoints).length
          ? { breakpoints: config.breakpoints }
          : {}),
      },
      null,
      2,
    ) + "\n",
  );
  await writeFile(
    resolve(directory, "index.html"),
    `<!doctype html><html><head></head><body><div id="box"><span id="target" class="${candidates.join(" ")}">x</span></div></body></html>\n`,
  );
}

async function browserFor(name) {
  const { chromium, firefox, webkit } = await import("@playwright/test");
  const kind = { chromium, firefox, webkit }[name];
  const executablePath = kind.executablePath();
  return { browser: await kind.launch({ headless: true, executablePath }), executablePath };
}

async function pilotResults(
  path,
  manifest,
  profile,
  pilotManifest,
  pilotObservations,
  extractionManifest,
  windBuild,
  reference,
  scanner,
  browserEnvironment,
  assessmentMode = false,
) {
  const report = await readJson(path);
  const expectedIdentity = await currentPilotIdentity({
    profile,
    manifest: pilotManifest,
    windBuild,
    reference,
    scanner,
    browserEnvironment,
  });
  if (assessmentMode) {
    validatePilotAssessmentEnvelope(
      report,
      profile,
      pilotManifest,
      pilotObservations,
      extractionManifest,
      expectedIdentity,
    );
    await validatePilotAssessmentArtifacts(
      report,
      path,
      profile,
      pilotObservations,
      reference,
      scanner,
    );
  } else {
    validatePilotEnvelope(
      report,
      profile,
      pilotManifest,
      pilotObservations,
      extractionManifest,
      expectedIdentity,
    );
    await validatePilotArtifacts(
      report,
      path,
      profile,
      pilotManifest,
      pilotObservations,
      extractionManifest,
      reference,
      scanner,
    );
  }
  const results = {};
  const engine = browserEnvironment.name;
  for (const row of report.cases) {
    if (manifest.pilotCaseIds.includes(row.caseId))
      results[`pilot/${row.caseId}/${engine}`] = {
        outcome: row.outcome,
        reportId: report.reportId,
      };
  }
  const controls = Object.fromEntries(report.controls.map((row) => [row.controlId, row]));
  return { results, controls, reportId: report.reportId, reportPath: resolve(path) };
}

async function nativeResults(binary, manifest, empty, output) {
  const fixture = resolve(output, "native-fixtures");
  const nativeOutput = resolve(output, "native-wind");
  await mkdir(fixture, { recursive: true });
  await writeFile(
    resolve(fixture, "manifest.json"),
    JSON.stringify({ caseIds: manifest.nativeGuaranteeIds }),
  );
  for (const guarantee of empty.nativeGuarantees)
    await makeFixture(fixture, guarantee.id, [guarantee.candidate], { tokens: {} });
  wind(binary, fixture, nativeOutput);
  const results = {};
  for (const guarantee of empty.nativeGuarantees) {
    const css = await readFile(resolve(nativeOutput, guarantee.id, "wind.css"), "utf8");
    const report = await readJson(resolve(nativeOutput, guarantee.id, "report.json"));
    const rules = ruleWrites(css, guarantee.candidate);
    const actual = rules[0] ?? {};
    const expected = guarantee.writes;
    const contract = {
      tokens: {},
      registrations: {},
      rules: [
        {
          candidate: guarantee.candidate,
          selector: escapedCandidate(guarantee.candidate),
          media: null,
          writes: Object.entries(expected),
        },
      ],
    };
    const structure = {
      expected: digest(expectedWindTree(contract)),
      actual: digest(parseCssStructure(css).map(canonical)),
    };
    const pass =
      rules.length === 1 &&
      digest(report.rules) === digest([guarantee.candidate]) &&
      structure.actual === structure.expected;
    results[`native/${guarantee.id}/chromium`] = {
      outcome: pass ? "matched" : "unexpected-mismatch",
      scope: "Wind declaration-only; no browser equivalence claim",
      expected,
      actual,
      structure,
      windCssPath: resolve(nativeOutput, guarantee.id, "wind.css"),
      cssSha256: sha256(css),
    };
  }
  return results;
}

async function assertNativeReferenceBoundaries(reference) {
  const input = "@theme { --*: initial; } @tailwind utilities;";
  for (const candidate of [
    "basis-0",
    "basis-min",
    "basis-max",
    "basis-fit",
    "basis-content",
    "basis-0.5/2",
  ]) {
    const compiler = await reference.compile(input);
    if (referenceCandidateHeads(compiler.build([candidate])).length !== 0)
      throw Error(`Empty-theme reference unexpectedly emitted ${candidate}`);
  }
  for (const [candidate, value] of [
    ["basis-0/2", "calc(0 / 2 * 100%)"],
    ["basis-1/0", "calc(1 / 0 * 100%)"],
    ["basis-1000001/2", "calc(1000001 / 2 * 100%)"],
  ]) {
    const compiler = await reference.compile(input);
    const css = compiler.build([candidate]);
    const writes = ruleWrites(css, candidate);
    if (digest(writes) !== digest([{ "flex-basis": value }]))
      throw Error(`Reference bounded-fraction difference changed: ${candidate}`);
  }
  for (const [candidate, property, value] of [
    ["-order-1", "order", "calc(1 * -1)"],
    ["basis-1/2", "flex-basis", "calc(1 / 2 * 100%)"],
    ["fill-current", "fill", "currentcolor"],
    ["stroke-current", "stroke", "currentcolor"],
  ]) {
    const compiler = await reference.compile(input);
    if (
      digest(ruleWrites(compiler.build([candidate]), candidate)) !== digest([{ [property]: value }])
    )
      throw Error(`Reference shared native spelling changed: ${candidate}`);
  }
  const opacity = await reference.compile(input);
  const opacityCss = opacity.build(["fill-current/50"]);
  if (
    digest(referenceCandidateHeads(opacityCss)) !== digest([escapedCandidate("fill-current/50")]) ||
    !opacityCss.includes("@supports") ||
    !opacityCss.includes("color-mix(in oklab, currentcolor 50%, transparent)")
  )
    throw Error("Reference SVG opacity fallback difference changed");
}

async function supplementalResults(
  binary,
  reference,
  empty,
  configurations,
  contracts,
  browser,
  output,
) {
  const fixture = resolve(output, "supplemental-fixtures");
  const windOutput = resolve(output, "supplemental-wind");
  await mkdir(fixture, { recursive: true });
  await writeFile(
    resolve(fixture, "manifest.json"),
    JSON.stringify({ caseIds: empty.supplementalCaseIds }),
  );
  for (const id of empty.supplementalCaseIds) {
    const row = empty.cases.find((x) => x.caseId === id);
    await makeFixture(fixture, id, row.explicitCandidates, configurations[row.configuration]);
  }
  wind(binary, fixture, windOutput);
  const results = {};
  const sourceHtml = await readFile(
    fromRoot("tests/wind-compatibility/empty-token/index.html"),
    "utf8",
  );
  for (const id of empty.supplementalCaseIds) {
    const row = empty.cases.find((x) => x.caseId === id);
    const css = await readFile(resolve(windOutput, id, "wind.css"), "utf8");
    const report = await readJson(resolve(windOutput, id, "report.json"));
    const declarations = Object.entries(row.expectedWind.utilityWrites).every(
      ([candidate, writes]) => {
        const rules = ruleWrites(css, candidate);
        return (
          rules.length === 1 &&
          Object.entries(writes).every(([name, value]) => rules[0][name] === value)
        );
      },
    );
    const expectedCandidates = Object.keys(row.expectedWind.utilityWrites);
    const exactRules = digest([...report.rules].sort()) === digest([...expectedCandidates].sort());
    const vars = Object.entries(row.expectedWind.tokenDeclarations).every(
      ([name, value]) => css.includes(`${name}: ${value}`) || css.includes(`${name}:${value}`),
    );
    const prelude =
      (css.match(/@layer zw-reset, zw-tokens, zfb-hi, base, components;/g) ?? []).length ===
      row.expectedWind.preludeOccurrences;
    const config = configurations[row.configuration];
    const referenceInput = theme(config, row);
    const compiler = await reference.compile(referenceInput);
    const referenceCss = compiler.build(row.explicitCandidates);
    const referenceDir = resolve(output, "reference-supplemental", id);
    await mkdir(referenceDir, { recursive: true });
    const referenceCssPath = resolve(referenceDir, "reference.css");
    const referenceInputPath = resolve(referenceDir, "reference-input.css");
    await writeFile(referenceCssPath, referenceCss);
    await writeFile(referenceInputPath, referenceInput);
    const structure = compareCorpusStructure(contracts.supplemental[id], css, referenceCss);
    const observations = [];
    const plan = supplementalProbePlan(row, sourceHtml);
    for (const { candidate, key, property, role, value } of plan) {
      const documentHtml = `<!doctype html><html><head></head><body><div id="box"><span id="target" class="${candidate}">x</span><span id="control">x</span></div></body></html>`;
      const probe = {
        name: `${id}/${candidate}/${key}`,
        property,
        selector: role === "control" ? "#control" : "#target",
        wind: value,
        reference: value,
        documentHtml,
        authoredCss:
          ":where(#target,#control){padding:24px;margin:0;color:#222;background-color:transparent;}",
      };
      observations.push(await observePair(browser, css, referenceCss, candidate, [probe]));
    }
    const transportPass = observations
      .flat()
      .every((pair) => pair.wind.verified && pair.reference.verified);
    const browserPass =
      observations.length === plan.length &&
      observations.flat().every((pair) => pair.wind.pass && pair.reference.pass);
    results[`supplemental/${id}/chromium`] = {
      outcome: !transportPass
        ? "infrastructure-failure"
        : declarations && exactRules && vars && prelude && structure.pass && browserPass
          ? "matched"
          : "unexpected-mismatch",
      declarations,
      exactRules,
      vars,
      prelude,
      structure,
      observationCount: plan.length,
      transitionMetadata: row.browserExpected["p-4"]?.previousConfiguredPadding ?? null,
      transportPass,
      browserPass,
      observations,
      windCssPath: resolve(windOutput, id, "wind.css"),
      windCssSha256: sha256(css),
      referenceCssPath,
      referenceCssSha256: sha256(referenceCss),
      referenceInputPath,
      referenceInputSha256: sha256(referenceInput),
    };
  }
  return results;
}

async function upstreamResults(
  binary,
  reference,
  configurations,
  manifest,
  contracts,
  inventory,
  browser,
  engine,
  output,
) {
  const fixture = resolve(corpusRoot, "upstream");
  const windOutput = resolve(output, "upstream-wind");
  wind(binary, fixture, windOutput);
  const results = {},
    cssById = {};
  for (const row of manifest.upstreamCases.filter((x) => x.engines.includes(engine))) {
    const directory = resolve(fixture, row.id);
    const definition = await readJson(resolve(directory, "case.json"));
    if (
      digest(definition.explicitCandidates) !== digest(row.candidates) ||
      definition.caseId !== row.id
    )
      throw Error(`Upstream fixture drift: ${row.id}`);
    const policy = configurations[row.configuration];
    if (
      !policy ||
      Object.entries(definition.spacing ?? {}).some(
        ([key, value]) => policy.tokens?.spacing?.[key] !== value,
      ) ||
      Object.entries(definition.colors ?? {}).some(
        ([key, value]) => policy.tokens?.colors?.[key] !== value,
      ) ||
      (definition.spacingUnit ?? null) !== (policy.tokens?.spacingUnit ?? null)
    )
      throw Error(`Upstream fixture configuration drift: ${row.id}`);
    const html = await readFile(resolve(directory, "index.html"), "utf8");
    const probes = (await readJson(resolve(directory, "probes.json")))
      .filter((probe) => !probe.engines || probe.engines.includes(engine))
      .map((probe) => ({ ...probe, documentHtml: html }));
    const css = await readFile(resolve(windOutput, row.id, "wind.css"), "utf8");
    const windReport = await readJson(resolve(windOutput, row.id, "report.json"));
    const referenceInput = upstreamTheme(definition);
    const compiler = await reference.compile(referenceInput);
    const referenceCss = compiler.build(row.candidates);
    const referenceDir = resolve(output, "reference-upstream", row.id);
    await mkdir(referenceDir, { recursive: true });
    const referenceCssPath = resolve(referenceDir, "reference.css");
    const referenceInputPath = resolve(referenceDir, "reference-input.css");
    await writeFile(referenceCssPath, referenceCss);
    await writeFile(referenceInputPath, referenceInput);
    const structure = compareCorpusStructure(contracts.cases[row.id], css, referenceCss);
    let reorderPass = true;
    if (row.id === "duplicate-reordered") {
      const reversed = ["flex-row", "flex", "flex"];
      const reverseRoot = resolve(output, "reordered-fixture");
      const reverseId = "duplicate-reordered-reverse";
      await mkdir(reverseRoot, { recursive: true });
      await writeFile(
        resolve(reverseRoot, "manifest.json"),
        JSON.stringify({ caseIds: [reverseId] }),
      );
      await makeFixture(reverseRoot, reverseId, reversed, { tokens: {} });
      const reverseOutput = resolve(output, "reordered-wind");
      wind(binary, reverseRoot, reverseOutput);
      const reversedWind = await readFile(resolve(reverseOutput, reverseId, "wind.css"), "utf8");
      const reverseCompiler = await reference.compile(upstreamTheme(definition));
      const reversedReference = reverseCompiler.build(reversed);
      reorderPass = reversedWind === css && reversedReference === referenceCss;
    }
    const pairs = [];
    for (const probe of probes)
      pairs.push(await observePair(browser, css, referenceCss, row.candidates.join(" "), [probe]));
    const observed = pairs.flat();
    const transportPass = observed.every((pair) => pair.wind.verified && pair.reference.verified);
    const detectedDifferences = [];
    if (structure.pass && structure.windTreeSha256 !== structure.referenceTreeSha256)
      detectedDifferences.push("exact-tree-contract");
    if (
      structure.pass &&
      (row.id === "native-not-sr-only" || row.id === "native-sr-reversal") &&
      observed.every((pair) => pair.wind.pass && pair.reference.pass)
    )
      detectedDifferences.push("sr-reversal-clip-model");
    const semantic = {
      "padding-axis": "physical-logical-axis-composition",
      "margin-axis": "physical-logical-axis-composition",
      "pseudo-before": "pseudo-default-content",
      "space-hidden-children": "space-child-selection-axis",
    }[row.id];
    if (
      semantic &&
      observed.some(
        (pair) =>
          pair.wind.pass &&
          pair.reference.pass &&
          pair.wind.expected !== pair.reference.expected &&
          pair.wind.observation.value !== pair.reference.observation.value,
      )
    )
      detectedDifferences.push(semantic);
    const differencePass = digest(detectedDifferences) === digest(row.reviewedDifferenceIds);
    const expectedCandidates = [...new Set(row.candidates)].sort();
    const exactWindCandidates = digest([...windReport.rules].sort()) === digest(expectedCandidates);
    const exactReferenceCandidates =
      digest(referenceCandidateHeads(referenceCss).sort()) ===
      digest(expectedCandidates.map(escapedCandidate).sort());
    const pass =
      observed.every((pair) => pair.wind.pass && pair.reference.pass) &&
      structure.pass &&
      differencePass &&
      reorderPass &&
      exactWindCandidates &&
      exactReferenceCandidates &&
      windReport.inputMode === "compiler" &&
      windReport.specCaseId === row.id;
    const inventoryRowIds = inventory.rows
      .filter((item) =>
        row.candidates.some((candidate) => item.upstream?.name === candidate.split(":").at(-1)),
      )
      .map((item) => item.id);
    results[`upstream/${row.id}/${engine}`] = {
      outcome: observedUpstreamOutcome(transportPass, pass, detectedDifferences),
      reviewedDifferenceIds: detectedDifferences,
      differencePass,
      structure,
      source: `${row.upstreamPath}#${row.upstreamTest}`,
      originalInput: row.originalInput,
      adaptation: row.adaptation,
      inventoryIssue: "https://github.com/Takazudo/zudo-front-builder/issues/3812",
      inventoryRowIds,
      configDigest: digest(definition),
      htmlSha256: sha256(html),
      transportPass,
      reorderPass,
      exactWindCandidates,
      exactReferenceCandidates,
      probes: observed,
      windCssPath: resolve(windOutput, row.id, "wind.css"),
      windCssSha256: sha256(css),
      referenceCssPath,
      referenceCssSha256: sha256(referenceCss),
      referenceInputPath,
      referenceInputSha256: sha256(referenceInput),
    };
    cssById[row.id] = { css, referenceCss, probes, candidates: row.candidates };
  }
  return { results, cssById };
}

async function mutations(browser, cssById) {
  const display = cssById["display-flex"];
  const hover = cssById["hover-flex"];
  if (!display || !hover) return { outcome: "not-executed", reason: "Chromium controls required" };
  const displayProbe = display.probes[0];
  const hoverProbe = hover.probes.find((x) => x.name === "active");
  const validObserved = [
    await observeIsolated(browser, display.css, "flex", displayProbe, "wind"),
    await observeIsolated(browser, hover.css, "hover:flex", hoverProbe, "wind"),
  ];
  if (validObserved.some((row) => !row.verified))
    throw Error("Mutation baseline stylesheet transport failed");
  const valid = { "display-valid": validObserved[0].pass, "hover-valid": validObserved[1].pass };
  const wrong = display.css.replace(/display:\s*flex/, "display: none");
  const missingRule = display.css.replace(/\.flex\s*\{[^}]*\}/, "");
  const wrongSelector = display.css.replace(/\.flex\s*\{/, ".other {");
  const wrongMedia = hover.css.replace(/\(hover:\s*hover\)/, "(hover: none)");
  if (
    [wrong, missingRule, wrongSelector].some((x) => x === display.css) ||
    wrongMedia === hover.css
  )
    throw Error("CSS mutation did not alter stylesheet");
  const mutated = {};
  for (const [id, css] of [
    ["missing-stylesheet", ""],
    ["missing-rule", missingRule],
    ["wrong-declaration", wrong],
    ["wrong-selector", wrongSelector],
  ]) {
    const observation = await observeIsolated(browser, css, "flex", displayProbe, "wind");
    if (!observation.verified) throw Error("Mutation stylesheet transport failed");
    mutated[id] = observation.pass;
  }
  const mediaObservation = await observeIsolated(
    browser,
    wrongMedia,
    "hover:flex",
    hoverProbe,
    "wind",
  );
  if (!mediaObservation.verified) throw Error("Media mutation stylesheet transport failed");
  mutated["wrong-media"] = mediaObservation.pass;
  return checkMutations(valid, mutated);
}

async function seededControls(binary, reference, browser, scanner, output) {
  const checked = await readJson(resolve(corpusRoot, "seed-regressions.json"));
  if (
    checked.schemaVersion !== 1 ||
    checked.seed !== `0x${seed.toString(16)}` ||
    checked.generatedCount !== generatedCount ||
    checked.generatedWidthCount !== generatedWidthCount ||
    checked.generatedSourceCount !== generatedSourceCount ||
    digest(checked.permanentSpecimens) !==
      digest([
        ["flex", "flex-row", "flex"],
        ["relative", "flex", "flex-row"],
      ])
  )
    throw Error("Seeded specimen policy changed");
  const artifacts = [];
  async function compare(candidates, id) {
    const canonical = [...new Set(candidates)].sort();
    const root = resolve(output, "seed-fixtures", id);
    const windOutput = resolve(output, "seed-wind", id);
    await mkdir(root, { recursive: true });
    await writeFile(
      resolve(root, "manifest.json"),
      JSON.stringify({ caseIds: ["actual", "canonical"] }),
    );
    await makeFixture(root, "actual", candidates, { tokens: {} });
    await makeFixture(root, "canonical", canonical, { tokens: {} });
    wind(binary, root, windOutput);
    const actualWind = await readFile(resolve(windOutput, "actual/wind.css"), "utf8");
    const canonicalWind = await readFile(resolve(windOutput, "canonical/wind.css"), "utf8");
    const referenceActual = await reference.compile(
      "@theme { --*: initial; } @tailwind utilities;",
    );
    const referenceCanonical = await reference.compile(
      "@theme { --*: initial; } @tailwind utilities;",
    );
    const actualReference = referenceActual.build(candidates);
    const canonicalReference = referenceCanonical.build(canonical);
    const referenceDir = resolve(output, "reference-seed", id);
    await mkdir(referenceDir, { recursive: true });
    const referenceInputPath = resolve(referenceDir, "reference-input.css");
    const actualPath = resolve(referenceDir, "actual.css");
    const canonicalPath = resolve(referenceDir, "canonical.css");
    await writeFile(referenceInputPath, "@theme { --*: initial; } @tailwind utilities;");
    await writeFile(actualPath, actualReference);
    await writeFile(canonicalPath, canonicalReference);
    artifacts.push({
      kind: "candidate-permutation",
      id,
      candidates,
      canonical,
      windActualPath: resolve(windOutput, "actual/wind.css"),
      windActualSha256: sha256(actualWind),
      windCanonicalPath: resolve(windOutput, "canonical/wind.css"),
      windCanonicalSha256: sha256(canonicalWind),
      referenceInputPath,
      referenceInputSha256: sha256("@theme { --*: initial; } @tailwind utilities;"),
      actualPath,
      actualSha256: sha256(actualReference),
      canonicalPath,
      canonicalSha256: sha256(canonicalReference),
    });
    return (
      actualWind === canonicalWind &&
      actualReference === canonicalReference &&
      digest(referenceCandidateHeads(actualReference).sort()) ===
        digest(canonical.map(escapedCandidate).sort())
    );
  }
  const specimens = [...checked.permanentSpecimens, ...generatedCandidateLists()];
  for (let index = 0; index < specimens.length; index++) {
    const candidates = specimens[index];
    if (!(await compare(candidates, `specimen-${index}`))) {
      const shrunk = await shrinkFailure(
        candidates,
        async (values) =>
          !(await compare(
            values,
            `shrink-${index}-${sha256(JSON.stringify(values)).slice(0, 12)}`,
          )),
      );
      await writeFile(
        resolve(output, "shrunk-failure.json"),
        JSON.stringify(
          {
            seed: checked.seed,
            specimenIndex: index,
            original: candidates,
            minimized: shrunk,
            promotionRequired:
              "Review and add to tests/wind-compatibility/corpus/seed-regressions.json",
          },
          null,
          2,
        ) + "\n",
      );
      return { outcome: "unexpected-mismatch", specimenIndex: index, shrunk, artifacts };
    }
  }
  async function widthPass(width, id) {
    const candidate = `w-[${width}px]`;
    const root = resolve(output, "width-fixtures", id);
    const windOutput = resolve(output, "width-wind", id);
    await mkdir(root, { recursive: true });
    await writeFile(resolve(root, "manifest.json"), JSON.stringify({ caseIds: ["width"] }));
    await makeFixture(root, "width", [candidate], { tokens: {} });
    const html = `<!doctype html><html><head></head><body style="margin:0"><div id="box"><div id="target" class="${candidate}" style="height:10px">x</div></div></body></html>`;
    await writeFile(resolve(root, "width/index.html"), html);
    wind(binary, root, windOutput);
    const windCss = await readFile(resolve(windOutput, "width/wind.css"), "utf8");
    const windReport = await readJson(resolve(windOutput, "width/report.json"));
    const compiler = await reference.compile("@theme { --*: initial; } @tailwind utilities;");
    const referenceCss = compiler.build([candidate]);
    const referenceDir = resolve(output, "reference-width", id);
    await mkdir(referenceDir, { recursive: true });
    const referenceInputPath = resolve(referenceDir, "reference-input.css");
    const referenceCssPath = resolve(referenceDir, "reference.css");
    await writeFile(referenceInputPath, "@theme { --*: initial; } @tailwind utilities;");
    await writeFile(referenceCssPath, referenceCss);
    artifacts.push({
      kind: "arbitrary-width",
      id,
      candidate,
      windCssPath: resolve(windOutput, "width/wind.css"),
      windCssSha256: sha256(windCss),
      referenceInputPath,
      referenceInputSha256: sha256("@theme { --*: initial; } @tailwind utilities;"),
      referenceCssPath,
      referenceCssSha256: sha256(referenceCss),
    });
    const probe = {
      name: `seeded-width-${width}`,
      property: "width",
      wind: `${width}px`,
      reference: `${width}px`,
      documentHtml: html,
    };
    const [pair] = await observePair(browser, windCss, referenceCss, candidate, [probe]);
    if (!pair.wind.verified || !pair.reference.verified)
      throw Error("Seeded width stylesheet transport failed");
    return (
      pair.wind.pass &&
      pair.reference.pass &&
      digest(windReport.rules) === digest([candidate]) &&
      digest(referenceCandidateHeads(referenceCss)) === digest([escapedCandidate(candidate)])
    );
  }
  const widths = generatedWidths();
  for (let index = 0; index < widths.length; index++) {
    const width = widths[index];
    if (!(await widthPass(width, `generated-${index}`))) {
      const shrunk = await shrinkWidthFailure(
        width,
        async (value) => !(await widthPass(value, `shrink-${index}-${value}`)),
      );
      await writeFile(
        resolve(output, "shrunk-failure.json"),
        JSON.stringify(
          {
            seed: checked.seed,
            kind: "arbitrary-width",
            specimenIndex: index,
            original: width,
            minimized: shrunk,
            promotionRequired:
              "Review and add a permanent upstream fixture and seed-regressions.json entry",
          },
          null,
          2,
        ) + "\n",
      );
      return {
        outcome: "unexpected-mismatch",
        kind: "arbitrary-width",
        specimenIndex: index,
        shrunk,
        artifacts,
      };
    }
  }
  const sources = generatedSourceStrings();
  const sourceRoot = resolve(output, "source-fixtures");
  const sourceOutput = resolve(output, "source-wind");
  await mkdir(sourceRoot, { recursive: true });
  await writeFile(
    resolve(sourceRoot, "manifest.json"),
    JSON.stringify({ caseIds: sources.map((_, index) => `source-${index}`) }),
  );
  for (let index = 0; index < sources.length; index++) {
    const id = `source-${index}`;
    const dir = resolve(sourceRoot, id);
    await mkdir(dir, { recursive: true });
    await writeFile(
      resolve(dir, "case.json"),
      JSON.stringify({
        caseId: id,
        reset: "none",
        sourceFile: "index.html",
        expectedCandidates: ["block", "hidden"],
      }),
    );
    await writeFile(resolve(dir, "index.html"), sources[index]);
  }
  wind(binary, sourceRoot, sourceOutput, "extract");
  const sourceFailures = [];
  for (let index = 0; index < sources.length; index++) {
    const id = `source-${index}`;
    const windReport = await readJson(resolve(sourceOutput, id, "report.json"));
    const scanned = scanOriginal(scanner.Scanner, sources[index], "html");
    const referenceInput = "@theme { --*: initial; } @tailwind utilities;";
    const compiler = await reference.compile(referenceInput);
    const referenceCss = compiler.build(scanned);
    const referenceDir = resolve(output, "reference-source", id);
    await mkdir(referenceDir, { recursive: true });
    const referenceInputPath = resolve(referenceDir, "reference-input.css");
    const referenceCssPath = resolve(referenceDir, "reference.css");
    const referenceCandidatesPath = resolve(referenceDir, "scanner-candidates.json");
    await writeFile(referenceInputPath, referenceInput);
    await writeFile(referenceCssPath, referenceCss);
    const candidatesJson = JSON.stringify(scanned, null, 2) + "\n";
    await writeFile(referenceCandidatesPath, candidatesJson);
    const windCss = await readFile(resolve(sourceOutput, id, "wind.css"));
    artifacts.push({
      kind: "source-string",
      id,
      sourcePath: resolve(sourceRoot, id, "index.html"),
      windCssPath: resolve(sourceOutput, id, "wind.css"),
      windCssSha256: sha256(windCss),
      referenceInputPath,
      referenceInputSha256: sha256(referenceInput),
      referenceCssPath,
      referenceCssSha256: sha256(referenceCss),
      referenceCandidatesPath,
      referenceCandidatesSha256: sha256(candidatesJson),
      sourceSha256: sha256(sources[index]),
    });
    const pass =
      digest(windReport.extractedCandidates) === digest(["block", "hidden"]) &&
      digest(windReport.rules) === digest(["block", "hidden"]) &&
      digest(scanned) === digest(["block", "class", "hidden"]) &&
      digest(referenceCandidateHeads(referenceCss)) === digest([".block", ".hidden"]) &&
      windReport.sourceBytes === Buffer.byteLength(sources[index]);
    if (!pass)
      sourceFailures.push({
        index,
        source: sources[index],
        windCandidates: windReport.extractedCandidates,
        referenceCandidates: scanned,
      });
  }
  if (sourceFailures.length) {
    const first = sourceFailures.sort((a, b) => a.source.length - b.source.length)[0];
    async function fails(candidate) {
      const id = "source-shrink";
      const root = resolve(output, "source-shrink-fixture", sha256(candidate).slice(0, 12));
      const dir = resolve(root, id);
      await mkdir(dir, { recursive: true });
      await writeFile(resolve(root, "manifest.json"), JSON.stringify({ caseIds: [id] }));
      await writeFile(
        resolve(dir, "case.json"),
        JSON.stringify({
          caseId: id,
          reset: "none",
          sourceFile: "index.html",
          expectedCandidates: ["block", "hidden"],
        }),
      );
      await writeFile(resolve(dir, "index.html"), candidate);
      const runOutput = resolve(output, "source-shrink-wind", sha256(candidate).slice(0, 12));
      wind(binary, root, runOutput, "extract");
      const report = await readJson(resolve(runOutput, id, "report.json"));
      return (
        digest(report.extractedCandidates) !== digest(["block", "hidden"]) ||
        digest(report.rules) !== digest(["block", "hidden"]) ||
        digest(scanOriginal(scanner.Scanner, candidate, "html")) !==
          digest(["block", "class", "hidden"])
      );
    }
    const minimized = await shrinkSourceFailure(first.source, fails);
    await writeFile(
      resolve(output, "shrunk-failure.json"),
      JSON.stringify(
        {
          seed: checked.seed,
          kind: "source-string",
          original: first,
          minimized,
          promotionRequired: "Review and add to extraction fixtures and seed-regressions.json",
        },
        null,
        2,
      ) + "\n",
    );
    return { outcome: "unexpected-mismatch", kind: "source-string", minimized, artifacts };
  }
  return {
    outcome: "matched",
    seed: checked.seed,
    permanentSpecimens: checked.permanentSpecimens.length,
    generatedSpecimens: generatedCount,
    generatedArbitraryWidths: widths,
    generatedSourceStrings: sources.length,
    scannerIdentity: scanner.identity,
    artifacts,
  };
}

async function main(argv) {
  const args = options(argv);
  const output = await outsideCheckout(args.output);
  await mkdir(output, { recursive: true });
  const [
    manifest,
    profile,
    empty,
    pilot,
    pilotObservations,
    extractionManifest,
    upstream,
    configurations,
    referenceSelection,
    inventory,
    catalog,
    contracts,
  ] = await Promise.all([
    readJson(resolve(corpusRoot, "manifest.json")),
    readJson(fromRoot("tests/wind-compatibility/profile.json")),
    readJson(fromRoot("tests/wind-compatibility/empty-token/manifest.json")),
    readJson(fromRoot("tests/wind-compatibility/pilot/manifest.json")),
    readJson(fromRoot("tests/wind-compatibility/pilot/observations.json")),
    readJson(fromRoot("tests/wind-compatibility/extraction/manifest.json")),
    readJson(resolve(corpusRoot, "upstream/manifest.json")),
    readJson(fromRoot("tests/wind-compatibility/empty-token/configurations.json")),
    readJson(
      args.reference
        ? resolve(args.reference)
        : fromRoot("tests/wind-compatibility/reference/bootstrap.json"),
    ),
    readJson(fromRoot("tests/wind-compatibility/inventory.v1.json")),
    readJson(fromRoot("crates/zudo-wind/catalog/zudo-wind-catalog.v1.json")),
    readJson(resolve(corpusRoot, "structure-contracts.json")),
  ]);
  const probesById = Object.fromEntries(
    await Promise.all(
      manifest.upstreamCases.map(async (row) => [
        row.id,
        await readJson(resolve(corpusRoot, "upstream", row.id, "probes.json")),
      ]),
    ),
  );
  const allExpected = validateCorpus(
    manifest,
    profile,
    empty,
    pilot,
    upstream,
    pilotObservations,
    probesById,
  );
  validateStructureContracts(contracts, manifest, empty);
  const actualDirs = (await readdir(resolve(corpusRoot, "upstream"), { withFileTypes: true }))
    .filter((x) => x.isDirectory())
    .map((x) => x.name)
    .sort();
  if (digest(actualDirs) !== digest(upstream.caseIds.toSorted()))
    throw Error("Upstream fixture import skipped or unlisted");
  const source = await verifyUpstreamProvenance(manifest, args.cache);
  const windBuild = await verifyWindBuild(args["wind-binary"], args["wind-build-manifest"]);
  const reference = await loadReference(args.cache, referenceSelection);
  await assertNativeReferenceBoundaries(reference);
  const scanner = await loadIndependentScanner(args.cache);
  const { browser, executablePath } = await browserFor(args.engine);
  let report;
  try {
    const browserEnvironment = await browserIdentity(browser, executablePath);
    if (
      !requiredMatrixMember(profile, browserEnvironment) ||
      browserEnvironment.name !== args.engine
    )
      throw Error(`Browser outside required matrix: ${args.engine}`);
    browserEnvironment.requiredMatrixMember = true;
    const executed = {};
    const pilotEvidence = await pilotResults(
      args["pilot-report"],
      manifest,
      profile,
      pilot,
      pilotObservations,
      extractionManifest,
      windBuild,
      reference,
      scanner,
      browserEnvironment,
      args["assessment-mode"] === "yes",
    );
    Object.assign(executed, pilotEvidence.results);
    if (args.engine === "chromium") {
      Object.assign(executed, await nativeResults(args["wind-binary"], manifest, empty, output));
      Object.assign(
        executed,
        await supplementalResults(
          args["wind-binary"],
          reference,
          empty,
          configurations,
          contracts,
          browser,
          output,
        ),
      );
    }
    const { results, cssById } = await upstreamResults(
      args["wind-binary"],
      reference,
      configurations,
      manifest,
      contracts,
      inventory,
      browser,
      args.engine,
      output,
    );
    Object.assign(executed, results);
    const controls = {
      pilot: pilotEvidence.controls,
      pilotReportId: pilotEvidence.reportId,
      pilotReportPath: pilotEvidence.reportPath,
    };
    if (args.engine === "chromium") {
      controls.mutations = await mutations(browser, cssById);
      controls.seeded = await seededControls(
        args["wind-binary"],
        reference,
        browser,
        scanner,
        output,
      );
    }
    const expected = allExpected.filter((id) => id.endsWith(`/${args.engine}`));
    const accounting = completeCorpus(expected, executed, expectedOutcomes(manifest, profile));
    const differenceSummary = manifest.reviewedDifferences.map((difference) => {
      const requiredCaseIds = difference.caseIds.filter((id) =>
        manifest.upstreamCases.find((row) => row.id === id)?.engines.includes(args.engine),
      );
      const observedCaseIds = requiredCaseIds.filter((id) =>
        executed[`upstream/${id}/${args.engine}`]?.reviewedDifferenceIds.includes(difference.id),
      );
      return {
        id: difference.id,
        requiredCaseIds,
        reviewedCount: requiredCaseIds.length,
        observedCaseIds,
        observedCount: observedCaseIds.length,
        pass: digest(requiredCaseIds) === digest(observedCaseIds),
      };
    });
    const differencesPass = differenceSummary.every((row) => row.pass);
    const identity = await currentCorpusIdentity({
      manifest,
      profile,
      contracts,
      pilotObservations,
      executed,
      engine: args.engine,
      source,
      reference,
      scanner,
      windBuild,
      browserEnvironment,
    });
    report = {
      schemaVersion: 1,
      kind: "wind-independent-corpus",
      issue: 3831,
      engine: args.engine,
      expected,
      executed,
      accounting,
      controls,
      differenceSummary,
      finiteInventory: finiteInventoryAccounting(inventory, catalog, manifest, empty, profile),
      identity,
      complete:
        accounting.complete &&
        differencesPass &&
        digest(Object.keys(controls.pilot)) === digest(profile.requiredControls) &&
        Object.values(controls.pilot).every((item) => item.outcome === "matched") &&
        controls.pilot["wrong-value-detection"]?.mutations?.wrongValueDetected === true &&
        controls.pilot["missing-rule-detection"]?.mutations?.removedStylesheetDetected === true &&
        controls.pilot["missing-rule-detection"]?.mutations?.wrongSelectorDetected === true &&
        (args.engine !== "chromium" ||
          (controls.mutations.outcome === "matched" && controls.seeded.outcome === "matched")),
    };
    report.exitCode = report.complete ? 0 : 1;
    report.reportId = `sha256:${digest(report)}`;
  } finally {
    await browser.close();
  }
  await writeFile(resolve(output, "report.json"), JSON.stringify(report, null, 2) + "\n");
  process.stdout.write(`${resolve(output, "report.json")}\n`);
  process.exitCode = report.exitCode;
}

main(process.argv.slice(2)).catch(async (error) => {
  console.error(error);
  try {
    const args = options(process.argv.slice(2));
    const output = await outsideCheckout(args.output);
    await mkdir(output, { recursive: true });
    const report = {
      schemaVersion: 1,
      kind: "wind-independent-corpus",
      issue: 3831,
      engine: args.engine,
      complete: false,
      exitCode: 1,
      infrastructure: { outcome: "infrastructure-failure", message: error.message },
    };
    report.reportId = `sha256:${digest(report)}`;
    await writeFile(resolve(output, "report.json"), JSON.stringify(report, null, 2) + "\n");
  } catch (reportError) {
    console.error("Unable to write corpus infrastructure report:", reportError);
  }
  process.exitCode = 1;
});
