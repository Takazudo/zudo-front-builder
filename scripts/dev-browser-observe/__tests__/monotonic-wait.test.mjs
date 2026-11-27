import { describe, expect, it } from "vitest";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ObserverPage } from "../lib/browser.mjs";
import { performWrites } from "../lib/fixture.mjs";
import { hrtimeNs, waitAtLeast } from "../lib/util.mjs";

const MS = 1_000_000n;

// A fake monotonic clock whose timer fires `shortNs` before the requested delay,
// like the 0.19 ms early wake that stopped the #4878 A/A r3 cohort at s1-A.
function earlyClock({ startNs = 476_633_597_000_000n, shortNs = 200_000n } = {}) {
  let now = startNs;
  const calls = [];
  return {
    calls,
    clock: () => now,
    timer: async (ms) => {
      calls.push(ms);
      const step = BigInt(ms) * MS - shortNs;
      now += step > 0n ? step : 1n;
    },
  };
}

describe("waitAtLeast: a declared minimum holds on the monotonic clock", () => {
  it("re-arms an early timer until the clock confirms the full interval", async () => {
    const f = earlyClock();
    const start = f.clock();
    const end = await waitAtLeast(3000, { clock: f.clock, startNs: start, timer: f.timer });
    expect(end - start).toBeGreaterThanOrEqual(3000n * MS);
    expect(f.calls[0]).toBe(3000);
    expect(f.calls.length).toBeGreaterThan(1);
    // The remainder re-arms in whole milliseconds, never a sub-millisecond spin.
    expect(f.calls.length).toBeLessThanOrEqual(3);
    expect(f.calls.slice(1).every((ms) => ms >= 1)).toBe(true);
  });

  it("returns at once when the interval already elapsed, without arming a timer", async () => {
    const f = earlyClock();
    const end = await waitAtLeast(5, {
      clock: f.clock,
      startNs: f.clock() - 6n * MS,
      timer: f.timer,
    });
    expect(f.calls).toEqual([]);
    expect(end).toBe(f.clock());
  });

  it("treats a non-positive or non-finite duration as one zero-length yield", async () => {
    for (const ms of [0, -1, Number.NaN, undefined]) {
      const f = earlyClock();
      await waitAtLeast(ms, { clock: f.clock, timer: f.timer });
      expect(f.calls).toEqual([0]);
    }
  });

  it("stops on abort instead of re-arming", async () => {
    const f = earlyClock();
    const ac = new AbortController();
    const timer = async (ms) => {
      await f.timer(ms);
      ac.abort(new Error("stop"));
    };
    await expect(waitAtLeast(3000, { clock: f.clock, timer, signal: ac.signal })).rejects.toThrow(
      "stop",
    );
    expect(f.calls).toEqual([3000]);
  });

  it("holds on the real clock with the default timer", async () => {
    const start = hrtimeNs();
    const end = await waitAtLeast(15, { startNs: start });
    expect(end - start).toBeGreaterThanOrEqual(15n * MS);
    expect(hrtimeNs() - start).toBeGreaterThanOrEqual(15n * MS);
  });
});

describe("ObserverPage.quietWindow: observedMs never falls short of the declared window", () => {
  it("reports at least 3000 ms when every timer wakes 0.2 ms early", async () => {
    const f = earlyClock();
    const p = new ObserverPage(null, "", f.clock);
    p.timer = f.timer;
    const q = await p.quietWindow(3000);
    expect(q.quietMs).toBe(3000);
    expect(q.observedMs).toBeGreaterThanOrEqual(3000);
    expect(f.calls.length).toBeGreaterThan(1);
  });

  it("reports at least the window on the real clock", async () => {
    const q = await new ObserverPage(null, "").quietWindow(20);
    expect(q.observedMs).toBeGreaterThanOrEqual(20);
  });
});

describe("ObserverPage.goneCheck: the second read comes at least quietMs after the first", () => {
  it("re-arms an early real-clock timer before the stable re-read", async () => {
    const view = { ok: true, httpStatus: 404, notInCache: true, containsMarker: false };
    const p = new ObserverPage(null, "", () => 1n);
    const readsAt = [];
    p.observePage = async (path) => {
      readsAt.push(hrtimeNs());
      return { path, ...view };
    };
    const calls = [];
    // Real timer that wakes one millisecond before the requested delay.
    p.timer = (ms) => {
      calls.push(ms);
      return new Promise((r) => setTimeout(r, Math.max(0, ms - 1)));
    };
    const r = await p.goneCheck({ kind: "gone", path: "/gone/" }, { quietMs: 30 });
    expect(r.basis).toBe("stable-reads");
    expect(readsAt).toHaveLength(2);
    expect(readsAt[1] - readsAt[0]).toBeGreaterThanOrEqual(30n * MS);
    expect(calls.length).toBeGreaterThan(1);
  });
});

describe("performWrites: the default inter-step gap is a monotonic minimum", () => {
  it("separates consecutive writes by at least afterMs on the real clock", async () => {
    const dir = mkdtempSync(join(tmpdir(), "dbo-gap-"));
    try {
      const path = join(dir, "a.md");
      writeFileSync(path, "x");
      const steps = [
        { method: "in-place", path, bytes: Buffer.from("a") },
        { method: "in-place", path, bytes: Buffer.from("b"), afterMs: 12 },
      ];
      // A global timer that wakes 2 ms early must not shorten the declared gap.
      const realSetTimeout = globalThis.setTimeout;
      globalThis.setTimeout = (fn, ms, ...args) =>
        realSetTimeout(fn, Math.max(0, (ms ?? 0) - 2), ...args);
      let r;
      try {
        r = await performWrites(steps, hrtimeNs);
      } finally {
        globalThis.setTimeout = realSetTimeout;
      }
      const done = r.steps ?? r;
      expect(done).toHaveLength(2);
      expect(BigInt(done[1].tNs) - BigInt(done[0].tNs)).toBeGreaterThanOrEqual(12n * MS);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
