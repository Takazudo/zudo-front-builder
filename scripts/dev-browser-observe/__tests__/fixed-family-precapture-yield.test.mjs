import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { test } from "vitest";
import {
  blankBrowserAudit,
  runOriginalFamilyBatch,
  yieldThenRunOriginalFamilyBatch,
} from "../lib/fixed-family-admission.mjs";
import { startFixedRssSafety } from "../lib/fixed-rss-safety.mjs";
import { deps, fakeFs, fakePage } from "./fakes.mjs";

// Real-timer replay of #4862 r5 slot 02 (cclogs 4862-rapid-r5-slot02-failure):
// epoch 10 dispatched at L and settled in 52 ms; browser-ready came at L+470 ms,
// and the synchronous post-launch chain then held the loop for 3380 ms, past the
// tick due at L+1000 ms, until the family capture.
const MS = 1_000_000n;
const SETTLE_MS = 52;
const READY_PHASE_MS = 470;
const SYNC_BLOCK_MS = 3380;
// Nine 1000 ms cadence periods to epoch 10, plus 3.85 s of replay and the join:
// about 13.1 s of wall time. 30 s guards a stuck run; it is not an assertion.
const REPLAY_TIMEOUT_MS = 30_000;

const identity = (pid, ppid, pgid) => ({
  pid,
  ppid,
  pgid,
  startTicks: String(pid),
  bootId: "boot",
  birthVersion: "linux-proc-v1",
  state: "S",
});
const root = identity(3, 2, 3),
  dev = identity(4, 3, 3),
  worker = identity(5, 4, 5);

const blockSync = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
const immediate = () => new Promise((resolve) => setImmediate(resolve));
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function realHarness() {
  const settled = [];
  const failures = [];
  let dispatches = 0;
  const monitor = startFixedRssSafety({
    joinOwnedFamily: true,
    server: { handle: { native: { pid: 3 } } },
    fixedExecutables: { protocol: "linux-fixed-zfb.v4", root: { path: "/node" } },
    limits: { rssCeilingKiB: 4194304, sampleEveryMs: 1000 },
    append() {},
    abort: (error) => failures.push(error),
    custody: {
      familyProof: () => ({
        root,
        dev,
        worker,
        terminal: { epoch: 1 },
        release: { receipt: { epoch: 1 } },
      }),
    },
    sample(_server, _path, { onHelperSpawn }) {
      dispatches++;
      const done = Promise.withResolvers();
      const now = process.hrtime.bigint();
      const helper = {
        pid: 100 + dispatches,
        entryNs: String(now),
        spawnHrtimeNs: String(now),
        deadlineNs: String(now + 750n * MS),
      };
      onHelperSpawn(helper);
      setTimeout(
        () =>
          done.resolve({
            summedRssKiB: 174772,
            helperResult: "complete",
            processes: [root, dev].map((p) => ({ ...p, disposition: "live" })),
            startupEpochCoverage: { complete: true },
            helper: { ...helper, closed: true, pipesClosed: true },
            timing: {
              clock: { qualified: true },
              parent: {
                entryNs: helper.entryNs,
                deadlineNs: helper.deadlineNs,
                resultNs: String(process.hrtime.bigint()),
              },
            },
          }),
        SETTLE_MS,
      );
      settled.push(done.promise);
      return done.promise;
    },
  });
  return {
    monitor,
    failures,
    settled,
    get dispatches() {
      return dispatches;
    },
  };
}

function familyOptions(h, rows) {
  const audit = blankBrowserAudit(),
    ctx = new EventEmitter();
  const page0 = Object.assign(new EventEmitter(), { url: () => "about:blank" });
  ctx.pages = () => [];
  audit.attach(ctx);
  ctx.pages = () => [page0];
  const input = {
    scenarios: [{ id: "family-admission-only", ops: [] }],
    deps: deps({ page: fakePage(), fs: audit.trackFixture(fakeFs()) }),
  };
  const options = {
    strategy: "blank-join-owned.v2",
    safety: h.monitor,
    browserAudit: audit,
    snapshot: () => [root, dev],
    known: new Map([root, dev, worker].map((x) => [x.pid, x])),
    verifyImages() {},
    deadlineNs: process.hrtime.bigint() + 60_000n * MS,
    signal: new AbortController().signal,
    append: (r) => rows.push({ ...structuredClone(r), dispatchesAtAppend: h.dispatches }),
  };
  return { input, options };
}

// Returns the epoch-10 start L and the monotonic end of the synchronous block.
async function stallAfterEpoch10(h) {
  while (h.settled.length < 10) await sleep(50);
  await h.settled[9];
  await immediate();
  const accepted = h.monitor.acceptedSample();
  assert.equal(accepted.epoch, 10);
  const L = BigInt(accepted.startedNs);
  const toReady = Number(L + BigInt(READY_PHASE_MS) * MS - process.hrtime.bigint()) / 1e6;
  assert(toReady > 0, "epoch 10 was accepted after the browser-ready phase");
  await sleep(Math.ceil(toReady));
  // executeLocked resumes the post-launch chain from the check phase.
  await immediate();
  const blockStartNs = process.hrtime.bigint();
  assert(blockStartNs < L + 1000n * MS, "the stall must start before the next tick is due");
  blockSync(SYNC_BLOCK_MS);
  return { L, blockStartNs, blockEndNs: process.hrtime.bigint() };
}

async function outcomeOf(promise) {
  try {
    return { value: await promise };
  } catch (error) {
    return { error };
  }
}

test(
  "without a pre-capture yield the slot-02 stall refuses at the deferral entry check",
  async () => {
    const h = realHarness();
    try {
      const { L, blockEndNs } = await stallAfterEpoch10(h);
      const rows = [];
      const { input, options } = familyOptions(h, rows);
      assert.equal(h.dispatches, 10, "the tick due at L+1000 ms is still undelivered");
      const { error } = await outcomeOf(runOriginalFamilyBatch(input, options));
      assert.match(String(error?.message), /family next cadence dispatch expired/);
      const refusal = rows.at(-1);
      assert.equal(refusal.admitted, false);
      assert.equal(refusal.dispatchesAtAppend, 10, "decided before the overdue tick dispatched");
      const { setup } = refusal;
      assert.equal(setup.preCaptureYield, undefined);
      assert.equal(setup.dispatchLatenessNs, undefined);
      const deferral = setup.idleDeferral;
      assert.equal(deferral.outcome, "refused");
      assert.equal(deferral.firstCapture.pending, false);
      assert.equal(deferral.firstCapture.previousEpoch, 10);
      assert.equal(deferral.cadenceDeadlineNs, String(L + 1000n * MS));
      assert.equal(deferral.waitBoundNs, String(L + 1750n * MS));
      // The entry check (fixed-family-admission.mjs `begin < boundNs`) fired: the
      // wait never began, so no helper observation was stamped.
      assert.equal(deferral.helperObservedNs, undefined);
      assert(BigInt(deferral.waitStartedNs) >= blockEndNs);
      const overBound = BigInt(deferral.waitStartedNs) - BigInt(deferral.waitBoundNs);
      assert(overBound >= 2090n * MS && overBound < 2400n * MS, String(overBound));
      const remaining = BigInt(deferral.remainingCadenceNs);
      assert(remaining <= -2840n * MS && remaining > -3150n * MS, String(remaining));
      assert(!input.deps.recorder.records.some((r) => r.type === "scenario-start"));
      assert.deepEqual(h.failures, []);
    } finally {
      await h.monitor.stop();
    }
  },
  REPLAY_TIMEOUT_MS,
);

test(
  "the pre-capture yield joins the overdue original dispatch and records its lateness",
  async () => {
    const h = realHarness();
    try {
      const { L, blockEndNs } = await stallAfterEpoch10(h);
      const rows = [];
      const { input, options } = familyOptions(h, rows);
      assert.equal(h.dispatches, 10);
      const { error } = await outcomeOf(yieldThenRunOriginalFamilyBatch(input, options));
      assert.equal(error, undefined, String(error));
      const proofRow = rows.find((r) => r.status === "pre-batch-proof");
      assert.equal(proofRow.dispatchesAtAppend, 11, "exactly one added original dispatch");
      const start = input.deps.recorder.records.find((r) => r.type === "scenario-start");
      const { setup, decisionNs } = start.familyAdmission;
      assert.equal(setup.settlementOutcome, "accepted");
      assert.equal(setup.idleDeferral, undefined, "joined the pending dispatch, no deferral");
      assert.equal(setup.pending, true);
      assert.equal(setup.previousEpoch, 10);
      assert.equal(setup.cycle.epoch, 11);
      assert.equal(setup.cycle.helper.pid, 111, "the sampler's own eleventh dispatch");
      assert.equal(setup.priorCustody.sample.epoch, 10);
      assert.equal(setup.priorCustody.sample.cadenceDeadlineNs, String(L + 1000n * MS));
      const { startNs, endNs } = setup.preCaptureYield;
      assert(BigInt(startNs) >= blockEndNs && BigInt(endNs) >= BigInt(startNs));
      assert(BigInt(setup.cycle.startedNs) >= BigInt(startNs));
      assert(BigInt(setup.cycle.startedNs) <= BigInt(endNs));
      assert(BigInt(endNs) <= BigInt(setup.capturedNs));
      const lateness = BigInt(setup.dispatchLatenessNs);
      assert.equal(
        lateness,
        BigInt(setup.cycle.startedNs) - BigInt(setup.priorCustody.sample.cadenceDeadlineNs),
      );
      assert(lateness >= 2840n * MS && lateness < 3150n * MS, String(lateness));
      assert(BigInt(decisionNs) < BigInt(setup.cycle.startedNs) + 1000n * MS);
      assert.equal(h.dispatches, 11, "the harness dispatched no sample of its own");
      assert.deepEqual(h.failures, []);
    } finally {
      await h.monitor.stop();
    }
  },
  REPLAY_TIMEOUT_MS,
);
