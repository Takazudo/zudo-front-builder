import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fromRoot } from "../../scripts/wind-compatibility/reference.mjs";
import {
  classify,
  completeReport,
  outcomes,
  preludeCheck,
  validatePilot,
} from "../../scripts/wind-compatibility/differential-core.mjs";
import { unexpectedSelector } from "../../scripts/wind-compatibility/browser-adapter.mjs";

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
  const altered = structuredClone(observations);
  delete altered.block;
  assert.throws(() => validatePilot(profile, manifest, altered));
  const expanded = structuredClone(profile);
  expanded.requiredCases
    .find((row) => row.id === "block")
    .reviewedDifferenceIds.push("physical-logical-margin");
  assert.throws(() => validatePilot(expanded, manifest, observations));
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
});

test("unexpected selectors and failed observations cannot become reviewed matches", () => {
  assert.equal(unexpectedSelector([{ selector: ".block" }], "block"), false);
  assert.equal(
    unexpectedSelector([{ selector: ".block" }, { selector: ".injected" }], "block"),
    true,
  );
  const row = profile.requiredCases.find((item) => item.id === "mx-auto");
  const checks = { identity: true, structure: true, observations: true, diagnostics: true };
  assert.equal(classify(row, checks), outcomes.difference);
  assert.equal(classify(row, { ...checks, observations: false }), outcomes.mismatch);
  assert.equal(classify(row, { ...checks, infrastructure: true }), outcomes.infrastructure);
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
