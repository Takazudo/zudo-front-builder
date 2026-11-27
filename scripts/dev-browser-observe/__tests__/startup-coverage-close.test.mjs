import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { test } from "vitest";
import { registerStartupBirth } from "../lib/fixed-rss-custody.mjs";
import { sampleRendererMemory } from "../lib/linux-rss-sample.mjs";

// Complete the real sample parser's protocol with in-memory pipes. Deliver
// the newly owned process after every output frame, before ChildProcess.close.
test("a late scout birth preserves the subtotal and invalidates a completed helper frame", async () => {
  const base = {
    birthVersion: "linux-proc-v1",
    bootId: "11111111-2222-3333-4444-555555555555",
    startTicks: "42",
    state: "S",
    comm: "node",
  };
  const root = { ...base, pid: 1001, ppid: process.pid, pgid: 1001, argv: ["/shim", "dev"] };
  const scout = { ...base, pid: 1003, ppid: 1002, pgid: 1003 };
  const image = { path: "/node", inode: "123", device: "456" };
  const child = Object.assign(new EventEmitter(), {
    pid: 1004,
    stdin: new EventEmitter(),
    stdout: new EventEmitter(),
    stderr: new EventEmitter(),
    kill() {},
    unref() {},
  });
  child.stdin.end = () => {};
  const handle = { native: root, known: new Map([[root.pid, root]]), onNewOwned() {} };
  const custody = {
    begin: () => ({ session: "fixed", epoch: 1, tokens: {} }),
    selectStartupCycle: () => {
      throw Error("incomplete sample entered selector");
    },
  };
  const result = sampleRendererMemory({ handle, isAlive: () => ({ alive: true }) }, image.path, {
    launch: () => child,
    fixedExecutables: { protocol: "linux-fixed-zfb.v4", root: image, child: image, launch: {} },
    custody,
  });
  const n = String(process.hrtime.bigint());
  let seq = 0;
  const timing = (stage, count, extra = {}) => ({
    type: "timing",
    version: "linux-rss-timing.v1",
    seq: seq++,
    stage,
    ns: Array(count).fill(n),
    ...extra,
  });
  const rows = [
    timing("request", 4, {
      clock: {
        implementation: "clock_gettime(CLOCK_MONOTONIC)",
        bootId: base.bootId,
        timeNamespace: null,
      },
    }),
    { type: "helper", birth: { ...base, pid: child.pid, ppid: process.pid, pgid: child.pid } },
    { type: "executable", pathInput: image.path, executable: image },
    { type: "bound", birth: root, binding: { pidfdFlags: 0, fdinfoPid: root.pid, verifiedNs: n } },
    { type: "rss", source: "VmRSS", birth: root, rssKiB: 123, observedNs: n },
    { type: "rss", source: "rollup.Rss", birth: root, rssKiB: 120, observedNs: n },
    {
      type: "observation",
      process: {
        ...root,
        disposition: "live",
        statusKiB: { VmRSS: 123 },
        smapsRollupKiB: { Rss: 120 },
        threads: { listed: 1, rendererHosts: [] },
        startedNs: n,
        endNs: n,
        rssKiB: 123,
        executable: image,
      },
    },
    timing("observationOutput", 4, { pid: root.pid }),
    timing("finalRootBegin", 1),
    timing("finalRoot", 2),
    timing("cleanupBegin", 1),
    timing("fdCleanup", 2),
    { type: "complete", readCalls: 1, procInputBytes: 64, helperMaxRssKiB: 100 },
    timing("terminalOutput", 6),
    timing("preExit", 1),
  ];
  child.emit("spawn");
  child.stdin.emit("finish");
  child.stdout.emit("data", Buffer.from(rows.map((row) => JSON.stringify(row) + "\n").join("")));
  registerStartupBirth(handle, scout);
  for (const stream of [child.stdin, child.stdout, child.stderr]) stream.emit("close");
  child.emit("exit", 0, null);
  child.emit("close", 0, null);
  await assert.rejects(result, (error) => {
    const d = error.details;
    assert.equal(d.stage, "startup-population", JSON.stringify(d));
    assert.equal(d.helperResult, "complete");
    assert.equal(d.helper.closed, true);
    assert.equal(d.helper.pipesClosed, true);
    assert.equal(d.rssCoverage, "partial");
    assert.equal(d.summedRssKiB, null);
    assert.equal(d.observedSubtotalKiB, 123);
    assert.equal(d.startupEpochCoverage.missing[0].birth.pid, 1003);
    assert.equal(d.startupEpochCoverage.missing[0].rssKiB, null);
    return true;
  });
});
