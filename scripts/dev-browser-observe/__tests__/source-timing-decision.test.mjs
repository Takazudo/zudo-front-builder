import assert from "node:assert/strict";
import { afterEach, test, vi } from "vitest";
import { DECISION_SETTLE, startSourceTimingSafety } from "../lib/source-timing-safety.mjs";

afterEach(() => vi.useRealTimers());
const MS = 1000000n;
const ms = (v) => BigInt(Math.round(v * 1e6));
const ORIGIN = 1000000000n;
const root = {
  pid: 51,
  ppid: 50,
  pgid: 51,
  startTicks: "123",
  bootId: "1edafe52-089e-4f16-a478-1143d29052bd",
  birthVersion: "linux-proc-v1",
};
const image = { path: "/controlled/zfb", device: "1", inode: "2" };
function response(ns) {
  const stamp = String(ns);
  return {
    helperResult: "complete",
    verifiedDiscoveries: [],
    journalFailures: [],
    summedRssKiB: 77,
    observedRssKiB: { 51: { VmRSS: 77, "rollup.Rss": 70 } },
    helper: {
      pid: 63,
      birth: { ...root, pid: 63 },
      spawnHrtimeNs: stamp,
      closed: true,
      pipesClosed: true,
      code: 0,
      signal: null,
    },
    processes: [
      {
        ...root,
        disposition: "live",
        rssKiB: 77,
        statusKiB: { VmRSS: 77 },
        smapsRollupKiB: { Rss: 70 },
        executable: image,
      },
    ],
    timing: {
      clock: {
        qualified: true,
        parent: {
          platform: "linux",
          clockId: "CLOCK_MONOTONIC",
          bootId: root.bootId,
          timeNamespace: "time:[1]",
        },
        helper: {
          implementation: "clock_gettime(CLOCK_MONOTONIC)",
          bootId: root.bootId,
          timeNamespace: "time:[1]",
        },
      },
      parent: {
        entryNs: stamp,
        spawnCallNs: stamp,
        spawnReturnNs: stamp,
        exitCode: 0,
        exitSignal: null,
        resultNs: stamp,
        deadlineNs: String(ns + 750000000n),
        exitNs: stamp,
        closeNs: stamp,
        stdinCloseNs: stamp,
        stdoutCloseNs: stamp,
        stderrCloseNs: stamp,
      },
    },
  };
}

// The injected clock and the fake timer queue move together, except that
// `block(ns)` advances only the clock: a busy event loop whose due timers
// have not run yet. That is how a late tick is produced.
function world({ workDeadlineNs } = {}) {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  let ns = ORIGIN;
  const operations = [],
    rows = [],
    aborts = [];
  const server = {
    handle: { native: root, known: new Map([[root.pid, root]]) },
    isAlive: () => ({ alive: true }),
  };
  const monitor = startSourceTimingSafety({
    server,
    executable: image.path,
    image,
    limits: { sampleEveryMs: 1000, rssCeilingKiB: 4194304 },
    clock: () => ns,
    append: (r) => rows.push(r),
    abort: (e) => aborts.push(e),
    workDeadlineNs,
    sample: () => {
      const op = Promise.withResolvers();
      operations.push(op);
      return op.promise;
    },
  });
  return {
    monitor,
    operations,
    rows,
    aborts,
    server,
    now: () => ns,
    block: (delta) => {
      ns += delta;
    },
    advance: async (delta) => {
      ns += delta;
      await vi.advanceTimersByTimeAsync(Number(delta / MS));
    },
    settle: async (mutate = () => {}) => {
      const data = response(ns);
      mutate(data);
      operations.at(-1).resolve(data);
      await vi.advanceTimersByTimeAsync(0);
    },
    flush: () => vi.advanceTimersByTimeAsync(0),
  };
}
// Bring the next tick (scheduled at `sched`) to dispatch `late` after its slot.
async function dispatchLate(w, sched, late) {
  await w.advance(sched - 1n * MS - w.now());
  w.block(1n * MS + late);
  await vi.advanceTimersByTimeAsync(1);
}
const tracked = (p) => {
  const t = { done: false, value: null, error: null };
  t.promise = p.then(
    (v) => ((t.done = true), (t.value = v)),
    (e) => ((t.done = true), (t.error = e)),
  );
  return t;
};

// aa100-csec-body-r1-s1-A records.jsonl: per-epoch lateness and helper time (ms).
const S1A = {
  late: [0, 0.754, 0.405, 1.075, 0.333, 270.763, 7.635],
  helper: [36.893, 31.126, 35.393, 32.276, 31.066, 51.157, 172.715],
  decisionAfterEpoch7Ms: 173.619,
};
async function replayToEpoch7(w) {
  const sched = (k) => ORIGIN + BigInt(k - 1) * 1000n * MS;
  w.block(ms(S1A.helper[0]));
  await w.settle();
  await w.monitor.ready();
  for (let k = 2; k <= 7; k++) {
    // The only queued timer is the sampler's own dispatch: run it at the recorded start.
    w.block(sched(k) + ms(S1A.late[k - 1]) - w.now());
    await vi.advanceTimersToNextTimerAsync();
    if (k === 7) break;
    w.block(ms(S1A.helper[k - 1]));
    await w.settle();
  }
  assert.equal(w.operations.length, 7);
  w.block(sched(7) + ms(S1A.decisionAfterEpoch7Ms) - w.now());
  return sched(7);
}

test("s1-A replay: the legacy gate refuses; the settled decision uses epoch 7", async () => {
  const old = world();
  await replayToEpoch7(old);
  assert.throws(() => old.monitor.current(), /pending, stale, missing or stopped/);
  assert.equal(old.aborts.length, 1);
  vi.useRealTimers();

  const w = world();
  const sched7 = await replayToEpoch7(w);
  const d = tracked(w.monitor.decide({ site: "pre-batch" }));
  await w.flush();
  assert.equal(d.done, false, "must wait for the in-flight epoch 7");
  w.block(sched7 + ms(S1A.late[6] + S1A.helper[6]) - w.now());
  await w.settle();
  await d.promise;
  assert.equal(d.error, null, String(d.error));
  const { decision, epoch, deadlineNs } = d.value;
  assert.equal(epoch, 7);
  assert.equal(decision.settledEpoch, 7);
  assert.equal(decision.fromEpoch, 6);
  assert.equal(decision.pendingAtDecision, true);
  assert.equal(decision.staleAtDecision, true);
  assert.equal(decision.outcome, "settled");
  assert.equal(
    BigInt(decision.waitedNs),
    ms(S1A.late[6] + S1A.helper[6]) - ms(S1A.decisionAfterEpoch7Ms),
  );
  assert.equal(BigInt(decision.decisionNs), w.now());
  assert(BigInt(decision.decisionNs) < BigInt(deadlineNs));
  assert.equal(w.aborts.length, 0);
  assert.equal(w.operations.length, 7, "no extra dispatch");
  await w.monitor.stop();
});

async function phase({ late, helper, phi, legacy = false }) {
  const w = world();
  await w.settle();
  await w.monitor.ready();
  const sched = ORIGIN + 1000n * MS;
  let d;
  const call = () =>
    legacy
      ? tracked(Promise.resolve().then(() => w.monitor.current()))
      : tracked(w.monitor.decide({ site: "sweep" }));
  if (phi < late) {
    await w.advance(sched - 1n * MS - w.now());
    w.block(1n * MS + phi * MS);
    d = call();
    await w.flush();
    w.block((late - phi) * MS);
    await vi.advanceTimersByTimeAsync(1);
    w.block(helper * MS);
    await w.settle();
  } else {
    await dispatchLate(w, sched, late * MS);
    if (phi < late + helper) {
      w.block((phi - late) * MS);
      d = call();
      await w.flush();
      w.block((late + helper - phi) * MS);
      await w.settle();
    } else {
      w.block(helper * MS);
      await w.settle();
      w.block((phi - late - helper) * MS);
      d = call();
      if (!legacy && phi >= 1000n - BigInt(DECISION_SETTLE.freshMarginMs)) {
        // Inside the fresh margin: the decision takes the tick path to the next slot.
        await w.flush();
        assert.equal(d.done, false, `phase ${phi} must not decide inside the fresh margin`);
        w.block(sched + 1000n * MS - w.now());
        await vi.advanceTimersByTimeAsync(1000);
        w.block(helper * MS);
        await w.settle();
      }
    }
  }
  await d.promise;
  const out = {
    ok: !d.error,
    value: d.value,
    error: d.error,
    ops: w.operations.length,
    aborts: w.aborts.length,
    sched,
  };
  if (!legacy) await w.monitor.stop();
  vi.useRealTimers();
  return out;
}

test.each([
  ["idle (s1-A epochs 1-5)", 1n, 35n],
  ["loaded (s1-A epoch 6 lateness, epoch 7 helper)", 271n, 173n],
])(
  "phase sweep %s: every decision phase ends on a fresh accepted sample",
  async (_, late, helper) => {
    let legacyRefusals = 0,
      waited = 0;
    for (let phi = 0n; phi < 1000n; phi += 1n) {
      const r = await phase({ late, helper, phi });
      assert(r.ok, `phase ${phi}: ${r.error}`);
      assert.equal(r.aborts, 0);
      const margin = phi >= 1000n - BigInt(DECISION_SETTLE.freshMarginMs);
      assert.equal(r.ops, margin ? 3 : 2, "only the scheduled sampler dispatches");
      assert.equal(r.value.epoch, margin ? 3 : 2);
      if (margin) assert.equal(r.value.decision.withinFreshMargin, true);
      assert(BigInt(r.value.decisionNs) >= r.sched + (late + helper) * MS);
      assert(BigInt(r.value.decisionNs) < BigInt(r.value.deadlineNs));
      assert.equal(r.value.decision.outcome, phi < late + helper || margin ? "settled" : "fresh");
      if (r.value.decision.outcome === "settled") waited++;
      const old = await phase({ late, helper, phi, legacy: true });
      if (!old.ok) legacyRefusals++;
    }
    assert.equal(waited, Number(late + helper) + DECISION_SETTLE.freshMarginMs);
    assert.equal(
      legacyRefusals,
      Number(late + helper),
      "legacy refuses exactly the pending/stale phases",
    );
  },
);

async function pendingWorld(opts) {
  const w = world(opts);
  await w.settle();
  await w.monitor.ready();
  const sched = ORIGIN + 1000n * MS;
  await dispatchLate(w, sched, 0n);
  return { w, sched };
}

test("refuses a sample that never settles at its core plus closure bound", async () => {
  const { w, sched } = await pendingWorld();
  w.block(10n * MS);
  const d = tracked(w.monitor.decide({ site: "write" }));
  await w.advance(939n * MS);
  assert.equal(d.done, false);
  await w.advance(1n * MS);
  await d.promise;
  assert.match(String(d.error), /never-settles/);
  assert.equal(
    d.error.decision.boundNs,
    String(sched + BigInt(DECISION_SETTLE.coreMs + DECISION_SETTLE.closureMs) * MS),
  );
  assert.equal(w.aborts.length, 1);
  assert.equal(w.operations.length, 2);
});

test("refuses a failed or late sample instead of deciding", async () => {
  for (const mode of ["failure", "late-result"]) {
    const { w } = await pendingWorld();
    const d = tracked(w.monitor.decide({ site: "write" }));
    await w.flush();
    w.block(40n * MS);
    if (mode === "failure") {
      w.operations.at(-1).reject(Error("helper failed"));
      await w.flush();
    } else
      await w.settle((r) => {
        r.timing.parent.resultNs = String(BigInt(r.timing.parent.deadlineNs) + 1n);
      });
    await d.promise;
    assert(d.error, mode);
    assert.notEqual(d.error.decision?.outcome, "settled");
    assert.equal(w.aborts.length, 1);
    vi.useRealTimers();
  }
});

test("refuses when stopped during the wait", async () => {
  const { w } = await pendingWorld();
  const d = tracked(w.monitor.decide({ site: "restore" }));
  await w.flush();
  const stopping = w.monitor.stop();
  w.block(30n * MS);
  await w.settle();
  await d.promise;
  await stopping;
  assert.match(String(d.error), /stopped/);
});

test("a stale decision waits only for a tick inside the declared skew allowance", async () => {
  assert.equal(DECISION_SETTLE.tickSkewMs, 400, "owner-approved allowance (#4878)");
  for (const [late, expect] of [
    [350, null],
    [DECISION_SETTLE.tickSkewMs - 1, null],
    [450, /tick-late/],
  ]) {
    const w = world();
    await w.settle();
    await w.monitor.ready();
    const sched = ORIGIN + 1000n * MS;
    await w.advance(sched - 1n * MS - w.now());
    w.block(1n * MS);
    const d = tracked(w.monitor.decide({ site: "precheck" }));
    await w.flush();
    w.block(BigInt(late) * MS);
    await vi.advanceTimersByTimeAsync(1);
    if (!expect) {
      w.block(30n * MS);
      await w.settle();
    }
    await d.promise;
    if (expect) {
      assert.match(String(d.error), expect);
      assert.equal(
        d.error.decision.boundNs,
        String(sched + BigInt(DECISION_SETTLE.tickSkewMs) * MS),
      );
    } else {
      assert.equal(d.error, null, String(d.error));
      assert.equal(d.value.decision.outcome, "settled");
      assert.equal(d.value.decision.tickStartNs, String(sched + BigInt(late) * MS));
    }
    assert.equal(w.operations.length, 2);
    vi.useRealTimers();
  }
});

test("refuses a settled sample that is already stale, a dead server, and the caller's cap", async () => {
  {
    const w = world();
    await w.settle();
    await w.monitor.ready();
    await dispatchLate(w, ORIGIN + 1000n * MS, 900n * MS);
    const d = tracked(w.monitor.decide({ site: "write" }));
    await w.flush();
    w.block(150n * MS);
    await w.settle();
    await d.promise;
    assert.match(String(d.error), /stale-after-settle/);
    vi.useRealTimers();
  }
  {
    const { w } = await pendingWorld();
    const d = tracked(w.monitor.decide({ site: "write" }));
    await w.flush();
    w.server.isAlive = () => ({ alive: false });
    w.block(30n * MS);
    await w.settle();
    await d.promise;
    assert.match(String(d.error), /server-not-alive/);
    vi.useRealTimers();
  }
  {
    const { w, sched } = await pendingWorld();
    const d = tracked(w.monitor.decide({ site: "write", capNs: sched + 100n * MS }));
    await w.advance(100n * MS);
    await d.promise;
    assert.match(String(d.error), /deadline-before-settle/);
    vi.useRealTimers();
  }
});

// Review-1 finding 1: the write site runs synchronous work between the decision and
// its T0 current(). A decision must leave at least that much coverage.
const LOADED_PRE_T0_MAX_NS = ms(5.5); // measured: 24 busy loops on 12 cores, max 5.5 ms
async function freshWorld() {
  const w = world();
  await w.settle();
  await w.monitor.ready();
  return { w, deadline: ORIGIN + 1000n * MS };
}

test("write-site probe: a decision with too little coverage left takes the tick path, so T0 never goes stale", async () => {
  assert.equal(DECISION_SETTLE.freshMarginMs, 25);
  const { w, deadline } = await freshWorld();
  await w.advance(deadline - 1n * MS - w.now());
  w.block(1n * MS - ms(0.1));
  const d = tracked(w.monitor.awaitDecision({ site: "write" }));
  await w.flush();
  assert.equal(d.done, false, "0.1 ms left must not count as fresh");
  w.block(ms(0.1));
  await vi.advanceTimersByTimeAsync(1);
  w.block(30n * MS);
  await w.settle();
  await d.promise;
  assert.equal(d.error, null, String(d.error));
  assert.equal(d.value.outcome, "settled");
  assert.equal(d.value.withinFreshMargin, true);
  w.block(LOADED_PRE_T0_MAX_NS);
  const t0 = w.monitor.current();
  assert.equal(t0.epoch, 2);
  assert.equal(w.aborts.length, 0);
  await w.monitor.stop();
});

test("near-deadline write sweep: with the loaded worst-case pre-T0 work, no T0 refuses", async () => {
  let viaTick = 0;
  for (let left = 0n; left <= 40n * MS; left += ms(0.5)) {
    const { w, deadline } = await freshWorld();
    await w.advance(deadline - 50n * MS - w.now());
    w.block(50n * MS - left);
    const d = tracked(w.monitor.awaitDecision({ site: "write" }));
    await w.flush();
    if (!d.done) {
      viaTick++;
      w.block(deadline - w.now());
      await vi.advanceTimersByTimeAsync(100);
      w.block(30n * MS);
      await w.settle();
    }
    await d.promise;
    assert.equal(d.error, null, `left ${left}: ${d.error}`);
    w.block(LOADED_PRE_T0_MAX_NS);
    assert.doesNotThrow(() => w.monitor.current(), `left ${left} ns`);
    assert.equal(w.aborts.length, 0);
    await w.monitor.stop();
    vi.useRealTimers();
  }
  assert.equal(viaTick, 51, "every left <= 25 ms takes the tick path");
});

test("an abort ends a settle wait at once, and a failure wakes a tick wait at once", async () => {
  {
    const { w } = await pendingWorld();
    const ac = new AbortController();
    const d = tracked(w.monitor.awaitDecision({ site: "write", signal: ac.signal }));
    await w.flush();
    ac.abort(Error("session stop"));
    await w.flush();
    assert.equal(d.done, true, "abort must not wait for the 950 ms bound");
    assert.match(String(d.error), /session stop/);
    vi.useRealTimers();
  }
  {
    const { w, deadline } = await freshWorld();
    await w.advance(deadline - 1n * MS - w.now());
    w.block(2n * MS);
    const d = tracked(w.monitor.awaitDecision({ site: "precheck" }));
    await w.flush();
    assert.equal(d.done, false);
    assert.throws(() => w.monitor.current(), /stale/);
    await w.flush();
    assert.equal(d.done, true, "the failure must wake the tick wait, not the 400 ms bound");
    assert.match(String(d.error), /stale/);
    vi.useRealTimers();
  }
});

test("a sample that settles inside the fresh margin of its own deadline refuses", async () => {
  const w = world();
  await w.settle();
  await w.monitor.ready();
  await dispatchLate(w, ORIGIN + 1000n * MS, 900n * MS);
  const d = tracked(w.monitor.decide({ site: "write" }));
  await w.flush();
  w.block(90n * MS);
  await w.settle();
  await d.promise;
  assert.match(String(d.error), /stale-after-settle/);
});
