import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  buildBootstrapClassification,
  runEphemeralPromotion,
} from "../../../scripts/wind-compatibility/bootstrap-admission.mjs";
import { digest } from "../../../scripts/wind-compatibility/reference.mjs";

const deltaCounts = {
  upstreamDelta: 112,
  browserDelta: 358,
  controlDelta: 33,
  windDelta: 189,
};
const sourceSha = "a".repeat(40);
const testedInputsDigest = `sha256:${"b".repeat(64)}`;

function seal(value, key) {
  const body = Object.fromEntries(Object.entries(value).filter(([name]) => name !== key));
  return { ...body, [key]: `sha256:${digest(body)}` };
}

function fixture() {
  const profile = {
    schemaVersion: 1,
    profileId: "wind-preset-free",
    profileVersion: 1,
    profileRevision: 5,
    referencePolicy: {
      initialCandidateVersion: "4.3.2",
      initialState: { acceptedReference: null, reviewedThrough: null },
    },
  };
  const accepted = { schemaVersion: 1, acceptedReference: null };
  const reviewed = { schemaVersion: 1, reviewedThrough: null };
  const plan = seal(
    {
      schemaVersion: 1,
      kind: "wind-reference-plan",
      channel: "stable",
      candidate: { package: "tailwindcss", version: "4.3.2" },
      previousAccepted: null,
      previousReviewedThrough: null,
      changes: { bootstrap: true },
      profile: { id: "wind-preset-free", version: 1, revision: 5 },
      inputs: { profile: "c".repeat(64) },
    },
    "planId",
  );
  const assessment = {
    schemaVersion: 1,
    kind: "wind-reference-assessment",
    status: "ready-for-comparison",
    planId: plan.planId,
    captureIdentity: `sha256:${"d".repeat(64)}`,
  };
  const comparison = seal(
    {
      schemaVersion: 1,
      kind: "wind-three-way-comparison",
      planId: plan.planId,
      assessmentIdentity: assessment.captureIdentity,
      testedSourceSha: sourceSha,
      testedInputs: { schemaVersion: 2, digest: testedInputsDigest },
      profile: {
        id: "wind-preset-free",
        version: 1,
        revision: 5,
        digest: plan.inputs.profile,
      },
      candidate: {
        passing: true,
        windBuild: { gitSha: sourceSha },
        reference: { version: "4.3.2" },
        reports: Object.fromEntries(
          [
            "corpus-chromium",
            "corpus-firefox",
            "corpus-webkit",
            "pilot-chromium",
            "pilot-firefox",
            "pilot-webkit",
          ].map((name) => [name, `sha256:${"e".repeat(64)}`]),
        ),
      },
      accepted: null,
      candidateInventory: null,
      inventoryMembership: "bootstrap-pinned-inventory",
      admission: "pending-independent-review-and-shipping-evidence",
      upstreamDelta: Array.from({ length: deltaCounts.upstreamDelta }, () => ({ changed: null })),
      browserDelta: Array.from({ length: deltaCounts.browserDelta }, () => ({
        changed: null,
        localChanged: null,
      })),
      controlDelta: Array.from({ length: deltaCounts.controlDelta }, () => ({ changed: null })),
      windDelta: Array.from({ length: deltaCounts.windDelta }, () => ({ changed: null })),
    },
    "reportId",
  );
  return { profile, accepted, reviewed, plan, assessment, comparison };
}

function classify(values = fixture()) {
  return buildBootstrapClassification({
    ...values,
    sourceSha,
    testedInputsDigest,
  });
}

test("bootstrap classification binds the empty policy to the exact current plan and report", () => {
  const { plan, comparison } = fixture();
  assert.deepEqual(classify(), {
    schemaVersion: 1,
    kind: "wind-reference-classification",
    bootstrapPolicy: "wind-first-reference-empty-baseline-v1",
    planId: plan.planId,
    comparisonReportId: comparison.reportId,
    disposition: "accept",
    upstreamChanges: [],
    browserChanges: [],
    controlChanges: [],
    windChanges: [],
    inventoryChanges: [],
  });
});

test("bootstrap classification rejects any non-null or malformed delta flag", () => {
  for (const changed of [true, false, "false", 0, undefined]) {
    const values = fixture();
    values.comparison.upstreamDelta[0].changed = changed;
    values.comparison = seal(values.comparison, "reportId");
    assert.throws(() => classify(values), /change flag/);
  }
  const values = fixture();
  values.comparison.browserDelta[0].localChanged = false;
  values.comparison = seal(values.comparison, "reportId");
  assert.throws(() => classify(values), /localChanged flag/);
});

test("bootstrap classification rejects changed membership and stale report bindings", () => {
  const wrongCount = fixture();
  wrongCount.comparison.controlDelta.pop();
  wrongCount.comparison = seal(wrongCount.comparison, "reportId");
  assert.throws(() => classify(wrongCount), /membership/);

  const stale = fixture();
  stale.comparison.planId = `sha256:${"f".repeat(64)}`;
  stale.comparison = seal(stale.comparison, "reportId");
  assert.throws(() => classify(stale), /Comparison is stale/);
});

test("bootstrap classification rejects a later state, candidate, or source", () => {
  const accepted = fixture();
  accepted.accepted.acceptedReference = { version: "4.3.2" };
  assert.throws(() => classify(accepted), /state records to be null/);

  const candidate = fixture();
  candidate.plan.candidate.version = "4.3.3";
  candidate.plan = seal(candidate.plan, "planId");
  assert.throws(() => classify(candidate), /first stable bootstrap/);

  const source = fixture();
  assert.throws(
    () =>
      buildBootstrapClassification({ ...source, sourceSha: "f".repeat(40), testedInputsDigest }),
    /Comparison is stale/,
  );
});

async function withStateFixture(run) {
  const directory = await mkdtemp(join(tmpdir(), "wind-bootstrap-state-"));
  const paths = {
    accepted: join(directory, "accepted.json"),
    reviewed: join(directory, "reviewed-through.json"),
    dry: join(directory, "dry-run.json"),
    applied: join(directory, "applied.json"),
    stateOutput: join(directory, "state"),
  };
  const original = {
    accepted: Buffer.from('{ "schemaVersion": 1, "acceptedReference": null }\n'),
    reviewed: Buffer.from('{ "schemaVersion": 1, "reviewedThrough": null }\n'),
  };
  await writeFile(paths.accepted, original.accepted);
  await writeFile(paths.reviewed, original.reviewed);
  try {
    await run({ directory, paths, original });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

function proposedResult(dryRun) {
  const next = {
    accepted: { schemaVersion: 1, acceptedReference: { version: "4.3.2" } },
    reviewed: { schemaVersion: 1, reviewedThrough: { version: "4.3.2" } },
  };
  return {
    dryRun,
    next,
    testedInputs: { schemaVersion: 2, digest: `sha256:${"9".repeat(64)}` },
  };
}

async function ephemeralFixture({ paths, runApply, writeStateArtifact, runDry } = {}) {
  let recoveries = 0;
  const defaults = {
    runDry: async () => JSON.stringify(proposedResult(true)),
    runApply: async () => {
      const result = proposedResult(false);
      await writeFile(paths.accepted, `${JSON.stringify(result.next.accepted, null, 2)}\n`);
      await writeFile(paths.reviewed, `${JSON.stringify(result.next.reviewed, null, 2)}\n`);
      return JSON.stringify(result);
    },
    writeStateArtifact: async ({ accepted, reviewed }) => {
      await mkdir(paths.stateOutput, { recursive: true });
      await writeFile(join(paths.stateOutput, "accepted.json"), accepted);
      await writeFile(join(paths.stateOutput, "reviewed-through.json"), reviewed);
    },
  };
  const result = await runEphemeralPromotion({
    acceptedPath: paths.accepted,
    reviewedPath: paths.reviewed,
    runDry: runDry ?? defaults.runDry,
    runApply: runApply ?? defaults.runApply,
    recover: async () => {
      recoveries++;
    },
    validateDry: (value) => assert.equal(value.dryRun, true),
    validateApplied: (value) => assert.equal(value.dryRun, false),
    writeDryOutput: (raw) => writeFile(paths.dry, raw),
    writeAppliedOutput: (raw) => writeFile(paths.applied, raw),
    writeStateArtifact: writeStateArtifact ?? defaults.writeStateArtifact,
  });
  return { result, recoveries };
}

test("ephemeral promotion checks dry-run/apply parity, copies the pair, and restores exact bytes", async () => {
  await withStateFixture(async ({ paths, original }) => {
    const { result, recoveries } = await ephemeralFixture({ paths });
    assert.deepEqual(result.dry.next, result.applied.next);
    assert.deepEqual(result.dry.testedInputs, result.applied.testedInputs);
    assert.deepEqual(
      await readFile(join(paths.stateOutput, "accepted.json")),
      result.state.accepted,
    );
    assert.deepEqual(
      await readFile(join(paths.stateOutput, "reviewed-through.json")),
      result.state.reviewed,
    );
    assert.deepEqual(await readFile(paths.accepted), original.accepted);
    assert.deepEqual(await readFile(paths.reviewed), original.reviewed);
    assert.equal(recoveries, 1);
  });
});

test("ephemeral promotion restores after apply or artifact-copy failure", async () => {
  await withStateFixture(async ({ paths, original }) => {
    await assert.rejects(
      () =>
        ephemeralFixture({
          paths,
          runApply: async () => {
            await writeFile(paths.accepted, "partial state\n");
            throw Error("synthetic apply failure");
          },
        }),
      /synthetic apply failure/,
    );
    assert.deepEqual(await readFile(paths.accepted), original.accepted);
    assert.deepEqual(await readFile(paths.reviewed), original.reviewed);
  });

  await withStateFixture(async ({ paths, original }) => {
    await assert.rejects(
      () =>
        ephemeralFixture({
          paths,
          writeStateArtifact: async () => {
            throw Error("synthetic artifact-copy failure");
          },
        }),
      /synthetic artifact-copy failure/,
    );
    assert.deepEqual(await readFile(paths.accepted), original.accepted);
    assert.deepEqual(await readFile(paths.reviewed), original.reviewed);
  });
});

test("ephemeral promotion restores unexpected dry-run mutations and keeps primary errors", async () => {
  await withStateFixture(async ({ paths, original }) => {
    let recoveries = 0;
    await assert.rejects(
      () =>
        runEphemeralPromotion({
          acceptedPath: paths.accepted,
          reviewedPath: paths.reviewed,
          runDry: async () => {
            await writeFile(paths.accepted, "unexpected dry-run mutation\n");
            throw Error("primary dry-run failure");
          },
          runApply: async () => {
            throw Error("apply must not run");
          },
          recover: async () => {
            recoveries++;
            throw Error("secondary recovery failure");
          },
          validateDry: () => {},
          validateApplied: () => {},
          writeDryOutput: async () => {},
          writeAppliedOutput: async () => {},
          writeStateArtifact: async () => {},
        }),
      (error) =>
        error instanceof AggregateError &&
        error.errors[0].message === "primary dry-run failure" &&
        error.errors[1].message === "secondary recovery failure",
    );
    assert.equal(recoveries, 1);
    assert.deepEqual(await readFile(paths.accepted), original.accepted);
    assert.deepEqual(await readFile(paths.reviewed), original.reviewed);
  });
});
