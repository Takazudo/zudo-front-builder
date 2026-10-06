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
import {
  matchesExpected,
  resolvedWindExpectation,
} from "../../../scripts/wind-compatibility/browser-adapter.mjs";

const root = new URL("../../../", import.meta.url);
const json = async (path) => JSON.parse(await readFile(new URL(path, root), "utf8"));
const [fixture, profile, manifest, observations] = await Promise.all([
  json("tests/wind-compatibility/reference/fixtures/native-matrix-37404316064.v1.json"),
  json("tests/wind-compatibility/profile.json"),
  json("tests/wind-compatibility/corpus/manifest.json"),
  json("tests/wind-compatibility/pilot/observations.json"),
]);
const nativeMargin = await json(
  "tests/wind-compatibility/reference/fixtures/native-margin-37405347552.v1.json",
);

function environment(engine) {
  const policy = profile.browserPolicy;
  const member = policy.requiredMatrix.find((row) => row.browser === engine);
  return {
    name: engine,
    version: member.browserVersion,
    manifestBrowserVersion: member.browserVersion,
    revision: member.revision,
    platform: member.os,
    hostPlatform: member.hostPlatform,
    linuxDistribution: { id: "ubuntu", versionId: "24.04" },
    architecture: "x64",
    playwrightVersion: policy.playwrightTestVersion,
    playwrightCoreVersion: policy.playwrightCoreVersion,
    browserManifestSha256: policy.browserManifest.sha256,
  };
}

test("pinned native CSS control isolates WebKit resolved margins without changing geometry", () => {
  assert.equal(nativeMargin.verticalRl.length, 18);
  const probes =
    profile.profileRevision === 5
      ? Object.fromEntries(observations["mx-auto"].probes.map((row) => [row.name, row]))
      : null;
  assert.ok(probes);
  for (const engine of ["chromium", "firefox", "webkit"]) {
    const identity = nativeMargin.browsers[engine];
    const member = profile.browserPolicy.requiredMatrix.find((row) => row.browser === engine);
    assert.deepEqual(
      [identity.version, identity.revision, identity.hostPlatform],
      [member.browserVersion, member.revision, member.hostPlatform],
    );
    for (const direction of ["ltr", "rtl"]) {
      const rows = Object.fromEntries(
        nativeMargin.verticalRl
          .filter((row) => row.engine === engine && row.direction === direction)
          .map((row) => [row.kind, row]),
      );
      assert.deepEqual(Object.keys(rows).sort(), [
        "logical-auto",
        "physical-auto",
        "physical-zero",
      ]);
      for (const row of Object.values(rows)) {
        assert.equal(row.servedSha256, row.stylesheetSha256);
        assert.equal(row.observed.box.x, 200);
        assert.equal(row.observed.box.width, 100);
        assert.equal(row.observed.box.height, 100);
        assert.equal(row.observed.box.parentWidth, 300);
        assert.equal(row.observed.box.parentHeight, 300);
      }
      const physicalY = direction === "ltr" ? 0 : 200;
      assert.equal(rows["physical-auto"].observed.box.y, physicalY);
      assert.equal(rows["physical-zero"].observed.box.y, physicalY);
      assert.equal(rows["logical-auto"].observed.box.y, 100);
      assert.deepEqual(
        rows["physical-auto"].observed.margins,
        rows["physical-zero"].observed.margins,
      );
      const trailing = direction === "ltr" ? "margin-bottom" : "margin-top";
      const probe = probes[`vertical-rl-${direction}-${trailing}`];
      const expected = resolvedWindExpectation(probe, environment(engine), profile);
      assert.equal(rows["physical-auto"].observed.margins[trailing], expected);
      assert.equal(rows["logical-auto"].observed.margins[trailing], "100px");
      const geometryProbe = probes[`vertical-rl-${direction}-geometry-top`];
      assert.equal(matchesExpected({ value: String(physicalY) }, geometryProbe.wind), true);
      assert.equal(matchesExpected({ value: String(physicalY + 1) }, geometryProbe.wind), false);
    }
  }
  const webkitProbe = probes["vertical-rl-ltr-margin-bottom"];
  const wrongVersion = { ...environment("webkit"), version: "26.6" };
  assert.throws(
    () => resolvedWindExpectation(webkitProbe, wrongVersion, profile),
    /Unreviewed browser/,
  );
  const wrongEngine = { ...environment("firefox"), name: "webkit" };
  assert.throws(
    () => resolvedWindExpectation(webkitProbe, wrongEngine, profile),
    /Unreviewed browser/,
  );
  assert.equal(resolvedWindExpectation(webkitProbe, environment("firefox"), profile), "0px");
});

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

const nativeRoot =
  process.env.WIND_NATIVE_MARGIN_REPORT_ROOT ??
  "/Users/takazudo/repos/myoss/zfb/worktrees/_manager/preset-free-3825-live/native-margin-37405347552";
test(
  "native control projection matches retained 54-observation report and probe source",
  { skip: !existsSync(`${nativeRoot}/report.json`) && "Retained native margin report unavailable" },
  async () => {
    const raw = await readFile(`${nativeRoot}/report.json`);
    const probe = await readFile(`${nativeRoot}/probe.mjs`);
    assert.equal(createHash("sha256").update(raw).digest("hex"), nativeMargin.rawReportSha256);
    assert.equal(createHash("sha256").update(probe).digest("hex"), nativeMargin.sourceProbeSha256);
    assert.equal(JSON.parse(raw).results.length, 54);
  },
);
