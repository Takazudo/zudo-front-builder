import assert from "node:assert/strict";
import { once } from "node:events";
import { Worker } from "node:worker_threads";
import { describe, expect, it } from "vite-plus/test";
import {
  collectPerTestDurations,
  quantile,
  summarizeRuns,
  withOwnedWorkers,
} from "../root-script-timing.mjs";

describe("root script timing harness", () => {
  it("summarizes per-test wall-clock observations across repeated JSON reports", () => {
    const first = {
      testResults: [
        {
          name: "scripts/__tests__/sample.test.mjs",
          assertionResults: [
            { fullName: "suite fast case", status: "passed", duration: 2 },
            { fullName: "suite slow case", status: "passed", duration: 20 },
          ],
        },
      ],
    };
    const second = {
      testResults: [
        {
          name: "scripts/__tests__/sample.test.mjs",
          assertionResults: [
            { fullName: "suite fast case", status: "passed", duration: 4 },
            { fullName: "suite slow case", status: "failed", duration: 40 },
          ],
        },
      ],
    };

    expect(collectPerTestDurations(first, "run-01")).toHaveLength(2);
    const summary = summarizeRuns([
      { id: "run-01", exitCode: 0, report: first },
      { id: "run-02", exitCode: 1, report: second },
    ]);

    expect(summary.testSampleCount).toBe(4);
    expect(summary.distributionAcrossPerTestObservations).toMatchObject({
      n: 4,
      minMs: 2,
      p50Ms: 20,
      p90Ms: 40,
      maxMs: 40,
    });
    expect(summary.slowestTestsByObservedMax[0]).toMatchObject({
      test: "suite slow case",
      sampleCount: 2,
      failedSamples: 1,
      distribution: { p50Ms: 40, maxMs: 40 },
    });
  });

  it("uses round(p * (n - 1)), capped at n - 1, for even sample counts", () => {
    assert.equal(quantile([40, 10, 30, 20], 0.5), 30);
    assert.equal(quantile([], 0.5), null);
  });

  it("keeps a run report but flags it when the runner JSON has no timed assertions", () => {
    const summary = summarizeRuns([
      {
        id: "run-01",
        exitCode: 0,
        report: {
          testResults: [
            {
              name: "scripts/__tests__/sample.test.mjs",
              assertionResults: [
                { fullName: "suite skipped case", status: "skipped", duration: null },
              ],
            },
          ],
        },
      },
    ]);

    expect(summary.reportCount).toBe(1);
    expect(summary.testSampleCount).toBe(0);
    expect(summary.analysisErrors).toEqual([
      {
        runId: "run-01",
        message: "Vitest JSON contains no assertionResults with numeric duration values",
      },
    ]);
  });

  it("stops owned workers when a measured command rejects", async () => {
    const stopBuffer = new SharedArrayBuffer(Int32Array.BYTES_PER_ELEMENT);
    const worker = new Worker(
      `const { workerData } = require("node:worker_threads"); Atomics.wait(new Int32Array(workerData.stopBuffer), 0, 0);`,
      { eval: true, workerData: { stopBuffer } },
    );
    await once(worker, "online");
    const runFailure = new Error("simulated runner failure");

    try {
      await expect(
        withOwnedWorkers(
          1,
          async () => ({ worker, stopBuffer }),
          async () => {
            throw runFailure;
          },
        ),
      ).rejects.toBe(runFailure);
      expect(worker.threadId).toBe(-1);
    } finally {
      if (worker.threadId !== -1) await worker.terminate();
    }
  });
});
