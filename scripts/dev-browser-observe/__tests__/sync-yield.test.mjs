// #4856 r2: a synchronous controller run kept the event loop from polling the
// in-flight sampler's stdout pipe for ~720 ms. These tests pin the drain/yield
// mitigation: settle(), the peer-cycle yield and the runBatch yields.
import assert from "node:assert/strict";
import { test } from "vitest";
import { startFixedRssSafety } from "../lib/fixed-rss-safety.mjs";
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

test("the peer cycle yields between the sample append and the role check", async () => {
  const log = [];
  const monitor = startFixedRssSafety({
    ...peerConfig,
    sample: async () => peerSample(),
    append: (r) => log.push(r.type),
    abort: (e) => log.push("abort:" + e.message),
    yieldLoop: async () => {
      log.push("yield");
      await yieldToLoop();
    },
    checkRoles: () => {
      log.push("roles");
      return { chain: [role] };
    },
  });
  await monitor.settle();
  assert.deepEqual(log, ["fixed-rss-sample", "yield", "roles", "fixed-peer-roles"]);
  await monitor.stop();
});

test("a failure during the yield skips the role check", async () => {
  const log = [];
  let failNow;
  const monitor = startFixedRssSafety({
    ...peerConfig,
    sample: async () => peerSample(),
    append: (r) => log.push(r.type),
    abort: (e) => log.push("abort:" + e.message),
    yieldLoop: async () => {
      log.push("yield");
      failNow();
      await yieldToLoop();
    },
    checkRoles: () => {
      log.push("roles");
      return { chain: [role] };
    },
  });
  // Surface an external failure (the readiness expiry path) during the yield.
  failNow = () => monitor.verifyReady(process.hrtime.bigint() - 4_000_000_000n).catch(() => {});
  await monitor.settle();
  assert(!log.includes("roles"), JSON.stringify(log));
  assert.match(monitor.report().failure.error, /expired/);
  await monitor.stop();
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
