import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { afterEach, test, vi } from "vitest";
import { startFixedCustody } from "../lib/fixed-rss-custody.mjs";

const fake = vi.hoisted(() => ({ pid: 654321, image: "", spawn: null }));
vi.mock("node:child_process", async (load) => ({
  ...(await load()),
  spawn: (image, ...args) => {
    fake.image = image;
    return fake.spawn(...args);
  },
}));
vi.mock("node:fs", async (load) => {
  const fs = await load();
  return {
    ...fs,
    readlinkSync: (p) => (p === `/proc/${fake.pid}/exe` ? fake.image : fs.readlinkSync(p)),
  };
});
vi.mock("../lib/linux-birth.mjs", async (load) => ({
  ...(await load()),
  linuxBirth: () => ({
    pid: fake.pid,
    ppid: process.pid,
    pgid: 17,
    bootId: "fake",
    startTicks: "12",
    birthVersion: "linux-proc-v1",
  }),
}));

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

function setup({
  spawnMs = 0,
  spawnJournalMs = 0,
  readyJournalMs = 0,
  failOn = "",
  startup = true,
} = {}) {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  let ms = 0;
  vi.spyOn(process.hrtime, "bigint").mockImplementation(() => BigInt(ms) * 1000000n);
  const child = Object.assign(new EventEmitter(), {
    pid: fake.pid,
    exitCode: null,
    signalCode: null,
    kill: vi.fn(),
    stdio: Array.from({ length: 4 }, () => new EventEmitter()),
  });
  [child.stdin, child.stdout, child.stderr] = child.stdio;
  child.stdin.end = vi.fn();
  child.stdio[3].destroy = vi.fn();
  fake.spawn = () => {
    ms += spawnMs;
    return child;
  };
  const records = [],
    aborts = [],
    writes = [],
    intents = [];
  child.stdin.write = (data) => {
    if (failOn === "greeting") throw Error("greeting failed");
    writes.push({ ms, data });
  };
  const holder = startFixedCustody({
    startup,
    intent: {
      update: (row) => {
        intents.push(row);
        if (failOn === "intent:" + row.phase) throw Error("intent failed");
      },
    },
    append: (row) => {
      records.push(row);
      if (row.type === "fixed-custody-spawn") ms += spawnJournalMs;
      if (row.type === "fixed-custody-ready") ms += readyJournalMs;
      if (failOn === row.type) throw Error("record failed");
    },
    abort: (e) => aborts.push(e),
  });
  const reply = () =>
    child.stdout.emit(
      "data",
      Buffer.from(
        JSON.stringify({
          ready: true,
          pid: fake.pid,
          session: holder.identity.session,
          timeNamespace: process.getBuiltinModule("node:fs").readlinkSync("/proc/self/ns/time"),
        }) + "\n",
      ),
    );
  return {
    child,
    holder,
    records,
    aborts,
    writes,
    intents,
    reply,
    time: (n) => {
      ms = n;
    },
  };
}

test.each([true, false])(
  "greeting overlaps the durable spawn journal with startup=%s",
  async (startup) => {
    const f = setup({ spawnJournalMs: 700, startup });
    assert.equal(f.writes.length, 1);
    assert.equal(f.writes[0].ms, 0);
    assert.throws(() => f.holder.begin({}), /unavailable/);
    if (startup) assert.throws(() => f.holder.attachStartup({}, {}), /invalid startup attachment/);
    f.reply();
    await f.holder.ready;
    assert.deepEqual(
      f.records.map((r) => r.type),
      ["fixed-custody-spawn", "fixed-custody-ready"],
    );
    assert.deepEqual(
      f.intents.map((r) => r.phase),
      ["fixed-custody-spawn", "fixed-custody-ready"],
    );
    assert.equal(f.holder.begin({}).epoch, 1);
  },
);

test.each([750, 751])("late ready at %d ms refuses even when the timer has not run", async (ms) => {
  const f = setup();
  f.time(ms);
  f.reply();
  await assert.rejects(f.holder.ready, /readiness timeout/);
  assert.equal(f.child.kill.mock.calls.length, 1);
  assert.equal(f.records.length, 1);
});

test("a late callback after the original timeout cannot append readiness", async () => {
  const f = setup();
  f.time(750);
  vi.advanceTimersByTime(750);
  await assert.rejects(f.holder.ready, /readiness timeout/);
  f.reply();
  assert.equal(f.records.length, 1);
  assert.equal(f.aborts.length, 1);
  assert.throws(() => f.holder.begin({}), /readiness timeout/);
  const stopping = f.holder.stop();
  f.child.emit("exit", null, "SIGKILL");
  f.child.emit("close", null, "SIGKILL");
  await assert.rejects(stopping, /incomplete holder shutdown/);
  const stop = f.records.find((r) => r.type === "fixed-custody-stop");
  assert.equal(stop.fault, f.aborts[0].message);
});

test("a ready journal crossing the deadline cannot release the holder", async () => {
  const f = setup({ readyJournalMs: 100 });
  f.time(650);
  f.reply();
  await assert.rejects(f.holder.ready, /readiness timeout/);
  assert.throws(() => f.holder.begin({}), /readiness timeout/);
});

test.each([
  "fixed-custody-spawn",
  "fixed-custody-ready",
  "intent:fixed-custody-spawn",
  "intent:fixed-custody-ready",
])("durability failure at %s preserves the first failure", async (failOn) => {
  const f = setup({ failOn });
  f.reply();
  await assert.rejects(f.holder.ready, /failed/);
  const count = f.records.length;
  f.reply();
  assert.equal(f.records.length, count);
  assert.equal(f.aborts.length, 1);
  assert.equal(f.child.kill.mock.calls.length, 1);
});

test("spawn and durable spawn journal share the original 750 ms", async () => {
  const f = setup({ spawnMs: 460, spawnJournalMs: 290 });
  await assert.rejects(f.holder.ready, /readiness timeout/);
  assert.equal(f.child.kill.mock.calls.length, 1);
  f.reply();
  assert.equal(f.records.length, 1);
}, 1000);

test("an early timer wake retains the original cutoff", async () => {
  const f = setup();
  f.time(749);
  vi.advanceTimersByTime(750);
  f.reply();
  await f.holder.ready;
  assert.equal(f.child.kill.mock.calls.length, 0);
});

test("a failed greeting still records the spawned birth before cleanup", async () => {
  const f = setup({ failOn: "greeting" });
  await assert.rejects(f.holder.ready, /greeting failed/);
  assert.deepEqual(
    f.records.map((r) => r.type),
    ["fixed-custody-spawn"],
  );
  assert.equal(f.records[0].birth.pid, f.child.pid);
  assert.equal(f.intents[0].fixedCustody.pid, f.child.pid);
  f.reply();
  assert.equal(f.aborts.length, 1);
  assert.equal(f.child.kill.mock.calls.length, 1);
});
