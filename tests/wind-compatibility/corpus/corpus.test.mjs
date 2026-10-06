import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { spawnSync } from "node:child_process";
import {
  completeCorpus,
  expectedObligations,
  expectedOutcomes,
  finiteInventoryAccounting,
  validateCorpus,
  validateTargetedExecutionKeys,
  checkMutations,
} from "../../../scripts/wind-compatibility/corpus-core.mjs";
import {
  generatedCandidateLists,
  generatedSourceStrings,
  generatedWidths,
  seed,
  shrinkFailure,
  shrinkSourceFailure,
  shrinkWidthFailure,
} from "../../../scripts/wind-compatibility/corpus-seeds.mjs";
import {
  canonical,
  compareCorpusStructure,
  expectedWindTree,
  validateStructureContracts,
} from "../../../scripts/wind-compatibility/corpus-structure.mjs";
import { supplementalProbePlan } from "../../../scripts/wind-compatibility/corpus-supplemental.mjs";
import {
  signedPilotReportId,
  validatePilotAssessmentEnvelope,
  validatePilotEnvelope,
  validatePilotCorruptionControls,
  validatePilotControlPairs,
  validatePilotNativeObservations,
} from "../../../scripts/wind-compatibility/corpus-pilot.mjs";
import {
  documentFor,
  hostPlatformIdentity,
  requiredMatrixMember,
} from "../../../scripts/wind-compatibility/browser-adapter.mjs";
import { parseCssStructure } from "../../../scripts/wind-compatibility/structure.mjs";

const root = new URL("./", import.meta.url);
const json = async (path) => JSON.parse(await readFile(new URL(path, root), "utf8"));
const [manifest, profile, empty, pilot, observations, upstream] = await Promise.all([
  json("manifest.json"),
  json("../profile.json"),
  json("../empty-token/manifest.json"),
  json("../pilot/manifest.json"),
  json("../pilot/observations.json"),
  json("upstream/manifest.json"),
]);
const probes = Object.fromEntries(
  await Promise.all(
    manifest.upstreamCases.map(async (row) => [
      row.id,
      await json(`upstream/${row.id}/probes.json`),
    ]),
  ),
);
const contracts = await json("structure-contracts.json");
const copy = (value) => structuredClone(value);
const validate = (
  m = manifest,
  p = profile,
  e = empty,
  pi = pilot,
  u = upstream,
  o = observations,
  pr = probes,
) => validateCorpus(m, p, e, pi, u, o, pr);

test("exact corpus membership, source adaptations and composition probes are accounted", () => {
  const expected = validate();
  assert.equal(manifest.pilotCaseIds.length, 15);
  assert.equal(new Set(manifest.pilotCaseIds).size, 15);
  assert.equal(manifest.pilotCaseIds.filter((id) => id === "contents-gap").length, 1);
  assert.deepEqual(
    manifest.pilotPolicies.find((row) => row.id === "contents-gap"),
    {
      id: "contents-gap",
      disposition: "equivalent-shared",
      reviewedDifferenceIds: ["wind-layer-order-prelude"],
    },
  );
  assert.equal(expected.length, 120);
  assert.equal(expected.filter((id) => id.endsWith("/chromium")).length, 66);
  assert.equal(expected.filter((id) => id.endsWith("/firefox")).length, 27);
  assert.equal(expected.filter((id) => id.endsWith("/webkit")).length, 27);
  assert.equal(manifest.counts.upstreamProbes, 81);
});

test("native display and appearance fixtures use the adapter target and separate exact probes", async () => {
  for (const id of ["native-display", "native-appearance"]) {
    const row = manifest.upstreamCases.find((item) => item.id === id);
    const html = await readFile(new URL("upstream/" + id + "/index.html", root), "utf8");
    const target = /<[^>]*id="target"[^>]*class="([^"]+)"[^>]*>/.exec(html)?.[1];
    assert.ok(html.includes('id="box"'), id);
    assert.deepEqual(target?.split(/\s+/), row.candidates, id);
    for (const probe of probes[id])
      assert.ok(html.includes('id="' + probe.selector.slice(1) + '"'));
  }
  const displayHtml = await readFile(new URL("upstream/native-display/index.html", root), "utf8");
  for (const tag of [
    '<table id="table"',
    '<caption id="table-caption"',
    '<colgroup id="table-column-group"',
    '<col id="table-column"',
    '<thead id="table-header-group"',
    '<tbody id="table-row-group"',
    '<tr id="table-row"',
    '<td id="table-cell"',
    '<tfoot id="table-footer-group"',
  ])
    assert.ok(displayHtml.includes(tag), tag);
});

test("native flex, SVG and reversal cases retain their exact probe and engine policy", async () => {
  const expected = {
    "native-order": [6, ["chromium"]],
    "native-basis": [4, ["chromium"]],
    "native-svg": [4, ["chromium"]],
    "native-not-sr-only": [4, ["chromium", "firefox", "webkit"]],
    "native-sr-reversal": [8, ["chromium", "firefox", "webkit"]],
  };
  for (const [id, [count, engines]] of Object.entries(expected)) {
    const row = manifest.upstreamCases.find((item) => item.id === id);
    assert.deepEqual(row.engines, engines);
    assert.equal(probes[id].length, count);
    const html = await readFile(new URL(`upstream/${id}/index.html`, root), "utf8");
    assert.ok(html.includes('id="target"'));
    assert.ok(html.includes('id="box"'));
    for (const candidate of row.candidates)
      assert.ok(html.includes(candidate), `${id}/${candidate}`);
    for (const probe of probes[id]) {
      assert.ok(
        html.includes(`id="${(probe.selector ?? "#target").slice(1)}"`),
        `${id}/${probe.name}`,
      );
    }
  }
  assert.deepEqual(
    manifest.nativeGuaranteeIds.slice(-5),
    ["zero", "min", "max", "fit", "content"].map((name) => `native-basis-${name}`),
  );
  assert.deepEqual(
    empty.nativeGuarantees.slice(-5).map(({ writes }) => writes),
    [
      { "flex-basis": "0" },
      { "flex-basis": "min-content" },
      { "flex-basis": "max-content" },
      { "flex-basis": "fit-content" },
      { "flex-basis": "content" },
    ],
  );
  const downgraded = copy(manifest);
  downgraded.upstreamCases.find((row) => row.id === "native-sr-reversal").engines = ["chromium"];
  assert.throws(() => validate(downgraded), /Engine obligation downgraded/);
});

test("targeted execution map binds every policy obligation to current pilot/control evidence", () => {
  const mapping = validateTargetedExecutionKeys(manifest, profile);
  assert.deepEqual(Object.keys(mapping), profile.browserPolicy.targetedObligations);
  assert.deepEqual(mapping["wrong-value-or-missing-rule-detection"], [
    "control/wrong-value-detection",
    "control/missing-rule-detection",
  ]);
  for (const value of [
    undefined,
    ["pilot/p-0-mapped", "pilot/p-0-mapped"],
    ["pilot/p-0-mapped/chromium"],
    ["pilot/renamed"],
  ]) {
    const changed = copy(manifest);
    if (value === undefined) delete changed.targetedExecutionKeys["p-0-mapped"];
    else changed.targetedExecutionKeys["p-0-mapped"] = value;
    assert.throws(() => validate(changed), /Targeted browser execution mapping/);
  }
  const oneCorruption = copy(manifest);
  oneCorruption.targetedExecutionKeys["wrong-value-or-missing-rule-detection"].pop();
  assert.throws(() => validate(oneCorruption), /Targeted browser execution mapping/);
});

test("missing fixtures, duplicate IDs, empty candidate lists and missing execution fail", () => {
  const missing = copy(manifest);
  missing.upstreamCases.pop();
  assert.throws(() => validate(missing), /membership|counts/);
  const duplicate = copy(manifest);
  duplicate.upstreamCases[1].id = duplicate.upstreamCases[0].id;
  assert.throws(() => validate(duplicate), /membership|duplicate/);
  const emptyCandidates = copy(manifest);
  emptyCandidates.upstreamCases[0].candidates = [];
  assert.throws(() => validate(emptyCandidates), /Empty or duplicate candidates/);
  const obligations = expectedObligations(manifest);
  const result = completeCorpus(
    obligations,
    Object.fromEntries(obligations.slice(1).map((id) => [id, { outcome: "matched" }])),
  );
  assert.equal(result.complete, false);
  assert.deepEqual(result.missing, [obligations[0]]);
  assert.throws(() => completeCorpus(obligations, { bogus: { outcome: "matched" } }), /Unreviewed/);
  const allowed = expectedOutcomes(manifest, profile);
  const downgraded = Object.fromEntries(obligations.map((id) => [id, { outcome: allowed[id] }]));
  downgraded["upstream/display-flex/chromium"].outcome = "expected-unsupported";
  assert.equal(completeCorpus(obligations, downgraded, allowed).complete, false);
  assert.deepEqual(completeCorpus(obligations, downgraded, allowed).failures, [
    "upstream/display-flex/chromium",
  ]);
  const missingFirefoxPilot = { ...downgraded };
  missingFirefoxPilot["upstream/display-flex/chromium"] = { outcome: "matched" };
  delete missingFirefoxPilot["pilot/mx-auto/firefox"];
  assert.deepEqual(completeCorpus(obligations, missingFirefoxPilot, allowed).missing, [
    "pilot/mx-auto/firefox",
  ]);
  const coordinatedPilotDeletion = copy(profile);
  const coordinatedManifestDeletion = copy(manifest);
  const coordinatedPilotFixtureDeletion = copy(pilot);
  const coordinatedObservationDeletion = copy(observations);
  coordinatedPilotDeletion.requiredCases.pop();
  coordinatedManifestDeletion.pilotCaseIds.pop();
  coordinatedPilotFixtureDeletion.caseIds.pop();
  delete coordinatedObservationDeletion["breakpoint-block"];
  assert.throws(
    () =>
      validate(
        coordinatedManifestDeletion,
        coordinatedPilotDeletion,
        empty,
        coordinatedPilotFixtureDeletion,
        upstream,
        coordinatedObservationDeletion,
      ),
    /reviewed 15-case set/,
  );
});

test("downgrades and recategorized reviewed differences fail policy validation", () => {
  const downgrade = copy(profile);
  downgrade.requiredCases[0].disposition = "implementation-gap";
  assert.throws(() => validate(manifest, downgrade), /policy changed/);
  const difference = copy(profile);
  difference.requiredCases[0].reviewedDifferenceIds = [];
  assert.throws(() => validate(manifest, difference), /policy changed/);
  const staleContents = copy(profile);
  staleContents.requiredCases.find((row) => row.id === "contents-gap").disposition =
    "implementation-gap";
  const staleContentsManifest = copy(manifest);
  staleContentsManifest.pilotPolicies.find((row) => row.id === "contents-gap").disposition =
    "implementation-gap";
  assert.throws(() => validate(staleContentsManifest, staleContents), /contents-gap/);
  const reclassified = copy(manifest);
  reclassified.upstreamCases.find((x) => x.id === "padding-axis").reviewedDifferenceIds = [];
  assert.throws(() => validate(reclassified), /Unreviewed difference/);
  const removedProbe = copy(probes);
  removedProbe["dark-flex"] = removedProbe["dark-flex"].filter((x) => x.name !== "ancestor");
  assert.throws(
    () => validate(manifest, profile, empty, pilot, upstream, observations, removedProbe),
    /probe accounting|Composition reference/,
  );
  const skippedProbe = copy(probes);
  skippedProbe["hover-flex"].find((x) => x.name === "active").engines = [];
  assert.throws(
    () => validate(manifest, profile, empty, pilot, upstream, observations, skippedProbe),
    /observation policy/,
  );
});

test("valid and mutated controls cannot both pass", () => {
  const valid = { "display-valid": true, "hover-valid": true };
  const mutated = {
    "missing-stylesheet": false,
    "missing-rule": false,
    "wrong-declaration": false,
    "wrong-selector": false,
    "wrong-media": false,
  };
  assert.equal(checkMutations(valid, mutated).killedMutations, 5);
  assert.throws(() => checkMutations(valid, { ...mutated, "wrong-media": true }), /survived/);
  assert.throws(() => checkMutations({ ...valid, "hover-valid": false }, mutated), /Valid control/);
  assert.throws(() => checkMutations({}, {}), /skipped/);
  const oneMissing = { ...mutated };
  delete oneMissing["wrong-selector"];
  assert.throws(() => checkMutations(valid, oneMissing), /skipped/);
});

test("engine and named condition downgrades fail", () => {
  const engineDowngrade = copy(manifest);
  engineDowngrade.upstreamCases.find((row) => row.id === "hover-flex").engines = ["chromium"];
  assert.throws(() => validate(engineDowngrade), /Engine obligation downgraded/);
  const conditionDowngrade = copy(manifest);
  conditionDowngrade.compositionObligations["keyboard-focus"] = ["pilot/block/display"];
  assert.throws(() => validate(conditionDowngrade), /Composition condition membership changed/);
});

test("exact structure detects extra declarations, layers, selector, media and order", () => {
  assert.equal(validateStructureContracts(contracts, manifest, empty), true);
  const alteredContract = copy(contracts);
  alteredContract.cases["display-flex"].rules[0].writes[0][1] = "grid";
  assert.throws(
    () => validateStructureContracts(alteredContract, manifest, empty),
    /expectations changed/,
  );
  const emit = (nodes) =>
    nodes
      .map((node) =>
        node.kind === "statement"
          ? `${node.text};`
          : node.kind === "declaration"
            ? `${node.name}:${node.value};`
            : `${node.head}{${emit(node.children)}}`,
      )
      .join("");
  const block = contracts.cases["display-flex"];
  const validCss = emit(expectedWindTree(block));
  assert.equal(compareCorpusStructure(block, validCss, "").windPass, true);
  for (const corrupted of [
    validCss.replace("display:flex;", "display:flex;color:red;"),
    validCss.replace(".flex{", "@layer base{.flex{") + "}",
    validCss.replace(".flex{", ".wrong{"),
    validCss.replace("display:flex;", "display:grid;"),
  ])
    assert.equal(compareCorpusStructure(block, corrupted, "").windPass, false);
  const hover = contracts.cases["hover-flex"];
  const hoverCss = emit(expectedWindTree(hover));
  assert.equal(
    compareCorpusStructure(hover, hoverCss.replace("hover: hover", "hover: none"), "").windPass,
    false,
  );
  const ordered = contracts.cases["flex-direction"];
  const orderedCss = emit(expectedWindTree(ordered));
  assert.equal(
    compareCorpusStructure(
      ordered,
      orderedCss.replace(/(\.flex\{[^}]+\})(\.flex-row\{[^}]+\})/, "$2$1"),
      "",
    ).windPass,
    false,
  );
  const nativeDisplay = contracts.cases["native-display"];
  const nativeDisplayWind = emit(expectedWindTree(nativeDisplay));
  const nativeDisplayReference =
    ".contents{display:contents;}.flow-root{display:flow-root;}.inline-table{display:inline-table;}.list-item{display:list-item;}.table{display:table;}.table-caption{display:table-caption;}.table-cell{display:table-cell;}.table-column{display:table-column;}.table-column-group{display:table-column-group;}.table-footer-group{display:table-footer-group;}.table-header-group{display:table-header-group;}.table-row{display:table-row;}.table-row-group{display:table-row-group;}";
  const nativeDisplayResult = compareCorpusStructure(
    nativeDisplay,
    nativeDisplayWind,
    nativeDisplayReference,
  );
  assert.equal(nativeDisplayResult.pass, true);
  assert.equal(nativeDisplay.rules.length, 13);
  const nativeAppearance = contracts.cases["native-appearance"];
  const nativeAppearanceWind = emit(expectedWindTree(nativeAppearance));
  const nativeAppearanceReference =
    ".appearance-auto{appearance:auto;}.appearance-none{appearance:none;}";
  assert.equal(
    compareCorpusStructure(nativeAppearance, nativeAppearanceWind, nativeAppearanceReference).pass,
    true,
  );
  const reversal = contracts.cases["native-not-sr-only"];
  const reversalWind = emit(expectedWindTree(reversal));
  const reversalReference =
    ".not-sr-only{position:static;width:auto;height:auto;padding:0;margin:0;overflow:visible;clip-path:none;white-space:normal;}";
  assert.equal(compareCorpusStructure(reversal, reversalWind, reversalReference).pass, true);
  assert.equal(
    compareCorpusStructure(reversal, reversalWind.replace("clip:auto;", ""), reversalReference)
      .windPass,
    false,
  );
  assert.equal(nativeAppearance.rules.length, 2);
  const quoted = (css) => parseCssStructure(css).map(canonical);
  assert.notDeepEqual(quoted(".x{content:'a  b';}"), quoted(".x{content:'a b';}"));
  assert.notDeepEqual(quoted(".x{content:'.5';}"), quoted(".x{content:'0.5';}"));
  assert.notDeepEqual(
    quoted('.x[data-note="a  b"]{display:block;}'),
    quoted('.x[data-note="a b"]{display:block;}'),
  );
});

test("finite mapped static domain reports covered and omitted members", async () => {
  const inventory = await json("../inventory.v1.json");
  const catalog = await json("../../../crates/zudo-wind/catalog/zudo-wind-catalog.v1.json");
  const accounting = finiteInventoryAccounting(inventory, catalog, manifest, empty, profile);
  assert.equal(accounting.domainCount, accounting.coveredCount + accounting.omittedCount);
  assert.ok(accounting.coveredRowIds.includes("utility-static:block"));
  assert.ok(accounting.omittedCount > 0);
  assert.equal(accounting.exhaustive, false);
  assert.equal(
    accounting.keywordDomain.domainCount,
    accounting.keywordDomain.coveredCount + accounting.keywordDomain.omittedCount,
  );
  assert.ok(accounting.keywordDomain.omittedCount > 0);
});

test("seeded candidate permutations are deterministic and failures shrink", async () => {
  assert.equal(seed, 0x3831c0de);
  const lists = generatedCandidateLists();
  assert.equal(lists.length, 8);
  for (const list of lists)
    assert.deepEqual([...list].sort(), ["flex", "flex", "flex-row", "relative"].sort());
  const minimized = await shrinkFailure(
    ["relative", "flex", "flex-row", "flex"],
    async (values) => values.includes("flex-row") && values.includes("flex"),
  );
  assert.deepEqual(minimized, ["flex-row", "flex"]);
  await assert.rejects(() => shrinkFailure(["relative"], async () => false), /passing specimen/);
  const widths = generatedWidths();
  assert.equal(widths.length, 6);
  assert.deepEqual(widths.slice(0, 3), [0, 1, 17]);
  assert.ok(widths.every((value) => Number.isInteger(value) && value >= 0 && value <= 255));
  assert.equal(await shrinkWidthFailure(17, async (value) => value >= 1), 1);
  const sourceStrings = generatedSourceStrings();
  assert.equal(sourceStrings.length, 6);
  assert.ok(
    sourceStrings.every(
      (value) => value.includes('class="') && value.includes("block") && value.includes("hidden"),
    ),
  );
  assert.equal(
    await shrinkSourceFailure(sourceStrings[0], async () => true),
    '<span class="block hidden">x</span>',
  );
});

test("supplemental observations have exact target/control and metadata accounting", async () => {
  const html = await readFile(new URL("../empty-token/index.html", root), "utf8");
  const counts = [17, 17, 2, 2];
  empty.supplementalCaseIds.forEach((id, index) => {
    const row = empty.cases.find((item) => item.caseId === id);
    const plan = supplementalProbePlan(row, html);
    assert.equal(plan.length, counts[index]);
    assert.ok(plan.some((item) => item.role === "target"));
    assert.ok(plan.some((item) => item.role === "control"));
  });
  const original = empty.cases.find((item) => item.caseId === "numeric-spacing-removed");
  const altered = () => copy(original);
  const emptyObject = altered();
  emptyObject.browserExpected["p-4"] = {};
  assert.throws(() => supplementalProbePlan(emptyObject, html), /observable keys/);
  const unknown = altered();
  unknown.browserExpected["p-4"].invented = "24px";
  assert.throws(() => supplementalProbePlan(unknown, html), /observable keys/);
  const malformed = altered();
  malformed.browserExpected["p-4"].padding = { value: "24px" };
  assert.throws(() => supplementalProbePlan(malformed, html), /malformed/);
  const metadata = altered();
  metadata.browserExpected["p-4"].previousConfiguredPadding = "24px";
  assert.throws(() => supplementalProbePlan(metadata, html), /transition metadata/);
  const missingControl = altered();
  delete missingControl.browserControls["p-4"];
  assert.throws(() => supplementalProbePlan(missingControl, html), /membership/);
  assert.throws(
    () =>
      supplementalProbePlan(original, html.replace(`id="${original.browserTargets["p-4"]}"`, "")),
    /source membership/,
  );
  assert.throws(
    () =>
      supplementalProbePlan(original, html + `<div id="${original.browserTargets["p-4"]}"></div>`),
    /source membership/,
  );
});

test("pilot admission rejects edited and rehashed row sets, outcomes and identities", async () => {
  const extraction = await json("../extraction/manifest.json");
  const identity = { proof: "current-inputs" };
  const outcome = {
    "equivalent-shared": "matched",
    "equivalent-mapped": "mapped-match",
    "intentional-difference": "reviewed-difference",
    "implementation-gap": "expected-unsupported",
  };
  const valid = {
    schemaVersion: 1,
    kind: "wind-differential-pilot",
    identity,
    pilotCaseIds: pilot.caseIds,
    cases: profile.requiredCases.map((row) => ({
      caseId: row.id,
      disposition: row.disposition,
      outcome: outcome[row.disposition],
      checks: { identity: true, structure: true, observations: true, diagnostics: true },
      structure: { pass: true },
      observations: Array(observations[row.id].probes.length).fill({}),
    })),
    controls: profile.requiredControls.map((controlId) => ({ controlId, outcome: "matched" })),
    extraction: extraction.caseIds.map((caseId) => ({ caseId, outcome: "matched" })),
    admission: "pilot-only",
    complete: true,
    exitCode: 0,
  };
  valid.reportId = signedPilotReportId(valid);
  assert.equal(
    validatePilotEnvelope(valid, profile, pilot, observations, extraction, identity),
    true,
  );
  assert.throws(
    () =>
      validatePilotEnvelope(valid, profile, pilot, observations, extraction, {
        browserEnvironment: { name: "firefox" },
      }),
    /Pilot identity differs/,
  );
  const changed = (mutate) => {
    const report = copy(valid);
    mutate(report);
    report.reportId = signedPilotReportId(report);
    return report;
  };
  const observedMismatch = changed((report) => {
    report.cases[0].outcome = "unexpected-mismatch";
    report.cases[0].checks.structure = false;
    report.complete = false;
    report.exitCode = 1;
  });
  assert.equal(
    validatePilotAssessmentEnvelope(
      observedMismatch,
      profile,
      pilot,
      observations,
      extraction,
      identity,
    ),
    true,
  );
  assert.throws(
    () =>
      validatePilotEnvelope(observedMismatch, profile, pilot, observations, extraction, identity),
    /incomplete or invalid admission/,
  );
  assert.throws(
    () =>
      validatePilotAssessmentEnvelope(
        changed((report) => {
          report.cases.pop();
          report.complete = false;
          report.exitCode = 1;
        }),
        profile,
        pilot,
        observations,
        extraction,
        identity,
      ),
    /assessment case rows/,
  );
  assert.throws(
    () =>
      validatePilotAssessmentEnvelope(
        changed((report) => {
          report.cases[0].outcome = "infrastructure-failure";
          report.complete = false;
          report.exitCode = 1;
        }),
        profile,
        pilot,
        observations,
        extraction,
        identity,
      ),
    /truncated or inconsistent/,
  );
  assert.throws(
    () =>
      validatePilotEnvelope(
        changed((report) => {
          report.cases[1].caseId = report.cases[0].caseId;
        }),
        profile,
        pilot,
        observations,
        extraction,
        identity,
      ),
    /case rows/,
  );
  assert.throws(
    () =>
      validatePilotEnvelope(
        changed((report) => {
          report.controls[1].controlId = report.controls[0].controlId;
        }),
        profile,
        pilot,
        observations,
        extraction,
        identity,
      ),
    /control rows/,
  );
  assert.throws(
    () =>
      validatePilotEnvelope(
        changed((report) => {
          report.extraction.pop();
        }),
        profile,
        pilot,
        observations,
        extraction,
        identity,
      ),
    /row count/,
  );
  assert.throws(
    () =>
      validatePilotEnvelope(
        changed((report) => {
          report.cases[0].outcome = "expected-unsupported";
        }),
        profile,
        pilot,
        observations,
        extraction,
        identity,
      ),
    /downgraded/,
  );
  assert.throws(
    () =>
      validatePilotEnvelope(
        changed((report) => {
          report.identity.proof = "stale";
        }),
        profile,
        pilot,
        observations,
        extraction,
        identity,
      ),
    /identity/,
  );
  assert.throws(
    () =>
      validatePilotEnvelope(
        { ...valid, reportId: "sha256:wrong" },
        profile,
        pilot,
        observations,
        extraction,
        identity,
      ),
    /reportId/,
  );
});

test("assessment mode is explicit and cannot be mislabeled", () => {
  const runner = new URL("../../../scripts/wind-compatibility/corpus-runner.mjs", import.meta.url);
  const result = spawnSync(
    process.execPath,
    [
      runner.pathname,
      "--wind-binary",
      "missing",
      "--wind-build-manifest",
      "missing",
      "--output",
      "/tmp/wind-invalid-assessment-mode",
      "--pilot-report",
      "missing",
      "--assessment-mode",
      "no",
    ],
    { encoding: "utf8" },
  );
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /--assessment-mode must be yes/);
});

test("pilot corruption controls require both observed failures and served CSS hashes", async () => {
  const { createHash } = await import("node:crypto");
  const hash = (value) => createHash("sha256").update(value).digest("hex");
  const html = await readFile(new URL("../pilot/block/index.html", root), "utf8");
  const probe = observations.block.probes[0];
  const css = ".block { display: block; }";
  const wrong = css.replace(/display:\s*block/, "display: none");
  const selector = css.replace(/\.block\s*\{/, ".wrong { ");
  const negative = (style) => ({
    engine: "wind",
    probe: probe.name,
    verified: true,
    pass: false,
    observation: {
      value: "inline",
      cssTree: [],
      cssRules: [],
      box: {
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        parentX: 0,
        parentY: 0,
        parentWidth: 100,
        parentHeight: 100,
      },
    },
    expected: probe.wind,
    stylesheetSha256: hash(style),
    servedSha256: hash(style),
    settings: {
      documentSha256: hash(html),
      authoredCssSha256: hash(""),
      selector: "#target",
      viewportWidth: 800,
      viewportHeight: 700,
      writingMode: "horizontal-tb",
      direction: "ltr",
      hover: false,
      keyboardFocus: false,
      pseudo: null,
      relativeTo: "#box",
    },
  });
  const controls = [
    {
      controlId: "missing-rule-detection",
      mutations: {
        removedStylesheetDetected: true,
        wrongSelectorDetected: true,
        removedStylesheet: negative(""),
        wrongSelector: negative(selector),
      },
    },
    {
      controlId: "wrong-value-detection",
      mutations: { wrongValueDetected: true, wrongValue: negative(wrong) },
    },
  ];
  assert.equal(validatePilotCorruptionControls(controls, css, probe, html), true);
  const missing = copy(controls);
  missing[1].mutations.wrongValue = null;
  assert.throws(
    () => validatePilotCorruptionControls(missing, css, probe, html),
    /browser controls/,
  );
  const forged = copy(controls);
  forged[0].mutations.removedStylesheet.servedSha256 = hash("wrong");
  assert.throws(
    () => validatePilotCorruptionControls(forged, css, probe, html),
    /browser controls/,
  );
  const passed = copy(controls);
  passed[1].mutations.wrongValue.pass = true;
  assert.throws(
    () => validatePilotCorruptionControls(passed, css, probe, html),
    /browser controls/,
  );
  const emptyObservation = copy(controls);
  emptyObservation[1].mutations.wrongValue.observation = {};
  assert.throws(
    () => validatePilotCorruptionControls(emptyObservation, css, probe, html),
    /browser controls/,
  );
});

test("pilot native and scoped controls bind exact ordered probes, sides, settings and raw CSS", async () => {
  const { createHash } = await import("node:crypto");
  const hash = (value) => createHash("sha256").update(value).digest("hex");
  const html = await readFile(new URL("../native/native-reset/index.html", root), "utf8");
  const windCss = ".block { display: block; }";
  const referenceCss = ".block { display: block; }";
  const names = [
    "native-reset-box-sizing",
    "native-authored-cascade",
    "native-heading",
    "native-list",
    "native-border",
    "native-form",
  ];
  const selectors = ["#target", "#target", "#heading", "#list", "#border", "#form"];
  const expected = [
    ["border-box", "border-box"],
    ["inline", "inline"],
    ["32px", "16px"],
    ["disc", "none"],
    ["inset", "solid"],
    ["outset", "solid"],
  ];
  const probes = names.map((name, index) => ({
    name,
    selector: selectors[index],
    wind: expected[index][0],
    reference: expected[index][1],
    documentHtml: html,
  }));
  const row = (side, css, candidate, probe) => ({
    engine: side,
    probe: probe.name,
    expected: probe[side],
    pass: true,
    verified: true,
    stylesheetSha256: hash(css),
    servedSha256: hash(css),
    observation: {
      value: probe[side],
      cssTree: [],
      cssRules: [],
      box: {
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        parentX: 0,
        parentY: 0,
        parentWidth: 100,
        parentHeight: 100,
      },
    },
    settings: {
      documentSha256: hash(probe.documentHtml),
      servedDocumentSha256: hash(documentFor(candidate, probe)),
      authoredCssSha256: hash(probe.authoredCss ?? ""),
      scopeVars: probe.scopeVars ?? "",
      selector: probe.selector ?? "#target",
      viewportWidth: probe.viewportWidth ?? 800,
      viewportHeight: 700,
      writingMode: probe.writingMode ?? "horizontal-tb",
      direction: probe.direction ?? "ltr",
      hover: Boolean(probe.hover),
      keyboardFocus: Boolean(probe.keyboardFocus),
      pseudo: probe.pseudo ?? null,
      relativeTo: probe.relativeTo ?? "#box",
    },
  });
  const native = probes.flatMap((probe) => [
    row("wind", windCss, "block native", probe),
    row("reference", referenceCss, "block native", probe),
  ]);
  assert.equal(validatePilotNativeObservations(native, probes, windCss, referenceCss), true);
  for (const change of [
    (rows) => rows.fill(copy(rows[0])),
    (rows) => {
      rows[5] = copy(rows[0]);
    },
    (rows) => {
      rows.pop();
    },
    (rows) => {
      rows[3].settings.selector = "#wrong";
    },
    (rows) => {
      rows[7].stylesheetSha256 = hash("stale");
    },
  ]) {
    const changed = copy(native);
    change(changed);
    assert.throws(
      () => validatePilotNativeObservations(changed, probes, windCss, referenceCss),
      /native control probe/,
    );
  }
  const cascadeProbe = {
    name: "authored-cascade",
    selector: "#target",
    wind: "inline",
    reference: "inline",
    authoredCss: ".block { display: inline; }",
    documentHtml: html,
  };
  const pair = [
    {
      wind: row("wind", windCss, "block", cascadeProbe),
      reference: row("reference", referenceCss, "block", cascadeProbe),
    },
  ];
  assert.equal(
    validatePilotControlPairs(pair, [cascadeProbe], windCss, referenceCss, "block"),
    true,
  );
  for (const change of [
    (items) => {
      delete items[0].wind.stylesheetSha256;
      delete items[0].wind.servedSha256;
    },
    (items) => {
      items[0].reference.engine = "wrong-side";
    },
    (items) => {
      items[0].wind.probe = "unrelated";
    },
    (items) => {
      items[0].reference.settings.scopeVars = "unexpected";
    },
  ]) {
    const changed = copy(pair);
    change(changed);
    assert.throws(
      () => validatePilotControlPairs(changed, [cascadeProbe], windCss, referenceCss, "block"),
      /control probe/,
    );
  }
});

test("browser matrix fails closed for distro, architecture and toolchain drift", () => {
  const host = hostPlatformIdentity("linux", "x64", 'ID=ubuntu\nVERSION_ID="24.04"\n');
  assert.deepEqual(host, {
    hostPlatform: "ubuntu24.04-x64",
    linuxDistribution: { id: "ubuntu", versionId: "24.04" },
  });
  assert.equal(hostPlatformIdentity("linux", "x64", "ID=ubuntu\n").hostPlatform, null);
  assert.equal(
    hostPlatformIdentity("linux", "arm64", "ID=ubuntu\nVERSION_ID=24.04\n").hostPlatform,
    "ubuntu24.04-arm64",
  );
  const browser = {
    platform: "linux",
    architecture: "x64",
    hostPlatform: host.hostPlatform,
    linuxDistribution: host.linuxDistribution,
    name: "chromium",
    revision: "1228",
    version: "149.0.7827.55",
    manifestBrowserVersion: "149.0.7827.55",
    playwrightVersion: profile.browserPolicy.playwrightTestVersion,
    playwrightCoreVersion: profile.browserPolicy.playwrightCoreVersion,
    browserManifestSha256: profile.browserPolicy.browserManifest.sha256,
  };
  assert.equal(requiredMatrixMember(profile, browser), true);
  for (const changed of [
    { hostPlatform: "debian12-x64" },
    { hostPlatform: "ubuntu24.04-arm64" },
    { architecture: "arm64" },
    { linuxDistribution: { id: "debian", versionId: "12" } },
    { playwrightVersion: "0.0.0" },
    { playwrightCoreVersion: "0.0.0" },
    { browserManifestSha256: "0".repeat(64) },
    { manifestBrowserVersion: "0.0.0" },
  ])
    assert.equal(requiredMatrixMember(profile, { ...browser, ...changed }), false);
});
