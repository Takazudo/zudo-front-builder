import assert from "node:assert/strict";
import { afterEach, test, vi } from "vitest";
import { startSourceTimingSafety } from "../lib/source-timing-safety.mjs";

afterEach(() => vi.useRealTimers());
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
  const stamp = String(ns),
    v = 77;
  return {
    helperResult: "complete",
    verifiedDiscoveries: [],
    journalFailures: [],
    summedRssKiB: v,
    observedRssKiB: { 51: { VmRSS: v, "rollup.Rss": 70 } },
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
        rssKiB: v,
        statusKiB: { VmRSS: v },
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
function setup() {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  let ns = 1000000000n;
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
    sample: () => {
      const op = Promise.withResolvers();
      operations.push(op);
      return op.promise;
    },
  });
  return {
    monitor,
    tick: (elapsed) => {
      ns += elapsed;
    },
    operations,
    rows,
    aborts,
    server,
    resolve: (mutate = () => {}) => {
      const data = response(ns);
      mutate(data);
      operations.at(-1).resolve(data);
    },
    advance: async (ms) => {
      ns += BigInt(ms) * 1000000n;
      await vi.advanceTimersByTimeAsync(ms);
    },
  };
}
test.each(["unknown-metric", "missing-known-birth", "late", "not-closed", "wrong-exe", "over-cap"])(
  "complete source timing refuses %s",
  async (mode) => {
    const x = setup();
    if (mode === "missing-known-birth") x.server.handle.known.set(52, { ...root, pid: 52 });
    x.resolve((r) => {
      if (mode === "unknown-metric") delete r.processes[0].statusKiB.VmRSS;
      if (mode === "late")
        r.timing.parent.resultNs = String(BigInt(r.timing.parent.deadlineNs) + 1n);
      if (mode === "not-closed") r.helper.pipesClosed = false;
      if (mode === "wrong-exe") r.processes[0].executable = { ...image, inode: "9" };
      if (mode === "over-cap") {
        r.processes[0].rssKiB = r.processes[0].statusKiB.VmRSS = r.summedRssKiB = 4194305;
      }
    });
    await assert.rejects(x.monitor.ready());
    assert.throws(() => x.monitor.current());
    await x.monitor.stop();
    assert.equal(x.aborts.length, 1);
    assert.equal(x.monitor.report().accepted, 0);
    if (mode === "not-closed") assert.equal(x.monitor.unresolvedHelper.pid, 63);
  },
);
test("missed cadence or a pending cycle stops before another helper; original settlement retains first failure", async () => {
  const x = setup();
  x.resolve();
  await x.monitor.ready();
  assert.equal(x.monitor.current().summedRssKiB, 77);
  await x.advance(1000);
  assert.equal(x.operations.length, 2);
  await x.advance(1000);
  assert.match(x.aborts[0].message, /overlap/);
  assert.equal(x.operations.length, 2);
  let stopped = false;
  const stop = x.monitor.stop().then(() => {
    stopped = true;
  });
  await Promise.resolve();
  assert.equal(stopped, false);
  const partial = {
    helper: { pid: 90, closed: false, pipesClosed: false },
    observedRssKiB: { 51: { VmRSS: 1234 } },
  };
  x.operations[1].reject(Object.assign(Error("partial late result"), { details: partial }));
  await stop;
  assert.equal(x.monitor.unresolvedHelper, partial.helper);
  assert.equal(x.monitor.report().peakFailedObservedSubtotalKiB, 1234);
  assert.match(x.monitor.report().failure.error, /overlap/);
  assert.match(x.monitor.report().secondary.error, /partial/);
});
test("source timing refuses elapsed nominal opportunity independently of partial numeric evidence", async () => {
  const x = setup();
  x.resolve();
  await x.monitor.ready();
  await x.advance(2000);
  assert.match(x.aborts[0].message, /missed a dispatch/);
  assert.equal(x.operations.length, 1);
  await x.monitor.stop();
});

test("liveness work crossing the cadence refuses the write stamp", async () => {
  const x = setup();
  x.resolve();
  await x.monitor.ready();
  x.server.isAlive = () => {
    x.tick(1000000000n);
    return { alive: true };
  };
  assert.throws(() => x.monitor.current(), /stale/);
  await x.monitor.stop();
});
