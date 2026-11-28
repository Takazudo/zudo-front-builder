import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { readFile } from "node:fs";
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
// Upper bound on the synchronous harness code between the end of the block and
// the capture stamp. Windows are relative to measured stamps, so only this
// on-CPU stretch, not timer lateness, enters an assertion.
const SYNC_TAIL_NS = 100n * MS;

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
// An fs callback runs in the poll phase, where one setImmediate would run in the
// same iteration's check phase before any timer.
const pollPhase = () => new Promise((resolve) => readFile(import.meta.filename, () => resolve()));
const RESUME = { check: immediate, poll: pollPhase };

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

// Returns the epoch-10 start L and the measured bounds of the synchronous block.
async function stallAfterEpoch10(h, resume = "check") {
  while (h.settled.length < 10) await sleep(50);
  await h.settled[9];
  await immediate();
  const accepted = h.monitor.acceptedSample();
  assert.equal(accepted.epoch, 10);
  const L = BigInt(accepted.startedNs);
  const toReady = Number(L + BigInt(READY_PHASE_MS) * MS - process.hrtime.bigint()) / 1e6;
  assert(toReady > 0, "epoch 10 was accepted after the browser-ready phase");
  await sleep(Math.ceil(toReady));
  // Recorded slot 02: executeLocked resumed the chain from the check phase.
  await RESUME[resume]();
  const blockStartNs = process.hrtime.bigint();
  // Replay preconditions: the stall must cover the tick due at L+1000 ms.
  assert(blockStartNs >= L + BigInt(READY_PHASE_MS) * MS);
  assert(blockStartNs < L + 1000n * MS, "the stall must start before the next tick is due");
  blockSync(SYNC_BLOCK_MS);
  const blockEndNs = process.hrtime.bigint();
  assert(blockEndNs - blockStartNs >= BigInt(SYNC_BLOCK_MS) * MS);
  return { L, blockStartNs, blockEndNs };
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
      // The block alone carried the decision past the wait bound.
      assert(blockEndNs > BigInt(deferral.waitBoundNs));
      const captured = BigInt(deferral.firstCapture.capturedNs);
      const waitStarted = BigInt(deferral.waitStartedNs);
      assert(blockEndNs <= captured && captured <= waitStarted);
      assert(waitStarted - blockEndNs < SYNC_TAIL_NS, String(waitStarted - blockEndNs));
      assert.equal(BigInt(deferral.remainingCadenceNs), L + 1000n * MS - captured);
      assert(!input.deps.recorder.records.some((r) => r.type === "scenario-start"));
      assert.deepEqual(h.failures, []);
    } finally {
      await h.monitor.stop();
    }
  },
  REPLAY_TIMEOUT_MS,
);

test.each(["check", "poll"])(
  "the pre-capture yield joins the overdue original dispatch after a %s-phase resume",
  async (resume) => {
    const h = realHarness();
    try {
      const { L, blockEndNs } = await stallAfterEpoch10(h, resume);
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
      assert(BigInt(endNs) - blockEndNs < SYNC_TAIL_NS, String(BigInt(endNs) - blockEndNs));
      assert(BigInt(setup.cycle.startedNs) >= BigInt(startNs));
      assert(BigInt(setup.cycle.startedNs) <= BigInt(endNs));
      assert(BigInt(endNs) <= BigInt(setup.capturedNs));
      const lateness = BigInt(setup.dispatchLatenessNs);
      assert.equal(
        lateness,
        BigInt(setup.cycle.startedNs) - BigInt(setup.priorCustody.sample.cadenceDeadlineNs),
      );
      // The tick fired inside the yield, so its lateness is the block's overrun
      // of the cadence deadline plus at most the synchronous tail.
      assert(lateness >= blockEndNs - (L + 1000n * MS), String(lateness));
      assert(BigInt(decisionNs) < BigInt(setup.cycle.startedNs) + 1000n * MS);
      assert.equal(h.dispatches, 11, "the harness dispatched no sample of its own");
      assert.deepEqual(h.failures, []);
    } finally {
      await h.monitor.stop();
    }
  },
  REPLAY_TIMEOUT_MS,
);
