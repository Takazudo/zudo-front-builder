import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import {
  existsSync,
  mkdtempSync,
  readlinkSync,
  realpathSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test, vi } from "vitest";
import { linuxBirth } from "../lib/linux-birth.mjs";
import { checkFixedV2Observation, sampleRendererMemory } from "../lib/linux-rss-sample.mjs";
import { startFixedRssSafety } from "../lib/fixed-rss-safety.mjs";
import { runSpecialMode } from "../lib/special-modes.mjs";

const linux = test.skipIf(process.platform !== "linux");
const executable = (path) => {
  const real = realpathSync(path),
    st = statSync(real, { bigint: true });
  return { path: real, device: String(st.dev), inode: String(st.ino) };
};

linux(
  "a live two-image group needs both exact identities and the child's direct parent",
  async () => {
    const root = spawn(
      "python3",
      [
        "-I",
        "-u",
        "-c",
        [
          "import subprocess,sys",
          "p=subprocess.Popen(['/usr/bin/sleep','30'])",
          "try:",
          " print(p.pid,flush=True)",
          " sys.stdin.readline()",
          "finally:",
          " p.terminate();p.wait()",
        ].join("\n"),
      ],
      { detached: true, stdio: ["pipe", "pipe", "inherit"] },
    );
    const closed = once(root, "close");
    let child;
    try {
      const [output] = await once(root.stdout, "data");
      child = Number(output.toString());
      const native = { ...linuxBirth(root.pid), argv: ["test-shim", "30"] };
      const server = {
        handle: { native, known: new Map([[native.pid, native]]), onNewOwned() {} },
        isAlive: () => ({ alive: root.exitCode === null }),
      };
      const identities = {
        root: executable(readlinkSync(`/proc/${root.pid}/exe`)),
        child: executable("/usr/bin/sleep"),
      };
      await assert.rejects(
        sampleRendererMemory(server, identities.root.path),
        (e) => e.details?.helper?.closed && e.details?.helper?.pipesClosed,
      );
      const good = await sampleRendererMemory(server, identities.root.path, {
        fixedExecutables: identities,
      });
      assert.equal(good.processes.length, 2);
      assert.deepEqual(good.processes.find((p) => p.pid === child).executable, identities.child);
      assert.equal(
        good.summedRssKiB,
        good.processes.reduce((n, p) => n + p.rssKiB, 0),
      );
      for (const replacement of [identities.root, { ...identities.child, inode: "0" }]) {
        await assert.rejects(
          sampleRendererMemory(server, identities.root.path, {
            fixedExecutables: { ...identities, child: replacement },
          }),
          (e) => e.details?.helper?.closed && e.details?.helper?.pipesClosed,
        );
      }
    } finally {
      root.stdin.end();
      await closed;
    }
    assert(!existsSync(`/proc/${child}`));
  },
);

linux(
  "a live third process requires the pinned worker image, private argv and native parent",
  async () => {
    const directory = mkdtempSync(join(tmpdir(), "fixed-worker-"));
    const nativeImage = realpathSync("/usr/bin/python3");
    writeFileSync(join(directory, "__zfb-compiler-worker"), "import time; time.sleep(30)\n");
    const nativeArgs = [
      "-u",
      "-c",
      [
        "import os,subprocess,sys",
        "p=subprocess.Popen([os.path.realpath(sys.executable),'__zfb-compiler-worker'],process_group=0)",
        "try:",
        " print(p.pid,flush=True)",
        " sys.stdin.readline()",
        "finally:",
        " p.terminate();p.wait()",
      ].join("\n"),
    ];
    const driver = join(directory, "test-shim.cjs");
    writeFileSync(
      driver,
      `require("node:child_process").spawn(${JSON.stringify(nativeImage)},process.argv.slice(2),{stdio:"inherit"})`,
    );
    const root = spawn(process.execPath, [driver, ...nativeArgs], {
      cwd: directory,
      detached: true,
      stdio: ["pipe", "pipe", "inherit"],
    });
    const closed = once(root, "close");
    try {
      const [output] = await once(root.stdout, "data");
      const workerPid = Number(output.toString());
      const native = { ...linuxBirth(root.pid), argv: [driver, ...nativeArgs] };
      const server = {
        handle: { native, known: new Map([[native.pid, native]]), onNewOwned() {} },
        isAlive: () => ({ alive: root.exitCode === null }),
      };
      const fixedExecutables = {
        protocol: "linux-fixed-zfb.v2",
        root: executable(process.execPath),
        child: executable(nativeImage),
        launch: { cwd: directory, shim: executable(driver) },
      };
      await assert.rejects(
        sampleRendererMemory(server, fixedExecutables.root.path, {
          fixedExecutables: { ...fixedExecutables, protocol: "linux-fixed-zfb.v1" },
        }),
        /unexpected-fixed-process-role/,
      );
      for (const [launch, reason] of [
        [{ cwd: "/", shim: executable(driver) }, /unexpected-fixed-root-cwd/],
        [{ cwd: directory, shim: executable(nativeImage) }, /unexpected-fixed-root-argv/],
        [
          { cwd: directory, shim: { ...executable(driver), inode: "0" } },
          /unexpected-fixed-shim-identity/,
        ],
      ]) {
        await assert.rejects(
          sampleRendererMemory(server, fixedExecutables.root.path, {
            fixedExecutables: { ...fixedExecutables, launch },
          }),
          reason,
        );
      }
      const observed = await sampleRendererMemory(server, fixedExecutables.root.path, {
        fixedExecutables,
      });
      const worker = observed.processes.find((p) => p.pid === workerPid);
      assert.deepEqual(worker.executable, {
        ...fixedExecutables.child,
        workerArg: "__zfb-compiler-worker",
      });
      assert.equal(worker.pgid, worker.pid);
      assert.equal(observed.processes.find((p) => p.pid === worker.ppid).ppid, native.pid);
      assert.equal(
        observed.summedRssKiB,
        observed.processes.reduce((n, p) => n + p.rssKiB, 0),
      );
    } finally {
      root.stdin.end();
      await closed;
      rmSync(directory, { recursive: true });
    }
  },
);

test("v2 terminal observations reconcile their bound parent and any captured image", () => {
  const root = { pid: 3, pgid: 3 },
    binding = { ppid: 4 };
  const fixed = { child: { path: "/dev", device: "1", inode: "234" } };
  const parent = { pid: 4, ppid: 3, disposition: "live", executable: fixed.child };
  const p = {
    pid: 5,
    ppid: 4,
    pgid: 5,
    disposition: "proved-departure",
    terminalProof: { birth: { ppid: 4 } },
    fixedRole: "unknown",
    executable: null,
    rssKiB: null,
    statusKiB: null,
    smapsRollupKiB: null,
    threads: null,
  };
  const verify = (row) => checkFixedV2Observation(row, binding, root, fixed, [parent]);
  verify(p);
  for (const change of [
    { ppid: 3 },
    { terminalProof: { birth: { ppid: 3 } } },
    { fixedRole: "compiler" },
    { rssKiB: 0 },
    { disposition: "live" },
    { fixedRole: undefined, executable: { ...fixed.child, path: "/foreign" } },
    { fixedRole: undefined, executable: { ...fixed.child, workerArg: "wrong" } },
  ])
    assert.throws(() => verify({ ...p, ...change }));
  verify({
    ...p,
    fixedRole: undefined,
    executable: { ...fixed.child, workerArg: "__zfb-compiler-worker" },
    rssKiB: 122,
    statusKiB: { VmRSS: 122 },
  });
});

test("periodic fixed safety failure during hidden observation aborts before navigation", async () => {
  const controller = new AbortController(),
    records = [];
  const server = { handle: { native: { pid: 100 } } };
  let cycles = 0,
    writes = 0,
    navigations = 0;
  const safety = startFixedRssSafety({
    server,
    fixedExecutables: { protocol: "linux-fixed-zfb.v2", root: { path: "/node" } },
    limits: { sampleEveryMs: 10, rssCeilingKiB: 100 },
    append: (row) => records.push(row),
    abort: (reason) => controller.abort(reason),
    sample: async () => ({
      summedRssKiB: ++cycles === 3 ? 101 : 10,
      processes: [
        { pid: 100, disposition: "live" },
        { pid: 101, ppid: 100, disposition: "live", bootId: "boot", startTicks: "12" },
        { pid: 102, ppid: 101, disposition: "live" },
      ],
    }),
  });
  try {
    await safety.verifyReady();
    await assert.rejects(
      runSpecialMode("hidden", {
        attempt: {
          hiddenPath: "/hidden",
          targetPath: "/",
          writes: [{ method: "in-place", path: "mock" }],
          observeMs: 50,
          accessDelayMs: 10,
        },
        page: {
          requests: [],
          mark: () => 0,
          since: () => ({ requests: [], newDocuments: 0 }),
          navigateAndAwait() {
            navigations++;
          },
        },
        fs: {
          performWrite() {
            writes++;
            return 1n;
          },
        },
        clock: { now: () => 1n },
        timeouts: { attemptMs: 150 },
        recBase: {},
        signal: controller.signal,
      }),
      /RSS invalid or over ceiling/,
    );
    assert.equal(writes, 1);
    assert.equal(navigations, 0);
    assert.equal(records.filter((r) => r.type === "fixed-rss-sample").length, 3);
    assert.equal(records.filter((r) => r.type === "fixed-rss-failure").length, 1);
  } finally {
    await safety.stop();
  }
});

test("failed fixed sample preserves conservative partial RSS and unresolved helper", async () => {
  const partial = { summedRssKiB: 4718592, helper: { pid: 12, closed: false, pipesClosed: false } };
  const records = [];
  const safety = startFixedRssSafety({
    server: {},
    fixedExecutables: { root: { path: "/node" } },
    limits: { sampleEveryMs: 1000, rssCeilingKiB: 4194304 },
    append: (r) => records.push(r),
    abort: () => {},
    sample: async () => {
      throw Object.assign(new Error("unverified birth"), { details: partial });
    },
  });
  await assert.rejects(safety.verifyReady(), /unverified birth/);
  assert.equal((await safety.stop()).details, partial);
  assert.equal(safety.unresolvedHelper, partial.helper);
  assert.equal(records[0].failure.details.summedRssKiB, 4718592);
});

test("a readiness observation coalesces with the periodic clock rather than colliding at its boundary", async () => {
  vi.useFakeTimers();
  const root = { pid: 5 },
    child = { pid: 6, ppid: 5, startTicks: "6", bootId: "boot", disposition: "live" };
  const starts = [],
    failures = [];
  let ready = false;
  const monitor = startFixedRssSafety({
    server: { handle: { native: root } },
    fixedExecutables: { root: { path: "/node" } },
    limits: { rssCeilingKiB: 100, sampleEveryMs: 1000 },
    append() {},
    abort: (e) => failures.push(e),
    sample: async () => {
      starts.push(Date.now());
      const nativeVisible = ready;
      await new Promise((resolve) => setTimeout(resolve, 50));
      return {
        processes: [{ pid: 5, disposition: "live" }, ...(nativeVisible ? [child] : [])],
        summedRssKiB: 10,
      };
    },
  });
  try {
    await vi.advanceTimersByTimeAsync(990);
    ready = true;
    const verified = monitor.verifyReady();
    await vi.advanceTimersByTimeAsync(60);
    await verified;
    assert.equal(starts.length, 2);
    assert.equal(starts[1] - starts[0], 1000);
    assert.deepEqual(failures, []);
  } finally {
    await monitor.stop();
    vi.useRealTimers();
  }
});

test("an evidence write failure aborts even when its own failure record cannot be written", async () => {
  const failures = [];
  const monitor = startFixedRssSafety({
    server: {},
    fixedExecutables: { root: { path: "/node" } },
    limits: { rssCeilingKiB: 100, sampleEveryMs: 1000 },
    append() {
      throw new Error("disk write refused");
    },
    abort: (e) => failures.push(e.message),
    sample: async () => ({ summedRssKiB: 24, processes: [] }),
  });
  await assert.rejects(monitor.verifyReady(), /disk write refused/);
  assert.match((await monitor.stop()).receiptError, /disk write refused/);
  assert.deepEqual(failures, ["disk write refused"]);
});

test("a late partial helper failure survives an earlier cadence refusal", async () => {
  vi.useFakeTimers();
  const pending = Promise.withResolvers();
  const messages = [],
    records = [];
  const monitor = startFixedRssSafety({
    server: {},
    fixedExecutables: { root: { path: "/node" } },
    limits: { rssCeilingKiB: 4194304, sampleEveryMs: 1000 },
    append: (r) => records.push(r),
    abort: (e) => messages.push(e.message),
    sample: () => pending.promise,
  });
  try {
    await vi.advanceTimersByTimeAsync(1000);
    assert.match(messages[0], /still occupied/);
    const details = {
      stage: "metadata",
      summedRssKiB: 5000000,
      helper: { pid: 81, closed: false, pipesClosed: false },
    };
    pending.reject(Object.assign(new Error("unverified metadata"), { details }));
    const first = await monitor.stop();
    assert.match(first.error, /still occupied/);
    assert.equal(messages.length, 1);
    assert.equal(monitor.report().peakSummedOwnedRssKiB, 5000000);
    assert.equal(monitor.report().secondaryHelperFailure.details, details);
    assert.deepEqual(
      records.map((r) => r.type),
      ["fixed-rss-failure", "fixed-rss-secondary-helper-failure"],
    );
    assert.deepEqual(monitor.unresolvedHelper, details.helper);
  } finally {
    await monitor.stop();
    vi.useRealTimers();
  }
});

test("readiness rejects the first refused cycle before an independent abort listener begins cleanup", async () => {
  vi.useFakeTimers();
  const failures = [];
  let cycles = 0;
  const monitor = startFixedRssSafety({
    server: { handle: { native: { pid: 5 } } },
    fixedExecutables: { root: { path: "/node" } },
    limits: { rssCeilingKiB: 100, sampleEveryMs: 1000 },
    append() {},
    abort: (e) => failures.push(e.message),
    sample: async () => {
      if (++cycles === 1) return { processes: [{ pid: 5, disposition: "live" }], summedRssKiB: 20 };
      throw new Error("readiness cycle refused");
    },
  });
  try {
    await vi.advanceTimersByTimeAsync(990);
    const rejection = assert.rejects(monitor.verifyReady(), /readiness cycle refused/);
    await vi.advanceTimersByTimeAsync(10);
    await rejection;
    assert.deepEqual(failures, ["readiness cycle refused"]);
  } finally {
    await monitor.stop();
    vi.useRealTimers();
  }
});
