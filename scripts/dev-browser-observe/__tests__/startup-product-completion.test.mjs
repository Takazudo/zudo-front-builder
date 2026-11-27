import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { afterEach, test, vi } from "vitest";
import {
  checkCompletedTransient,
  checkProductCompletion,
  requireStartupRelease,
  startFixedCustody,
} from "../lib/fixed-rss-custody.mjs";

const BOOT = "8116e36c-eeea-4ca0-973f-b1af4f6144d6";
const fake = vi.hoisted(() => ({ pid: 654321, image: "", spawn: null, births: new Map() }));
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
  linuxBirth: (pid) => fake.births.get(pid) ?? null,
}));

afterEach(() => {
  vi.restoreAllMocks();
});

const birth = (pid, ppid, pgid, startTicks) => ({
  pid,
  ppid,
  pgid,
  bootId: BOOT,
  startTicks: String(startTicks),
  birthVersion: "linux-proc-v1",
});
const root = birth(500, process.pid, 500, 1000);
const native = birth(501, 500, 500, 1002);
const statLine = (b, { state = "Z", exit = "0", ppid = b.ppid, pgid = b.pgid } = {}) => {
  const fields = [state, String(ppid), String(pgid), ...Array(47).fill("0")];
  fields[19] = b.startTicks;
  fields[49] = exit;
  return `${b.pid} (zfb) ` + fields.join(" ");
};
const worker = birth(502, 501, 502, 1003);
const transient = (over = {}) => ({
  ...worker,
  state: "Z",
  comm: "zfb",
  completedTransient: {
    role: "__zfb-compiler-worker",
    exitCode: 0,
    stat: statLine(worker),
    observedNs: "10",
    parent: { pid: native.pid, startTicks: native.startTicks },
  },
  ...over,
});
const completion = (over = {}) => ({
  outcome: "product-completion",
  root,
  native,
  bracket: {
    lower: { kind: "native-birth", startTicks: native.startTicks, nativeCminflt: 0 },
    upper: { sweep: 3, ns: "20", nativeCminflt: 220, children: 0 },
  },
  trace: [{ sweep: 3, ns: "20", nativeCminflt: 220, children: 0 }],
  traceLength: 1,
  reapedMidBind: [],
  observedNs: "21",
  ...over,
});

test("the controller re-derives a completed transient record from its own stat line", () => {
  assert(checkCompletedTransient(transient(), native));
  const t = (patch) => {
    const b = transient();
    patch(b, b.completedTransient);
    return checkCompletedTransient(b, native);
  };
  for (const [label, patch] of [
    ["exit code", (_b, c) => (c.exitCode = 256)],
    ["stat exit", (_b, c) => (c.stat = statLine(worker, { exit: "1" }))],
    ["stat state", (_b, c) => (c.stat = statLine(worker, { state: "S" }))],
    ["birth state", (b) => (b.state = "S")],
    ["role", (_b, c) => (c.role = "dev")],
    ["parent", (_b, c) => (c.parent = { pid: native.pid, startTicks: "999" })],
    ["stat ppid", (_b, c) => (c.stat = statLine(worker, { ppid: 999 }))],
    ["shared group", (b, c) => ((b.pgid = 500), (c.stat = statLine(worker, { pgid: 500 })))],
    [
      "older than zfb",
      (b, c) => ((b.startTicks = "1001"), (c.stat = statLine({ ...worker, startTicks: "1001" }))),
    ],
    ["comm", (b) => (b.comm = "node")],
    ["short stat", (_b, c) => (c.stat = c.stat.split(" ").slice(0, 40).join(" "))],
  ])
    assert.equal(t(patch), false, label);
});

test("a product-completion record needs zfb's birth and a 0 to above-0 cminflt bracket", () => {
  assert(checkProductCompletion(completion(), root));
  assert(
    checkProductCompletion(
      completion({
        bracket: {
          lower: { kind: "observed-zero", sweep: 2, ns: "15", nativeCminflt: 0, children: 0 },
          upper: { sweep: 3, ns: "20", nativeCminflt: 220, children: 0 },
        },
      }),
      root,
    ),
  );
  for (const [label, over] of [
    [
      "no reap",
      { bracket: { ...completion().bracket, upper: { sweep: 3, nativeCminflt: 0, children: 0 } } },
    ],
    [
      "child listed",
      { bracket: { ...completion().bracket, upper: { sweep: 3, nativeCminflt: 9, children: 1 } } },
    ],
    ["foreign root", { root: { ...root, startTicks: "1" } }],
    ["native outside the shim group", { native: { ...native, pgid: 501 } }],
    [
      "unbracketed",
      {
        bracket: {
          ...completion().bracket,
          lower: { kind: "observed-zero", sweep: 4, nativeCminflt: 0, children: 0 },
        },
      },
    ],
    [
      "wrong birth lower bound",
      { bracket: { ...completion().bracket, lower: { kind: "native-birth", startTicks: "1" } } },
    ],
    ["outcome", { outcome: "live-custody" }],
  ])
    assert.equal(checkProductCompletion(completion(over), root), false, label);
});

test("only a confirmed product completion replaces the terminal and release cycles", () => {
  assert.throws(() => requireStartupRelease("worker-not-observed", null, null, completion()));
  requireStartupRelease("worker-not-observed", null, null, {
    ...completion(),
    confirmed: { epoch: 4 },
  });
  assert.throws(() =>
    requireStartupRelease("retained", null, null, { ...completion(), confirmed: { epoch: 4 } }),
  );
  assert.throws(() =>
    requireStartupRelease("worker-not-observed", { t: 1 }, { r: 1 }, { confirmed: { epoch: 4 } }),
  );
});

function holder() {
  const child = Object.assign(new EventEmitter(), {
    pid: fake.pid,
    exitCode: null,
    signalCode: null,
    kill: vi.fn(),
    stdio: Array.from({ length: 4 }, () => new EventEmitter()),
  });
  [child.stdin, child.stdout, child.stderr] = child.stdio;
  child.stdin.write = () => {};
  child.stdin.end = vi.fn();
  fake.spawn = () => child;
  fake.births = new Map([
    [fake.pid, birth(fake.pid, process.pid, 17, 12)],
    [root.pid, root],
  ]);
  const records = [];
  const aborts = [];
  const h = startFixedCustody({
    startup: true,
    append: (r) => records.push(r),
    abort: (e) => aborts.push(e),
  });
  const send = (msg) => child.stdout.emit("data", Buffer.from(JSON.stringify(msg) + "\n"));
  return { h, records, aborts, send };
}

async function readyWithCompletion(record = completion()) {
  const f = holder();
  f.send({
    ready: true,
    pid: fake.pid,
    session: f.h.identity.session,
    timeNamespace: process.getBuiltinModule("node:fs").readlinkSync("/proc/self/ns/time"),
  });
  await f.h.ready;
  f.h.attachStartup({ native: root, known: new Map() }, {});
  f.send({ startupCompletion: record, session: f.h.identity.session, publishedNs: "22" });
  f.send({
    startupReport: {
      status: "worker-not-observed",
      error: null,
      sweeps: 3,
      reads: 30,
      procBytes: 900,
      startNs: "1",
      deadlineNs: "3000000001",
      finishedNs: "23",
    },
    session: f.h.identity.session,
  });
  return f;
}

const sample = (live = [root, native], over = {}) => ({
  strict: true,
  epoch: 4,
  startedNs: "30",
  live,
  ...over,
});

test("a product completion is confirmed at readiness and carried into the family proof", async () => {
  const f = await readyWithCompletion();
  assert.deepEqual(f.aborts, []);
  assert(f.records.some((r) => r.type === "fixed-startup-product-completion"));
  assert.equal(f.h.startupCompletionPending, true);
  assert.throws(() => f.h.familyProof(), /terminal and later release/);
  f.h.confirmStartupCompletion({
    sample: sample(),
    controllerChildren: [500, fake.pid, 900],
    expectedChildren: [500, fake.pid, 900],
  });
  assert.equal(f.h.startupCompletionPending, false);
  const proof = f.h.familyProof();
  assert.deepEqual([proof.worker, proof.terminal, proof.release], [null, null, null]);
  assert.deepEqual(proof.completion.confirmed, { epoch: 4, startedNs: "30" });
  assert.deepEqual(f.h.startupOutcome(), {
    outcome: "product-completion",
    scoutStatus: "worker-not-observed",
    counts: { "live-custody": 0, "zombie-custody": 0, "product-completion": 1 },
  });
});

test.each([
  ["an unknown (adopted) controller child", { children: [500, fake.pid, 4242] }],
  // live.length === 2 with both seed and zfb matched also excludes any zfb child.
  ["a zfb child at readiness", { live: [root, native, birth(503, 501, 503, 1004)] }],
  ["a replaced zfb", { live: [root, { ...native, startTicks: "1009" }] }],
  ["an incomplete census", { strict: false }],
])("the readiness confirmation refuses %s", async (_label, c) => {
  const f = await readyWithCompletion();
  assert.throws(
    () =>
      f.h.confirmStartupCompletion({
        sample: sample(c.live, c.strict === false ? { strict: false } : {}),
        controllerChildren: c.children ?? [500, fake.pid],
        expectedChildren: [500, fake.pid],
      }),
    /product completion unconfirmed/,
  );
  const row = f.records.find((r) => r.type === "fixed-startup-completion-confirmation");
  assert.equal(row.ok, false);
  if (c.children) assert.deepEqual(row.unknownControllerChildren, [4242]);
  assert.throws(() => f.h.familyProof(), /terminal and later release/);
});

test("an invalid product-completion record aborts the custody", async () => {
  const f = await readyWithCompletion(
    completion({
      bracket: { ...completion().bracket, upper: { sweep: 3, nativeCminflt: 0, children: 0 } },
    }),
  );
  assert.match(String(f.aborts[0]?.message), /invalid startup product-completion record/);
});

test.each([
  [
    "a verified completed transient",
    transient(),
    "fixed-startup-transient-custody",
    "zombie-custody",
  ],
  [
    "a live worker",
    { ...worker, state: "S", comm: "zfb" },
    "fixed-startup-live-custody",
    "live-custody",
  ],
  [
    "a forged zombie record",
    transient({ completedTransient: { ...transient().completedTransient, exitCode: 256 } }),
    null,
    null,
  ],
  ["a terminal birth without a record", { ...worker, state: "Z", comm: "zfb" }, null, null],
])("the controller records %s under its own type", async (_label, startupBirth, type, outcome) => {
  const f = holder();
  f.send({
    ready: true,
    pid: fake.pid,
    session: f.h.identity.session,
    timeNamespace: process.getBuiltinModule("node:fs").readlinkSync("/proc/self/ns/time"),
  });
  await f.h.ready;
  f.h.attachStartup({ native: root, known: new Map() }, {});
  f.send({
    startupBirth,
    dev: { ...native, state: "S", comm: "zfb" },
    root: { ...root, state: "S", comm: "node-MainThread" },
    token: "ab".repeat(16),
    descriptors: [
      ["1", "2"],
      ["3", "4"],
    ],
    session: f.h.identity.session,
    liveVerifiedNs: "11",
    acquiredNs: "12",
  });
  if (type) {
    assert.deepEqual(f.aborts, []);
    assert.equal(f.records.at(-1).type, type);
    assert.equal(f.h.startupOutcome().outcome, outcome);
  } else assert.match(String(f.aborts[0]?.message), /invalid startup compiler role/);
});
