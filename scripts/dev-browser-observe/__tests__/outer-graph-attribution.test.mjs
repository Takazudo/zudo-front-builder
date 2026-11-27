import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { test, vi } from "vitest";
import * as peerRoles from "../lib/fixed-peer-roles.mjs";
import { attributeGraphRefusal, startFixedRssSafety } from "../lib/fixed-rss-safety.mjs";
import { linuxBirth, sameLinuxBirth } from "../lib/linux-birth.mjs";
import {
  describeGraphChild,
  exactChildren,
  nextOuterChildren,
} from "../lib/next-outer-provider.mjs";

// The ninth ordinary RSS helper recorded by the sealed #4870 R3 slot
// (independent-terminal/arm-overlap.json): arm 454624551510535 fell inside its life.
const R3_HELPER = {
  pid: 2358708,
  pgid: 2358325,
  ppid: 2358372,
  bootId: "1edafe52-089e-4f16-a478-1143d29052bd",
  startTicks: "45462448",
  birthVersion: "linux-proc-v1",
  state: "R",
  comm: "python3",
};
const R3_EXPECTED = [2358379, 2358437, 2358559, 2358623];
const r3GraphError = (extra) => {
  const error = new Error("unexpected outer graph child");
  error.graphRefusal = {
    parent: { pid: 2358372, startTicks: "1", bootId: R3_HELPER.bootId },
    expected: R3_EXPECTED,
    actual: [...R3_EXPECTED, ...extra.map((c) => c.pid)].sort((a, b) => a - b),
    extra,
    missing: [],
  };
  return error;
};
const child = (birth) => ({
  pid: birth?.pid ?? 4194301,
  observedNs: "1",
  birth,
  birthError: birth ? null : "absent",
});

const sleepUntil = async (cond, ms = 5000) => {
  const end = Date.now() + ms;
  while (!cond()) {
    assert(Date.now() < end, "condition not reached");
    await new Promise((r) => setTimeout(r, 20));
  }
};

test("census refusal carries each extra child's full birth, exe and argv at refusal time", async () => {
  const parent = spawn("bash", ["-c", "sleep 30 & sleep 30 & wait"], {
    stdio: "ignore",
    detached: true,
  });
  try {
    let kids = [];
    await sleepUntil(() => (kids = nextOuterChildren(parent.pid)).length === 2);
    const prior = linuxBirth(parent.pid);
    const [a, b] = kids.map((pid) => linuxBirth(pid));
    exactChildren(prior, [a, b], { tasks: 0, bytes: 0 });
    assert.throws(
      () => exactChildren(prior, [a], { tasks: 0, bytes: 0 }),
      (error) => {
        assert.match(error.message, /unexpected outer graph child/);
        const row = error.graphRefusal;
        assert.deepEqual(row.expected, [a.pid]);
        assert.deepEqual(row.actual, kids);
        assert.equal(row.extra.length, 1);
        assert(sameLinuxBirth(row.extra[0].birth, b));
        assert.equal(row.extra[0].birth.ppid, parent.pid);
        assert.match(row.extra[0].exe, /\/sleep$/);
        assert.deepEqual(row.extra[0].argv, ["sleep", "30"]);
        assert(/^\d+$/.test(row.extra[0].observedNs));
        assert.equal(row.parent.startTicks, prior.startTicks);
        return true;
      },
    );
    assert.throws(
      () => exactChildren(prior, [], { tasks: 0, bytes: 0 }),
      (error) =>
        error.graphRefusal.extra.length === 2 && error.graphRefusal.extra.every((c) => c.birth),
    );
  } finally {
    process.kill(-parent.pid, "SIGKILL");
  }
});

test("a child that has already gone is described without a birth", async () => {
  const gone = spawn("true");
  await new Promise((r) => gone.once("close", r));
  const row = describeGraphChild(gone.pid);
  assert.equal(row.birth, null);
  assert.equal(row.birthError, "absent");
});

test("attribution uses the full recorded helper birth and never admits", () => {
  const helpers = { pending: null, lastAccepted: R3_HELPER };
  const out = attributeGraphRefusal(
    r3GraphError([
      child({ ...R3_HELPER }),
      child({ ...R3_HELPER, startTicks: "45462449" }),
      child({ ...R3_HELPER, pid: 2358709, startTicks: "45462450" }),
      child(null),
    ]).graphRefusal,
    helpers,
  );
  assert.deepEqual(
    out.extra.map((c) => c.attribution),
    ["closed-ordinary-helper", "helper-pid-different-birth", null, "no-birth"],
  );
  assert.equal(
    attributeGraphRefusal(r3GraphError([child({ ...R3_HELPER })]).graphRefusal, {
      pending: R3_HELPER,
      lastAccepted: null,
    }).extra[0].attribution,
    "pending-ordinary-helper",
  );
  assert.equal(
    attributeGraphRefusal(
      r3GraphError([child({ ...R3_HELPER, bootId: "other-boot" })]).graphRefusal,
      helpers,
    ).extra[0].attribution,
    "helper-pid-different-birth",
  );
});

function harness() {
  vi.useFakeTimers({
    toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "hrtime"],
  });
  const worker = { pid: 13 },
    ancestors = [{ pid: 10 }, { pid: 11 }, { pid: 12 }];
  const known = new Map([...ancestors, worker].map((p) => [p.pid, p]));
  const owner = { qualified: worker, isWorker: (p) => p.pid === 13 };
  const verify = vi.spyOn(peerRoles, "peerProcessIdentity").mockReturnValue([]);
  const ctl = { hold: false, deferred: Promise.withResolvers() };
  const sample = vi.fn(async () => {
    if (ctl.hold) await ctl.deferred.promise;
    const ns = process.hrtime.bigint();
    return {
      summedRssKiB: 4,
      processes: [
        { disposition: "live", pid: 10, statusKiB: { VmRSS: 4 }, smapsRollupKiB: { Rss: 4 } },
      ],
      helper: { spawnHrtimeNs: String(ns), closed: true, pipesClosed: true },
      timing: {
        clock: { qualified: true },
        parent: { entryNs: String(ns), resultNs: String(ns), deadlineNs: String(ns + 750000000n) },
      },
    };
  });
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
    ancestors,
    ctl,
    async done() {
      ctl.deferred.resolve();
      await monitor.stop();
      verify.mockRestore();
      vi.useRealTimers();
    },
  };
}

test("R3 replay: an arm inside the ninth helper's life refuses on coverage, before any census", async () => {
  const h = harness();
  try {
    const ready = h.monitor.verifyReady(process.hrtime.bigint());
    await vi.advanceTimersByTimeAsync(2000);
    await ready;
    const accepted = h.monitor.verifyOuterGraph(
      () => h.ancestors,
      () => {},
    );
    h.ctl.hold = true;
    await vi.advanceTimersByTimeAsync(1000);
    const active = h.sample.mock.calls.at(-1)[2].outerCycle;
    active.helperBirth = { ...R3_HELPER };
    // R3 arm: 66 ms after the ninth spawn, 70.881 ms after the previous cadence cutoff.
    await vi.advanceTimersByTimeAsync(66);
    const census = vi.fn(() => {
      throw r3GraphError([child({ ...R3_HELPER })]);
    });
    const product = vi.fn(() => h.ancestors);
    // Order in the sealed R3 source (360c59fdc): observer census, then coverage.
    assert.throws(
      () => {
        census();
        h.monitor.verifyOuterGraph(product);
      },
      (error) => error.message === "unexpected outer graph child" && !error.outerRefusal,
    );
    assert.equal(product.mock.calls.length, 0);
    census.mockClear();
    assert.throws(
      () => h.monitor.verifyOuterGraph(product, census, "prewrite-decision"),
      (error) => {
        const row = error.outerRefusal;
        assert.equal(row.coverage, "ordinary-sample-pending");
        assert.equal(row.productGraph, "not-checked");
        assert.equal(row.observerGraph, "not-checked");
        assert.equal(row.observerGraphDetail, null);
        assert.deepEqual(row.pending.helperBirth, R3_HELPER);
        assert.equal(row.lastAccepted.cadenceDeadlineNs, accepted.cadenceDeadlineNs);
        assert(BigInt(row.entryNs) >= BigInt(accepted.cadenceDeadlineNs));
        return true;
      },
    );
    assert.equal(census.mock.calls.length, 0);
    assert.equal(product.mock.calls.length, 0);
  } finally {
    await h.done();
  }
});

test("inside coverage every extra controller child refuses with its attribution recorded", async () => {
  const h = harness();
  try {
    const ready = h.monitor.verifyReady(process.hrtime.bigint());
    await vi.advanceTimersByTimeAsync(2000);
    await ready;
    h.ctl.hold = true;
    await vi.advanceTimersByTimeAsync(1000);
    h.sample.mock.calls.at(-1)[2].outerCycle.helperBirth = { ...R3_HELPER };
    h.ctl.hold = false;
    h.ctl.deferred.resolve();
    await vi.advanceTimersByTimeAsync(1);
    const cases = [
      [[child({ ...R3_HELPER })], ["closed-ordinary-helper"]],
      [[child({ ...R3_HELPER, startTicks: "45462449" })], ["helper-pid-different-birth"]],
      [
        [child({ ...R3_HELPER }), child({ ...R3_HELPER, pid: 2358790, startTicks: "45462500" })],
        ["closed-ordinary-helper", null],
      ],
      [[child(null)], ["no-birth"]],
    ];
    for (const [extra, expected] of cases)
      assert.throws(
        () =>
          h.monitor.verifyOuterGraph(
            () => h.ancestors,
            () => {
              throw r3GraphError(extra);
            },
            "pre-truncate",
          ),
        (error) => {
          const row = error.outerRefusal;
          assert.equal(row.coverage, "accepted");
          assert.equal(row.productGraph, "accepted");
          assert.equal(row.observerGraph, "failed");
          assert.deepEqual(row.lastAccepted.helperBirth, R3_HELPER);
          assert.deepEqual(
            row.observerGraphDetail.extra.map((c) => c.attribution),
            expected,
          );
          return true;
        },
      );
    const ok = h.monitor.verifyOuterGraph(
      () => h.ancestors,
      () => {},
    );
    assert.deepEqual(Object.keys(ok).sort(), ["cadenceDeadlineNs", "epoch"]);
  } finally {
    await h.done();
  }
});
