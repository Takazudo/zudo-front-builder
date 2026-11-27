import assert from "node:assert/strict";
import { afterEach, test, vi } from "vitest";
import * as peerRoles from "../lib/fixed-peer-roles.mjs";
import { startFixedRssSafety } from "../lib/fixed-rss-safety.mjs";
import { outerPrewriteDecision, outerTerminalGate } from "../lib/next-outer-terminal.mjs";

const MS = 1_000_000n;
// #4870 R3 (sealed; independent-terminal/arm-overlap.json): the arm fell 70.881 ms
// after the previous cadence cutoff, inside ordinary helper 9, whose result arrived
// about 30.6 ms after the arm. Reconstructed at fake-timer millisecond resolution.
const R3_ARM_AFTER_DISPATCH_MS = 66;
const R3_HELPER_MS = 97;

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

const birth = (pid) => ({
  birthVersion: "linux-proc-v1",
  pid,
  startTicks: "1234",
  pgid: 10,
  ppid: 11,
  bootId: "8116e36c-eeea-4ca0-973f-b1af4f6144d6",
});

function harness() {
  vi.useFakeTimers({
    toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "hrtime"],
  });
  const worker = { pid: 13 },
    ancestors = [{ pid: 10 }, { pid: 11 }, { pid: 12 }];
  const known = new Map([...ancestors, worker].map((p) => [p.pid, p]));
  const owner = { qualified: worker, isWorker: (p) => p.pid === 13 };
  vi.spyOn(peerRoles, "peerProcessIdentity").mockReturnValue([]);
  const plans = new Map();
  const hanging = [];
  let calls = 0;
  const sample = vi.fn(() => {
    const n = ++calls;
    const plan = plans.get(n) ?? {};
    const start = process.hrtime.bigint();
    const done = Promise.withResolvers();
    const settle = () => {
      if (plan.fail) return done.reject(Error("ordinary sample failed"));
      const end = process.hrtime.bigint();
      done.resolve({
        summedRssKiB: 4,
        processes: [
          { disposition: "live", pid: 10, statusKiB: { VmRSS: 4 }, smapsRollupKiB: { Rss: 4 } },
        ],
        helper: { spawnHrtimeNs: String(start), closed: true, pipesClosed: true },
        timing: {
          clock: { qualified: true },
          parent: {
            entryNs: String(start),
            resultNs: String(end),
            deadlineNs: String(start + 750n * MS),
          },
        },
      });
    };
    if (plan.hang) hanging.push(done);
    else if (plan.ms) setTimeout(settle, plan.ms);
    else queueMicrotask(settle);
    return done.promise;
  });
  const lateTicks = new Map();
  let ticks = 0;
  const realSetInterval = globalThis.setInterval;
  // Model a late ordinary tick: the timer fires on time but the dispatch runs later.
  vi.spyOn(globalThis, "setInterval").mockImplementation((cb, ms) =>
    realSetInterval(() => {
      const late = lateTicks.get(++ticks);
      if (late) setTimeout(cb, late);
      else cb();
    }, ms),
  );
  const monitor = startFixedRssSafety({
    server: { handle: { known } },
    fixedExecutables: { protocol: "linux-fixed-peer.v4", root: { path: "/node" } },
    nextOwner: () => owner,
    peer: {},
    limits: { rssCeilingKiB: 4194304, sampleEveryMs: 1000 },
    sample,
    append() {},
    abort() {},
    checkRoles: () => ({
      chain: [{ pid: 10, role: "root", pgid: 10, ppid: 100, bootId: "boot", startTicks: "1" }],
    }),
  });
  return {
    monitor,
    sample,
    plans,
    lateTicks,
    get ticks() {
      return ticks;
    },
    ancestors,
    async done() {
      for (const d of hanging) d.reject(Error("test cleanup"));
      const stopped = monitor.stop();
      await vi.advanceTimersByTimeAsync(1000);
      await stopped;
    },
  };
}

// An authenticated, already-received receipt that stays valid until `validMs` from now.
function receivedGate(validMs = 10_000) {
  const now = process.hrtime.bigint();
  const authority = {
    outer: birth(300),
    worker: birth(301),
    credentials: { pid: 300, uid: 1000, gid: 1000 },
    nonce: "ab".repeat(16),
    token: "cd".repeat(16),
    descriptors: [
      ["6", "123"],
      ["7", "456"],
    ],
    startNs: String(now - 50n * MS),
    deadlineNs: String(now + BigInt(validMs) * MS),
  };
  const gate = outerTerminalGate({ authority });
  gate.accept({
    kernelSender: { ...authority.credentials },
    currentOuter: { ...authority.outer },
    packet: {
      worker: { ...authority.worker },
      nonce: authority.nonce,
      token: authority.token,
      descriptors: structuredClone(authority.descriptors),
      pidfdFlags: 0,
      pollMask: 17,
      firstLiveNs: String(now - 40n * MS),
      secondLiveNs: String(now - 30n * MS),
      terminalNs: String(now - 20n * MS),
      sentNs: String(now - 10n * MS),
    },
  });
  return gate;
}

// Ready plus two accepted post-ready cycles; returns at the start of the next dispatch.
async function ready(h) {
  const r = h.monitor.verifyReady(process.hrtime.bigint());
  await vi.advanceTimersByTimeAsync(2000);
  await r;
}

function decide(h, gate, { attemptMs = 60_000, workMs = 7_000_000, signal, census } = {}) {
  const now = process.hrtime.bigint();
  return outerPrewriteDecision({
    safety: h.monitor,
    gate,
    readProductGraph: () => h.ancestors,
    verifyWiring: census ?? (() => ({ census: "exact" })),
    attemptDeadlineNs: now + BigInt(attemptMs) * MS,
    workDeadlineNs: now + BigInt(workMs) * MS,
    signal,
  });
}

async function settled(promise, stepMs = 5, steps = 400) {
  let out;
  promise.then(
    (value) => (out = { value }),
    (error) => (out = { error }),
  );
  for (let i = 0; i < steps && !out; i++) await vi.advanceTimersByTimeAsync(stepMs);
  assert(out, "decision did not settle");
  return out;
}

test("R3 replay: the old composition refuses; settle-then-decide decides on the fresh sample", async () => {
  const h = harness();
  try {
    await ready(h);
    const next = h.sample.mock.calls.length + 1;
    h.plans.set(next, { ms: R3_HELPER_MS });
    await vi.advanceTimersByTimeAsync(1000);
    assert.equal(h.sample.mock.calls.length, next);
    await vi.advanceTimersByTimeAsync(R3_ARM_AFTER_DISPATCH_MS);
    // f0ce3b867 composition: consume the receipt against the current graph at once.
    const old = receivedGate();
    assert.throws(
      () =>
        old.consume(() => {
          h.monitor.verifyOuterGraph(
            () => h.ancestors,
            () => {},
            "prewrite-decision",
          );
          return true;
        }),
      (error) => error.outerRefusal.coverage === "ordinary-sample-pending",
    );
    const armNs = process.hrtime.bigint();
    const { value, error } = await settled(decide(h, receivedGate()), 1);
    assert.equal(error, undefined, String(error));
    const s = value.preDecisionSettle;
    assert.equal(s.pendingAtDecision, true);
    assert.equal(s.outcome, "settled");
    assert.equal(s.pendingEpoch, s.settledEpoch);
    assert.equal(value.outerCadence.epoch, s.settledEpoch);
    const waited = BigInt(s.waitedNs);
    assert(waited >= 30n * MS && waited <= 32n * MS, String(waited));
    const decision = BigInt(value.outerDecision.decisionNs);
    assert(decision >= armNs + waited && decision < BigInt(value.outerCadence.cadenceDeadlineNs));
    assert.equal(BigInt(s.boundNs), BigInt(s.enteredNs) - 66n * MS + 950n * MS);
  } finally {
    await h.done();
  }
});

// Decision phase relative to an ordinary dispatch, with the R3 helper duration.
// Phases inside the helper wait for it; after the cadence cutoff (dispatch +995 ms)
// and before the next dispatch there is no sample in flight, so the existing
// cadence-due refusal still applies there.
test.each([0, 1, 30, 66, 96, 97, 150, 500, 900, 994, 995, 999])(
  "decision at dispatch + %i ms",
  async (phaseMs) => {
    const h = harness();
    try {
      await ready(h);
      const base = h.sample.mock.calls.length;
      for (let n = base + 1; n <= base + 3; n++) h.plans.set(n, { ms: R3_HELPER_MS });
      // The receipt is already received before the arm, as the contract requires.
      const gate = receivedGate();
      await vi.advanceTimersByTimeAsync(1000);
      if (phaseMs) await vi.advanceTimersByTimeAsync(phaseMs);
      const { value, error } = await settled(decide(h, gate), 1);
      assert.equal(error, undefined, String(error));
      const s = value.preDecisionSettle;
      // After the cadence cutoff (dispatch + 995 ms) nothing is in flight; the
      // decision joins the ordinary timer's next dispatch and settles it.
      const joined = phaseMs >= 995;
      assert.equal(s.pendingAtDecision, phaseMs < R3_HELPER_MS);
      assert.equal(
        s.outcome,
        joined ? "joined-next-dispatch" : phaseMs < R3_HELPER_MS ? "settled" : "not-pending",
      );
      if (!joined && phaseMs >= R3_HELPER_MS) assert.equal(s.waitedNs, "0");
      if (joined) {
        assert.equal(BigInt(s.waitedNs), BigInt(1000 - phaseMs + R3_HELPER_MS) * MS);
        assert.equal(s.settledEpoch, s.pendingEpoch);
        assert.equal(value.outerCadence.epoch, s.pendingEpoch);
        assert.equal(BigInt(s.dispatchBoundNs), BigInt(s.scheduledDispatchNs) + 300n * MS);
      }
      assert(BigInt(value.outerDecision.decisionNs) < BigInt(value.outerCadence.cadenceDeadlineNs));
    } finally {
      await h.done();
    }
  },
);

test.each([
  [250, "joined"],
  [299, "joined"],
  [350, "refused"],
])("a decision after the cutoff with the next tick %i ms late is %s", async (lateMs, verdict) => {
  const h = harness();
  try {
    await ready(h);
    const base = h.sample.mock.calls.length;
    for (let n = base + 1; n <= base + 3; n++) h.plans.set(n, { ms: R3_HELPER_MS });
    const gate = receivedGate();
    await vi.advanceTimersByTimeAsync(1000 + 997);
    h.lateTicks.set(h.ticks + 1, lateMs);
    const { value, error } = await settled(decide(h, gate), 1);
    if (verdict === "joined") {
      assert.equal(error, undefined, String(error));
      assert.equal(value.preDecisionSettle.outcome, "joined-next-dispatch");
      assert.equal(h.sample.mock.calls.length, base + 2, "only the ordinary timer dispatched");
    } else {
      assert.equal(error?.outerRefusal?.coverage, "cadence-due", String(error));
      const s = error.outerRefusal.preDecisionSettle;
      assert.equal(s.outcome, "dispatch-late");
      assert.equal(BigInt(s.waitedNs), BigInt(s.dispatchBoundNs) - BigInt(s.enteredNs));
      assert.equal(h.sample.mock.calls.length, base + 1, "no sample was dispatched to rescue it");
    }
  } finally {
    await h.done();
  }
});

test.each([
  "receipt-expires-during-wait",
  "sample-fails-during-wait",
  "settle-bound-expires",
  "attempt-deadline-first",
  "aborted-during-wait",
  "stopped-during-wait",
  "census-fails-after-settle",
  "no-receipt",
])("a pending decision refuses when %s", async (mode) => {
  const h = harness();
  try {
    await ready(h);
    const next = h.sample.mock.calls.length + 1;
    h.plans.set(
      next,
      mode === "sample-fails-during-wait"
        ? { ms: R3_HELPER_MS, fail: true }
        : mode === "settle-bound-expires"
          ? { hang: true }
          : { ms: R3_HELPER_MS },
    );
    await vi.advanceTimersByTimeAsync(1000 + R3_ARM_AFTER_DISPATCH_MS);
    const ac = new AbortController();
    const gate =
      mode === "no-receipt"
        ? null
        : receivedGate(mode === "receipt-expires-during-wait" ? 10 : 10_000);
    const p = decide(h, gate, {
      attemptMs: mode === "attempt-deadline-first" ? 10 : 60_000,
      signal: ac.signal,
      census:
        mode === "census-fails-after-settle"
          ? () => {
              throw Error("unexpected outer graph child");
            }
          : undefined,
    });
    if (mode === "aborted-during-wait") ac.abort(Error("cancelled"));
    if (mode === "stopped-during-wait") await vi.advanceTimersByTimeAsync(5);
    const stop = mode === "stopped-during-wait" ? h.monitor.stop() : null;
    const { value, error } = await settled(p, 5, 400);
    await stop;
    assert(error, "must refuse: " + JSON.stringify(value?.preDecisionSettle));
    const row = error.outerRefusal;
    assert(row, String(error));
    const s = row.preDecisionSettle;
    const expected = {
      "receipt-expires-during-wait": [/receipt unavailable before decision/, "settled"],
      "sample-fails-during-wait": [/sampler-failed/, "settled"],
      "settle-bound-expires": [/ordinary-sample-pending/, "bound-expired"],
      "attempt-deadline-first": [/ordinary-sample-pending/, "bound-expired"],
      "aborted-during-wait": [/cancelled/, undefined],
      "stopped-during-wait": [/sampler-stopped/, "settled"],
      "census-fails-after-settle": [/unexpected outer graph child/, "settled"],
      "no-receipt": [/outer receipt unavailable/, "settled"],
    }[mode];
    assert.match(String(error.message), expected[0]);
    if (mode === "aborted-during-wait") assert.equal(row.stage, "prewrite-settle");
    else {
      assert.equal(s.pendingAtDecision, true);
      assert.equal(s.outcome, expected[1]);
    }
    if (mode === "settle-bound-expires")
      assert(BigInt(s.waitedNs) <= 950n * MS - BigInt(R3_ARM_AFTER_DISPATCH_MS) * MS + MS);
    if (mode === "attempt-deadline-first") assert(BigInt(s.waitedNs) <= 11n * MS);
  } finally {
    await h.done();
  }
});

test("a decision inside coverage with nothing pending is unchanged and waits zero", async () => {
  const h = harness();
  try {
    await ready(h);
    const gate = receivedGate();
    await vi.advanceTimersByTimeAsync(400);
    const { value, error } = await settled(decide(h, gate), 1);
    assert.equal(error, undefined, String(error));
    assert.deepEqual(
      {
        pending: value.preDecisionSettle.pendingAtDecision,
        waited: value.preDecisionSettle.waitedNs,
        outcome: value.preDecisionSettle.outcome,
      },
      { pending: false, waited: "0", outcome: "not-pending" },
    );
    assert.equal(h.sample.mock.calls.length, 3, "no extra sampler dispatch");
  } finally {
    await h.done();
  }
});

test("a receipt consumed before a later expiry refusal stays attributable", async () => {
  const h = harness();
  try {
    await ready(h);
    const gate = receivedGate();
    await vi.advanceTimersByTimeAsync(400);
    const { error } = await settled(decide(h, gate, { attemptMs: 0 }), 1);
    assert.match(String(error?.message), /outer decision expired/);
    assert.equal(error.outerDecision.packet.pollMask, 17);
    assert.equal(error.outerRefusal.preDecisionSettle.outcome, "not-pending");
    assert.throws(() => gate.consume(() => true), /outer receipt unavailable/);
  } finally {
    await h.done();
  }
});
