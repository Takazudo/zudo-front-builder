import assert from "node:assert/strict";
import { afterEach, test, vi } from "vitest";
import * as peerRoles from "../lib/fixed-peer-roles.mjs";
import {
  PREWRITE_FRESH_MARGIN_NS,
  PREWRITE_MAX_WAIT_NS,
  joinDispatchBound,
  startFixedRssSafety,
} from "../lib/fixed-rss-safety.mjs";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readPeerTarget, snapshotPaths } from "../lib/fixture.mjs";
import {
  NEXT_OUTER_OPERATIONS,
  nextOuterBatchHooks,
  nextOuterOutcomeStatus,
  outerPrewriteDecision,
  outerTerminalGate,
} from "../lib/next-outer-terminal.mjs";
import { EXIT, batchExitCode, nextOuterOutcomeRecorder, nextOuterRunOutcome } from "../lib/run.mjs";
import { runBatch } from "../lib/state-machine.mjs";
import { sha256 } from "../lib/util.mjs";
import { deps, fakeFs, fakePage } from "./fakes.mjs";

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

function harness({ append = () => {} } = {}) {
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
    append,
    abort() {},
    checkRoles: () => ({
      chain: [{ pid: 10, role: "root", pgid: 10, ppid: 100, bootId: "boot", startTicks: "1" }],
    }),
  });
  return {
    monitor,
    owner,
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

const MARGIN_MS = Number(PREWRITE_FRESH_MARGIN_NS / MS);
// Last phase still inside coverage: the cutoff (dispatch + 995 ms) less the margin.
const FRESH_UNTIL_MS = 995 - MARGIN_MS;
// Owner decision (2026-11-28): a join entered at dispatch + phase ms keeps the
// 1355 ms from-entry maximum, so its lateness allowance is the budget left after
// the 950 ms settle (1355 - 950 - (1000 - phase) = phase - 595 ms), at most 400 ms.
const allowanceMs = (phaseMs) => Math.min(400, phaseMs - 595);

// Decision phase relative to an ordinary dispatch, with the R3 helper duration.
// Phases inside the helper wait for it. From the fresh margin before the cadence
// cutoff (dispatch + 995 ms) to the next dispatch nothing is in flight, so the
// decision joins the ordinary timer's next dispatch and settles it, inside the
// margin with the reduced allowance.
test.each([
  0,
  1,
  30,
  66,
  96,
  97,
  150,
  500,
  FRESH_UNTIL_MS - 1,
  FRESH_UNTIL_MS,
  900,
  994,
  995,
  996,
  999,
])("decision at dispatch + %i ms", async (phaseMs) => {
  const h = harness();
  try {
    await ready(h);
    const base = h.sample.mock.calls.length;
    for (let n = base + 1; n <= base + 3; n++) h.plans.set(n, { ms: R3_HELPER_MS });
    // The receipt is already received before the arm, as the contract requires.
    const gate = receivedGate();
    await vi.advanceTimersByTimeAsync(1000);
    if (phaseMs) await vi.advanceTimersByTimeAsync(phaseMs);
    const dispatched = h.sample.mock.calls.length;
    const { value, error } = await settled(decide(h, gate), 1);
    assert.equal(error, undefined, String(error));
    const s = value.preDecisionSettle;
    const joined = phaseMs >= FRESH_UNTIL_MS;
    assert.equal(s.pendingAtDecision, phaseMs < R3_HELPER_MS);
    assert.equal(
      s.outcome,
      joined ? "joined-next-dispatch" : phaseMs < R3_HELPER_MS ? "settled" : "not-pending",
    );
    if (!joined && phaseMs >= R3_HELPER_MS) assert.equal(s.waitedNs, "0");
    assert(BigInt(s.waitedNs) <= PREWRITE_MAX_WAIT_NS, s.waitedNs);
    assert.equal(s.maxWaitNs, String(PREWRITE_MAX_WAIT_NS));
    if (joined) {
      assert.equal(s.withinFreshMargin, phaseMs < 995);
      assert.equal(BigInt(s.waitedNs), BigInt(1000 - phaseMs + R3_HELPER_MS) * MS);
      assert.equal(s.settledEpoch, s.pendingEpoch);
      assert.equal(value.outerCadence.epoch, s.pendingEpoch);
      assert.equal(s.declaredAllowanceNs, String(400n * MS));
      assert.equal(BigInt(s.allowanceNs), BigInt(allowanceMs(phaseMs)) * MS);
      assert.equal(
        BigInt(s.dispatchBoundNs),
        BigInt(s.scheduledDispatchNs) + BigInt(s.allowanceNs),
      );
      assert.equal(h.sample.mock.calls.length, dispatched + 1, "only the ordinary dispatch");
    } else assert.equal(s.allowanceNs, undefined);
    assert(BigInt(value.outerDecision.decisionNs) < BigInt(value.outerCadence.cadenceDeadlineNs));
  } finally {
    await h.done();
  }
});

test.each([
  [250, "joined"],
  [350, "joined"],
  [399, "joined"],
  [450, "refused"],
])("a decision after the cutoff with the next tick %i ms late is %s", async (lateMs, verdict) => {
  const h = harness();
  try {
    await ready(h);
    const base = h.sample.mock.calls.length;
    for (let n = base + 1; n <= base + 3; n++) h.plans.set(n, { ms: R3_HELPER_MS });
    const gate = receivedGate();
    await vi.advanceTimersByTimeAsync(1000 + 997);
    h.lateTicks.set(h.ticks + 1, lateMs);
    const { value, error } = await settled(decide(h, gate), 1, 1000);
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

// Run level: the hooks executeLocked installs, driven by the real runBatch on a
// decision-only edit-body cell. Proves the pre-write decision is awaited, the
// settle record lands on the next-outer-decision row, and a refusal carries it.
test.each(["decided-after-settle", "refused-receipt-expired"])(
  "runBatch awaits the outer pre-write decision: %s",
  async (mode) => {
    const dir = mkdtempSync(join(tmpdir(), "outer-settle-"));
    const h = harness();
    try {
      const path = join(dir, "article-00005.md");
      writeFileSync(path, "v0");
      await ready(h);
      h.plans.set(h.sample.mock.calls.length + 1, { ms: R3_HELPER_MS });
      await vi.advanceTimersByTimeAsync(1000 + R3_ARM_AFTER_DISPATCH_MS);
      const gate = receivedGate(mode === "refused-receipt-expired" ? 10 : 10_000);
      const fs = fakeFs();
      fs.snapshot = () => snapshotPaths({ files: [path] });
      fs.readPeerTarget = readPeerTarget;
      let writes = 0;
      // The outer T0 check runs inside the write's clock read, before any syscall.
      fs.performWrite = (_w, now) => {
        now();
        writes++;
        throw Error("no write may happen in a decision-only cell");
      };
      fs.prepareAttempt = (op) => ({
        token: op.id,
        targetPath: "/articles/article-00005/",
        predicate: { kind: "text", selector: ".body", includes: op.id },
        write: { method: "in-place", path, bytes: Buffer.from(op.id) },
        supplemental: [],
      });
      const page = fakePage();
      const d = deps({ page, fs, clock: { now: () => process.hrtime.bigint() } });
      const order = [];
      Object.assign(
        d,
        nextOuterBatchHooks({
          safety: h.monitor,
          gate: () => gate,
          readProductGraph: () => h.ancestors,
          verifyWiring: () => ({ census: "exact" }),
          operation: "decision-only",
          workDeadlineNs: process.hrtime.bigint() + 7_000_000n * MS,
          signal: new AbortController().signal,
          append: (r) => d.recorder.append(r),
          onDecision: () => order.push("decision"),
        }),
        {
          beforePeerMutation: () => order.push("mutation-gate"),
          // executeLocked installs a stage budget for every fixed peer; the
          // outer decision derives its attempt deadline from that attemptStart.
          stageBudget: (ms) => ms,
        },
      );
      const op = { id: "edit-05", op: "edit-body", article: "article-00005", expect: "classify" };
      const { value, error } = await settled(
        runBatch({ deps: d, scenarios: [{ id: "fn-edit-body", ops: [op] }] }),
        1,
      );
      assert.equal(error, undefined, String(error));
      assert(value.stopped);
      assert.equal(writes, 0);
      assert.equal(readFileSync(path, "utf8"), "v0");
      const attempt = d.recorder.records.find((r) => r.type === "attempt");
      if (mode === "decided-after-settle") {
        assert.deepEqual(order, ["decision", "mutation-gate"], "decision awaited before the gate");
        const row = d.recorder.records.find((r) => r.type === "next-outer-decision");
        assert.equal(row.preDecisionSettle.outcome, "settled");
        assert.equal(row.preDecisionSettle.pendingAtDecision, true);
        assert(BigInt(row.preDecisionSettle.waitedNs) >= 30n * MS);
        assert.equal(row.outerCadence.epoch, row.preDecisionSettle.settledEpoch);
        assert(d.recorder.records.some((r) => r.type === "next-outer-no-write"));
        assert.equal(attempt.outcome, "outer-decision-only");
        assert.equal(value.accounting.stopReason, "outer-decision-only");
      } else {
        assert.deepEqual(order, [], "no decision and no mutation gate after a refused receipt");
        assert(!d.recorder.records.some((r) => r.type === "next-outer-decision"));
        assert.equal(attempt.outcome, "identity-rejected");
        assert.match(attempt.error, /receipt unavailable before decision/);
        assert.equal(attempt.outerRefusal.preDecisionSettle.outcome, "settled");
        // The gate's own clock check refuses after the graph callback returned, so
        // the checkpoint reports the graph as unknown (unchanged behaviour).
        assert.equal(attempt.outerRefusal.stage, "prewrite-decision");
        assert.equal(attempt.outerRefusal.coverage, "unknown");
        assert.equal(value.accounting.stopReason, "identity-rejected");
      }
      assert.equal(
        d.recorder.records.find((r) => r.type === "cleanup").liveSkipped,
        "before-mutation-no-write",
      );
    } finally {
      await h.done();
      rmSync(dir, { recursive: true, force: true });
    }
  },
);

// Review 1 finding 3: executeLocked's outerDecision feeds verifyPeerProcesses and the
// terminal-proof reconciliation, so a receipt consumed before a later refusal must
// still reach onDecision.
test("the hooks mirror a consumed receipt to onDecision when the decision then expires", async () => {
  const h = harness();
  try {
    await ready(h);
    const gate = receivedGate();
    await vi.advanceTimersByTimeAsync(400);
    const seen = [];
    const hooks = nextOuterBatchHooks({
      safety: h.monitor,
      gate: () => gate,
      readProductGraph: () => h.ancestors,
      verifyWiring: () => ({ census: "exact" }),
      operation: "decision-only",
      workDeadlineNs: process.hrtime.bigint() + 7_000_000n * MS,
      signal: new AbortController().signal,
      append: () => assert.fail("no decision row after a refusal"),
      onDecision: (decision, cadence) => seen.push({ decision, cadence }),
    });
    const { error } = await settled(
      hooks.beforePeerDecision({ attemptStart: process.hrtime.bigint(), attemptMs: 0 }),
      1,
    );
    assert.match(String(error?.message), /outer decision expired/);
    assert.equal(seen.length, 1);
    assert.equal(seen[0].decision, error.outerDecision);
    assert.equal(seen[0].decision.packet.worker.pid, 301);
    assert.throws(() => gate.consume(() => true), /outer receipt unavailable/);
  } finally {
    await h.done();
  }
});

// Review 1 note 4: an overlapping pre-write identity decision never starts a wait,
// and stop wakes a decision that is waiting for the next dispatch.
test("settle does not wait while a pre-write identity decision is pending", async () => {
  const h = harness();
  try {
    await ready(h);
    await vi.advanceTimersByTimeAsync(997);
    const poll = Promise.withResolvers();
    Object.assign(h.owner, { pollBeforeMutation: () => poll.promise });
    const identity = h.monitor.verifyPrewriteTerminal({
      getOwned: () => h.ancestors,
      workDeadlineNs: process.hrtime.bigint() + 7_000_000n * MS,
    });
    const s = await h.monitor.settleOrdinaryBeforeDecision({
      deadlineNs: process.hrtime.bigint() + 60_000n * MS,
    });
    assert.deepEqual([s.outcome, s.waitedNs, s.pendingAtDecision], ["not-pending", "0", false]);
    poll.resolve();
    await identity;
  } finally {
    await h.done();
  }
});

test("stopping the monitor wakes a decision waiting for the next dispatch", async () => {
  const h = harness();
  try {
    await ready(h);
    await vi.advanceTimersByTimeAsync(997);
    let s;
    const waiting = h.monitor
      .settleOrdinaryBeforeDecision({ deadlineNs: process.hrtime.bigint() + 60_000n * MS })
      .then((r) => (s = r));
    await vi.advanceTimersByTimeAsync(1);
    assert.equal(s, undefined, "waiting for the next dispatch");
    await h.monitor.stop();
    await waiting;
    assert.equal(s.outcome, "dispatch-unavailable");
    assert(BigInt(s.waitedNs) <= 2n * MS, s.waitedNs);
  } finally {
    await h.done();
  }
});

// F6 (#4870 body cell): the normal "body" operation through the real hooks, the
// real gate and runBatch. One in-place write happens at the outer T0, the edit
// is classified, restoration and live checks run, and the terminal status is
// "body" for success and "body-classified" for a declared stale/timeout.
async function bodyCell(outcome, { operation = "body", phaseMs = 300, cost = null } = {}) {
  const h = harness();
  try {
    await ready(h);
    const base = h.sample.mock.calls.length;
    for (let n = base + 1; n <= base + 3; n++) h.plans.set(n, { ms: R3_HELPER_MS });
    // The receipt arrives before the arm; the decision then falls at `phaseMs`
    // after the ordinary dispatch.
    const gate = receivedGate();
    await vi.advanceTimersByTimeAsync(phaseMs);
    // Synchronous census work blocks the loop: the clock advances by its cost
    // while no timer can run, exactly as on the real host.
    let skew = 0n;
    const charge = (ms) => {
      if (cost) skew += BigInt(Math.ceil(ms * 1e6));
    };
    if (cost) {
      const fake = process.hrtime.bigint;
      vi.spyOn(process.hrtime, "bigint").mockImplementation(() => fake.call(process.hrtime) + skew);
    }
    const fs = fakeFs();
    let proofs = 0;
    const refusals = {};
    const write = fs.performWrite;
    fs.performWrite = (w, now) => {
      w.evidence = { inodeBefore: 13, inodeAfter: 13 };
      return write(w, now);
    };
    fs.hashOf = (path) => sha256(fs.files[path]);
    const restore = fs.restore;
    // The real fixture reports `restored` as the paths of its actions (fixture.mjs).
    fs.restore = (snapshot) => {
      const actions = fs.writes.length
        ? [{ path: "a.md", method: "in-place", evidence: { inodeBefore: 13, inodeAfter: 13 } }]
        : [];
      return { ...restore(snapshot), actions, restored: actions.map((a) => a.path) };
    };
    const page = fakePage({
      onWait: (token, _late, p) => {
        vi.advanceTimersByTime(5);
        const now = process.hrtime.bigint();
        p.evidence.docInits = outcome === "stale" ? [{ ns: String(now) }] : [];
        p.evidence.newDocuments = p.evidence.docInits.length;
        p.evidence.wsFrames = [];
        p.evidence.navigations = [];
        if (outcome === "success")
          return Promise.resolve({ kind: "notify", t1: now, payload: { token, docUUID: "doc-1" } });
        return Promise.resolve({ kind: "timeout" });
      },
      liveCheck: () => ({ ok: true, docUUID: "fresh-detail" }),
    });
    page.peerSocketTracking = new Map([[1, { firstFrameNs: "10" }]]);
    const arm = page.arm;
    page.arm = async (...args) => ({ ...(await arm(...args)), actual: "v0" });
    page.settle = async () => ({
      settled: true,
      nativeUpdateObserved: true,
      bodyRevision: { matched: true, docUUID: "doc-1" },
    });
    const d = {
      ...deps({ page, fs, clock: { now: () => process.hrtime.bigint() } }),
      workDeadlineNs: process.hrtime.bigint() + 7_000_000n * MS,
      verifyBodyRevision: true,
      fixedPeer: true,
      stageBudget: (ms) => ms,
      beforePeerMutation: () => charge(cost?.late ?? 0),
    };
    // The original Next attempt budget, so only the cadence cutoff can expire.
    d.timeouts = { ...d.timeouts, attemptMs: 60000 };
    const hooks = nextOuterBatchHooks({
      safety: h.monitor,
      gate: () => gate,
      readProductGraph: () => (charge(cost?.product ?? 0), h.ancestors),
      verifyWiring: () => (charge(cost?.wiring ?? 0), { census: "exact" }),
      operation,
      workDeadlineNs: d.workDeadlineNs,
      signal: new AbortController().signal,
      append: (r) => (charge(cost?.append ?? 0), d.recorder.append(r)),
      onRefusal: (stage) => (refusals[stage] = (refusals[stage] ?? 0) + 1),
    });
    const outerPeerWriteTime = hooks.outerPeerWriteTime;
    hooks.outerPeerWriteTime = (now) => (proofs++, outerPeerWriteTime(now));
    Object.assign(d, hooks);
    const op = { id: "body-05", op: "edit-body", article: "a", expect: "classify" };
    const { value, error } = await settled(
      runBatch({ deps: d, scenarios: [{ id: "fn-edit-body", ops: [op] }] }),
      1,
    );
    assert.equal(error, undefined, String(error));
    const { accounting, stopped } = value;
    // The exit code and outcome derivations executeLocked itself uses.
    const exitCode = batchExitCode({ accounting, stopped, fixedPeer: true });
    const decision = d.recorder.records.find((r) => r.type === "next-outer-decision");
    const terminal = (overrides = {}) =>
      nextOuterOutcomeStatus({
        operation,
        records: d.recorder.records,
        closed: true,
        proofComplete: true,
        exitCode,
        exit: EXIT,
        accounting,
        outerDecision: decision?.outerDecision,
        ...overrides,
      });
    const runOutcome = (overrides = {}) =>
      nextOuterRunOutcome({
        stopReport: { absent: true },
        stopFailure: null,
        unresolved: [],
        evidenceFailed: false,
        population: { ok: true },
        safetyFailure: null,
        operation,
        records: d.recorder.records,
        exitCode,
        accounting,
        outerDecision: decision?.outerDecision,
        refusals,
        ...overrides,
      });
    return {
      d,
      fs,
      accounting,
      stopped,
      exitCode,
      proofs,
      decision,
      terminal,
      runOutcome,
      refusals,
    };
  } finally {
    await h.done();
  }
}

test.each([
  ["success", "body", null, EXIT.ok],
  ["stale", "body-classified", "stale", EXIT.stopped],
  ["timeout", "body-classified", "timeout", EXIT.stopped],
])("the body operation writes once at the outer T0: %s -> %s", async (outcome, status, cls, rc) => {
  const { d, fs, accounting, exitCode, proofs, decision, terminal } = await bodyCell(outcome);
  assert.equal(proofs, 1);
  assert.deepEqual(fs.writes, ["a.md"]);
  assert.equal(fs.files["a.md"], "v0", "restored");
  assert(!d.recorder.records.some((r) => r.type === "next-outer-no-write"));
  const attempt = d.recorder.records.find((r) => r.type === "attempt");
  assert.equal(attempt.outcome, outcome);
  // T0 is the outer proof's stamp, drawn after the pre-truncate census.
  assert.equal(attempt.prewriteDecision.t0Ns, String(attempt.t0Ns ?? fs.lastT0));
  assert.equal(String(fs.lastT0), attempt.prewriteDecision.t0Ns);
  assert(BigInt(attempt.prewriteDecision.t0Ns) < BigInt(decision.outerCadence.cadenceDeadlineNs));
  assert.equal(decision.preDecisionSettle.outcome, "not-pending");
  const disk = d.recorder.records.find((r) => r.type === "fixed-peer-edit-disk");
  assert(disk.ok);
  const cleanup = d.recorder.records.find((r) => r.type === "cleanup");
  assert(cleanup.ok && cleanup.liveSkipped === null && cleanup.live.every((l) => l.ok));
  assert.equal(accounting.attempted, 1);
  assert.equal(exitCode, rc);
  assert.deepEqual(terminal(), cls ? { status, classification: cls } : { status });
  // Each gate of the terminal status is load-bearing.
  for (const overrides of [
    { closed: false },
    { proofComplete: false },
    { outerDecision: undefined },
    { operation: "decision-only" },
    { exitCode: rc === EXIT.ok ? EXIT.stopped : EXIT.ok },
    { exitCode: EXIT.controllerError },
    { accounting: { ...accounting, restorationFailures: 1 } },
    { accounting: { ...accounting, error: 1 } },
    { records: d.recorder.records.filter((r) => r.type !== "cleanup") },
    {
      records: d.recorder.records.map((r) =>
        r.type === "cleanup" ? { ...r, live: [{ ok: false }] } : r,
      ),
    },
    {
      records: d.recorder.records.map((r) =>
        r.type === "cleanup" ? { ...r, liveSkipped: "fail-stop-cleanup" } : r,
      ),
    },
    {
      records: d.recorder.records.map((r) =>
        r.type === "cleanup"
          ? { ...r, actions: [{ ...r.actions[0], method: "atomic-rename" }] }
          : r,
      ),
    },
    { records: [...d.recorder.records, { type: "next-outer-no-write" }] },
    // Review 1 finding 3 (mutants H, J, L): each gate below is load-bearing.
    {
      records: d.recorder.records.map((r) =>
        r.type === "cleanup" ? { ...r, actions: [{ ...r.actions[0], path: "other.md" }] } : r,
      ),
    },
    {
      records: d.recorder.records.map((r) =>
        r.type === "cleanup" ? { ...r, settle: { ...r.settle, settled: false } } : r,
      ),
    },
    {
      // A no-write refusal beside a successful attempt is contradictory, never a body.
      records: [
        ...d.recorder.records,
        { phase: "before-mutation", writePerformed: false, outcome: "identity-rejected" },
      ],
    },
    // Mutants I and T apply only to the declared classify stop.
    ...(cls
      ? [
          { accounting: { ...accounting, stoppedByClassifyTimeout: false } },
          {
            accounting: {
              ...accounting,
              stale: cls === "stale" ? 0 : 1,
              timeout: cls === "timeout" ? 0 : 1,
            },
          },
        ]
      : []),
  ])
    assert.deepEqual(
      terminal(overrides),
      { status: "incomplete" },
      JSON.stringify(Object.keys(overrides)),
    );
});

test("a stale/timeout without the declared classify expectation is not a classified body", async () => {
  const { d, terminal } = await bodyCell("timeout");
  const records = d.recorder.records.map((r) =>
    r.type === "attempt" ? { ...r, expect: "success" } : r,
  );
  assert.deepEqual(terminal({ records }), { status: "incomplete" });
});

test("only the two declared outer operations build hooks", () => {
  assert.deepEqual([...NEXT_OUTER_OPERATIONS], ["decision-only", "body"]);
  for (const operation of ["write", "", undefined])
    assert.throws(
      () =>
        nextOuterBatchHooks({
          safety: {},
          gate: () => null,
          readProductGraph: () => [],
          verifyWiring: () => ({}),
          operation,
          workDeadlineNs: 0n,
          signal: new AbortController().signal,
          append() {},
        }),
      /undeclared Next outer operation/,
    );
});

// F1: the real decision -> T0 path charges its measured census cost. Measured
// workload-free on this WSL host (campaign 4870-next-body-r1/bench, loaded.json):
// the components of the slowest loaded iteration (232.0 ms total), rounded up.
// Pre-truncate charges product + wiring again (36.5 ms vs 28.0 measured), so the
// charged path is 242 ms. Every phase across the margin and the cutoff writes
// exactly once with T0 inside the decided epoch's coverage, within the from-entry
// maximum; inside the margin the join uses the reduced allowance.
const LOADED_MAX_MS = { product: 26, wiring: 11, append: 12, late: 156 };
const sweep = [];
for (let p = FRESH_UNTIL_MS - 12; p <= 999; p += p < 1000 - 20 ? 3 : 1) sweep.push(p);
test.each(sweep)(
  "a decision at dispatch + %i ms with the loaded census cost still writes",
  async (phaseMs) => {
    const { fs, proofs, decision, d, refusals, runOutcome } = await bodyCell("success", {
      phaseMs,
      cost: LOADED_MAX_MS,
    });
    const attempt = d.recorder.records.find((r) => r.type === "attempt");
    assert.equal(attempt.outcome, "success", attempt.error);
    assert.equal(runOutcome().status, "body");
    assert.deepEqual(refusals, {});
    assert(BigInt(decision.preDecisionSettle.waitedNs) <= PREWRITE_MAX_WAIT_NS);
    assert.equal(proofs, 1);
    assert.deepEqual(fs.writes, ["a.md"]);
    const s = decision.preDecisionSettle;
    assert.equal(s.outcome, phaseMs >= FRESH_UNTIL_MS ? "joined-next-dispatch" : "not-pending");
    if (phaseMs >= FRESH_UNTIL_MS)
      assert.equal(BigInt(s.allowanceNs), BigInt(allowanceMs(phaseMs)) * MS);
    assert(BigInt(attempt.prewriteDecision.t0Ns) < BigInt(decision.outerCadence.cadenceDeadlineNs));
  },
);

// Control: the sweep is not vacuous. A path costing more than the margin, entered
// just inside it, crosses the cutoff before T0 and refuses without writing.
test("a census path longer than the margin refuses before the write (fail closed)", async () => {
  const over = { product: MARGIN_MS, wiring: 1, append: 0, late: 1 };
  const { fs, proofs, d, refusals, runOutcome } = await bodyCell("success", {
    phaseMs: FRESH_UNTIL_MS - 1,
    cost: over,
  });
  const attempt = d.recorder.records.find((r) => r.type === "attempt");
  assert.equal(attempt.outcome, "identity-rejected");
  assert.match(attempt.error, /outside current RSS coverage: cadence-due|outer T0 expired/);
  assert.equal(proofs, 1);
  assert.deepEqual(fs.writes, []);
  // Review 1 finding 4: the pre-truncate refusal is counted in the run outcome.
  assert.deepEqual(refusals, { "pre-truncate": 1 });
  assert.deepEqual(runOutcome().refusals, { "pre-truncate": 1 });
});

// F2: a millisecond timer can fire before the monotonic bound (#4878 A/A r3 woke
// 0.19 ms short). The dispatch wait must re-check the clock and keep the whole
// 400 ms allowance instead of reporting dispatch-late early.
test("the next-dispatch allowance is held on the monotonic clock when its timer fires early", async () => {
  const h = harness();
  try {
    await ready(h);
    const base = h.sample.mock.calls.length;
    for (let n = base + 1; n <= base + 3; n++) h.plans.set(n, { ms: R3_HELPER_MS });
    const gate = receivedGate();
    await vi.advanceTimersByTimeAsync(1000 + 997);
    h.lateTicks.set(h.ticks + 1, 600);
    let lag = 0n;
    const fake = process.hrtime.bigint;
    vi.spyOn(process.hrtime, "bigint").mockImplementation(() => fake.call(process.hrtime) - lag);
    const timeout = globalThis.setTimeout;
    vi.spyOn(globalThis, "setTimeout").mockImplementation((cb, ms, ...rest) =>
      timeout(
        ms >= 380 && ms <= 420
          ? () => {
              // The bound timer fires while the monotonic clock is 1 ms short.
              lag = MS;
              cb();
            }
          : cb,
        ms,
        ...rest,
      ),
    );
    const { error } = await settled(decide(h, gate), 1, 2000);
    const s = error?.outerRefusal?.preDecisionSettle;
    assert.equal(s?.outcome, "dispatch-late", String(error));
    assert.equal(lag, MS, "the early timer fired");
    assert(
      BigInt(s.waitedNs) >= BigInt(s.dispatchBoundNs) - BigInt(s.enteredNs),
      `waited ${s.waitedNs} ns, short of the ${BigInt(s.dispatchBoundNs) - BigInt(s.enteredNs)} ns allowance`,
    );
  } finally {
    vi.restoreAllMocks();
    await h.done();
  }
});

// Defensive half of the margin: verifyOuterGraph itself refuses a decision that
// enters with less than the margin left, even if a caller skipped the settle
// (a settled sample always leaves more: core <= 750 ms of the 995 ms coverage).
// Pre-truncate keeps the plain cutoff, so a decided epoch can still reach T0.
test.each([
  ["prewrite-decision", FRESH_UNTIL_MS - 1, null],
  ["prewrite-decision", FRESH_UNTIL_MS, "cadence-margin"],
  ["prewrite-decision", 994, "cadence-margin"],
  ["pre-truncate", 994, null],
  ["pre-truncate", 995, "cadence-due"],
])("verifyOuterGraph at %s, dispatch + %i ms: %s", async (stage, phaseMs, refusal) => {
  const h = harness();
  try {
    await ready(h);
    await vi.advanceTimersByTimeAsync(phaseMs);
    const run = () => h.monitor.verifyOuterGraph(() => h.ancestors, null, stage);
    if (!refusal) assert(run().cadenceDeadlineNs);
    else
      assert.throws(run, (error) => {
        assert.equal(error.outerRefusal.coverage, refusal);
        return true;
      });
  } finally {
    await h.done();
  }
});

// Review 1 finding 1, owner decision 2026-11-28. A decision entering inside the
// fresh margin, before the cutoff, used to join with the whole 400 ms allowance
// and could wait (cutoff - entry) + 5 + 400 + 950 ms from entry, about 1.6 s.
// It now joins with the allowance cut to phase - 595 ms, so the from-entry wait
// stays within 1355 ms; a tick later than the reduced allowance refuses (the
// wait is exactly the budget before the settle, 405 ms) and nothing retries.
test.each([
  [FRESH_UNTIL_MS, 149, 97, "joined"],
  [FRESH_UNTIL_MS, 151, 97, "refused"],
  [850, 254, 97, "joined"],
  [850, 256, 97, "refused"],
  [994, 398, 97, "joined"],
  [994, 400, 97, "refused"],
  [FRESH_UNTIL_MS, 149, 740, "joined"],
  [995, 0, 740, "joined"],
  [995, 399, 550, "joined"],
  [999, 399, 550, "joined"],
])(
  "from-entry bound: decision at dispatch + %i ms, next tick %i ms late, helper %i ms: %s",
  async (phaseMs, lateMs, helperMs, verdict) => {
    const h = harness();
    try {
      await ready(h);
      const base = h.sample.mock.calls.length;
      h.plans.set(base + 1, { ms: R3_HELPER_MS });
      h.plans.set(base + 2, { ms: helperMs });
      const gate = receivedGate();
      await vi.advanceTimersByTimeAsync(1000 + phaseMs);
      h.lateTicks.set(h.ticks + 1, lateMs);
      const { value, error } = await settled(decide(h, gate), 1, 2000);
      const s =
        verdict === "refused" ? error?.outerRefusal?.preDecisionSettle : value?.preDecisionSettle;
      assert(s, String(error));
      assert.equal(BigInt(s.allowanceNs), BigInt(allowanceMs(phaseMs)) * MS);
      assert.equal(s.withinFreshMargin, phaseMs < 995);
      if (verdict === "refused") {
        assert.equal(s.outcome, "dispatch-late");
        assert.equal(error.outerRefusal.coverage, "cadence-due", String(error));
        assert.equal(BigInt(s.waitedNs), 405n * MS);
        assert.equal(BigInt(s.dispatchBoundNs), BigInt(s.enteredNs) + 405n * MS);
        assert.equal(h.sample.mock.calls.length, base + 1, "no sample was dispatched to rescue it");
        await vi.advanceTimersByTimeAsync(2000);
        assert.throws(() => gate.consume(() => true), /outer receipt unavailable/, "no retry");
        return;
      }
      assert.equal(error, undefined, String(error));
      assert.equal(s.outcome, "joined-next-dispatch");
      assert.equal(BigInt(s.waitedNs), BigInt(1000 - phaseMs + lateMs + helperMs) * MS);
      assert(BigInt(s.waitedNs) <= PREWRITE_MAX_WAIT_NS);
      // The worst accepted join from this entry is exactly the maximum.
      assert(BigInt(s.dispatchBoundNs) + 950n * MS - BigInt(s.enteredNs) <= PREWRITE_MAX_WAIT_NS);
    } finally {
      await h.done();
    }
  },
);

// The budget either fits the scheduled dispatch plus the 950 ms settle or the
// join refuses. With the declared constants it fits for any entry from 405 ms
// before the scheduled dispatch, so every entry in the 250 ms margin fits.
test("joinDispatchBound: reduced allowance, budget boundary and declared cap", () => {
  const scheduled = 10_000n * MS;
  const at = (offsetNs) =>
    joinDispatchBound({ enteredNs: scheduled + offsetNs, scheduledDispatchNs: scheduled });
  assert.deepEqual(at(-405n * MS), { fits: true, dispatchBoundNs: scheduled, allowanceNs: 0n });
  assert.deepEqual(at(-405n * MS - 1n), {
    fits: false,
    dispatchBoundNs: scheduled - 1n,
    allowanceNs: -1n,
  });
  assert.equal(at(-255n * MS).allowanceNs, 150n * MS, "entry at the margin start");
  assert.equal(at(-5n * MS - 1n).allowanceNs, 400n * MS - 1n, "one ns before the cutoff");
  assert.equal(at(-5n * MS).allowanceNs, 400n * MS, "at the cutoff: the declared allowance");
  assert.equal(at(300n * MS).allowanceNs, 400n * MS, "never above the declared allowance");
  for (const offset of [-405n, -255n, -5n, 0n, 300n].map((ms) => ms * MS)) {
    const j = at(offset);
    assert(j.dispatchBoundNs + 950n * MS - (scheduled + offset) <= PREWRITE_MAX_WAIT_NS);
  }
});

test.each(["join-budget-unfit", "max-wait-exceeded"])(
  "a %s settle refuses the decision without consuming the receipt",
  async (outcome) => {
    const gate = receivedGate();
    const safety = {
      settleOrdinaryBeforeDecision: async () => ({ outcome, waitedNs: "0" }),
      verifyOuterGraph: () => assert.fail("no graph check after a settle refusal"),
    };
    const now = process.hrtime.bigint();
    await assert.rejects(
      outerPrewriteDecision({
        safety,
        gate,
        readProductGraph: () => [],
        verifyWiring: () => ({}),
        attemptDeadlineNs: now + 60_000n * MS,
        workDeadlineNs: now + 60_000n * MS,
      }),
      (error) => {
        assert.match(error.message, new RegExp(`outside current RSS coverage: ${outcome}`));
        assert.equal(error.outerRefusal.stage, "prewrite-settle");
        assert.equal(error.outerRefusal.coverage, outcome);
        assert.equal(error.outerRefusal.preDecisionSettle.outcome, outcome);
        return true;
      },
    );
    assert.throws(() => gate.consume(() => true), /outer receipt unavailable/);
  },
);

test("the declared maximum pre-write wait is the owner's bound from the cutoff", () => {
  // Gap to the scheduled dispatch (5 ms) + 400 ms allowance + 950 ms settle.
  assert.equal(PREWRITE_MAX_WAIT_NS, 1_355n * MS);
  assert.equal(PREWRITE_FRESH_MARGIN_NS, 250n * MS, "declared margin unchanged");
});

// A wait that still runs past the declared maximum (here a stalled synchronous
// roles append after the joined sample, modelled by a clock jump) refuses. The
// bound itself is admitted; one nanosecond past it is not.
async function joinedWithStall(extraNs, run) {
  let armed = false,
    skew = 0n,
    entryNs;
  const h = harness({
    append: (r) => {
      if (armed && r.type === "fixed-peer-roles") {
        armed = false;
        skew += entryNs + PREWRITE_MAX_WAIT_NS + extraNs - process.hrtime.bigint();
      }
    },
  });
  try {
    await ready(h);
    const base = h.sample.mock.calls.length;
    h.plans.set(base + 1, { ms: R3_HELPER_MS });
    h.plans.set(base + 2, { ms: R3_HELPER_MS });
    const gate = receivedGate(60_000);
    await vi.advanceTimersByTimeAsync(1000 + 995);
    const fake = process.hrtime.bigint;
    vi.spyOn(process.hrtime, "bigint").mockImplementation(() => fake.call(process.hrtime) + skew);
    entryNs = process.hrtime.bigint();
    armed = true;
    return await run(h, gate, entryNs);
  } finally {
    vi.restoreAllMocks();
    await h.done();
  }
}

test.each([
  [0n, "joined-next-dispatch"],
  [1n, "max-wait-exceeded"],
])("a joined wait of the maximum + %i ns settles as %s", async (extraNs, outcome) => {
  const s = await joinedWithStall(extraNs, async (h, _gate, entryNs) => {
    const p = h.monitor.settleOrdinaryBeforeDecision({ deadlineNs: entryNs + 60_000n * MS });
    return (await settled(p, 1, 2000)).value;
  });
  assert.equal(s.outcome, outcome);
  assert.equal(BigInt(s.waitedNs), PREWRITE_MAX_WAIT_NS + extraNs);
});

test("a decision whose wait exceeds the maximum refuses before consuming the receipt", async () => {
  const { error, gate } = await joinedWithStall(1n, async (h, gate) => ({
    ...(await settled(decide(h, gate, { attemptMs: 60_000 }), 1, 2000)),
    gate,
  }));
  assert(error, "must refuse");
  assert.match(error.message, /outside current RSS coverage: max-wait-exceeded/);
  assert.equal(error.outerRefusal.stage, "prewrite-settle");
  assert.equal(error.outerRefusal.coverage, "max-wait-exceeded");
  assert.equal(error.outerDecision, undefined, "no receipt consumed");
  assert.throws(() => gate.consume(() => true), /outer receipt unavailable/);
});

// Review 1 finding 2 (mutant V): executeLocked derives the exit code and the
// next-outer-outcome.json body through these two functions.
test.each([
  ["success", "body", null],
  ["timeout", "body-classified", "timeout"],
])(
  "the run outcome derivation for a %s body is gated by closure and proof",
  async (outcome, status, cls) => {
    const { runOutcome } = await bodyCell(outcome);
    assert.deepEqual(runOutcome(), {
      closureVerified: true,
      proofComplete: true,
      status,
      classification: cls,
      refusals: {},
    });
    for (const [overrides, closed, proof] of [
      [{ stopReport: { absent: false } }, false, true],
      [{ stopFailure: "stop failed" }, false, true],
      [{ unresolved: [{ kind: "helper" }] }, false, true],
      [{ evidenceFailed: true }, true, false],
      [{ population: { ok: false } }, true, false],
      [{ population: undefined }, true, false],
      [{ safetyFailure: { error: "sampler failed" } }, true, false],
    ]) {
      const got = runOutcome(overrides);
      const label = JSON.stringify(overrides);
      assert.equal(got.closureVerified, closed, label);
      assert.equal(got.proofComplete, proof, label);
      assert.equal(got.status, "incomplete", label);
      assert.equal(got.classification, null, label);
    }
  },
);

test("batchExitCode: late identity, controller and fixed-peer success gates", async () => {
  const ok = await bodyCell("success");
  const to = await bodyCell("timeout");
  assert.equal(
    batchExitCode({ accounting: ok.accounting, stopped: ok.stopped, fixedPeer: true }),
    EXIT.ok,
  );
  assert.equal(
    batchExitCode({ accounting: to.accounting, stopped: to.stopped, fixedPeer: true }),
    EXIT.stopped,
  );
  const base = { accounting: ok.accounting, stopped: ok.stopped, fixedPeer: true };
  assert.equal(batchExitCode({ ...base, lateFailure: {} }), EXIT.identityRejected);
  assert.equal(batchExitCode({ ...base, studyFailure: {} }), EXIT.controllerError);
  assert.equal(batchExitCode({ ...base, rendererFailure: {} }), EXIT.controllerError);
  assert.equal(
    batchExitCode({ ...base, lateFailure: {}, studyFailure: {} }),
    EXIT.identityRejected,
    "a late identity failure wins",
  );
  const none = { ...ok.accounting, success: 0 };
  assert.equal(batchExitCode({ ...base, accounting: none }), EXIT.stopped);
  assert.equal(batchExitCode({ ...base, accounting: none, fixedPeer: false }), EXIT.ok);
});

// X8: the attempt deadline still caps a join's dispatch bound below the allowance.
test("a join waits no longer than the attempt deadline", async () => {
  const h = harness();
  try {
    await ready(h);
    const base = h.sample.mock.calls.length;
    h.plans.set(base + 1, { ms: R3_HELPER_MS });
    const gate = receivedGate();
    await vi.advanceTimersByTimeAsync(1000 + 995);
    h.lateTicks.set(h.ticks + 1, 300);
    const { error } = await settled(decide(h, gate, { attemptMs: 100 }), 1, 2000);
    const s = error?.outerRefusal?.preDecisionSettle;
    assert.equal(s?.outcome, "dispatch-late", String(error));
    assert.equal(BigInt(s.allowanceNs), 400n * MS, "the recorded allowance is the budget's");
    assert.equal(BigInt(s.dispatchBoundNs), BigInt(s.enteredNs) + 100n * MS);
    assert.equal(BigInt(s.waitedNs), 100n * MS);
    assert.equal(h.sample.mock.calls.length, base + 1);
  } finally {
    await h.done();
  }
});

// Review 2 correction E (and W8): a join refused inside the margin (the next tick
// later than the reduced allowance) ends as the real terminal status
// prewrite-refused, with the disk audit, the no-write cleanup and the refusal
// counted at the decision stage.
test("a margin join refused for a late tick is a prewrite-refused body cell", async () => {
  const dir = mkdtempSync(join(tmpdir(), "outer-margin-"));
  const h = harness();
  try {
    const path = join(dir, "article-00005.md");
    writeFileSync(path, "v0");
    await ready(h);
    const base = h.sample.mock.calls.length;
    h.plans.set(base + 1, { ms: R3_HELPER_MS });
    h.plans.set(base + 2, { ms: R3_HELPER_MS });
    const gate = receivedGate();
    await vi.advanceTimersByTimeAsync(1000 + FRESH_UNTIL_MS);
    // The reduced allowance at the margin start is 150 ms; the tick is 200 ms late.
    h.lateTicks.set(h.ticks + 1, 200);
    const fs = fakeFs();
    fs.snapshot = () => snapshotPaths({ files: [path] });
    fs.readPeerTarget = readPeerTarget;
    fs.performWrite = () => assert.fail("no write after a pre-write refusal");
    fs.prepareAttempt = (op) => ({
      token: op.id,
      targetPath: "/articles/article-00005/",
      predicate: { kind: "text", selector: ".body", includes: op.id },
      write: { method: "in-place", path, bytes: Buffer.from(op.id) },
      supplemental: [],
    });
    const d = deps({ page: fakePage(), fs, clock: { now: () => process.hrtime.bigint() } });
    // The original Next attempt budget, so only the reduced allowance can expire.
    d.timeouts = { ...d.timeouts, attemptMs: 60000 };
    const outcome = nextOuterOutcomeRecorder({ safety: () => h.monitor });
    let outerDecision;
    Object.assign(
      d,
      nextOuterBatchHooks({
        safety: h.monitor,
        gate: () => gate,
        readProductGraph: () => h.ancestors,
        verifyWiring: () => ({ census: "exact" }),
        operation: "body",
        workDeadlineNs: process.hrtime.bigint() + 7_000_000n * MS,
        signal: new AbortController().signal,
        append: (r) => d.recorder.append(r),
        onDecision: (decision) => (outerDecision = decision),
        onRefusal: outcome.onRefusal,
      }),
      { beforePeerMutation: () => {}, stageBudget: (ms) => ms },
    );
    const op = { id: "body-05", op: "edit-body", article: "article-00005", expect: "classify" };
    const { value, error } = await settled(
      runBatch({ deps: d, scenarios: [{ id: "fn-edit-body", ops: [op] }] }),
      1,
      2000,
    );
    assert.equal(error, undefined, String(error));
    assert.equal(readFileSync(path, "utf8"), "v0");
    const attempt = d.recorder.records.find((r) => r.type === "attempt");
    assert.equal(attempt.outcome, "identity-rejected");
    assert.equal(attempt.outerRefusal.preDecisionSettle.outcome, "dispatch-late");
    assert.equal(attempt.outerRefusal.preDecisionSettle.withinFreshMargin, true);
    assert.equal(BigInt(attempt.outerRefusal.preDecisionSettle.allowanceNs), 150n * MS);
    assert.equal(attempt.outerRefusal.coverage, "cadence-due");
    assert.equal(BigInt(attempt.outerRefusal.preDecisionSettle.waitedNs), 405n * MS);
    assert(d.recorder.records.find((r) => r.type === "fixed-peer-refused-disk-audit").unchanged);
    const got = outcome.outcome({
      stopReport: { absent: true },
      stopFailure: null,
      unresolved: [],
      evidenceFailed: false,
      population: { ok: true },
      operation: "body",
      records: d.recorder.records,
      exitCode: batchExitCode({
        accounting: value.accounting,
        stopped: value.stopped,
        fixedPeer: true,
      }),
      accounting: value.accounting,
      outerDecision,
    });
    assert.equal(got.status, "prewrite-refused");
    assert.deepEqual(got.refusals, {
      "prewrite-settle": 0,
      "prewrite-decision": 1,
      "pre-truncate": 0,
    });
  } finally {
    await h.done();
    rmSync(dir, { recursive: true, force: true });
  }
});

// Review 2 correction C: the recorder owns the counter and reads the sampler
// failure itself; executeLocked passes neither.
test("the outcome recorder reads the sampler failure and counts refusals", async () => {
  const cell = await bodyCell("success");
  let report = { failure: null };
  let safety = null;
  const recorder = nextOuterOutcomeRecorder({ safety: () => safety });
  const call = () =>
    recorder.outcome({
      stopReport: { absent: true },
      stopFailure: null,
      unresolved: [],
      evidenceFailed: false,
      population: { ok: true },
      operation: "body",
      records: cell.d.recorder.records,
      exitCode: cell.exitCode,
      accounting: cell.accounting,
      outerDecision: cell.decision.outerDecision,
    });
  assert.equal(call().status, "body", "a monitor not started yet reports no failure");
  safety = { report: () => report };
  assert.equal(call().proofComplete, true);
  report = { failure: { error: "sampler failed" } };
  assert.deepEqual([call().proofComplete, call().status], [false, "incomplete"]);
  recorder.onRefusal("pre-truncate");
  recorder.onRefusal("pre-truncate");
  recorder.onRefusal("prewrite-settle");
  assert.deepEqual(call().refusals, {
    "prewrite-settle": 1,
    "prewrite-decision": 0,
    "pre-truncate": 2,
  });
});

// Static wiring check (correction C, mutants X2/X3 of review 2): executeLocked
// cannot run here without a live outer observer, so check that its one recorder
// is built from the real monitor and feeds both the hooks and the outcome file.
// This is a source check, weaker than an executeLocked run.
test("executeLocked wires the outcome recorder to the monitor, the hooks and the file", () => {
  const src = readFileSync(fileURLToPath(new URL("../lib/run.mjs", import.meta.url)), "utf8");
  const body = src.slice(src.indexOf("async function executeLocked("));
  assert.equal(body.match(/nextOuterOutcomeRecorder\(/g)?.length, 1);
  assert.match(
    body,
    /const outerOutcome = nextOuterOutcomeRecorder\(\{ safety: \(\) => fixedSafety \}\);/,
  );
  assert.match(body, /nextOuterBatchHooks\(\{[^]*?onRefusal: outerOutcome\.onRefusal,[^]*?\}\)/);
  const call = body.match(/\.\.\.outerOutcome\.outcome\(\{([^]*?)\}\),/)?.[1];
  assert(call, "the outcome file is written through the recorder");
  assert.doesNotMatch(call, /safetyFailure|refusals/);
  assert.doesNotMatch(body, /nextOuterRunOutcome\(/, "no second, unrecorded derivation");
});
