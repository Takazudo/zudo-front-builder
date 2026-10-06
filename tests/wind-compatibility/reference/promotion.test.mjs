import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile, rm, rename, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { testedPaths, sameTestedInputs } from "../../../scripts/wind-compatibility/reference-identity.mjs";
import { validateClassification, transition, writeTransitionAtomically, withTransitionLock } from "../../../scripts/wind-compatibility/reference-promotion.mjs";
import { digest } from "../../../scripts/wind-compatibility/reference.mjs";
import { validateShippingManifest } from "../../../scripts/wind-compatibility/reference-shipping.mjs";
import { readRecordedArtifact, requirePassingCurrent, validateRun } from "../../../scripts/wind-compatibility/reference-comparison.mjs";
import { sha256 } from "../../../scripts/wind-compatibility/reference.mjs";

const plan = { planId: "plan", channel: "stable", candidate: {
  package: "tailwindcss", version: "4.3.2", integrity: "sha512-test", source: { status: "unknown" },
}, previousAccepted: null, inputs: { lock: "lock" }, toolchain: { node: "test" } };
const comparison = { reportId: "comparison", testedSourceSha: "a".repeat(40),
  testedInputs: { digest: "input" }, profile: { id: "wind-preset-free", version: 1,
    revision: 2, digest: "profile" }, candidate: { passing: true,
    reference: { integrity: "sha512-test", artifactSha256: "artifact" } },
  accepted: null, upstreamDelta: [], browserDelta: [] };
const state = { accepted: { schemaVersion: 1, acceptedReference: null },
  reviewed: { schemaVersion: 1, reviewedThrough: null } };

test("tested closure includes MDX fixtures and excludes only transition records", () => {
  const paths = testedPaths();
  assert.ok(paths.includes("tests/wind-compatibility/extraction/mdx/index.mdx"));
  assert.ok(paths.includes("research/wind-compatibility-profile.md"));
  assert.ok(!paths.includes("tests/wind-compatibility/reference/accepted.json"));
  const a = { schemaVersion: 1, files: [["fixture.mdx", "old"]] };
  a.digest = `sha256:${digest(a.files)}`;
  const b = { schemaVersion: 1, files: [["fixture.mdx", "changed"]] };
  b.digest = `sha256:${digest(b.files)}`;
  assert.equal(sameTestedInputs(a, b), false);
});

test("reviewed rejection advances only reviewed-through; acceptance requires shipping evidence", async () => {
  const classification = { schemaVersion: 1, kind: "wind-reference-classification",
    planId: "plan", comparisonReportId: "comparison", disposition: "review-only",
    upstreamChanges: [], browserChanges: [], inventoryChanges: [] };
  const next = await transition({ plan, assessment: { captureIdentity: "assessment" },
    comparison, classification, shipping: null, state, finalSha: "b".repeat(40) });
  assert.deepEqual(next.accepted, state.accepted);
  assert.equal(next.reviewed.reviewedThrough.disposition, "review-only");
  await assert.rejects(transition({ plan, assessment: { captureIdentity: "assessment" },
    comparison, classification: { ...classification, disposition: "accept" },
    shipping: null, state, finalSha: "b".repeat(40) }), /shipping manifest missing/);
});

test("classification rejects omitted or unexplained exact three-way differences", () => {
  const changed = { ...comparison, upstreamDelta: [{ path: "pilot/x/reference.css",
    acceptedSha256: "old", candidateSha256: "new", changed: true }], browserDelta: [{
      id: "pilot/x", index: 0, engine: "chromium", acceptedObservation: { value: "1px" },
      candidateObservation: { value: "2px" }, changed: true,
    }] };
  const base = { schemaVersion: 1, kind: "wind-reference-classification", planId: "plan",
    comparisonReportId: "comparison", disposition: "accept", upstreamChanges: [], browserChanges: [], inventoryChanges: [] };
  assert.throws(() => validateClassification(base, changed, plan), /upstream change membership/);
  const classified = { ...base, upstreamChanges: [{ path: "pilot/x/reference.css",
    acceptedSha256: "old", candidateSha256: "new", category: "upstream-semantic",
    rationale: "exact change reviewed" }], browserChanges: [{ id: "pilot/x", index: 0,
      engine: "chromium", acceptedObservation: { value: "1px" },
      candidateObservation: { value: "2px" }, category: "unexplained-drift", rationale: "pending" }] };
  assert.throws(() => validateClassification(classified, changed, plan), /Unexplained drift/);
});

test("two-record transition restores on second rename failure and rejects stale replay", async () => {
  const dir = await mkdtemp(join(tmpdir(), "wind-transition-"));
  const acceptedPath = join(dir, "accepted.json"), reviewedPath = join(dir, "reviewed.json");
  const opts = { acceptedPath, reviewedPath, lockPath: join(dir, "lock"),
    journalPath: join(dir, "journal.json") };
  const old = { accepted: { schemaVersion: 1, acceptedReference: null },
    reviewed: { schemaVersion: 1, reviewedThrough: null } };
  const next = { accepted: { schemaVersion: 1, acceptedReference: { version: "4.3.2" } },
    reviewed: { schemaVersion: 1, reviewedThrough: { version: "4.3.2" } } };
  try {
    await writeFile(acceptedPath, JSON.stringify(old.accepted));
    await writeFile(reviewedPath, JSON.stringify(old.reviewed));
    let calls = 0;
    await assert.rejects(writeTransitionAtomically(next, old, { ...opts,
      renameFile: async (...args) => {
        if (++calls === 2) throw Error("injected second rename failure");
        return rename(...args);
      },
    }), /injected second rename failure/);
    assert.deepEqual(JSON.parse(await readFile(acceptedPath)), old.accepted);
    assert.deepEqual(JSON.parse(await readFile(reviewedPath)), old.reviewed);
    await writeTransitionAtomically(next, old, opts);
    await assert.rejects(writeTransitionAtomically(next, old, opts), /state mismatch/);
    // Simulate a killed writer after replacing only the first record.
    await writeFile(opts.journalPath, JSON.stringify({ schemaVersion: 1,
      oldAccepted: JSON.stringify(old.accepted), oldReviewed: JSON.stringify(old.reviewed) }));
    await withTransitionLock(async () => {}, opts);
    assert.deepEqual(JSON.parse(await readFile(acceptedPath)), old.accepted);
    assert.deepEqual(JSON.parse(await readFile(reviewedPath)), old.reviewed);
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test("shipping manifest cannot omit a required consumer mechanism or reuse an ID", () => {
  const mechanisms = ["source-add", "source-edit", "source-remove", "source-rename",
    "token-change", "token-removal", "css-build", "css-dev", "css-ssr",
    "missing-css-detection", "component-state"];
  const manifest = { schemaVersion: 1, kind: "wind-shipping-manifest", revision: 1,
    fixtureRoot: "tests/wind-compatibility/shipping/fixtures",
    configFiles: ["tests/wind-compatibility/shipping/config.json"],
    required: mechanisms.map((mechanism) => ({ id: mechanism, mechanism, mode: "build",
      engine: "chromium", configuration: "semantic", fixture: "fixture",
      expectedObservation: { value: "1px" }, screenshotRequired: false })) };
  assert.equal(validateShippingManifest(manifest), true);
  assert.throws(() => validateShippingManifest({ ...manifest,
    required: manifest.required.slice(1) }), /incomplete/);
  assert.throws(() => validateShippingManifest({ ...manifest,
    required: manifest.required.map((row) => ({ ...row, id: "same" })) }), /duplicate/);
});

test("current accepted verification rejects failed and absent run reports", async () => {
  const failed = { passing: false, reports: { pilots: {}, chromium: {}, firefox: {}, webkit: {} } };
  assert.throws(() => requirePassingCurrent(failed), /failed or incomplete/);
  const dir = await mkdtemp(join(tmpdir(), "wind-current-missing-"));
  try {
    await assert.rejects(validateRun(dir, { version: "4.3.2" }, {},
      { requiredCases: [], requiredControls: [] }, {}), /ENOENT/);
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test("retained artifact remapping verifies bytes and rejects traversal or symlink escape", async () => {
  const root = await mkdtemp(join(tmpdir(), "wind-bundle-"));
  const output = join(root, "restored"), original = "/tmp/original-wind-evidence";
  try {
    await import("node:fs/promises").then((fs) => fs.mkdir(join(output, "pilot"), { recursive: true }));
    const file = join(output, "pilot", "reference.css");
    await writeFile(file, ".block{display:block}");
    const recorded = `${original}/pilot/reference.css`;
    assert.equal((await readRecordedArtifact(recorded,
      sha256(".block{display:block}"), output, original)).toString(), ".block{display:block}");
    await assert.rejects(readRecordedArtifact(recorded, sha256("changed"), output, original),
      /hash changed/);
    await assert.rejects(readRecordedArtifact("/tmp/outside/reference.css",
      sha256(".block{display:block}"), output, original), /outside recorded origin/);
    await rm(file);
    const outside = join(root, "outside.css");
    await writeFile(outside, ".block{display:block}");
    await symlink(outside, file);
    await assert.rejects(readRecordedArtifact(recorded,
      sha256(".block{display:block}"), output, original), /escapes declared bundle/);
  } finally { await rm(root, { recursive: true, force: true }); }
});
