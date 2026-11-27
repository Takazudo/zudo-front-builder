import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { afterEach, test, vi } from "vitest";
import { createRendererMemoryStudy } from "../lib/renderer-memory-study.mjs";
import { recordSamplerClosureAudits } from "../lib/run.mjs";
import * as sampler from "../lib/linux-rss-sample.mjs";
import { startFixedRssSafety } from "../lib/fixed-rss-safety.mjs";

afterEach(() => vi.restoreAllMocks());
function fixture(writeError = false, extra = null, outerCycle = null) {
  let ns = 0n;
  vi.spyOn(process.hrtime, "bigint").mockImplementation(() => ns);
  const tasks = [];
  vi.spyOn(globalThis, "setTimeout").mockImplementation((fn, ms) => {
    const task = { fn, ms };
    tasks.push(task);
    return task;
  });
  vi.spyOn(globalThis, "clearTimeout").mockImplementation((t) => {
    if (t) t.cleared = true;
  });
  const root = {
    pid: 900,
    argv: ["shim", "dev"],
    ppid: 1,
    pgid: 900,
    birthVersion: "linux-proc-v1",
    startTicks: "111",
    bootId: "1edafe52-089e-4f16-a478-1143d29052bd",
  };
  const server = {
    handle: { native: root, known: new Map([[root.pid, root]]) },
    isAlive: () => ({ alive: true }),
  };
  const child = Object.assign(new EventEmitter(), {
    pid: 901,
    exitCode: null,
    signalCode: null,
    kill: vi.fn(() => true),
    unref: vi.fn(),
  });
  for (const k of ["stdin", "stdout", "stderr"])
    child[k] = Object.assign(new EventEmitter(), {
      end() {
        if (writeError) throw Error("request refused");
      },
      destroy: vi.fn(),
    });
  const fd3 = Object.assign(new EventEmitter(), { destroy: vi.fn() });
  child.stdio = [child.stdin, child.stdout, child.stderr];
  if (extra) child.stdio.push(fd3);
  const custodyOptions =
    extra === "borrowed"
      ? {
          custody: { channel: fd3, begin: () => ({ tokens: {} }) },
          fixedExecutables: { protocol: "linux-fixed-zfb.v3", child: {}, launch: {} },
        }
      : extra === "next-borrowed"
        ? { nextCustody: { channel: fd3, begin: () => ({ tokens: {} }) } }
        : {};
  const result = sampler
    .sampleRendererMemory(server, "/unused", { ...custodyOptions, outerCycle, launch: () => child })
    .catch((e) => e);
  return {
    result,
    fd3,
    child,
    server,
    at: (n) => {
      ns = BigInt(n);
    },
    timer: () => {
      const t = tasks.shift();
      assert.ok(t && !t.cleared);
      t.fn();
    },
    events(missing) {
      if (missing !== "exit") child.emit("exit", null, "SIGKILL");
      for (const k of ["stdin", "stdout", "stderr"]) if (missing !== k) child[k].emit("close");
      if (missing !== "child") child.emit("close", null, "SIGKILL");
    },
  };
}
test("an early reserve timer leaves the final nanosecond for complete closure", async () => {
  const x = fixture();
  x.at(750000000);
  x.timer();
  x.at(949999998);
  x.timer();
  x.at(949999999);
  x.events();
  const { details: d } = await x.result;
  assert.equal(d.stage, "deadline");
  assert.equal(d.helper.closed, true);
  assert.equal(d.helper.pipesClosed, true);
  assert.equal(d.timing.parent.resultNs, "949999999");
});
test.each([949999999, 950000000, 950000001])(
  "closure decision at %s compares actual callbacks with the reserve",
  async (n) => {
    const x = fixture();
    x.at(750000000);
    x.timer();
    x.at(n);
    x.events();
    const { details: d } = await x.result;
    assert.equal(d.stage, "deadline");
    assert.equal(d.helper.closed, n <= 950000000);
    assert.equal(d.helper.pipesClosed, n <= 950000000);
    if (n > 950000000) assert.equal(sampler.samplerClosureAudit(d.helper).complete, true);
  },
);
test.each(["stdin", "stdout", "stderr", "exit", "child"])(
  "missing %s at cutoff stays unresolved despite later closure",
  async (missing) => {
    const x = fixture();
    x.at(750000000);
    x.timer();
    x.at(800000000);
    x.events(missing);
    x.at(950000000);
    x.timer();
    const { details: d } = await x.result;
    assert.equal(d.stage, "deadline");
    assert.equal(d.helper.pipesClosed, ["exit", "child"].includes(missing));
    assert.equal(d.helper.closed, !["exit", "child"].includes(missing));
    const frozen = JSON.stringify(d);
    assert.equal(sampler.samplerClosureAudit(d.helper).complete, false);
    x.at(980000000);
    if (missing === "exit" || missing === "child")
      x.child.emit(missing === "exit" ? "exit" : "close", null, "SIGKILL");
    else x.child[missing].emit("close");
    assert.equal(sampler.samplerClosureAudit(d.helper).complete, true);
    assert.equal(JSON.stringify(d), frozen);
    const safety = startFixedRssSafety({
      server: x.server,
      fixedExecutables: { root: { path: "/unused" } },
      limits: { sampleEveryMs: 1000, rssCeilingKiB: 4194304 },
      sample: async () => {
        throw Object.assign(Error(d.error), { details: d });
      },
      append() {},
      abort() {},
    });
    await safety.stop();
    assert.equal(safety.unresolvedHelper, d.helper);
  },
);
test("synchronous request failure cancels the original child and preserves the first error", async () => {
  const x = fixture(true);
  x.at(1000000);
  x.events();
  const error = await x.result;
  assert.equal(error.details?.stage, "pipe");
  assert.match(error.message, /request refused/);
  assert.equal(x.child.kill.mock.calls.length, 1);
});

test.each(["owned", "borrowed", "next-borrowed"])(
  "only newly created helper pipes incur closure obligations (%s)",
  async (kind) => {
    const f = fixture(false, kind);
    f.at(750000000);
    f.timer();
    f.at(810000000);
    f.events();
    if (kind === "owned") {
      f.at(950000000);
      f.timer();
    }
    const { details: d } = await f.result;
    assert.equal(d.stage, "deadline");
    assert.equal(d.helper.closed, true);
    assert.equal(d.helper.pipesClosed, kind !== "owned");
    assert.equal(f.fd3.destroy.mock.calls.length, kind === "owned" ? 1 : 0);
    if (kind === "owned") {
      f.at(960000000);
      f.fd3.emit("close");
      assert.equal(sampler.samplerClosureAudit(d.helper).events.fd3CloseNs, "960000000");
    }
  },
);
test("renderer memory retains the unclosed pipe after an independently observed late closure", async () => {
  const f = fixture();
  f.at(750000000);
  f.timer();
  f.at(760000000);
  f.events("stdin");
  f.at(950000000);
  f.timer();
  const error = await f.result;
  const study = createRendererMemoryStudy({
    append() {},
    supervisorStartedMs: 0,
    workDeadline: 240000,
    clock: () => 1000000000n,
    sample: async () => {
      throw error;
    },
  });
  try {
    await study.startSampling(f.server);
    f.at(1000000000);
    f.child.stdin.emit("close");
    assert.equal(study.unresolvedHelper, error.details.helper);
    assert.equal(study.unresolvedHelper.pipesClosed, false);
    assert.equal(sampler.samplerClosureAudit(study.unresolvedHelper).complete, true);
  } finally {
    study.finishSampling();
  }
});

test("delayed core cancellation starts exactly one reserve at the actual cancellation", async () => {
  const f = fixture();
  f.at(810000000);
  f.timer();
  f.child.stderr.emit("error", Error("secondary pipe fault"));
  f.at(1009999999);
  f.events();
  const { details: d } = await f.result;
  assert.equal(d.stage, "deadline");
  assert.equal(d.timing.parent.deadlineNs, "750000000");
  assert.equal(d.timing.parent.deadlineDispatchNs, "810000000");
  assert.equal(d.timing.parent.closureDeadlineNs, "1010000000");
  assert.equal(d.timing.parent.cancellationNs, "810000000");
  assert.equal(d.timing.parent.cancellationAfterCoreDeadlineNs, "60000000");
  assert.equal(d.helper.closed, true);
  assert.equal(d.helper.pipesClosed, true);
  assert.equal(f.child.kill.mock.calls.length, 1);
});

test("failed signal delivery cannot substitute for exit or pipe closure", async () => {
  const f = fixture();
  f.child.kill.mockReturnValue(false);
  f.at(750000000);
  f.timer();
  f.at(950000000);
  f.timer();
  const { details: d } = await f.result;
  assert.equal(d.stage, "deadline");
  assert.equal(d.timing.parent.killAccepted, false);
  assert.equal(d.helper.closed, false);
  assert.equal(d.helper.pipesClosed, false);
  assert.equal(sampler.samplerClosureAudit(d.helper).complete, false);
});

test("an owned auxiliary pipe error cancels once and keeps its closure obligation", async () => {
  const f = fixture(false, "owned");
  f.at(1000000);
  assert.doesNotThrow(() => f.fd3.emit("error", Error("owned fd3 EPIPE")));
  f.fd3.emit("error", Error("another fd3 error"));
  f.at(2000000);
  f.events();
  f.at(201000000);
  f.timer();
  f.timer();
  const { details: d } = await f.result;
  assert.equal(d.stage, "pipe");
  assert.equal(d.error, "owned fd3 EPIPE");
  assert.equal(f.child.kill.mock.calls.length, 1);
  assert.equal(d.helper.pipesClosed, false);
  f.at(202000000);
  f.fd3.emit("close");
  assert.equal(sampler.samplerClosureAudit(d.helper).complete, true);
});

test.each(["fixed-rss-sampler", "renderer-memory-sampler"])(
  "bounded %s stop retains evidence that becomes available by finalization",
  async (resource) => {
    const f = fixture();
    f.at(750000000);
    f.timer();
    f.at(850000000);
    f.events("stdin");
    f.at(950000000);
    f.timer();
    const { details } = await f.result;
    const monitor = { unresolvedHelper: null },
      holds = [{ resource, error: "original bounded stop timeout" }],
      audit = [];
    const args = resource === "fixed-rss-sampler" ? [monitor, null] : [null, monitor];
    recordSamplerClosureAudits(holds, ...args, (r) => audit.push(r));
    assert.equal(audit.length, 0);
    monitor.unresolvedHelper = details.helper;
    f.at(960000000);
    f.child.stdin.emit("close");
    recordSamplerClosureAudits(holds, ...args, (r) => audit.push(r));
    assert.equal(holds.length, 1);
    assert.equal(holds[0].error, "original bounded stop timeout");
    assert.equal(holds[0].helper, details.helper);
    assert.equal(holds[0].helper.pipesClosed, false);
    assert.equal(holds[0].closureAudit.complete, true);
    assert.equal(audit.length, 1);
  },
);

test.each([true, false])(
  "pending outer identity only reflects a validated original helper frame: %s",
  async (valid) => {
    const cycle = { helperBirth: null },
      f = fixture(false, null, cycle);
    assert.equal(cycle.helperBirth, null);
    const root = f.server.handle.native;
    const childBirth = {
      ...root,
      pid: f.child.pid,
      ppid: valid ? process.pid : process.pid + 1,
      pgid: f.child.pid,
      state: "S",
      comm: "python3",
    };
    const frames = [
      {
        type: "timing",
        version: "linux-rss-timing.v1",
        stage: "request",
        seq: 0,
        ns: ["0", "0", "0", "0"],
        clock: {
          implementation: "clock_gettime(CLOCK_MONOTONIC)",
          bootId: root.bootId,
          timeNamespace: null,
        },
      },
      { type: "helper", birth: childBirth },
    ];
    f.child.stdout.emit("data", Buffer.from(frames.map((r) => JSON.stringify(r) + "\n").join("")));
    if (valid) {
      assert.equal(cycle.helperBirth.pid, f.child.pid);
      assert.equal(cycle.helperBirth.ppid, process.pid);
    } else assert.equal(cycle.helperBirth, null);
    f.events();
    await f.result;
  },
);
