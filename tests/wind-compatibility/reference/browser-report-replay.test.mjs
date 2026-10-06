import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import test from "node:test";
import { assertTargetedExecution } from "../../../scripts/wind-compatibility/reference-comparison.mjs";
import {
  completeCorpus,
  expectedOutcomes,
  observedUpstreamOutcome,
} from "../../../scripts/wind-compatibility/corpus-core.mjs";

const root = new URL("../../../", import.meta.url);
const json = async (path) => JSON.parse(await readFile(new URL(path, root), "utf8"));
const [fixture, profile, manifest] = await Promise.all([
  json("tests/wind-compatibility/reference/fixtures/native-matrix-37404316064.v1.json"),
  json("tests/wind-compatibility/profile.json"),
  json("tests/wind-compatibility/corpus/manifest.json"),
]);

function firefoxReport() {
  return {
    executed: Object.fromEntries(fixture.firefox.executedPilotKeys.map((key) => [key, {}])),
    controls: {
      pilot: Object.fromEntries(
        Object.entries(fixture.firefox.controlOutcomes).map(([key, outcome]) => [key, { outcome }]),
      ),
    },
  };
}

test("actual Firefox targeted control projection retains every required control", () => {
  const report = firefoxReport();
  assert.equal(assertTargetedExecution(profile, manifest, report, "firefox"), true);
  const missingControl = structuredClone(report);
  delete missingControl.controls.pilot["nested-token-scope"];
  assert.throws(
    () => assertTargetedExecution(profile, manifest, missingControl, "firefox"),
    /firefox profile targeted obligation absent: nested-token-scope/,
  );
  const missingPilot = structuredClone(report);
  delete missingPilot.executed["pilot/mx-auto/firefox"];
  assert.throws(
    () => assertTargetedExecution(profile, manifest, missingPilot, "firefox"),
    /firefox profile targeted obligation absent: mx-auto/,
  );
});

test("actual reversal reports classify both reviewed differences on all engines", () => {
  const allowed = expectedOutcomes(manifest, profile);
  for (const [engine, evidence] of Object.entries(fixture.reversal)) {
    const keys = [];
    const corrected = {};
    const original = {};
    for (const [id, row] of Object.entries(evidence.rows)) {
      const key = `upstream/${id}/${engine}`;
      keys.push(key);
      assert.equal(row.outcome, "matched");
      assert.equal(row.differencePass && row.structurePass && row.transportPass, true);
      assert.deepEqual(row.reviewedDifferenceIds, [
        "exact-tree-contract",
        "sr-reversal-clip-model",
      ]);
      corrected[key] = {
        outcome: observedUpstreamOutcome(row.transportPass, true, row.reviewedDifferenceIds),
      };
      original[key] = { outcome: row.outcome };
      assert.equal(corrected[key].outcome, allowed[key]);
    }
    assert.deepEqual(completeCorpus(keys, original, allowed).failures, keys);
    assert.equal(completeCorpus(keys, corrected, allowed).complete, true);
  }
});

const actualRoot =
  process.env.WIND_HARNESS_REPORT_ROOT ??
  "/Users/takazudo/repos/myoss/zfb/worktrees/_manager/preset-free-3825-live/native-matrix-37404316064/wind-harness-evidence/comparison/candidate";
test(
  "compact regression fixture matches retained run 37404316064 raw reports",
  {
    skip:
      !existsSync(`${actualRoot}/corpus-firefox/report.json`) &&
      "Retained raw browser reports unavailable",
  },
  async () => {
    for (const [engine, evidence] of Object.entries(fixture.reversal)) {
      const bytes = await readFile(`${actualRoot}/corpus-${engine}/report.json`);
      assert.equal(createHash("sha256").update(bytes).digest("hex"), evidence.corpusReportSha256);
    }
  },
);
