import { describe, expect, it } from "vitest";
import { runBatch } from "../lib/state-machine.mjs";
import { deps, fakeFs, fakePage } from "./fakes.mjs";

// A settled source-timing safety whose T0 read (`current()` inside the write callback) fails once.
function safetyFailingAtT0() {
  const decision = (site) => ({ site, outcome: "fresh", decisionNs: "1" });
  let currentCalls = 0;
  return {
    get currentCalls() {
      return currentCalls;
    },
    decide: async ({ site }) => ({ decision: decision(site), summedRssKiB: 1 }),
    awaitDecision: async ({ site }) => decision(site),
    current() {
      currentCalls += 1;
      if (currentCalls === 1) throw new Error("source timing sample refused at T0");
      return { epoch: 1, acceptedNs: "1", decisionNs: "1" };
    },
  };
}

describe("source timing: a T0 refusal is a before-write refusal (#4886)", () => {
  it("records resource-limit before mutation, performs no write and stops the batch", async () => {
    const safety = safetyFailingAtT0();
    const fs = fakeFs();
    const d = {
      ...deps({ page: fakePage({ behaviors: [{ kind: "notify", t1: 10n }] }), fs }),
      sourceTimingSafety: safety,
      safetySignal: new AbortController().signal,
    };
    const { accounting } = await runBatch({
      scenarios: [
        {
          id: "s",
          ops: [
            { id: "a", op: "edit-body", article: "a", writeMethod: "in-place" },
            { id: "b", op: "edit-body", article: "b", writeMethod: "in-place" },
          ],
        },
      ],
      deps: d,
    });
    const attempt = d.recorder.records.find((r) => r.type === "attempt");
    expect(attempt).toMatchObject({
      outcome: "resource-limit",
      phase: "before-mutation",
      writePerformed: false,
      error: "source timing sample refused at T0",
    });
    expect(fs.writes).toEqual([]);
    expect(fs.files["a.md"]).toBe("v0");
    expect(accounting).toMatchObject({ stopReason: "resource-limit", notAttempted: 1 });
    expect(d.recorder.records.some((r) => r.outcome === "write-error")).toBe(false);
  });
});
