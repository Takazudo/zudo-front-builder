import assert from "node:assert/strict";
import { afterEach, test } from "vitest";
import { registerStartupBirth } from "../lib/fixed-rss-custody.mjs";
import { stallFs } from "./fs-stall.mjs";
import { MS, controller, identity, sampler, sleep } from "./family-replay-harness.mjs";

// Design §4, windows W-S and W-C: an 800 ms registry fsync stall while a sample is
// in flight, and inside the 750 ms custody readiness window.
const TIMEOUT_MS = 30_000;
const open = [];
afterEach(async () => {
  while (open.length) await open.pop()();
});

test.each([false, true])(
  "T-S: a registry fsync stall inside an in-flight sample (durable=%s)",
  async (durable) => {
    const stall = stallFs();
    const c = controller(durable);
    let ownedWrittenNs = null;
    // Frame processing of sample 3 discovers a new owned pid, as
    // linux-rss-sample.mjs:820-830 calls handle.onNewOwned mid-sample.
    const s = sampler(c, {
      onDispatch: (n, handle) => {
        if (n !== 3) return;
        setTimeout(() => {
          handle.onNewOwned(identity(77, 4, 77));
          ownedWrittenNs = process.hrtime.bigint();
        }, 10);
      },
    });
    open.push(async () => {
      stall.stop();
      await s.monitor.stop();
      c.close();
    });
    stall.arm('"type":"owned"', 800);
    while (s.dispatches < 4 && !s.failures.length) await sleep(50);
    await sleep(200);
    if (!durable) {
      assert.match(String(s.failures[0]?.message), /sampling helper deadline reached/);
      return;
    }
    assert.deepEqual(s.failures, [], "the sample settled inside its helper deadline");
    assert(ownedWrittenNs !== null);
    const done = await c.sink.barrier();
    assert.equal(stall.log.length, 1);
    assert.equal(stall.log[0].kind, "fsync", "the stall ran on the threadpool");
    assert(stall.log[0].startNs > ownedWrittenNs, "the registry row precedes its barrier");
    assert(done.throughSeq >= c.rec.records.at(-1).seq - 1);
  },
  TIMEOUT_MS,
);

// The custody readiness bound (fixed-rss-custody.mjs readinessDeadline, spawn +
// 750 ms) is modeled here: the real python holder is not started.
test.each([false, true])(
  "T-C: a registerStartupBirth registry stall inside the readiness window (durable=%s)",
  async (durable) => {
    const stall = stallFs();
    const c = controller(durable);
    open.push(async () => {
      stall.stop();
      c.close();
    });
    const handle = { known: new Map() };
    handle.onNewOwned = (p) => c.registry.append({ type: "owned", ...p });
    stall.arm('"type":"owned"', 800);
    const deadlineNs = process.hrtime.bigint() + 750n * MS;
    c.rec.append({ type: "fixed-custody-spawn" });
    setTimeout(() => registerStartupBirth(handle, identity(41, 3, 41)), 50);
    const outcome = await new Promise((resolve) =>
      setTimeout(() => {
        if (process.hrtime.bigint() >= deadlineNs) return resolve("readiness timeout");
        c.rec.append({ type: "fixed-custody-ready" });
        resolve("ready");
      }, 100),
    );
    if (!durable) {
      assert.equal(outcome, "readiness timeout");
      return;
    }
    assert.equal(outcome, "ready");
    const ready = c.rec.records.find((r) => r.type === "fixed-custody-ready");
    const spawn = c.rec.records.find((r) => r.type === "fixed-custody-spawn");
    assert(spawn.seq < ready.seq, "custody rows stay in order");
    const pre = await c.barrier("pre");
    assert.equal(pre.ok, true);
    assert(pre.throughSeq >= ready.seq, "B-pre covers the custody rows");
    assert(BigInt(pre.endNs) - BigInt(pre.startNs) >= 750n * MS, "B-pre absorbs the stall");
  },
  TIMEOUT_MS,
);
