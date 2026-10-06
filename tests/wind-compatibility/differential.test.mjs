import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fromRoot } from "../../scripts/wind-compatibility/reference.mjs";
import {
  classify,
  compareExtractionSets,
  completeReport,
  outcomes,
  preludeCheck,
  validatePilot,
} from "../../scripts/wind-compatibility/differential-core.mjs";
import {
  compareStructure,
  expectedStructure,
  parseCssStructure,
  observedDifferenceIds,
} from "../../scripts/wind-compatibility/structure.mjs";
import { diagnosticCheck } from "../../scripts/wind-compatibility/differential-runner.mjs";

const profile = JSON.parse(await readFile(fromRoot("tests/wind-compatibility/profile.json")));
const manifest = JSON.parse(
  await readFile(fromRoot("tests/wind-compatibility/pilot/manifest.json")),
);
const observations = JSON.parse(
  await readFile(fromRoot("tests/wind-compatibility/pilot/observations.json")),
);

test("pilot is a complete, reviewed profile manifest", () => {
  const rows = validatePilot(profile, manifest, observations);
  assert.deepEqual([...rows.keys()].sort(), [...manifest.caseIds].sort());
  assert.equal(manifest.caseIds.length, 15);
  assert.throws(() =>
    validatePilot(profile, { caseIds: [...manifest.caseIds, "invented"] }, observations),
  );
  for (const mutate of [
    (value) => value.caseIds.pop(),
    (value) => (value.caseIds[1] = value.caseIds[0]),
    (value) => (value.caseIds[1] = "invented"),
  ]) {
    const changed = structuredClone(manifest);
    mutate(changed);
    assert.throws(() => validatePilot(profile, changed, observations), /reviewed 15-case set/);
  }
  for (const mutate of [
    (value) => value.requiredCases.pop(),
    (value) => (value.requiredCases[1].id = value.requiredCases[0].id),
    (value) => (value.requiredCases[1].id = "invented"),
  ]) {
    const changed = structuredClone(profile);
    mutate(changed);
    assert.throws(() => validatePilot(changed, manifest, observations), /reviewed 15-case set/);
  }
  const altered = structuredClone(observations);
  delete altered.block;
  assert.throws(() => validatePilot(profile, manifest, altered));
  const unknownObservation = structuredClone(observations);
  unknownObservation.invented = structuredClone(unknownObservation.block);
  assert.throws(() => validatePilot(profile, manifest, unknownObservation));
  const staleContents = structuredClone(profile);
  staleContents.requiredCases.find((row) => row.id === "contents-gap").disposition =
    "implementation-gap";
  assert.throws(() => validatePilot(staleContents, manifest, observations), /contents-gap/);
  const expanded = structuredClone(profile);
  expanded.requiredCases
    .find((row) => row.id === "block")
    .reviewedDifferenceIds.push("physical-logical-margin");
  assert.throws(() => validatePilot(expanded, manifest, observations));
  const staleProfile = structuredClone(profile);
  staleProfile.profileRevision = 2;
  assert.throws(() => validatePilot(staleProfile, manifest, observations));
  const staleSpec = structuredClone(profile);
  staleSpec.sourceBaseline.languageSpecRevision = 12;
  assert.throws(() => validatePilot(staleSpec, manifest, observations));
});

test("prelude is exact and counted once per expected stylesheet", () => {
  const statement = profile.reviewedDifferences.find(
    (row) => row.id === "wind-layer-order-prelude",
  ).statement;
  assert.equal(preludeCheck(`${statement}\n.block { display: block; }`, true, statement), true);
  assert.equal(preludeCheck("", false, statement), true);
  assert.equal(preludeCheck(".block { display: block; }", true, statement), false);
  assert.equal(
    preludeCheck(`${statement}\n${statement}\n.block { display: block; }`, true, statement),
    false,
  );
  assert.equal(preludeCheck(`${statement}\n`, false, statement), false);
  const contents = expectedStructure("contents-gap");
  assert.equal(contents.wind.length, 2);
  assert.equal(contents.wind[0].text, statement.slice(0, -1));
  assert.deepEqual(
    compareStructure(
      "contents-gap",
      `${statement}\n.contents { display: contents; }`,
      ".contents { display: contents; }",
      ["wind-layer-order-prelude"],
      true,
    ).pass,
    true,
  );
});

test("failed observations cannot become reviewed matches", () => {
  const row = profile.requiredCases.find((item) => item.id === "mx-auto");
  const checks = { identity: true, structure: true, observations: true, diagnostics: true };
  assert.equal(classify(row, checks), outcomes.difference);
  assert.equal(classify(row, { ...checks, observations: false }), outcomes.mismatch);
  assert.equal(classify(row, { ...checks, infrastructure: true }), outcomes.infrastructure);
});

test("complete parsed structures reject same-selector, duplicate, order and layer corruption", () => {
  const prelude = "@layer zw-reset, zw-tokens, zfb-hi, base, components;";
  const wind = `${prelude}.block{display:block;}`;
  const reference = ".block{display:block;}";
  const expectedIds = ["wind-layer-order-prelude"];
  assert.equal(compareStructure("block", wind, reference, expectedIds, true).pass, true);
  for (const changed of [
    `${prelude}.block{display:block;color:red;}`,
    `${prelude}.block{display:block;}.block{display:block;}`,
    `.block{display:block;}${prelude}`,
    `${prelude}@layer zfb-hi{.block{display:block;}}`,
    `${prelude}.wrong{display:block;}`,
  ])
    assert.equal(compareStructure("block", changed, reference, expectedIds, true).pass, false);
  const duplicate = compareStructure(
    "block",
    `${prelude}${prelude}.block{display:block;}`,
    reference,
    expectedIds,
    true,
  );
  assert.equal(duplicate.differenceOccurrences["wind-layer-order-prelude"].observedCount, 2);
  assert.deepEqual(duplicate.differenceOccurrences["wind-layer-order-prelude"].paths, [
    "wind/0",
    "wind/1",
  ]);
  const numeric = expectedStructure("configured-p-4");
  assert.deepEqual(
    observedDifferenceIds("configured-p-4", numeric.wind, numeric.reference, true),
    [
      ...profile.requiredCases.find((row) => row.id === "configured-p-4").reviewedDifferenceIds,
    ].sort(),
  );
  const numericWind = `${prelude}@layer zw-tokens{:root{--zw-spacing-unit:.25rem;}}.p-4{padding-top:1rem;padding-right:1rem;padding-bottom:1rem;padding-left:1rem;}`;
  const numericReference =
    ":root, :host{--spacing:0.25rem;}.p-4{padding:calc(var(--spacing) * 4);}";
  assert.equal(
    compareStructure(
      "configured-p-4",
      numericWind,
      numericReference,
      profile.requiredCases.find((row) => row.id === "configured-p-4").reviewedDifferenceIds,
      true,
    ).pass,
    true,
  );
  assert.equal(parseCssStructure(".block{display:block;color:red;}")[0].children.length, 2);
  const quoted = parseCssStructure('.x{content:"/* literal */";--x:"/* token */";}');
  assert.deepEqual(
    quoted[0].children.map((node) => node.value),
    ['"/* literal */"', '"/* token */"'],
  );
  assert.throws(() => parseCssStructure('.x{content:"unterminated;}'));
});

test("semantic cases count exactly two configured Wind tokens and one used reference token", () => {
  const prelude = "@layer zw-reset, zw-tokens, zfb-hi, base, components;";
  const tokenRule = "@layer zw-tokens{:root{--zw-color-surface:#123456;--zw-spacing-hsp-sm:17px;}}";
  const pad = ["top", "right", "bottom", "left"]
    .map((side) => `padding-${side}:var(--zw-spacing-hsp-sm);`)
    .join("");
  const cases = [
    [
      "named-spacing",
      `.p-hsp-sm{${pad}}`,
      ":root, :host{--spacing-hsp-sm:17px;}.p-hsp-sm{padding:var(--spacing-hsp-sm);}",
      "--zw-color-surface",
    ],
    [
      "named-color",
      ".bg-surface{background-color:var(--zw-color-surface);}",
      ":root, :host{--color-surface:#123456;}.bg-surface{background-color:var(--color-surface);}",
      "--zw-spacing-hsp-sm",
    ],
  ];
  const policy = profile.reviewedDifferences.find(
    (item) => item.id === "named-token-representation",
  );
  assert.equal(profile.profileRevision, 3);
  for (const [id, utility, reference, unusedToken] of cases) {
    const wind = `${prelude}${tokenRule}${utility}`;
    const row = profile.requiredCases.find((item) => item.id === id);
    const result = compareStructure(id, wind, reference, row.reviewedDifferenceIds, true);
    assert.equal(result.pass, true, id);
    assert.deepEqual(result.differenceOccurrences["named-token-representation"], {
      expectedCount: 2,
      observedCount: 2,
      paths: ["wind/1/0/0", "wind/1/0/1"],
    });
    assert.equal(policy.expectedOccurrencesByCase[id].windDeclarations.length, 2);
    assert.equal(policy.expectedOccurrencesByCase[id].referenceDeclarations.length, 1);
    assert.equal(policy.expectedOccurrencesByCase[id].unusedWindDeclaration, unusedToken);
    for (const badTokens of [
      tokenRule.replace(`--zw-color-surface:#123456;`, ""),
      tokenRule.replace(`--zw-spacing-hsp-sm:17px;`, ""),
      tokenRule.replace(
        "--zw-color-surface:#123456;--zw-spacing-hsp-sm:17px;",
        "--zw-spacing-hsp-sm:17px;--zw-color-surface:#123456;",
      ),
      tokenRule.replace(
        "--zw-spacing-hsp-sm:17px;",
        "--zw-spacing-hsp-sm:17px;--zw-radius-extra:1px;",
      ),
    ])
      assert.equal(
        compareStructure(
          id,
          `${prelude}${badTokens}${utility}`,
          reference,
          row.reviewedDifferenceIds,
          true,
        ).pass,
        false,
      );
  }
});

test("mx-auto keeps all 24 mode, direction, physical-margin and geometry probes exact", () => {
  const probes = observations["mx-auto"].probes;
  assert.equal(probes.length, 24);
  for (const mode of ["horizontal-tb", "vertical-rl"])
    for (const direction of ["ltr", "rtl"]) {
      for (const side of ["left", "right", "top", "bottom"]) {
        const probe = probes.find((item) => item.name === `${mode}-${direction}-margin-${side}`);
        assert(probe);
        const physicalHorizontal = side === "left" || side === "right";
        assert.equal(probe.wind, mode === "horizontal-tb" && physicalHorizontal ? "100px" : "0px");
        assert.equal(
          probe.reference,
          (mode === "horizontal-tb" && physicalHorizontal) ||
            (mode === "vertical-rl" && !physicalHorizontal)
            ? "100px"
            : "0px",
        );
      }
      for (const axis of ["left", "top"])
        assert(probes.some((item) => item.name === `${mode}-${direction}-geometry-${axis}`));
    }
});

test("missing executions and controls remain visible and fail the pilot", () => {
  const report = completeReport(profile, manifest, {}, {}, { probe: true });
  assert.equal(report.cases.length, 15);
  assert.equal(report.controls.length, 11);
  assert.equal(report.cases[0].outcome, outcomes.missing);
  assert.equal(report.exitCode, 1);
  assert.equal(report.complete, false);
  const subset = completeReport(
    profile,
    { caseIds: ["block"] },
    { block: { caseId: "block", outcome: outcomes.shared } },
    Object.fromEntries(
      profile.requiredControls.map((id) => [id, { controlId: id, outcome: outcomes.shared }]),
    ),
    {},
  );
  assert.equal(subset.exitCode, 1);
  assert.equal(subset.cases.filter((row) => row.outcome === outcomes.missing).length, 14);
});

test("negative diagnostics require exact code, origin, severity and rejection", () => {
  const row = profile.requiredCases.find((item) => item.id === "unconfigured-p-4");
  const diagnostic = {
    code: "ZW006",
    severity: "error",
    candidate: "p-4",
    rejectionId: "R17",
    suggestion: null,
    message: "nonzero numeric spacing requires spacingUnit",
    origin: {
      kind: "manifest",
      producer: "wind-fixture-css",
      path: "unconfigured-p-4/case.json",
      index: 0,
    },
  };
  assert.equal(diagnosticCheck(row, { diagnostics: [diagnostic] }), true);
  for (const change of [
    { code: "ZW008" },
    { severity: "warning" },
    { rejectionId: null },
    { origin: { ...diagnostic.origin, kind: "source" } },
    { message: "unknown utility" },
  ])
    assert.equal(diagnosticCheck(row, { diagnostics: [{ ...diagnostic, ...change }] }), false);
  assert.equal(diagnosticCheck(row, { diagnostics: [diagnostic, diagnostic] }), false);
});

test("extraction compares complete scanner and emitted utility sets", () => {
  const reviewed = {
    expectedWindCandidates: ["block", "hidden"],
    expectedReferenceCandidates: ["block", "class", "hidden"],
    expectedWindRules: ["block", "hidden"],
    expectedReferenceRules: ["block", "hidden"],
  };
  const actual = {
    windCandidates: ["block", "hidden"],
    referenceCandidates: ["block", "class", "hidden"],
    windRules: ["block", "hidden"],
    referenceRules: ["block", "hidden"],
  };
  assert.equal(compareExtractionSets(actual, reviewed), true);
  assert.equal(
    compareExtractionSets(
      { ...actual, windCandidates: [...actual.windCandidates, "rogue"] },
      reviewed,
    ),
    false,
  );
  assert.equal(
    compareExtractionSets(
      { ...actual, referenceCandidates: [...actual.referenceCandidates, "rogue"] },
      reviewed,
    ),
    false,
  );
  assert.equal(
    compareExtractionSets({ ...actual, windRules: [...actual.windRules, "rogue"] }, reviewed),
    false,
  );
  assert.equal(
    compareExtractionSets(
      { ...actual, referenceRules: [...actual.referenceRules, "rogue"] },
      reviewed,
    ),
    false,
  );
  assert.equal(
    compareExtractionSets({ ...actual, referenceRules: ["block", "block"] }, reviewed),
    false,
  );
});
