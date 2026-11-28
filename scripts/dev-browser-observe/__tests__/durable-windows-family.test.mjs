import assert from "node:assert/strict";
import { afterEach, test } from "vitest";
import { yieldThenRunOriginalFamilyBatch } from "../lib/fixed-family-admission.mjs";
import { stallFs } from "./fs-stall.mjs";
import {
  MS,
  acceptedEpoch,
  controller,
  dev,
  familyBatch,
  outcomeOf,
  root,
  sampler,
  sleep,
  worker,
} from "./family-replay-harness.mjs";

// Design §4, windows W0 and W2: the same storage stall replayed against today's
// writer (fsyncSync blocks the loop) and the durable mode (fsync completes late on
// the threadpool). Each case takes about 3-6 s of real time; 30 s guards a hang.
const TIMEOUT_MS = 30_000;
const EPOCH = 3;
const ENTER_STAGES = [
  "enter:journal-append",
  "enter:snapshot",
  "enter:verify-images",
  "enter:verify-family-admission",
];

const open = [];
afterEach(async () => {
  while (open.length) await open.pop()();
});

async function setUp(durable, samplerOptions) {
  const stall = stallFs();
  const c = controller(durable);
  const s = sampler(c, samplerOptions);
  open.push(async () => {
    stall.stop();
    await s.monitor.stop();
    c.close();
  });
  return { stall, c, s };
}

const scenarioStart = (c) => c.rec.records.find((r) => r.type === "scenario-start");

// W2 invariants the family auditors check (validate.py:206 / family.py:191 and the
// exact setup.stages list): no sample between the proof and scenario-start, and
// barrier evidence never enters setup.stages (m6).
function assertW2Shape(c) {
  const records = c.rec.records;
  const proof = records.findIndex((r) => r.status === "pre-batch-proof");
  const start = records.findIndex((r) => r.type === "scenario-start");
  assert(proof >= 0 && start > proof);
  assert(!records.slice(proof, start).some((r) => r.type === "fixed-rss-sample"));
  assert.deepEqual(
    records[start].familyAdmission.setup.stages.map((r) => r.phase + ":" + r.stage),
    ENTER_STAGES,
  );
}

test.each([false, true])(
  "T0: a 3380 ms browser-ready fsync stall (durable=%s)",
  async (durable) => {
    const { stall, c, s } = await setUp(durable);
    const L = await acceptedEpoch(s, EPOCH);
    await sleep(Math.max(0, Math.ceil(Number(L + 470n * MS - process.hrtime.bigint()) / 1e6)));
    stall.arm('"type":"browser-ready"', 3380);
    const chainStart = process.hrtime.bigint();
    c.rec.append({ type: "browser-ready" });
    c.receipt.checkpoint({ phase: "browser-ready" });
    const chainNs = process.hrtime.bigint() - chainStart;
    const { input, options } = familyBatch(c, s);
    const { error } = await outcomeOf(yieldThenRunOriginalFamilyBatch(input, options));
    assert.equal(error, undefined, String(error));
    const { setup } = scenarioStart(c).familyAdmission;
    const lateness = BigInt(setup.dispatchLatenessNs);
    if (durable) {
      assert(chainNs < 100n * MS, "the W0 chain no longer waits for storage: " + chainNs);
      assert(lateness < 50n * MS, "the joined dispatch is on time: " + lateness);
      assert.equal(setup.idleDeferral.outcome, "joined");
    } else {
      assert(chainNs >= 3380n * MS, "today's writer blocks for the stall");
      assert(lateness >= 2800n * MS, "fix A joins the starved tick late: " + lateness);
    }
    assert.deepEqual(s.failures, []);
  },
  TIMEOUT_MS,
);

// Stalls and the refusal today's writer gives. A stall in the joined sample's own
// append (T1a, T1c) delays its settlement past the 750 ms helper core, so it
// refuses there, one check before the "cadence due" the design table predicted.
const W2 = {
  T1a: {
    stalls: [['"type":"fixed-rss-sample"', 960]],
    today: "family settlement deadline expired",
  },
  T1b: {
    stalls: [['"status":"pre-batch-proof"', 960]],
    today: "family RSS cadence due before admission",
  },
  T1c: {
    stalls: [
      ['"type":"fixed-rss-sample"', 960],
      ['"status":"pre-batch-proof"', 2500],
    ],
    today: "family settlement deadline expired",
  },
  T1d: { stalls: [['"type":"owned"', 960]], today: "family RSS cadence due before admission" },
};

test.each(
  Object.keys(W2).flatMap((name) => [
    [name, false],
    [name, true],
  ]),
)(
  "%s: a W2 storage stall between the join and the decision (durable=%s)",
  async (name, durable) => {
    // T1d: the native child is first seen by the decision's own snapshot, whose
    // remember() → onNewOwned → registry append carries the stall.
    const known = name === "T1d" ? [root, worker] : undefined;
    const { stall, c, s } = await setUp(durable, known && { known });
    const snapshot =
      name === "T1d"
        ? () => {
            for (const p of [root, dev])
              if (!s.handle.known.has(p.pid)) {
                s.handle.known.set(p.pid, p);
                s.handle.onNewOwned(p);
              }
            return [root, dev];
          }
        : undefined;
    const L = await acceptedEpoch(s, EPOCH);
    await sleep(Math.max(0, Math.ceil(Number(L + 600n * MS - process.hrtime.bigint()) / 1e6)));
    for (const [match, ms] of W2[name].stalls) stall.arm(match, ms);
    const { input, options } = familyBatch(c, s, { snapshot });
    const { error } = await outcomeOf(yieldThenRunOriginalFamilyBatch(input, options));
    const total = W2[name].stalls.reduce((n, [, ms]) => n + ms, 0);
    if (!durable) {
      assert.equal(error?.message, W2[name].today);
      assert(!scenarioStart(c), "refused before scenario-start");
      return;
    }
    assert.equal(error, undefined, String(error));
    const start = scenarioStart(c);
    const { setup, decisionNs } = start.familyAdmission;
    assert.equal(setup.settlementOutcome, "accepted");
    assert(BigInt(decisionNs) < BigInt(setup.cycle.startedNs) + 1000n * MS);
    assertW2Shape(c);
    const pre = c.rec.records.find((r) => r.type === "durability" && r.barrier === "pre");
    assert(pre.throughSeq >= start.seq, "B-pre covers scenario-start");
    const waited = BigInt(pre.endNs) - BigInt(pre.startNs);
    assert(waited >= BigInt(total) * MS - 50n * MS, "B-pre absorbs the stall: " + waited);
    assert.equal(stall.log.length, 1);
    assert.equal(stall.log[0].kind, "fsync");
    assert.deepEqual(s.failures, []);
  },
  TIMEOUT_MS,
);
