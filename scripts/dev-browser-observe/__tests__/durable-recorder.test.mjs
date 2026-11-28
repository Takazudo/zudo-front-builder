import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, test } from "vitest";
import { reserveSession } from "../lib/admission.mjs";
import {
  DurableSink,
  createBarrierPoint,
  createOwnershipJournal,
  createReceiptWriter,
} from "../lib/durable-io.mjs";
import { Recorder } from "../lib/recorder.mjs";
import { batchExitOk, runBatch } from "../lib/state-machine.mjs";
import { deps, fakeFs, fakePage, fakeRecorder } from "./fakes.mjs";
import { stallFs } from "./fs-stall.mjs";
import { traceFs } from "./fs-trace.mjs";

const MS = 1_000_000n;
const cleanups = [];
afterEach(() => {
  while (cleanups.length) cleanups.pop()();
});
const scratch = () => {
  const dir = mkdtempSync(join(tmpdir(), "dbo-durable-"));
  cleanups.push(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
};
const stalled = () => {
  const s = stallFs();
  cleanups.push(() => s.stop());
  return s;
};

test("a write made during an in-flight fsync is covered by a second round (m4)", async () => {
  const dir = scratch();
  const stall = stalled();
  const sink = new DurableSink();
  const rec = new Recorder(join(dir, "records.jsonl"), null, sink);
  const registry = createOwnershipJournal(join(dir, "owned.jsonl"), { sink });
  stall.arm('"type":"first"', 150);
  rec.append({ type: "first" });
  rec.append({ type: "first-tail" });
  const first = sink.barrier();
  // Both a records row and a registry row land while the first round is in flight.
  rec.append({ type: "second" });
  registry.append({ type: "owned", pid: 7 });
  const second = sink.barrier();
  const third = sink.barrier();
  assert.notEqual(first, second, "a write after the round started needs another round");
  assert.equal(second, third, "callers after that write share the queued round");
  const a = await first;
  const b = await second;
  assert.equal(a.throughSeq, 1);
  assert.equal(b.throughSeq, 2);
  assert(BigInt(b.startNs) >= BigInt(a.endNs), "the second round starts after the first ends");
  assert.equal(sink.durable, sink.written);
  assert.equal(stall.log.length, 1);
  assert.equal(stall.log[0].kind, "fsync");
  // Nothing new written: the next barrier is an immediate new round with the same seq.
  assert.equal((await sink.barrier()).throughSeq, 2);
  sink.close();
});

test("the barrier fsyncs the records fd, the registry fd and the renamed intent with its directory", async () => {
  const dir = scratch();
  const sink = new DurableSink();
  const rec = new Recorder(join(dir, "records.jsonl"), null, sink);
  const registry = createOwnershipJournal(join(dir, "owned.jsonl"), { sink });
  const trace = traceFs(dir);
  let intent;
  try {
    intent = reserveSession(join(dir, "state", "active.json"), { labelDir: dir }, { sink });
    rec.append({ type: "custody" });
    registry.append({ type: "owned", pid: 9 });
    intent.update({ phase: "server-running" });
    const done = await sink.barrier();
    assert.equal(done.throughSeq, 0);
  } finally {
    trace.stop();
  }
  const kinds = trace.events.map((e) => e.slice(0, 2).join(" "));
  // The reservation stays synchronous: fsync of the intent, then of its directory.
  assert.deepEqual(kinds.slice(0, 6), [
    "open state/active.json",
    "close state/active.json",
    "open state/active.json.tmp",
    "write state/active.json.tmp",
    "fsyncSync state/active.json.tmp",
    "close state/active.json.tmp",
  ]);
  const afterReserve = kinds.slice(kinds.indexOf("rename state/active.json.tmp") + 1);
  assert.equal(afterReserve[0], "fsyncSync state", "directory fsync at reservation");
  const update = afterReserve.indexOf("rename state/active.json.tmp");
  assert(update > 0, "the in-window update renames");
  assert(
    !afterReserve.slice(1, update).some((k) => k.startsWith("fsyncSync")),
    "the in-window update does not fsync synchronously",
  );
  const barrier = afterReserve.slice(update + 1).filter((k) => k.startsWith("fsync "));
  assert.deepEqual(barrier.sort(), [
    "fsync owned.jsonl",
    "fsync records.jsonl",
    "fsync state",
    "fsync state/active.json",
  ]);
  intent.release();
  sink.close();
});

test("a barrier point expires on the work deadline and records the refusal (O2′)", async () => {
  const dir = scratch();
  const stall = stalled();
  const sink = new DurableSink();
  const rec = new Recorder(join(dir, "records.jsonl"), null, sink);
  stall.arm('"type":"scenario-start"', 400);
  rec.append({ type: "scenario-start" });
  rec.append({ type: "tail" });
  const deadlineNs = process.hrtime.bigint() + 60n * MS;
  const point = createBarrierPoint({ sink, recorder: rec, deadlineNs });
  const result = await point("pre");
  assert.equal(result.ok, false);
  assert.equal(result.expired, true);
  assert.equal(result.reason, "durability barrier expired");
  assert.equal(result.throughSeq, null);
  const elapsed = BigInt(result.endNs) - BigInt(result.startNs);
  assert(elapsed >= 55n * MS && elapsed < 350n * MS, String(elapsed));
  assert.deepEqual(rec.records.at(-1).type, "durability");
  await sink.barrier();
  sink.close();
});

test("a receipt checkpoint still fsyncing cannot rename over the final receipt (T-rcpt, m9)", async () => {
  const dir = scratch();
  const stall = stalled();
  const sink = new DurableSink();
  const path = join(dir, "receipt.json");
  const writer = createReceiptWriter(path, { sink });
  stall.arm('"stage": "checkpoint"', 300);
  const checkpoint = writer.checkpoint({ stage: "checkpoint" });
  await writer.write({ stage: "final" });
  await checkpoint;
  await new Promise((resolve) => setTimeout(resolve, 50));
  assert.deepEqual(JSON.parse(readFileSync(path, "utf8")), { stage: "final" });
  assert.deepEqual(readdirSync(dir), ["receipt.json"], "no temp file is left behind");
  assert.equal(stall.log.length, 1, "the checkpoint fsync really was in flight");
});

test("without a sink every writer keeps the synchronous fsync path", () => {
  const dir = scratch();
  const trace = traceFs(dir);
  try {
    new Recorder(join(dir, "records.jsonl"), null).append({ type: "a" });
    createOwnershipJournal(join(dir, "owned.jsonl")).append({ type: "owned" });
    createReceiptWriter(join(dir, "receipt.json")).checkpoint({ a: 1 });
    reserveSession(join(dir, "active.json"), {}).update({ phase: "x" });
  } finally {
    trace.stop();
  }
  assert.equal(trace.events.filter((e) => e[0] === "fsync").length, 0);
  assert.deepEqual(
    trace.events.filter((e) => e[0] === "fsyncSync").map((e) => e[1]),
    ["records.jsonl", "owned.jsonl", "receipt.json.tmp", "active.json.tmp", "active.json.tmp"],
  );
});

// B-pre and B-clean in runBatch, driven through a real sink and barrier point.
// `deadlineMs` per barrier name is measured from that barrier's own start, so a
// slow real fsync in one barrier cannot spend another barrier's budget.
function durableBatch({ deadlineMs = {}, ops, warmOrder = "none" } = {}) {
  const dir = scratch();
  const sink = new DurableSink();
  const order = [];
  const rec = new Recorder(join(dir, "records.jsonl"), null, sink);
  const point = (name) =>
    createBarrierPoint({
      sink,
      recorder: rec,
      deadlineNs: process.hrtime.bigint() + BigInt(deadlineMs[name] ?? 60_000) * MS,
    })(name);
  const page = fakePage({
    behaviors: [{ kind: "notify", t1: 10n ** 18n }],
  });
  for (const name of ["prepare", "arm", "markAttempt"]) {
    const original = page[name];
    page[name] = (...args) => {
      order.push(name);
      return original.apply(page, args);
    };
  }
  const fs = fakeFs();
  const restore = fs.restore;
  fs.restore = (snap) => {
    order.push("restore");
    fs.restoreNs = process.hrtime.bigint();
    return restore(snap);
  };
  const d = {
    ...deps({ page, fs, recorder: rec }),
    durability: {
      barrier: async (name) => {
        order.push("barrier:" + name);
        const result = await point(name);
        order.push("barrier-end:" + name);
        return result;
      },
    },
  };
  const scenarios = [
    { id: "s", warmOrder, ops: ops ?? [{ id: "e", op: "edit-body", article: "a" }] },
  ];
  return { sink, rec, order, page, fs, run: () => runBatch({ scenarios, deps: d }) };
}

test("B-pre precedes every page operation of the attempt (m1, m5)", async () => {
  const b = durableBatch({ warmOrder: "index-first" });
  const { accounting, stopped } = await b.run();
  assert.equal(stopped, false, JSON.stringify(accounting));
  const firstPage = b.order.findIndex((o) => ["prepare", "arm", "markAttempt"].includes(o));
  assert.equal(b.order.indexOf("barrier-end:pre"), firstPage - 1, b.order.join(","));
  const pre = b.rec.records.find((r) => r.type === "durability" && r.barrier === "pre");
  const start = b.rec.records.find((r) => r.type === "scenario-start");
  assert(pre && pre.throughSeq >= start.seq);
  b.sink.close();
});

test("an expired B-pre refuses before any page operation (T-pre, O2′)", async () => {
  const stall = stalled();
  stall.arm('"type":"scenario-start"', 500);
  const b = durableBatch({ deadlineMs: { pre: 80 }, warmOrder: "index-first" });
  const { accounting, stopped } = await b.run();
  assert.equal(stopped, true);
  assert.equal(accounting.stopReason, "durability-barrier-expired");
  assert.equal(batchExitOk(accounting, stopped), false);
  assert.deepEqual(
    b.order.filter((o) => ["prepare", "arm", "markAttempt"].includes(o)),
    [],
    "no page operation after an expired barrier",
  );
  assert(!b.rec.records.some((r) => r.type === "attempt"));
  const pre = b.rec.records.find((r) => r.type === "durability" && r.barrier === "pre");
  assert.equal(pre.expired, true);
  await b.sink.barrier();
  b.sink.close();
});

test("B-clean completes before the restoration starts (T-clean, m1)", async () => {
  const stall = stalled();
  stall.arm('"type":"attempt"', 1000);
  const b = durableBatch();
  const { accounting, stopped } = await b.run();
  assert.equal(stopped, false, JSON.stringify(accounting));
  const clean = b.rec.records.find((r) => r.type === "durability" && r.barrier === "clean");
  assert(clean, "a clean barrier row");
  assert(BigInt(clean.endNs) - BigInt(clean.startNs) >= 990n * MS, "the stall was absorbed");
  assert(b.fs.restoreNs >= BigInt(clean.endNs), "restoreNs follows the barrier");
  const attempt = b.rec.records.find((r) => r.type === "attempt");
  assert(clean.throughSeq >= attempt.seq);
  assert.equal(b.order.indexOf("restore"), b.order.indexOf("barrier-end:clean") + 1);
  b.sink.close();
});

test("an expired B-clean still restores, but the cell fails acceptance (T-clean, O2′)", async () => {
  const stall = stalled();
  stall.arm('"type":"attempt"', 3000);
  const b = durableBatch({ deadlineMs: { clean: 1500 } });
  const { accounting, stopped } = await b.run();
  assert(b.order.includes("restore"), "restoration still runs");
  const cleanup = b.rec.records.find((r) => r.type === "cleanup");
  assert.equal(cleanup.restorationDurability, "unproven");
  assert.equal(accounting.stopReason, "restoration-durability-unproven");
  assert.equal(accounting.restorationDurabilityUnproven, 1);
  assert.equal(batchExitOk(accounting, stopped), false);
  await b.sink.barrier();
  b.sink.close();
}, 10_000);

test("a batch with no durability dependency takes no barrier", async () => {
  const page = fakePage({ behaviors: [{ kind: "notify", t1: 10n ** 18n }] });
  const recorder = fakeRecorder();
  await runBatch({
    scenarios: [{ id: "s", ops: [{ id: "e", op: "edit-body", article: "a" }] }],
    deps: deps({ page, recorder }),
  });
  assert(!recorder.records.some((r) => r.type === "durability"));
  assert(!existsSync(join(tmpdir(), "never")));
});
