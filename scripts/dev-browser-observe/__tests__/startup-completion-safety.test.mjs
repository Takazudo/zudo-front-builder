import assert from "node:assert/strict";
import { afterEach, test, vi } from "vitest";
import { startFixedRssSafety } from "../lib/fixed-rss-safety.mjs";

afterEach(() => vi.useRealTimers());

// The safety monitor confirms a pending product completion at family readiness with
// the controller census, and admits a family capture only after that confirmation.
function monitor({
  confirmed = true,
  census = { controllerChildren: [10, 7], expectedChildren: [10, 7] },
} = {}) {
  vi.useFakeTimers();
  const root = { pid: 10, disposition: "live" };
  const dev = { pid: 11, ppid: 10, startTicks: "3", bootId: "boot", disposition: "live" };
  const calls = [];
  let completion = { confirmed: null };
  let active,
    helper,
    n = 0;
  const custody = {
    get startupCompletionPending() {
      return !completion.confirmed;
    },
    confirmStartupCompletion(args) {
      calls.push(args);
      if (!confirmed) throw new Error("startup product completion unconfirmed at readiness");
      // The real check (fixed-rss-custody.mjs) requires an empty departure list.
      if (!Array.isArray(args.sample.departures) || args.sample.departures.length)
        throw new Error("startup product completion unconfirmed at readiness");
      completion = { confirmed: { epoch: args.sample.epoch } };
    },
    familyProof: () => ({ root, dev, worker: null, terminal: null, release: null, completion }),
  };
  const safety = startFixedRssSafety({
    joinOwnedFamily: true,
    server: { handle: { native: root } },
    fixedExecutables: { protocol: "linux-fixed-zfb.v4", root: { path: "/controlled" } },
    limits: { rssCeilingKiB: 100, sampleEveryMs: 1000 },
    append() {},
    abort() {},
    custody,
    startupCensus: census && (() => census),
    sample(_, __, { onHelperSpawn }) {
      n++;
      active = Promise.withResolvers();
      const start = process.hrtime.bigint();
      helper = {
        pid: 100 + n,
        spawnHrtimeNs: String(start),
        entryNs: String(start),
        deadlineNs: String(start + 750000000n),
      };
      onHelperSpawn(helper);
      return active.promise;
    },
  });
  const finish = (extra = []) =>
    active.resolve({
      summedRssKiB: 50,
      helperResult: "complete",
      processes: [root, dev, ...extra],
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
  return { safety, calls, finish };
}

test("family readiness confirms the product completion before any family capture", async () => {
  const m = monitor();
  try {
    m.finish();
    await vi.advanceTimersByTimeAsync(0);
    const ready = m.safety.verifyFamilyBrowserReady();
    await vi.advanceTimersByTimeAsync(1000);
    m.finish();
    await ready;
    assert.equal(m.calls.length, 1);
    assert.deepEqual(m.calls[0].controllerChildren, [10, 7]);
    // The readiness sample's own helper may still be closing: it is expected too.
    assert.deepEqual(m.calls[0].expectedChildren, [10, 7, 102]);
    assert.equal(m.calls[0].sample.epoch, 2);
    await vi.advanceTimersByTimeAsync(0);
    assert.equal(m.safety.captureFamilyCycle({}).settlement, null);
  } finally {
    m.finish();
    await m.safety.stop();
  }
});

test("an unconfirmed product completion refuses family readiness and every capture", async () => {
  const m = monitor({ confirmed: false });
  try {
    m.finish();
    await vi.advanceTimersByTimeAsync(0);
    const ready = m.safety.verifyFamilyBrowserReady();
    await vi.advanceTimersByTimeAsync(1000);
    m.finish();
    await assert.rejects(ready, /product completion unconfirmed/);
    await vi.advanceTimersByTimeAsync(0);
    assert.throws(
      () => m.safety.captureFamilyCycle({}),
      /release lacks a previously accepted epoch/,
    );
  } finally {
    m.finish();
    await m.safety.stop();
  }
});

test("a product completion without a controller census refuses family readiness", async () => {
  const m = monitor({ census: null });
  try {
    m.finish();
    await vi.advanceTimersByTimeAsync(0);
    const ready = m.safety.verifyFamilyBrowserReady();
    await vi.advanceTimersByTimeAsync(1000);
    m.finish();
    await assert.rejects(ready, /no controller census/);
    assert.equal(m.calls.length, 0);
  } finally {
    m.finish();
    await m.safety.stop();
  }
});

// review-3 finding 3: the sampler's own non-live rows reach the confirmation.
test("a readiness sample with a departed row carries it to the confirmation, which refuses", async () => {
  const m = monitor();
  try {
    m.finish();
    await vi.advanceTimersByTimeAsync(0);
    const ready = m.safety.verifyFamilyBrowserReady();
    await vi.advanceTimersByTimeAsync(1000);
    m.finish([
      { pid: 503, ppid: 11, startTicks: "4", bootId: "boot", disposition: "proved-departure" },
    ]);
    await assert.rejects(ready, /product completion unconfirmed/);
    assert.equal(m.calls.length, 1);
    assert.deepEqual(m.calls[0].sample.departures, [{ pid: 503, disposition: "proved-departure" }]);
    assert.deepEqual(
      m.calls[0].sample.live.map((p) => p.pid),
      [10, 11],
    );
  } finally {
    m.finish();
    await m.safety.stop();
  }
});
