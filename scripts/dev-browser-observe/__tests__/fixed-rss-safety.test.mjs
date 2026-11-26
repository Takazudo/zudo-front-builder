import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { existsSync, readlinkSync, realpathSync, statSync } from "node:fs";
import { test, vi } from "vitest";
import { linuxBirth } from "../lib/linux-birth.mjs";
import { sampleRendererMemory } from "../lib/linux-rss-sample.mjs";
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
      const native = linuxBirth(root.pid);
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

test("periodic fixed safety failure during hidden observation aborts before navigation", async () => {
  const controller = new AbortController(),
    records = [];
  const server = { handle: { native: { pid: 100 } } };
  let cycles = 0,
    writes = 0,
    navigations = 0;
  const safety = startFixedRssSafety({
    server,
    fixedExecutables: { root: { path: "/node" } },
    limits: { sampleEveryMs: 10, rssCeilingKiB: 100 },
    append: (row) => records.push(row),
    abort: (reason) => controller.abort(reason),
    sample: async () => ({
      summedRssKiB: ++cycles === 3 ? 101 : 10,
      processes: [
        { pid: 100, disposition: "live" },
        { pid: 101, disposition: "live", bootId: "boot", startTicks: "12" },
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
    child = { pid: 6, startTicks: "6", bootId: "boot", disposition: "live" };
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
