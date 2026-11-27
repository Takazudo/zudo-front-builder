import assert from "node:assert/strict";
import { processExecutable, processIdentityChecks } from "../lib/distribution.mjs";
import * as peerRoles from "../lib/fixed-peer-roles.mjs";
import { peerProcessIdentity, readPeerExecutable } from "../lib/fixed-peer-roles.mjs";
import { realpathSync, statSync } from "node:fs";
import { ObserverPage } from "../lib/browser.mjs";
import { sha256 } from "../lib/util.mjs";
import { peerArgv } from "../lib/fixed-peer-roles.mjs";
import { deps, fakeFs, fakePage } from "./fakes.mjs";
import { runBatch } from "../lib/state-machine.mjs";
import { readFileSync } from "node:fs";
import { profileSchemaFor, semanticErrors } from "../lib/profile.mjs";
import { validateAs } from "../lib/schema.mjs";
import { test, vi } from "vitest";
import { identifyPeerRoles as identify } from "../lib/fixed-peer-roles.mjs";
import { startFixedRssSafety } from "../lib/fixed-rss-safety.mjs";
test("only known Astro and Next readiness chains qualify", () => {
  const cfg = { rootPid: 50000, appDir: "/fresh/app", node: "/pinned/node", port: 45000 };
  const proc = (pid, ppid, argv, extra = {}) => ({ pid, ppid, argv, ...extra });
  const astro = proc(50000, 30, [
    cfg.node,
    "/fresh/app/node_modules/astro/bin/astro.mjs",
    "dev",
    "--ignore-lock",
    "--host",
    "127.0.0.1",
    "--port",
    "45000",
  ]);
  const n = proc(50000, 30, [
    cfg.node,
    "/fresh/app/scripts/dev-content.mjs",
    "--hostname",
    "127.0.0.1",
    "--port",
    "45000",
  ]);
  const c = proc(50001, 50000, [
    cfg.node,
    "/fresh/app/node_modules/next/dist/bin/next",
    "dev",
    "--hostname",
    "127.0.0.1",
    "--port",
    "45000",
  ]);
  const s = proc(50002, 50001, ["next-server (v16.4.6)"], { privateWorker: true, devServer: true });
  const a = { ...cfg, framework: "astro" },
    b = { ...cfg, framework: "next" };
  assert.deepEqual(
    identify([astro], a, true).map((x) => x.role),
    ["astro-cli"],
  );
  assert.deepEqual(
    identify([n, c, s], b, true).map((x) => x.role),
    ["content-sidecar", "next-cli", "next-server"],
  );
  assert.equal(
    identify(
      [
        n,
        c,
        { ...s, argv: [cfg.node, "/fresh/app/node_modules/next/dist/server/lib/start-server.js"] },
      ],
      b,
      true,
    ).at(-1).retitled,
    false,
  );
  assert.doesNotThrow(() => identify([n], b, false));
  assert.doesNotThrow(() => identify([n, c], b, false));
  for (const [rows, testCfg] of [
    [[n, c], b],
    [[n, c, { ...s, argv: ["next-server (v15.0.0)"] }], b],
    [[n, c, { ...s, privateWorker: false }], b],
    [[n, c, { ...s, devServer: false }], b],
    [[n, c, { ...s, ppid: 50000 }], b],
    [[n, c, { ...s, ppid: 999 }], b],
    [[n, c, s, { ...s, pid: 50003 }], b],
    [[n, c, { ...s, argv: [cfg.node, "/other/start-server.js"] }], b],
    [[{ ...n, argv: [cfg.node, "/unrelated"] }, c, s], b],
    [[astro, proc(50001, 50000, ["foreign"])], a],
    [[{ ...astro, argv: [...astro.argv.slice(0, -1), "45002"] }], a],
  ])
    assert.throws(() => identify(rows, testCfg, true));
});

test("peer readiness requires two new cycles and a later role loss aborts safety", async () => {
  vi.useFakeTimers({
    toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "hrtime"],
  });
  const cycles = [],
    failures = [];
  let missing = false;
  const sample = vi.fn(async () => {
    const ns = process.hrtime.bigint();
    return {
      summedRssKiB: 4,
      processes: [{ statusKiB: { VmRSS: 3 }, smapsRollupKiB: { Rss: 4 } }],
      helper: { spawnHrtimeNs: String(ns), closed: true, pipesClosed: true },
      timing: {
        clock: { qualified: true },
        parent: { entryNs: String(ns), deadlineNs: String(ns + 750000000n), resultNs: String(ns) },
      },
    };
  });
  const monitor = startFixedRssSafety({
    server: {},
    fixedExecutables: { protocol: "linux-fixed-peer.v1", root: { path: "/pinned/node" } },
    peer: {},
    limits: { rssCeilingKiB: 4194304, sampleEveryMs: 1000 },
    sample,
    append: (r) => cycles.push(r),
    abort: (e) => failures.push(e),
    checkRoles: () => ({
      chain: missing
        ? []
        : [{ role: "astro-cli", pid: 12, ppid: 3, pgid: 12, bootId: "boot", startTicks: "2" }],
    }),
  });
  try {
    let admitted = false;
    const ready = monitor.verifyReady(process.hrtime.bigint()).then(() => {
      admitted = true;
    });
    await vi.advanceTimersByTimeAsync(1000);
    assert.equal(admitted, false);
    await vi.advanceTimersByTimeAsync(1000);
    await ready;
    assert.equal(cycles.filter((r) => r.type === "fixed-peer-post-accepted").length, 2);
    assert(sample.mock.calls.every((args) => args.length === 2));
    missing = true;
    await vi.advanceTimersByTimeAsync(1000);
    assert.match(failures[0].message, /disappeared/);
  } finally {
    await monitor.stop();
    vi.useRealTimers();
  }
});

test("a scout-first missing worker needs official terminal proof even before registry publication", async () => {
  const worker = { pid: 42, startTicks: "100" };
  const custody = {
    qualified: worker,
    isWorker: (p) => p.pid === worker.pid,
    terminalVerified: false,
  };
  const ancestors = [{ pid: 1 }, { pid: 2 }, { pid: 3 }];
  const checks = ancestors.map((p) => ({ ...p, ok: true }));
  const check = vi.spyOn(peerRoles, "peerProcessIdentity").mockReturnValue(checks);
  let finish;
  const pending = new Promise((resolve) => {
    finish = resolve;
  });
  const monitor = startFixedRssSafety({
    server: {},
    fixedExecutables: { root: { path: "/node" } },
    peer: {},
    limits: { rssCeilingKiB: 4194304, sampleEveryMs: 1000 },
    nextOwner: () => custody,
    sample: () => pending,
    append() {},
    abort() {},
  });
  try {
    for (const known of [ancestors, [...ancestors, worker]]) {
      assert.throws(
        () => monitor.verifyPeerProcesses(ancestors, known),
        /missing before official terminal decision/,
      );
    }
    assert.equal(check.mock.calls.length, 0);
    custody.terminalVerified = true;
    assert.equal(monitor.verifyPeerProcesses(ancestors, ancestors), checks);
    assert.equal(check.mock.calls.length, 1);
  } finally {
    finish({ processes: [], summedRssKiB: null });
    await monitor.stop();
    check.mockRestore();
  }
});

test("the fixed peer mode admits only one archived 100-article body classification", () => {
  for (const framework of ["astro", "next"]) {
    const p = JSON.parse(
      readFileSync(
        new URL(
          "../profiles/field-notes-fixed-" + framework + "-100.example.json",
          import.meta.url,
        ),
      ),
    );
    delete p.calibration;
    Object.assign(p, {
      fixedRssSafety: "linux-fixed-peer.v1",
      fixedPeerBrowser: {
        executablePath: "/pinned/browser",
        executableSha256: "1".repeat(64),
        defaultExecutableSha256: "2".repeat(64),
      },
    });
    p.server.readyTimeoutMs = 300000;
    p.limits.wallCeilingMs = 7200000;
    Object.assign(p.timeouts, { attemptMs: 60000, supplementalMs: 60000, restoreMs: 120000 });
    p.scenarios = [
      {
        id: "fn-edit-body",
        set: "workload",
        warmOrder: "none",
        ops: [
          {
            id: "body-05",
            op: "edit-body",
            article: "article-00005",
            writeMethod: "in-place",
            expect: "classify",
          },
        ],
      },
    ];
    const errors = (q) => [...validateAs(profileSchemaFor(q), q), ...semanticErrors(q)];
    assert.deepEqual(errors(p), []);
    const v4 = structuredClone(p);
    v4.fixedRssSafety = "linux-fixed-peer.v4";
    assert(errors(v4).length, "missing independent provider");
    v4.fixedNextOuterObserver = {
      provider: "linux-next-outer-receipt.v1",
      pythonPath: "/fixed/python",
      pythonSha256: "a".repeat(64),
      scriptPath: "/fixed/wrapper.py",
      scriptSha256: "b".repeat(64),
      cwd: "/study",
      operation: "decision-only",
    };
    if (framework === "next") assert.deepEqual(errors(v4), []);
    else assert(errors(v4).length);
    v4.fixedRssSafety = "linux-fixed-peer.v3";
    assert(errors(v4).length, "legacy profile must not accept the v4 provider");
    for (const change of [
      (q) => (q.scenarios[0].ops[0].repeat = { warmups: 0, measured: 2 }),
      (q) => (q.scenarios[0].ops[0].article = "article-00004"),
      (q) => (q.fixture.scale = 1000),
      (q) => (q.framework = "zfb"),
      (q) => delete q.fixedPeerBrowser,
      (q) => (q.server.readyTimeoutMs = 180000),
      (q) => delete q.fixedRssSafety,
    ]) {
      const q = structuredClone(p);
      change(q);
      assert(errors(q).length);
    }
  }
});

test("only the exact designated Next server title can contain NUL padding", () => {
  assert.deepEqual(peerArgv("node\0cli\0\0", false), ["node", "cli", ""]);
  assert.deepEqual(peerArgv("next-server (v16.4.6)\0\0", true), ["next-server (v16.4.6)"]);
  assert.deepEqual(peerArgv("next-server (v16.4.6)\0\0", false), ["next-server (v16.4.6)", ""]);
  assert.deepEqual(peerArgv("next-server (v16.4.6)\0x\0", true), ["next-server (v16.4.6)", "x"]);
  assert.throws(() => peerArgv("node", false));
});

test.each([
  "success",
  "no-socket",
  "timeout",
  "stale",
  "late-notify",
  "late-document",
  "late-frame",
  "same-detail",
])(
  "peer body gate records %s without admitting a missing socket, late edit or reused document",
  async (outcome) => {
    const connected = outcome !== "no-socket";
    let ns = 1000000000n,
      deadline;
    const clock = { now: () => ns };
    const fs = fakeFs(),
      page = fakePage({
        onWait: (token, _late, p) => {
          ns = deadline;
          p.evidence.docInits = [];
          p.evidence.wsFrames = [];
          p.evidence.navigations = [];
          if (outcome === "late-frame") {
            p.evidence.wsFrames.push({ event: "rsc", ns: String(ns + 1n) });
            p.evidence.livereloadEvents = ["rsc"];
          }
          if (["timeout", "stale", "late-document"].includes(outcome)) {
            if (outcome !== "timeout")
              p.evidence.docInits.push({ ns: String(outcome === "stale" ? ns : ns + 1n) });
            p.evidence.newDocuments = p.evidence.docInits.length;
            return Promise.resolve({ kind: "timeout" });
          }
          if (outcome === "late-notify") ns += 1n;
          return Promise.resolve({ kind: "notify", t1: ns, payload: { token, docUUID: "doc-1" } });
        },
        liveCheck: () => ({
          ok: true,
          docUUID: outcome === "same-detail" ? "doc-1" : "fresh-detail",
        }),
      });
    page.peerSocketTracking = new Map(connected ? [[1, { firstFrameNs: "10" }]] : []);
    const oldArm = page.arm;
    page.arm = async (...args) => ({ ...(await oldArm(...args)), actual: "v0" });
    const write = fs.performWrite;
    fs.performWrite = (w, now) => {
      w.evidence = { inodeBefore: 13, inodeAfter: 13 };
      return write(w, now);
    };
    fs.hashOf = (path) => sha256(fs.files[path]);
    const oldRestore = fs.restore;
    fs.restore = (snapshot) => ({
      ...oldRestore(snapshot),
      actions: fs.writes.length
        ? [{ path: "a.md", method: "in-place", evidence: { inodeBefore: 13, inodeAfter: 13 } }]
        : [],
    });
    page.settle = async () => ({
      settled: true,
      nativeUpdateObserved: true,
      bodyRevision: { matched: true, docUUID: "doc-1" },
    });
    const d = {
      ...deps({ fs, page, clock }),
      workDeadlineNs: 100000000000n,
      verifyBodyRevision: true,
      fixedPeer: true,
      stageBudget: (max, start) => {
        assert.equal(typeof start, "bigint");
        const left = max - Number(ns - start) / 1000000;
        assert(left > 0, "stage exhausted");
        deadline = start + BigInt(max) * 1000000n;
        return left;
      },
    };
    const { accounting } = await runBatch({
      deps: d,
      scenarios: [
        {
          id: "fn-edit-body",
          ops: [{ id: "body-05", op: "edit-body", article: "a", expect: "classify" }],
        },
      ],
    });
    assert.equal(fs.writes.length, connected ? 1 : 0);
    assert.equal(
      accounting.success,
      ["success", "late-frame", "same-detail"].includes(outcome) ? 1 : 0,
    );
    if (connected) assert.equal(accounting.restorationFailures, outcome === "same-detail" ? 1 : 0);
    if (["timeout", "late-document", "late-notify"].includes(outcome))
      assert.equal(accounting.timeout, 1);
    if (outcome === "stale") assert.equal(accounting.stale, 1);
    if (outcome === "late-frame")
      assert.equal(d.recorder.records.find((r) => r.mechanism).mechanism, "in-document-update");
  },
);

test("the readiness deadline originates at the printed event, including controller delay", async () => {
  vi.useFakeTimers({
    toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "hrtime"],
  });
  const sample = Promise.withResolvers(),
    failures = [];
  const monitor = startFixedRssSafety({
    server: {},
    fixedExecutables: { root: { path: "/node" } },
    peer: {},
    sample: () => sample.promise,
    append: () => {},
    abort: (e) => failures.push(e),
    limits: { sampleEveryMs: 1000 },
  });
  try {
    await assert.rejects(
      monitor.verifyReady(process.hrtime.bigint() - 3000000001n),
      /window expired/,
    );
    assert.equal(failures.length, 1);
  } finally {
    sample.reject(new Error("controlled pending helper settlement"));
    await monitor.stop();
    vi.useRealTimers();
  }
});

test.each(["module", "rsc", "activity"])(
  "peer restoration retains update evidence for same-document %s",
  async (event) => {
    vi.useFakeTimers();
    try {
      const clock = () => BigInt(Date.now()) * 1000000n,
        observer = new ObserverPage(null, "", clock);
      observer.baseUrl = "http://127.0.0.1:1234";
      const path = "/articles/a/",
        url = observer.url(path);
      observer.docInits.push({ docUUID: "old", url, ns: String(clock()) });
      const mark = observer.mark();
      observer.page = {
        evaluate: async () => ({ docUUID: "old", url, loading: false, matched: true }),
      };
      const pending = observer.settle({
        mark,
        startNs: clock(),
        quietMs: 2,
        boundMs: 12,
        stepMs: 1,
        requireNativeUpdate: true,
        bodyRevision: {
          allowSameDocument: true,
          path,
          predicate: { kind: "text", equals: "original" },
        },
      });
      observer.wsFrames.push({ event, ns: String(clock()) });
      await vi.advanceTimersByTimeAsync(12);
      const result = await pending;
      assert.equal(result.settled, event !== "activity");
      assert.equal(result.nativeUpdateObserved, event !== "activity");
      assert.equal(result.bodyRevision.docUUID, "old");
    } finally {
      vi.useRealTimers();
    }
  },
);

test.each([false, true])(
  "a same-document frame during body evaluation invalidates the cached peer predicate (first match %s)",
  async (first) => {
    vi.useFakeTimers();
    try {
      const clock = () => BigInt(Date.now()) * 1000000n,
        p = new ObserverPage(null, "", clock);
      p.baseUrl = "http://127.0.0.1:4321";
      const path = "/articles/a/",
        url = p.url(path);
      p.docInits.push({ ns: String(clock()), docUUID: "same", url });
      const mark = p.mark(),
        startNs = clock();
      let reads = 0;
      p.wsFrames.push({ ns: String(startNs), event: "module" });
      p.page = {
        evaluate: async () => {
          const matched = ++reads === 1 ? first : !first;
          if (reads === 1) {
            await Promise.resolve();
            // Preserve multiplicity even if monotonic stamps happen to coincide.
            p.wsFrames.push({ ns: String(startNs), event: "module" });
            p.otherFrames.push({ ns: String(clock()), event: "unmapped" });
          }
          return { url, docUUID: "same", loading: false, matched };
        },
      };
      const pending = p.settle({
        mark,
        startNs,
        quietMs: 3,
        boundMs: 15,
        stepMs: 1,
        requireNativeUpdate: true,
        bodyRevision: {
          allowSameDocument: true,
          path,
          predicate: { kind: "text", equals: "original" },
        },
      });
      await vi.advanceTimersByTimeAsync(15);
      const result = await pending;
      assert.equal(reads, 2);
      assert.equal(result.settled, !first);
      assert.equal(result.nativeUpdateObserved, true);
      assert(result.waitedMs >= 6);
    } finally {
      vi.useRealTimers();
    }
  },
);

test.skipIf(process.platform !== "linux")(
  "a retitled Next server with unavailable lsof still needs a fresh birth-bound ELF check",
  () => {
    const image = { path: "/pinned/node", device: "2096", inode: "638090" };
    const processRow = {
      pid: 2062827,
      ppid: 2062815,
      pgid: 2062801,
      startTicks: "43738295",
      bootId: "1edafe52-089e-4f16-a478-1143d29052bd",
      birthVersion: "linux-proc-v1",
      comm: "next-server (v1",
      role: "next-server",
      image,
      elfHeader: "7f454c46",
    };
    const lookup = processExecutable(processRow.pid, (cmd) => {
      if (cmd === "lsof") throw Error("no txt path");
      return processRow.comm;
    });
    assert.deepEqual(lookup, { path: null, method: "unavailable" });
    const legacy = processIdentityChecks({
      processes: [processRow],
      roles: [processRow],
      nodeRealpath: image.path,
      appDir: "/app",
      exeOf: () => lookup,
      alive: () => true,
    });
    assert.equal(legacy[0].ok, false);
    const qualified = { chain: [processRow] },
      socket = { pid: processRow.pid, port: 41923, inode: "4860700" };
    const probe = vi.fn((_births, _cfg, ready, file) => {
      assert(ready && file);
      return { chain: [processRow], socket };
    });
    const run = (known = [processRow], read = probe) =>
      peerProcessIdentity([processRow], known, qualified, { image }, read);
    assert.equal(run()[0].actual.inode, image.inode);
    assert.equal(probe.mock.calls.length, 1);
    for (const fields of [
      { pid: 999 },
      { ppid: 0 },
      { startTicks: "9" },
      { role: "server" },
      { image: { ...image, inode: "42" } },
      { elfHeader: null },
    ]) {
      assert.throws(() =>
        run([processRow], () => ({ chain: [{ ...processRow, ...fields }], socket })),
      );
    }
    assert.throws(() => run([]));
    const actual = readPeerExecutable("/proc/self/exe");
    assert.equal(actual.imagePath, realpathSync(process.execPath));
    assert.equal(actual.elfHeader, "7f454c46");
    assert.equal(actual.info.ino, statSync(process.execPath, { bigint: true }).ino);
  },
);

test.each([false, true])(
  "a fourth live Next process is rejected with bounded, durable partial evidence (long argv %s)",
  async (large) => {
    const cfg = { framework: "next", node: "/pin/node", appDir: "/app", port: 3456, rootPid: 10 };
    const proc = (pid, ppid, argv) => ({
      pid,
      ppid,
      pgid: 10,
      startTicks: String(100 + pid),
      bootId: "boot",
      before: { pid, state: "R" },
      after: { pid, state: "S" },
      cwd: "/app",
      image: { path: "/pin/node", device: "7", inode: "10" },
      argv,
      privateWorker: true,
      devServer: true,
    });
    const rows = [
      proc(10, 1, [
        "/pin/node",
        "/app/scripts/dev-content.mjs",
        "--hostname",
        "127.0.0.1",
        "--port",
        "3456",
      ]),
      proc(11, 10, [
        "/pin/node",
        "/app/node_modules/next/dist/bin/next",
        "dev",
        "--hostname",
        "127.0.0.1",
        "--port",
        "3456",
      ]),
      proc(12, 11, ["next-server (v16.4.6)"]),
      proc(13, 12, ["/pin/node", large ? "x".repeat(65536) : "/app/unknown.js"]),
    ];
    const records = [],
      fail = Promise.withResolvers(),
      ns = process.hrtime.bigint();
    const monitor = startFixedRssSafety({
      server: {},
      fixedExecutables: { root: { path: cfg.node } },
      peer: cfg,
      limits: { rssCeilingKiB: 4194304, sampleEveryMs: 1000 },
      append: (row) => records.push(JSON.parse(JSON.stringify(row))),
      abort: fail.resolve,
      sample: async () => ({
        summedRssKiB: 4,
        helper: { closed: true, pipesClosed: true, spawnHrtimeNs: String(ns) },
        timing: {
          clock: { qualified: true },
          parent: {
            entryNs: String(ns),
            resultNs: String(ns),
            deadlineNs: String(ns + 750000000n),
          },
        },
        processes: rows.map((row) => ({
          pid: row.pid,
          statusKiB: { VmRSS: 1 },
          smapsRollupKiB: { Rss: 1 },
          disposition: "live",
        })),
      }),
      checkRoles: () => identify(rows, cfg, true),
    });
    try {
      assert.match((await fail.promise).message, /unknown role/);
      const failure = await monitor.stop();
      const saved = records.find((r) => r.type === "fixed-rss-failure").failure;
      assert.deepEqual(saved, JSON.parse(JSON.stringify(failure)));
      assert.equal(saved.details.roleQualified, false);
      assert.equal(saved.details.observed.length, 4);
      const last = saved.details.observed[3];
      if (large) {
        assert.equal(last.argvOmitted, true);
        assert.equal(last.argv, undefined);
        assert.equal(last.argvJsonSha256, sha256(Buffer.from(JSON.stringify(rows[3].argv))));
      } else assert.deepEqual(last.argv, rows[3].argv);
      assert.deepEqual(last.after, rows[3].after);
      assert.equal(last.role, undefined);
      assert.equal(
        records.some((r) => r.type === "fixed-peer-roles"),
        false,
      );
    } finally {
      await monitor.stop();
    }
  },
);

test("outer gate uses the current ordinary epoch without dispatching or joining a helper", async () => {
  vi.useFakeTimers({
    toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "hrtime"],
  });
  const worker = { pid: 13 },
    ancestors = [{ pid: 10 }, { pid: 11 }, { pid: 12 }];
  const known = new Map([...ancestors, worker].map((p) => [p.pid, p]));
  const owner = { qualified: worker, isWorker: (p) => p.pid === 13 };
  const verify = vi.spyOn(peerRoles, "peerProcessIdentity").mockReturnValue([]);
  let hold = false;
  const deferred = Promise.withResolvers();
  const sample = vi.fn(async () => {
    if (hold) await deferred.promise;
    const ns = process.hrtime.bigint();
    return {
      summedRssKiB: 4,
      processes: [
        { disposition: "live", pid: 10, statusKiB: { VmRSS: 4 }, smapsRollupKiB: { Rss: 4 } },
      ],
      helper: { spawnHrtimeNs: String(ns), closed: true, pipesClosed: true },
      timing: {
        clock: { qualified: true },
        parent: { entryNs: String(ns), resultNs: String(ns), deadlineNs: String(ns + 750000000n) },
      },
    };
  });
  const monitor = startFixedRssSafety({
    server: { handle: { known } },
    fixedExecutables: { protocol: "linux-fixed-peer.v4", root: { path: "/node" } },
    nextOwner: () => owner,
    peer: {},
    limits: { rssCeilingKiB: 4194304, sampleEveryMs: 1000 },
    sample,
    append() {},
    abort() {},
    checkRoles: () => ({
      chain: [{ pid: 10, role: "root", pgid: 10, ppid: 100, bootId: "boot", startTicks: "1" }],
    }),
  });
  try {
    const ready = monitor.verifyReady(process.hrtime.bigint());
    await vi.advanceTimersByTimeAsync(2000);
    await ready;
    let ownerGraphs = 0;
    const full = () => {
      ownerGraphs++;
    };
    const cycle = monitor.verifyOuterGraph(() => ancestors, full);
    assert.equal(ownerGraphs, 1);
    assert.throws(
      () =>
        monitor.verifyOuterGraph(
          () => ancestors,
          () => {
            throw Error("unexpected outer graph child");
          },
          "pre-truncate",
        ),
      (error) => {
        assert.equal(error.outerRefusal.coverage, "accepted");
        assert.equal(error.outerRefusal.productGraph, "accepted");
        assert.equal(error.outerRefusal.observerGraph, "failed");
        return true;
      },
    );
    assert.equal(cycle.epoch, 3);
    assert.equal(sample.mock.calls.length, 3);
    assert.throws(
      () => monitor.verifyOuterGraph(() => [...ancestors, worker]),
      /requires a missing worker/,
    );
    await vi.advanceTimersByTimeAsync(995);
    assert.throws(() => monitor.verifyOuterGraph(() => ancestors), /RSS coverage/);
    hold = true;
    await vi.advanceTimersByTimeAsync(5);
    const competingGraph = vi.fn(() => {
      throw Error("unexpected outer graph child");
    });
    const product = vi.fn(() => ancestors);
    let originalRefusal;
    const refuse = (stage) =>
      assert.throws(
        () => monitor.verifyOuterGraph(product, competingGraph, stage),
        (error) => {
          const row = error.outerRefusal;
          assert.equal(row.stage, stage);
          assert.equal(row.coverage, "ordinary-sample-pending");
          assert.equal(row.observerGraph, "not-checked");
          assert.equal(row.productGraph, "not-checked");
          assert.equal(row.pending.epoch, 4);
          assert.equal(row.lastAccepted.epoch, 3);
          assert.equal(row.lastAccepted.cadenceDeadlineNs, cycle.cadenceDeadlineNs);
          assert(BigInt(row.refusedNs) >= BigInt(row.entryNs));
          if (!originalRefusal) {
            originalRefusal = row;
            assert.equal(row.pending.helperBirth, null);
          } else assert.equal(row.pending.helperBirth.pid, 91);
          return true;
        },
      );
    refuse("prewrite-decision");
    const active = sample.mock.calls[3][2].outerCycle;
    active.helperBirth = { pid: 91, ppid: 1, pgid: 90, startTicks: "5", bootId: "controlled" };
    refuse("pre-truncate");
    assert.equal(originalRefusal.pending.helperBirth, null);
    assert.equal(product.mock.calls.length, 0);
    assert.equal(competingGraph.mock.calls.length, 0);
    assert.equal(sample.mock.calls.length, 4);
    assert.equal(verify.mock.calls.length, 2);
  } finally {
    deferred.resolve();
    await monitor.stop();
    verify.mockRestore();
    vi.useRealTimers();
  }
});
