// @vitest-environment node
import { expect, it } from "vitest";
import { batch, enqueue, flush, MAX_FLUSH_PASSES, type Job } from "../../zudo-react/scheduler.js";

it("batches nested writes and restores depth after a throw", async () => {
  const runs: string[] = [];
  const job: Job = {
    id: "nested",
    phase: "commit",
    active: () => true,
    run: () => runs.push("run"),
  };
  expect(() =>
    batch(() => {
      batch(() => {
        enqueue(job);
        enqueue(job);
      });
      throw new Error("fail");
    }),
  ).toThrow("fail");
  expect(runs).toEqual([]);
  await flush();
  expect(runs).toEqual(["run"]);
});

it("orders commit before effect and skips disposed jobs", async () => {
  const seen: string[] = [];
  enqueue({
    id: "effect",
    phase: "effect",
    active: () => true,
    run: () => {
      seen.push("effect");
    },
  });
  enqueue({
    id: "commit",
    phase: "commit",
    active: () => true,
    run: () => {
      seen.push("commit");
    },
  });
  enqueue({
    id: "dead",
    phase: "commit",
    active: () => false,
    run: () => {
      seen.push("dead");
    },
  });
  await flush();
  expect(seen).toEqual(["commit", "effect"]);
});

it("bounds self reentry, rejects waiters, and permits a later write", async () => {
  let runs = 0;
  const reports: string[] = [];
  const loop: Job = {
    id: "loop",
    phase: "effect",
    active: () => true,
    run: () => {
      runs++;
      enqueue(loop);
    },
    report: (code, actual) => reports.push(`${code}:${actual}`),
  };
  enqueue(loop);
  await expect(flush()).rejects.toThrow(/ZR_FLUSH_LIMIT/);
  expect(runs).toBe(MAX_FLUSH_PASSES);
  expect(reports).toEqual(["ZR_FLUSH_LIMIT:loop"]);
  enqueue({
    id: "later",
    phase: "commit",
    active: () => true,
    run: () => {
      runs++;
    },
  });
  await flush();
  expect(runs).toBe(MAX_FLUSH_PASSES + 1);
});
