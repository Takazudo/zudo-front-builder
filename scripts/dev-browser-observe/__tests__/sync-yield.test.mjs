// #4856 r2: a synchronous controller run kept the event loop from polling the
// in-flight sampler's stdout pipe for ~720 ms. These tests pin the drain/yield
// mitigation: settle(), the post-launch handoff, the runBatch yields and the
// structural bound a yield puts on a pending frame.
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { createConnection, createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "vitest";
import { ControllerTiming } from "../lib/controller-timing.mjs";
import { startFixedRssSafety } from "../lib/fixed-rss-safety.mjs";
import { controllerDiagnostics, postLaunchHandoff } from "../lib/run.mjs";
import { runBatch } from "../lib/state-machine.mjs";
import { yieldToLoop } from "../lib/util.mjs";
import { deps, fakePage } from "./fakes.mjs";

function peerSample() {
  const ns = process.hrtime.bigint();
  return {
    summedRssKiB: 4,
    processes: [{ statusKiB: { VmRSS: 3 }, smapsRollupKiB: { Rss: 4 } }],
    helper: { spawnHrtimeNs: String(ns), closed: true, pipesClosed: true },
    timing: {
      clock: { qualified: true },
      parent: { entryNs: String(ns), deadlineNs: String(ns + 750000000n), resultNs: String(ns) },
    },
  };
}

const peerConfig = {
  server: {},
  fixedExecutables: { protocol: "linux-fixed-astro-css.v1", root: { path: "/pinned/node" } },
  peer: {},
  limits: { rssCeilingKiB: 4194304, sampleEveryMs: 60_000 },
};
const role = { role: "astro-cli", pid: 12, ppid: 3, pgid: 12, bootId: "boot", startTicks: "2" };

test("settle() resolves only after the in-flight sample and its role check", async () => {
  const gate = Promise.withResolvers();
  const log = [];
  const monitor = startFixedRssSafety({
    ...peerConfig,
    sample: () => gate.promise,
    append: (r) => log.push(r.type),
    abort: (e) => log.push("abort:" + e.message),
    checkRoles: () => {
      log.push("roles");
      return { chain: [role] };
    },
  });
  let settled = false;
  const settling = monitor.settle().then(() => {
    settled = true;
  });
  await yieldToLoop();
  assert.equal(settled, false);
  gate.resolve(peerSample());
  await settling;
  assert.deepEqual(log, ["fixed-rss-sample", "roles", "fixed-peer-roles"]);
  assert.equal(monitor.report().failure, null);
  await monitor.stop();
});

test("settle() never rejects; the failure stays in report()", async () => {
  const monitor = startFixedRssSafety({
    ...peerConfig,
    sample: async () => {
      throw Object.assign(new Error("sampling helper deadline reached"), { details: null });
    },
    append() {},
    abort() {},
  });
  await monitor.settle();
  assert.equal(monitor.report().failure.error, "sampling helper deadline reached");
  await monitor.stop();
});

function busy(ms) {
  const end = process.hrtime.bigint() + BigInt(ms) * 1_000_000n;
  while (process.hrtime.bigint() < end);
}

// Review C1: the peer .then runs synchronously, so `pending` clears in the same
// microtask drain and an overdue interval can never find it occupied, however
// slow the fsynced append is. A yield inside that .then broke this.
test("a slow sample append never makes the next dispatch find the sampler occupied", async () => {
  let appends = 0;
  const monitor = startFixedRssSafety({
    ...peerConfig,
    limits: { ...peerConfig.limits, sampleEveryMs: 100 },
    sample: async () => peerSample(),
    append: (r) => {
      if (r.type === "fixed-rss-sample" && appends++ === 1) busy(150);
    },
    abort() {},
    checkRoles: () => ({ chain: [role] }),
  });
  await new Promise((resolve) => setTimeout(resolve, 450));
  await monitor.stop();
  const report = monitor.report();
  assert.equal(report.failure, null, JSON.stringify(report.failure));
  assert(report.measuredCycles >= 3, String(report.measuredCycles));
});

test("runBatch yields after scenario-start and after the precheck sample", async () => {
  const log = [];
  const page = fakePage();
  page.prepare = async () => {
    log.push("prepare");
    throw new Error("end after prepare");
  };
  const d = deps({ page });
  const append = d.recorder.append.bind(d.recorder);
  d.recorder.append = (r) => {
    log.push(r.type);
    return append(r);
  };
  const sampleResources = d.server.sampleResources;
  d.server.sampleResources = (...args) => {
    log.push("sample");
    return sampleResources(...args);
  };
  await runBatch({
    scenarios: [{ id: "s1", ops: [{ id: "o1", op: "edit-title", article: "a" }] }],
    deps: { ...d, yieldLoop: async () => log.push("yield") },
  });
  const start = log.indexOf("scenario-start");
  assert.deepEqual(log.slice(start, start + 5), [
    "scenario-start",
    "yield",
    "sample",
    "yield",
    "prepare",
  ]);
});

test("runBatch reports the first prepared page once, before the attempt proceeds", async () => {
  const log = [];
  const page = fakePage();
  const prepare = page.prepare.bind(page);
  page.prepare = async (...args) => {
    log.push("prepare");
    return prepare(...args);
  };
  page.markPre = () => log.push("markPre");
  const d = deps({ page });
  await runBatch({
    scenarios: [{ id: "s1", ops: [{ id: "o1", op: "edit-title", article: "a" }] }],
    deps: { ...d, onPrepared: () => log.push("prepared") },
  });
  assert.deepEqual(log.slice(0, 3), ["prepare", "prepared", "markPre"]);
});

// Review C2: deterministic replay of the r2 shape. The peer's write lands in
// the kernel before the synchronous chain starts, so the only question is
// which block boundary first lets the loop poll. No millisecond threshold.
async function socketPair() {
  const dir = mkdtempSync(join(tmpdir(), "dbo-sync-yield-"));
  const path = join(dir, "s");
  const server = createServer();
  const accepted = new Promise((resolve) => server.once("connection", resolve));
  await new Promise((resolve) => server.listen(path, resolve));
  const client = createConnection(path);
  await new Promise((resolve) => client.once("connect", resolve));
  const peer = await accepted;
  return {
    client,
    peer,
    close() {
      client.destroy();
      peer.destroy();
      server.close();
      rmSync(dir, { recursive: true, force: true });
    },
  };
}

async function blockAtDelivery({ blocks, yieldLoop }) {
  const pair = await socketPair();
  try {
    let completed = 0;
    const delivered = new Promise((resolve) => pair.peer.once("data", () => resolve(completed)));
    pair.client.write("frame\n");
    for (let i = 0; i < blocks; i++) {
      busy(5);
      completed++;
      if (yieldLoop) await yieldLoop();
    }
    return await delivered;
  } finally {
    pair.close();
  }
}

test("without yields a ready frame waits for the whole synchronous chain", async () => {
  assert.equal(await blockAtDelivery({ blocks: 4, yieldLoop: null }), 4);
});

test("yieldToLoop delivers a ready frame at the first block boundary", async () => {
  assert.equal(await blockAtDelivery({ blocks: 4, yieldLoop: yieldToLoop }), 1);
});

test("yieldToLoop crosses a poll even when called from the poll phase", async () => {
  // Start the chain from an I/O (poll-phase) callback; one setImmediate from
  // there runs in the same turn's check phase without polling.
  const pair = await socketPair();
  try {
    const trigger = await socketPair();
    try {
      const results = await new Promise((resolve) => {
        trigger.peer.once("data", async () => {
          let completed = 0;
          const delivered = new Promise((r) => pair.peer.once("data", () => r(completed)));
          pair.client.write("frame\n");
          busy(5);
          completed++;
          await yieldToLoop();
          busy(5);
          completed++;
          await yieldToLoop();
          resolve(await delivered);
        });
        trigger.client.write("go");
      });
      assert.equal(results, 1);
    } finally {
      trigger.close();
    }
  } finally {
    pair.close();
  }
});

test("postLaunchHandoff drains before browser-ready and yields around the receipt", async () => {
  const log = [];
  const timing = new ControllerTiming();
  await postLaunchHandoff({
    drain: async () => {
      log.push("drain:start");
      await yieldToLoop();
      log.push("drain:end");
    },
    signal: new AbortController().signal,
    appendReady: () => log.push("browser-ready"),
    writeReceipt: () => log.push("receipt"),
    yieldLoop: async () => log.push("yield"),
    timing,
  });
  assert.deepEqual(log, ["drain:start", "drain:end", "browser-ready", "yield", "receipt", "yield"]);
  assert(timing.report.stages.some((row) => row.name === "run.receipt.write"));
});

test("postLaunchHandoff stops before browser-ready when the drain observed a failure", async () => {
  const log = [];
  const abort = new AbortController();
  await assert.rejects(
    postLaunchHandoff({
      drain: async () => abort.abort(new Error("sampling helper deadline reached")),
      signal: abort.signal,
      appendReady: () => log.push("browser-ready"),
      writeReceipt: () => log.push("receipt"),
    }),
    /sampling helper deadline reached/,
  );
  assert.deepEqual(log, []);
});

test("postLaunchHandoff without a drain keeps the old order and no yields", async () => {
  const log = [];
  await postLaunchHandoff({
    appendReady: () => log.push("browser-ready"),
    writeReceipt: () => log.push("receipt"),
  });
  assert.deepEqual(log, ["browser-ready", "receipt"]);
});

test("controller diagnostics: the fixed Astro CSS cell writes timing and its event-loop file", () => {
  const css = controllerDiagnostics({ fixedRssSafety: "linux-fixed-astro-css.v1" });
  assert(css.timing && css.startupLoop && !css.browserTiming && css.loopThroughPrepare);
  assert.deepEqual(css.receiptFields, {
    controllerTiming: "controller-timing.json",
    controllerEventLoop: "controller-event-loop.json",
  });
  const dir = mkdtempSync(join(tmpdir(), "dbo-diag-"));
  try {
    css.write(dir);
    const file = join(dir, "controller-event-loop.json");
    assert(existsSync(file));
    assert.equal(
      JSON.parse(readFileSync(file, "utf8")).schema,
      "dev-browser-observe.controller-event-loop.v1",
    );
    for (const protocol of ["linux-fixed-astro-footer.v1", "linux-fixed-zfb.v4", undefined]) {
      const other = controllerDiagnostics({ fixedRssSafety: protocol });
      assert.equal(other.timing, null);
      assert.equal(other.startupLoop, null);
      assert.equal(other.loopThroughPrepare, false);
      assert.deepEqual(other.receiptFields, {});
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
