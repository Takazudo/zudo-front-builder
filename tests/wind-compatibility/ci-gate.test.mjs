import { test } from "node:test";
import assert from "node:assert/strict";
import { route } from "../../scripts/wind-compatibility/ci-route.mjs";
import { assertOutcome } from "../../scripts/wind-compatibility/ci-gate.mjs";
import { digest } from "../../scripts/wind-compatibility/reference.mjs";

const sourceSha = "a".repeat(40);
const inputsDigest = `sha256:${"b".repeat(64)}`;
const rows = ["pilot-chromium/report.json", "corpus-chromium/report.json"].map((path) => ({
  path,
  sha256: "c".repeat(64),
}));
function evidence(overrides = {}) {
  const body = {
    schemaVersion: 1,
    mode: "chromium",
    sourceSha,
    testedInputsDigest: inputsDigest,
    result: "passed",
    reports: rows,
    ...overrides,
  };
  return { ...body, id: `sha256:${digest({ ...body, id: undefined })}` };
}
const good = {
  detector: "success",
  relevant: "true",
  mode: "chromium",
  native: "success",
  wind: "success",
  evidence: evidence(),
  sourceSha,
  inputsDigest,
};

test("explicit irrelevant PR is the sole no-op", () => {
  assert.deepEqual(route({ event: "pull_request", base: sourceSha, paths: ["README.md"] }), {
    relevant: false,
    mode: "chromium",
  });
  assert.equal(
    assertOutcome({ ...good, relevant: "false", wind: "skipped", evidence: null }),
    "irrelevant-no-op",
  );
  assert.deepEqual(
    route({ event: "pull_request", base: sourceSha, paths: ["README.md", "new/unknown.txt"] }),
    { relevant: true, mode: "chromium" },
  );
  assert.deepEqual(route({ event: "push", paths: [] }), { relevant: true, mode: "full" });
  assert.deepEqual(route({ event: "workflow_dispatch", full: "true" }), {
    relevant: true,
    mode: "full",
  });
});

test("fail closed for each required job outcome and route error", () => {
  for (const key of ["detector", "native", "wind"])
    for (const outcome of ["failure", "cancelled", "skipped", "timed_out", ""])
      assert.throws(() => assertOutcome({ ...good, [key]: outcome }), `${key}: ${outcome}`);
  for (const relevant of ["", "unknown"]) assert.throws(() => assertOutcome({ ...good, relevant }));
  assert.throws(() => route({ event: "pull_request", base: sourceSha, paths: [] }));
  assert.throws(() => route({ event: "workflow_dispatch", full: "false" }));
});

test("missing, stale and substituted evidence cannot pass", () => {
  for (const candidate of [
    null,
    evidence({ sourceSha: "d".repeat(40) }),
    evidence({ testedInputsDigest: "bad" }),
    evidence({ reports: rows.slice(0, 1) }),
    evidence({ reports: [{ ...rows[0], path: "wrong" }, rows[1]] }),
    { ...evidence(), id: "sha256:bad" },
    evidence({ result: "failed" }),
  ])
    assert.throws(() => assertOutcome({ ...good, evidence: candidate }));
  assert.throws(() => assertOutcome({ ...good, relevant: "false", wind: "success" }));
  assert.throws(() =>
    assertOutcome({ ...good, relevant: "false", wind: "skipped", evidence: evidence() }),
  );
});

test("exact successful Chromium evidence passes", () => {
  assert.equal(assertOutcome(good), "passed");
});

test("full lane requires both comparison and shipping reports", () => {
  const fullRows = ["comparison/comparison.json", "shipping/report.json"].map((path) => ({
    path,
    sha256: "d".repeat(64),
  }));
  const full = { ...good, mode: "full", evidence: evidence({ mode: "full", reports: fullRows }) };
  assert.equal(assertOutcome(full), "passed");
  assert.throws(() =>
    assertOutcome({ ...full, evidence: evidence({ mode: "full", reports: fullRows.slice(0, 1) }) }),
  );
  assert.throws(() =>
    assertOutcome({ ...full, evidence: evidence({ mode: "full", reports: rows }) }),
  );
  assert.throws(() =>
    assertOutcome({ ...good, relevant: "false", mode: "full", wind: "skipped", evidence: null }),
  );
});
