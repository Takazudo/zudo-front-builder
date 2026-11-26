import { setTimeout as delay } from "node:timers/promises";
import { expect, it } from "vitest";
import { BrowserTiming } from "../lib/browser-timing.mjs";
import { StartupEventLoop } from "../lib/startup-event-loop.mjs";

const block = new Int32Array(new SharedArrayBuffer(4));

async function advanceHistogram(monitor, predicate) {
  for (let turn = 0; turn < 25; turn++) {
    monitor.browserReady();
    if (predicate(monitor.report.countAtBrowserReady)) return;
    await delay(20);
  }
  throw new Error("event-loop histogram did not initialize or advance");
}

it("qualifies a post-stall observation after the monitor is initialized and gets another turn", async () => {
  const timing = new BrowserTiming();
  const monitor = new StartupEventLoop();
  monitor.start();
  try {
    await advanceHistogram(monitor, (count) => count > 0);
    const dispatch = delay(5).then(() => process.hrtime.bigint());
    const countBeforeBlock = monitor.report.countAtBrowserReady;
    const blockedMs = 80;
    const blockStarted = process.hrtime.bigint();
    timing.syncSpan("controlled-block", null, {}, () => Atomics.wait(block, 0, 0, blockedMs));
    const blockReturned = process.hrtime.bigint();
    const [span] = timing.snapshot().syncSpans;
    expect(span).toMatchObject({ name: "controlled-block", outcome: "returned" });
    expect(BigInt(span.startNs)).toBeGreaterThanOrEqual(blockStarted);
    expect(BigInt(span.endNs) - BigInt(span.startNs)).toBeGreaterThanOrEqual(75_000_000n);
    expect(BigInt(span.endNs)).toBeLessThanOrEqual(blockReturned);
    expect(await dispatch).toBeGreaterThanOrEqual(blockReturned);
    await advanceHistogram(monitor, (count) => count > countBeforeBlock);
    // Mark a browser-ready boundary before the final async observation.
    await delay(65);
    monitor.stop("before-edits");
    expect(monitor.report).toMatchObject({
      disabled: true,
      resolutionMs: 20,
      histogramStatus: "observed",
      trailingObservation: "post-browser-ready-sample-observed",
      exceeds: 0,
    });
    expect(monitor.report.maxNs).toBeGreaterThanOrEqual(75_000_000);
    expect(BigInt(monitor.report.disableCallNs)).toBeGreaterThan(blockReturned);
    expect(monitor.report.utilizationDelta.active).toBeGreaterThanOrEqual(75);
    expect(monitor.report.utilizationDelta.idle).toBeGreaterThan(0);
  } finally {
    monitor.stop("finalizing");
  }
});

it("can record a dispatch during an asynchronous wait before disabling", async () => {
  const monitor = new StartupEventLoop();
  monitor.start();
  try {
    await advanceHistogram(monitor, (count) => count > 0);
    const waitStarted = process.hrtime.bigint();
    const { promise, resolve } = Promise.withResolvers();
    let waitEnded = null;
    const waiting = promise.then(() => {
      waitEnded = process.hrtime.bigint();
    });
    const dispatch = await delay(20).then(() => process.hrtime.bigint());
    expect(waitEnded).toBeNull();
    resolve();
    await waiting;
    expect(dispatch).toBeGreaterThan(waitStarted);
    expect(dispatch).toBeLessThanOrEqual(waitEnded);
  } finally {
    monitor.stop("before-edits");
  }
  expect(monitor.report.disabled).toBe(true);
});

it("does not manufacture a zero sample when startup fails in the blocked turn", () => {
  const monitor = new StartupEventLoop();
  monitor.start();
  try {
    Atomics.wait(block, 0, 0, 35);
    monitor.browserReady();
  } finally {
    monitor.stop("finalizing");
  }
  expect(monitor.report).toMatchObject({
    disabled: true,
    count: 0,
    maxNs: null,
    histogramStatus: "unavailable",
    histogramUnavailableReason: "no-in-range-samples",
    trailingObservation: "no-post-browser-ready-sample",
    stopReason: "finalizing",
  });
});

it.each([false, true])("disables sampling on cancellation (already aborted: %s)", (aborted) => {
  const abort = new AbortController();
  const monitor = new StartupEventLoop();
  if (aborted) abort.abort();
  monitor.start(abort.signal);
  if (!aborted) abort.abort();
  expect(monitor.report).toMatchObject({
    enabled: true,
    disabled: true,
    stopReason: "cancelled",
    trailingObservation: "unavailable",
  });
  monitor.stop("finalizing");
  expect(monitor.report.stopReason).toBe("cancelled");
});

it("reports unavailable metrics when startup was never entered", () => {
  const monitor = new StartupEventLoop();
  monitor.stop("finalizing");
  expect(monitor.report).toMatchObject({
    enabled: null,
    disabled: null,
    count: null,
    maxNs: null,
    utilizationDelta: null,
    histogramStatus: "unavailable",
    stopReason: "not-started",
  });
});
