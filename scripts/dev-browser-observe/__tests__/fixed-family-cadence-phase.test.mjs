import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { afterEach, test, vi } from "vitest";
import {
  FAMILY_IDLE_ADMISSION_RESERVE_NS,
  blankBrowserAudit,
  runOriginalFamilyBatch,
} from "../lib/fixed-family-admission.mjs";
import { startFixedRssSafety } from "../lib/fixed-rss-safety.mjs";
import { deps, fakeFs, fakePage } from "./fakes.mjs";

// Timing of the #4862 rapid slot-01 refusal (cclogs 4862-rapid-recovery-slot01-failure):
// proof sample epoch 5 started at T, blank setup captured at T+877.3 ms, and the
// synchronous journal/population work ran 527.1 ms before the cadence refusal.
const T = 464240583114825n;
const SETUP_PHASE_NS = 877_311_406n;
const ADMISSION_COST_NS = 527_062_550n;
const SETTLE_MS = 53;
const MS = 1_000_000n;

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

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

// Virtual monotonic clock: fake timer time plus synchronous work that blocks
// the event loop (it advances hrtime without letting any timer fire).
function harness(mode) {
  vi.useFakeTimers({
    toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"],
  });
  const d0 = Date.now();
  let blockedNs = 0n;
  vi.spyOn(process.hrtime, "bigint").mockImplementation(
    () => T - 4000n * MS + BigInt(Date.now() - d0) * MS + blockedNs,
  );
  const block = (ns) => (blockedNs += ns);
  let terminal = { epoch: 1 };
  let dispatches = 0;
  const failures = [];
  const hooks = { onDispatch: () => {} };
  const monitor = startFixedRssSafety({
    joinOwnedFamily: true,
    server: { handle: { native: { pid: 3 } } },
    fixedExecutables: { protocol: "linux-fixed-zfb.v4", root: { path: "/node" } },
    limits: { rssCeilingKiB: 4194304, sampleEveryMs: 1000 },
    append() {},
    abort: (error) => failures.push(error),
    custody: {
      familyProof: () => ({ root, dev, worker, terminal, release: { receipt: { epoch: 1 } } }),
    },
    sample(_server, _path, { onHelperSpawn }) {
      dispatches++;
      const plan = hooks.onDispatch(dispatches) ?? {};
      const done = Promise.withResolvers();
      const spawn = () => {
        const now = process.hrtime.bigint();
        const helper = {
          pid: 100 + dispatches,
          entryNs: String(now),
          spawnHrtimeNs: String(now),
          deadlineNs: String(now + 750n * MS),
        };
        onHelperSpawn(helper);
        const settle = () => {
          if (plan.fail) return done.reject(Error("next sample failed"));
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
          });
        };
        if (plan.settleMs === 0) queueMicrotask(settle);
        else setTimeout(settle, plan.settleMs ?? SETTLE_MS);
      };
      if (plan.failBeforeSpawn) done.reject(Error("next sample failed before its helper"));
      else if (plan.spawnDelayMs) setTimeout(spawn, plan.spawnDelayMs);
      else spawn();
      return done.promise;
    },
  });
  return {
    monitor,
    block,
    hooks,
    failures,
    get dispatches() {
      return dispatches;
    },
    setTerminal: (value) => (terminal = value),
    mode,
  };
}

function batch(h, { deadlineNs, snapshot = () => [root, dev], cost = ADMISSION_COST_NS } = {}) {
  const audit = blankBrowserAudit(),
    ctx = new EventEmitter();
  const page0 = Object.assign(new EventEmitter(), { url: () => "about:blank" });
  ctx.pages = () => [];
  audit.attach(ctx);
  ctx.pages = () => [page0];
  const page = fakePage();
  const input = {
    scenarios: [{ id: "family-admission-only", ops: [] }],
    deps: deps({ page, fs: audit.trackFixture(fakeFs()) }),
  };
  const rows = [];
  const run = () =>
    runOriginalFamilyBatch(input, {
      strategy: "blank-join-owned.v2",
      safety: h.monitor,
      browserAudit: audit,
      snapshot,
      known: new Map([root, dev, worker].map((x) => [x.pid, x])),
      verifyImages() {},
      deadlineNs: deadlineNs ?? process.hrtime.bigint() + 7_000_000_000n * 1000n,
      signal: new AbortController().signal,
      append: (r) => {
        rows.push(structuredClone(r));
        if (r.status === "pre-batch-proof") h.block(cost);
      },
    });
  return { run, rows, input, page, page0 };
}

// Run the cadence until epoch 5 (sample at T) has been accepted, then sit at T+phase.
async function atProofPhase(h, phaseNs) {
  await vi.advanceTimersByTimeAsync(4000 + SETTLE_MS + 1);
  const now = process.hrtime.bigint();
  assert.equal(now, T + BigInt(SETTLE_MS + 1) * MS);
  const target = T + phaseNs;
  const wholeMs = Number((target - now) / MS);
  await vi.advanceTimersByTimeAsync(wholeMs);
  h.block(target - process.hrtime.bigint());
  assert.equal(process.hrtime.bigint(), T + phaseNs);
}

async function stopMonitor(h) {
  const stopped = h.monitor.stop();
  await vi.advanceTimersByTimeAsync(2000);
  await stopped;
}

async function settleAll(promise) {
  let outcome;
  promise.then(
    (value) => (outcome = { value }),
    (error) => (outcome = { error }),
  );
  for (let i = 0; i < 40 && !outcome; i++) await vi.advanceTimersByTimeAsync(50);
  assert(outcome, "family batch did not settle within the bounded wait");
  return outcome;
}

test("the recorded slot-01 phase joins the next original cadence sample instead of refusing", async () => {
  const h = harness("replay");
  try {
    await atProofPhase(h, SETUP_PHASE_NS);
    const b = batch(h);
    const result = b.run();
    assert(result instanceof Promise, "the late idle phase must defer, not decide synchronously");
    const dispatchesBefore = h.dispatches;
    const { value, error } = await settleAll(result);
    assert.equal(error, undefined, String(error));
    assert.equal(h.dispatches, dispatchesBefore + 1, "exactly the next ordinary dispatch");
    const start = b.input.deps.recorder.records.find((r) => r.type === "scenario-start");
    const setup = start.familyAdmission.setup;
    assert.equal(setup.settlementOutcome, "accepted");
    assert.equal(setup.idleDeferral.outcome, "joined");
    assert.equal(setup.idleDeferral.reserveNs, String(FAMILY_IDLE_ADMISSION_RESERVE_NS));
    assert.equal(setup.idleDeferral.remainingCadenceNs, String(1000n * MS - SETUP_PHASE_NS));
    assert.equal(setup.idleDeferral.previousEpoch, 5);
    assert.equal(setup.cycle.epoch, 6);
    assert(BigInt(setup.idleDeferral.addedWaitNs) > 0n);
    assert(BigInt(setup.addedSettlementNs) > 0n);
    assert(BigInt(start.familyAdmission.decisionNs) < BigInt(setup.cycle.startedNs) + 1000n * MS);
    assert.equal(b.rows.filter((r) => r.status === "pre-batch-proof").length, 1);
    assert.equal(start.familyAdmission.httpRequests, 0);
    assert.equal(value.accounting.planned, 0);
    assert.deepEqual(h.failures, []);
  } finally {
    await stopMonitor(h);
  }
});

// With the recorded 527.1 ms admission cost, every readiness phase must reach
// the same verdict. A shorter reserve admits idle at 600 ms and then refuses.
test.each([20, 54, 300, 473, 600, 877.311406, 999])(
  "readiness at T+%s ms admits with the recorded admission cost",
  async (phaseMs) => {
    const h = harness("phase");
    try {
      const phaseNs = BigInt(Math.round(phaseMs * 1e6));
      if (phaseNs < BigInt(SETTLE_MS) * MS) {
        await vi.advanceTimersByTimeAsync(4000);
        await vi.advanceTimersByTimeAsync(phaseMs);
      } else await atProofPhase(h, phaseNs);
      const b = batch(h);
      const { error } = await settleAll(Promise.resolve().then(b.run));
      assert.equal(error, undefined, String(error));
      const start = b.input.deps.recorder.records.find((r) => r.type === "scenario-start");
      const { setup, decisionNs } = start.familyAdmission;
      assert(BigInt(decisionNs) < BigInt(setup.cycle?.startedNs ?? T) + 1000n * MS);
      assert.equal(
        setup.settlementOutcome,
        "accepted",
        "every phase decides on a joined cadence sample",
      );
    } finally {
      await stopMonitor(h);
    }
  },
);

test("reserve equality admits on the idle sample without waiting", async () => {
  const h = harness("equality");
  try {
    await vi.advanceTimersByTimeAsync(4000 + SETTLE_MS + 1);
    // The next sample settles in the same instant it starts, so the idle capture
    // sees exactly one full cadence period remaining.
    h.hooks.onDispatch = () => ({ settleMs: 0 });
    await vi.advanceTimersByTimeAsync(1000 - SETTLE_MS - 1);
    const resolveAt = process.hrtime.bigint();
    assert.equal(resolveAt, T + 1000n * MS);
    const b = batch(h, { cost: 50n * MS });
    const result = b.run();
    assert(
      b.rows.some((r) => r.status === "pre-batch-proof"),
      "reserve equality must journal the idle proof synchronously, not defer",
    );
    const awaited = await result;
    const setup = b.input.deps.recorder.records.find((r) => r.type === "scenario-start")
      .familyAdmission.setup;
    assert.equal(BigInt(setup.capturedNs), resolveAt);
    assert.equal(setup.settlementOutcome, "no-pending");
    assert.equal(setup.idleDeferral, undefined);
    assert.equal(setup.addedSettlementNs, "0");
    assert.equal(awaited.accounting.planned, 0);
  } finally {
    await stopMonitor(h);
  }
});

test("one nanosecond short of the reserve defers to the next cadence sample", async () => {
  const h = harness("reserve-minus-one");
  try {
    await vi.advanceTimersByTimeAsync(4000 + SETTLE_MS + 1);
    h.hooks.onDispatch = (n) => (n === 6 ? { settleMs: 0 } : {});
    await vi.advanceTimersByTimeAsync(1000 - SETTLE_MS - 1);
    h.block(1n);
    const b = batch(h, { cost: 50n * MS });
    const result = b.run();
    assert(result instanceof Promise);
    const { error } = await settleAll(result);
    assert.equal(error, undefined, String(error));
    const setup = b.input.deps.recorder.records.find((r) => r.type === "scenario-start")
      .familyAdmission.setup;
    assert.equal(
      setup.idleDeferral.remainingCadenceNs,
      String(FAMILY_IDLE_ADMISSION_RESERVE_NS - 1n),
    );
    assert.equal(setup.idleDeferral.outcome, "joined");
    assert.equal(setup.cycle.epoch, 7);
  } finally {
    await stopMonitor(h);
  }
});

test.each([
  "next-fails-before-helper",
  "next-fails-after-helper",
  "next-dispatch-late",
  "next-result-late",
  "custody-changed",
  "population-changed",
  "work-deadline",
  "stopped",
  "navigated",
])("a deferred idle admission refuses when %s", async (mode) => {
  const h = harness(mode);
  try {
    await atProofPhase(h, SETUP_PHASE_NS);
    let changed = false;
    h.hooks.onDispatch = () => {
      changed = true;
      if (mode === "custody-changed") h.setTerminal({ epoch: 2 });
      return {
        "next-fails-before-helper": { failBeforeSpawn: true },
        "next-fails-after-helper": { fail: true },
        "next-dispatch-late": { spawnDelayMs: 900 },
        "next-result-late": { settleMs: 800 },
      }[mode];
    };
    const b = batch(h, {
      ...(mode === "work-deadline" && { deadlineNs: process.hrtime.bigint() + 60n * MS }),
      snapshot: () =>
        mode === "population-changed" && changed ? [root, dev, identity(9, 4, 9)] : [root, dev],
    });
    const result = b.run();
    assert(result instanceof Promise);
    if (mode === "stopped") await h.monitor.stop();
    if (mode === "navigated") b.page0.emit("framenavigated");
    const { error } = await settleAll(result);
    assert(error, "deferred admission must refuse");
    const refusal = b.rows.at(-1);
    assert.equal(refusal.admitted, false);
    assert.equal(refusal.setup.settlementOutcome, "refused");
    assert.equal(
      refusal.setup.idleDeferral.outcome,
      ["next-fails-after-helper", "next-result-late"].includes(mode) ? "joined" : "refused",
    );
    assert.equal(refusal.setup.idleDeferral.reserveNs, String(FAMILY_IDLE_ADMISSION_RESERVE_NS));
    assert.match(
      String(error.message),
      {
        "next-fails-before-helper": /unavailable/,
        "next-fails-after-helper": /observation missing, failed/,
        "next-dispatch-late": /next cadence dispatch expired/,
        "next-result-late": /family settlement deadline expired/,
        "custody-changed": /custody changed during deferral/,
        "population-changed": /population changed/,
        "work-deadline": /next cadence dispatch expired/,
        stopped: /unavailable/,
        navigated: /left blank setup/,
      }[mode],
    );
    assert(!b.input.deps.recorder.records.some((r) => r.type === "scenario-start"));
    assert.equal(b.page.prepared.length, 0);
    if (mode === "work-deadline")
      assert.equal(refusal.setup.idleDeferral.waitBoundNs, refusal.setup.workDeadlineNs);
  } finally {
    await stopMonitor(h);
  }
});
