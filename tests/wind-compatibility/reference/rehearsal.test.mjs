import test from "node:test";
import assert from "node:assert/strict";
import {
  buildReviewOnlyClassification,
  validateFrozenTarget,
} from "../../../scripts/wind-compatibility/reference-rehearsal.mjs";
import { validateClassification } from "../../../scripts/wind-compatibility/reference-promotion.mjs";
import { digest } from "../../../scripts/wind-compatibility/reference.mjs";

const sourceSha = "a".repeat(40);

function seal(value, key) {
  const body = Object.fromEntries(Object.entries(value).filter(([name]) => name !== key));
  return { ...value, [key]: `sha256:${digest(body)}` };
}

function fixture() {
  const plan = seal(
    {
      schemaVersion: 1,
      kind: "wind-reference-plan",
      channel: "stable",
      candidate: {
        package: "tailwindcss",
        version: "4.3.3",
        integrity: "sha512-candidate",
        source: {
          repository: "https://github.com/tailwindlabs/tailwindcss.git",
          packageGitSha: null,
          status: "tag-observed-artifact-link-unverified",
          tag: "v4.3.3",
          observedTagCommit: "b".repeat(40),
        },
      },
      previousAccepted: {
        package: "tailwindcss",
        version: "4.3.2",
        channel: "stable",
        integrity: "sha512-accepted",
        artifactSha256: "1".repeat(64),
      },
      previousReviewedThrough: { version: "4.3.2" },
    },
    "planId",
  );
  const assessment = {
    schemaVersion: 1,
    kind: "wind-reference-assessment",
    status: "ready-for-comparison",
    planId: plan.planId,
    candidate: { version: "4.3.3", integrity: "sha512-candidate" },
    captureIdentity: `sha256:${"2".repeat(64)}`,
    sourceIdentityStatus: "tag-observed-artifact-link-unverified",
    packageGitSha: null,
    tagToNpmArtifactLink: "unverified",
    sections: {
      changelog: { complete: true, releaseVersions: ["4.3.3"] },
      artifacts: {
        complete: true,
        tarballSha256: "3".repeat(64),
      },
      source: { complete: true, tagCommit: "b".repeat(40) },
      tests: { complete: true },
    },
  };
  const comparison = seal(
    {
      schemaVersion: 1,
      kind: "wind-three-way-comparison",
      planId: plan.planId,
      assessmentIdentity: assessment.captureIdentity,
      testedSourceSha: sourceSha,
      testedInputs: { schemaVersion: 2, digest: `sha256:${"4".repeat(64)}` },
      admission: "pending-independent-review-and-shipping-evidence",
      candidate: {
        reference: {
          version: "4.3.3",
          integrity: "sha512-candidate",
          artifactSha256: "3".repeat(64),
        },
        windBuild: { gitSha: sourceSha },
        passing: false,
      },
      accepted: {
        reference: {
          version: "4.3.2",
          integrity: "sha512-accepted",
          artifactSha256: "1".repeat(64),
        },
        passing: true,
      },
      inventoryMembership: "candidate-inventory-refreshed",
      candidateInventory: {
        runtimeChanges: { static: { added: ["new-utility"], removed: [] } },
        sourceChanges: { functional: { added: [], removed: ["old-root"] } },
        sourceMetadataChanges: { utilities: { before: 1, after: 2 } },
      },
      upstreamDelta: [
        {
          path: "dist/lib.mjs",
          acceptedSha256: "5".repeat(64),
          candidateSha256: "6".repeat(64),
          changed: true,
        },
        { path: "unchanged", acceptedSha256: "7", candidateSha256: "7", changed: false },
      ],
      browserDelta: [
        {
          id: "pilot/display-flex",
          index: 0,
          engine: "chromium",
          acceptedObservation: { display: "block" },
          candidateObservation: { display: "flex" },
          changed: true,
        },
      ],
      controlDelta: [
        {
          id: "negative-control",
          engine: "chromium",
          acceptedDigest: "8".repeat(64),
          candidateDigest: "9".repeat(64),
          changed: true,
        },
      ],
      windDelta: [
        {
          path: "pilot/display-flex/wind.css",
          acceptedSha256: "a".repeat(64),
          candidateSha256: "b".repeat(64),
          changed: true,
        },
      ],
    },
    "reportId",
  );
  return { plan, assessment, comparison };
}

test("frozen target validation binds plan, acquisition, and complete assessment", () => {
  const { plan, assessment } = fixture(),
    targetRecord = {
      schemaVersion: 1,
      kind: "wind-manual-following-target",
      currentAccepted: {
        package: "tailwindcss",
        version: "4.3.2",
        channel: "stable",
        integrity: "sha512-accepted",
        artifactSha256: "1".repeat(64),
      },
      target: {
        package: "tailwindcss",
        version: "4.3.3",
        channel: "stable",
        integrity: "sha512-candidate",
        tarball: "https://registry.npmjs.org/tailwindcss/-/tailwindcss-4.3.3.tgz",
        registry: "https://registry.npmjs.org/tailwindcss/4.3.3",
        sha1: "c".repeat(40),
        preparationAcquisitionSha256: "3".repeat(64),
        source: {
          repository: "https://github.com/tailwindlabs/tailwindcss.git",
          tag: "v4.3.3",
          observedTagCommit: "b".repeat(40),
          packageGitSha: null,
          status: "tag-observed-artifact-link-unverified",
        },
      },
    },
    acquisition = {
      package: "tailwindcss",
      version: "4.3.3",
      integrity: "sha512-candidate",
      sha256: "3".repeat(64),
      source: {
        repository: "https://github.com/tailwindlabs/tailwindcss.git",
        packageGitSha: null,
      },
    };
  plan.candidate.tarball = targetRecord.target.tarball;
  plan.candidate.sha1 = targetRecord.target.sha1;
  plan.previousAccepted.integrity = targetRecord.currentAccepted.integrity;
  plan.previousAccepted.artifactSha256 = targetRecord.currentAccepted.artifactSha256;
  assert.equal(validateFrozenTarget({ plan, assessment, acquisition, targetRecord }), true);
  assert.throws(
    () =>
      validateFrozenTarget({
        plan,
        assessment,
        acquisition: { ...acquisition, sha256: "0".repeat(64) },
        targetRecord,
      }),
    /differs from the frozen target/,
  );
  const wrongSourceRepository = structuredClone(targetRecord);
  wrongSourceRepository.target.source.repository = "https://github.com/other/repo.git";
  assert.throws(
    () =>
      validateFrozenTarget({ plan, assessment, acquisition, targetRecord: wrongSourceRepository }),
    /does not match the frozen 4.3.2 to 4.3.3 target record/,
  );
});

test("proposal preserves exact observed deltas and defers every disposition", () => {
  const values = fixture(),
    classification = buildReviewOnlyClassification(values);
  assert.equal(classification.disposition, "review-only");
  assert.equal(classification.planId, values.plan.planId);
  assert.equal(classification.comparisonReportId, values.comparison.reportId);
  assert.deepEqual(
    classification.upstreamChanges.map((row) => row.path),
    ["dist/lib.mjs"],
  );
  assert.deepEqual(classification.browserChanges[0].candidateObservation, { display: "flex" });
  assert.deepEqual(classification.controlChanges[0].candidateDigest, "9".repeat(64));
  assert.deepEqual(classification.windChanges[0].candidateSha256, "b".repeat(64));
  assert.deepEqual(
    classification.inventoryChanges.map(({ source, domain, action, value }) => ({
      source,
      domain,
      action,
      value,
    })),
    [
      { source: "runtime", domain: "static", action: "added", value: "new-utility" },
      { source: "source", domain: "functional", action: "removed", value: "old-root" },
      {
        source: "source-metadata",
        domain: "utilities",
        action: "changed",
        value: { before: 1, after: 2 },
      },
    ],
  );
  assert.ok(
    [
      ...classification.upstreamChanges,
      ...classification.browserChanges,
      ...classification.controlChanges,
      ...classification.windChanges,
      ...classification.inventoryChanges,
    ].every((row) => row.category === "unexplained-drift" && row.rationale.trim()),
  );
  assert.equal(validateClassification(classification, values.comparison, values.plan), true);
});

test("proposal rejects incomplete assessment, stale report, or a failed accepted baseline", () => {
  const values = fixture();
  assert.throws(
    () =>
      buildReviewOnlyClassification({
        ...values,
        assessment: { ...values.assessment, status: "incomplete" },
      }),
    /Assessment is incomplete/,
  );
  assert.throws(
    () =>
      buildReviewOnlyClassification({
        ...values,
        comparison: { ...values.comparison, reportId: `sha256:${"0".repeat(64)}` },
      }),
    /Comparison is incomplete/,
  );
  const failedBaseline = seal(
    { ...values.comparison, accepted: { ...values.comparison.accepted, passing: false } },
    "reportId",
  );
  assert.throws(
    () => buildReviewOnlyClassification({ ...values, comparison: failedBaseline }),
    /failed for the accepted baseline/,
  );
});
