import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { once } from "node:events";
import {
  appendFileSync,
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
import { startFixedCustody } from "../lib/fixed-rss-custody.mjs";
import {
  checkCustodyProofTime,
  checkCustodyTransfer,
  checkFixedV2Observation,
  sampleRendererMemory,
} from "../lib/linux-rss-sample.mjs";
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

linux(
  "v3 proves a positively observed worker reaped between independent sampling helpers",
  async () => {
    const directory = mkdtempSync(join(tmpdir(), "fixed-custody-"));
    const nativeImage = realpathSync("/usr/bin/python3");
    const driver = join(directory, "test-shim.cjs");
    writeFileSync(
      driver,
      `require("node:child_process").spawn(${JSON.stringify(nativeImage)}, process.argv.slice(2), {stdio: "inherit"})`,
    );
    writeFileSync(join(directory, "__zfb-compiler-worker"), "import time; time.sleep(8)\n");
    const nativeArgs = [
      "-u",
      "-c",
      [
        "import os,signal,subprocess,sys",
        "signal.alarm(8)",
        "p=subprocess.Popen([os.path.realpath(sys.executable),'__zfb-compiler-worker'],process_group=0)",
        "try:",
        " print(p.pid,flush=True)",
        " sys.stdin.readline()",
        " p.terminate(); p.wait(); print('reaped',flush=True)",
        " sys.stdin.readline()",
        "finally:",
        " if p.poll() is None: p.kill(); p.wait()",
      ].join("\n"),
    ];
    const rows = [],
      failures = [];
    const receipt = process.env.ZFB_FIXED_CUSTODY_FIXTURE_RECEIPT;
    if (receipt) writeFileSync(receipt, "", { flag: "wx", mode: 0o600 });
    const trace = (type, payload) => {
      if (receipt) appendFileSync(receipt, JSON.stringify({ type, payload }) + "\n", "utf8");
    };
    const custody = startFixedCustody({
      append: (r) => {
        rows.push(r);
        trace("holder", r);
      },
      abort: (e) => {
        failures.push(e.message);
        trace("holder-failure", e.message);
      },
    });
    let root, closed, workerPid, primaryFailure;
    try {
      await custody.ready;
      root = spawn(process.execPath, [driver, ...nativeArgs], {
        cwd: directory,
        detached: true,
        stdio: ["pipe", "pipe", "inherit"],
      });
      trace("spawn", { pid: root.pid, birth: linuxBirth(root.pid) });
      closed = once(root, "close");
      const [ready] = await once(root.stdout, "data");
      workerPid = Number(ready.toString());
      trace("worker-birth", linuxBirth(workerPid));
      const native = { ...linuxBirth(root.pid), argv: [driver, ...nativeArgs] };
      const fixedExecutables = {
        protocol: "linux-fixed-zfb.v3",
        root: executable(process.execPath),
        child: executable(nativeImage),
        launch: { cwd: directory, shim: executable(driver) },
      };
      const server = {
        handle: { native, known: new Map([[native.pid, native]]), onNewOwned() {} },
        isAlive: () => ({ alive: root.exitCode === null }),
      };
      const sample = async () => {
        const result = await sampleRendererMemory(server, fixedExecutables.root.path, {
          fixedExecutables,
          custody,
        });
        trace("sample", result);
        return result;
      };
      const live = await sample();
      const worker = live.processes.find((p) => p.pid === workerPid);
      assert.equal(worker.disposition, "live");
      assert.equal(worker.executable.workerArg, "__zfb-compiler-worker");
      assert.match(worker.custodyToken, /^[0-9a-f]{32}$/);
      const pairVerifier = fileURLToPath(new URL("../lib/linux-rss-custody.py", import.meta.url));
      const pairChecks = execFileSync(
        "python3",
        [
          "-I",
          "-B",
          "-c",
          [
            "import contextlib,json,os,runpy,sys",
            "lib=runpy.run_path(sys.argv[1])",
            "root,parent,worker=json.loads(sys.argv[2])",
            "def check(pid, proc, valid):",
            " fd=os.pidfd_open(pid,0); directory=os.open('/proc/'+str(proc),os.O_RDONLY|os.O_DIRECTORY)",
            " try:",
            "  try: lib['verify_pair']([fd,directory],dict(birth=worker,parent=parent,descriptors=list(map(lib['identity'],[fd,directory]))),root)",
            "  except ValueError as e:",
            "   if valid: raise",
            "   print(str(e))",
            "  else: assert valid",
            " finally: os.close(fd); os.close(directory)",
            "check(worker['pid'],worker['pid'],True)",
            "check(parent['pid'],worker['pid'],False)",
            "check(worker['pid'],parent['pid'],False)",
          ].join("\n"),
          pairVerifier,
          JSON.stringify([native, live.processes.find((p) => p.pid === worker.ppid), worker]),
        ],
        { timeout: 1000, maxBuffer: 4096, encoding: "utf8" },
      );
      trace("fd-negatives", pairChecks);
      assert.deepEqual(pairChecks.trim().split("\n"), [
        "custody unpaired pidfd",
        "custody wrong process directory",
      ]);

      const reaped = once(root.stdout, "data");
      root.stdin.write("reap\n");
      assert.equal((await reaped)[0].toString().trim(), "reaped");
      assert.equal(existsSync(`/proc/${workerPid}`), false);
      const next = await sample();
      const departed = next.processes.find((p) => p.pid === workerPid);
      assert.notEqual(next.helper.pid, live.helper.pid);
      assert.equal(departed.disposition, "custodied-departure");
      assert.equal(departed.rssKiB, null);
      assert.equal(departed.custodyToken, worker.custodyToken);
      assert.equal(departed.terminalProof.birth.startTicks, worker.startTicks);
      assert([1, 16, 17].includes(departed.terminalProof.pollMask));
      const final = await sample();
      assert.equal(
        final.processes.find((p) => p.pid === workerPid).disposition,
        "retained-terminal-proof",
      );
      assert.equal(final.custodyReleases.length, 1);
      assert.equal(final.custodyReleases[0].token, worker.custodyToken);
      assert.equal(final.custodyReleases[0].receipt.helperPid, final.helper.pid);
      assert.deepEqual(failures, []);
    } catch (error) {
      primaryFailure = error;
      trace("primary-failure", { message: error.message, details: error.details });
      throw error;
    } finally {
      if (root) root.stdin.end();
      if (closed) await closed;
      try {
        await custody.stop();
      } catch (error) {
        trace("cleanup-failure", error.message);
        if (!primaryFailure) throw error;
      } finally {
        rmSync(directory, { recursive: true });
      }
    }
    assert.equal(rows.at(-1).stopReport.pairsClosed, 0);
    assert.equal(rows.at(-1).stopReport.epochs, 3);
    assert.equal(existsSync(`/proc/${rows.at(0).birth.pid}`), false);
  },
);

linux(
  "loss of the holder permanently refuses new cycles and carries unverified closure",
  async () => {
    const rows = [];
    const failure = Promise.withResolvers();
    const holder = startFixedCustody({
      append: (r) => rows.push(r),
      abort: (e) => failure.resolve(e),
    });
    await holder.ready;
    const birth = rows.find((r) => r.type === "fixed-custody-ready").birth;
    assert.equal(linuxBirth(birth.pid).startTicks, birth.startTicks);
    process.kill(birth.pid, "SIGKILL");
    assert.match((await failure.promise).message, /holder exited/);
    assert.throws(() => holder.begin(), /holder exited/);
    await assert.rejects(holder.stop(), /incomplete holder shutdown/);
    const last = rows.at(-1);
    assert.equal(last.type, "fixed-custody-stop-failure");
    assert.equal(last.birth.startTicks, birth.startTicks);
    assert.equal(last.exit.signal, "SIGKILL");
    assert.equal(existsSync(`/proc/${birth.pid}`), false);
  },
);

test("a descriptor transfer joins the session, helper epoch, both kernel descriptor IDs and observation time", () => {
  const cycle = { session: "fixture", epoch: 3, holderPid: 11 };
  const helper = { pid: 12 };
  const claim = {
    ...cycle,
    helperPid: 12,
    verifiedNs: "11",
    descriptors: [
      ["61", "303"],
      ["42", "159"],
    ],
  };
  const verify = (r) =>
    checkCustodyTransfer({ custodyReceipt: r, startedNs: "10", endNs: "12" }, cycle, helper);
  verify(claim);
  for (const change of [
    { session: "other" },
    { epoch: 2 },
    { helperPid: 3 },
    { holderPid: 3 },
    { verifiedNs: "9" },
    { verifiedNs: "14" },
    { descriptors: [["61", "303"]] },
    {
      descriptors: [
        ["61", "303"],
        ["?", 2],
      ],
    },
  ]) {
    assert.throws(() => verify({ ...claim, ...change }));
  }
});

test("an inter-cycle terminal poll must belong to this helper's observation interval", () => {
  const row = { startedNs: "20", endNs: "30", terminalProof: { observedNs: "25" } };
  checkCustodyProofTime(row, "18");
  for (const at of ["15", "31", null])
    assert.throws(() => checkCustodyProofTime({ ...row, terminalProof: { observedNs: at } }, "18"));
  assert.throws(() => checkCustodyProofTime(row, "22"));
});

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

test("an observation-only startup milestone never admits edits without a later release", async () => {
  vi.useFakeTimers();
  let cycles = 0;
  const monitor = startFixedRssSafety({
    server: { handle: { native: { pid: 5 } } },
    fixedExecutables: { protocol: "linux-fixed-zfb.v4", root: { path: "/node" } },
    limits: { rssCeilingKiB: 100, sampleEveryMs: 1000 },
    append() {},
    abort() {},
    custody: {
      verifyStartup() {
        if (cycles < 4) throw Error("original pair not released");
      },
    },
    sample: async () => {
      cycles++;
      return {
        summedRssKiB: 10,
        processes: [{ pid: 6, ppid: 5, disposition: "live", startTicks: "6", bootId: "boot" }],
      };
    },
  });
  try {
    const first = monitor.verifyZeroEditObservationReady();
    await vi.advanceTimersByTimeAsync(1000);
    await first;
    assert.equal(cycles, 2);
    await assert.rejects(monitor.verifyReady(), /not released/);
    await vi.advanceTimersByTimeAsync(2000);
    await monitor.verifyReady();
    assert.equal(cycles, 4);
  } finally {
    await monitor.stop();
    vi.useRealTimers();
  }
});
