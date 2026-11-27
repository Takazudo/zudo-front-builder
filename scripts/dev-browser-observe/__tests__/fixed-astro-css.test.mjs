import { identifyPeerRoles, verifyAstroPopulation } from "../lib/fixed-peer-roles.mjs";
import assert from "node:assert/strict";
import { sha256 } from "../lib/util.mjs";
import { updateSignalFrame } from "../lib/browser.mjs";
import { deps, fakeFs, fakePage } from "./fakes.mjs";
import { runBatch } from "../lib/state-machine.mjs";
import { readFileSync } from "node:fs";
import { profileSchemaFor, semanticErrors } from "../lib/profile.mjs";
import { validateAs } from "../lib/schema.mjs";
import { test, vi } from "vitest";
import { startFixedRssSafety } from "../lib/fixed-rss-safety.mjs";
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
    fixedExecutables: { protocol: "linux-fixed-astro-css.v1", root: { path: "/pinned/node" } },
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

test("the fixed peer mode admits only one archived 100-article Astro CSS classification", () => {
  for (const framework of ["astro"]) {
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
      fixedRssSafety: "linux-fixed-astro-css.v1",
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
        id: "fn-managed-css",
        set: "workload",
        warmOrder: "none",
        ops: [
          {
            id: "managed-css",
            op: "edit-managed-css",
            targetPath: "/",
            writeMethod: "in-place",
            expect: "classify",
          },
        ],
      },
    ];
    const errors = (q) => [...validateAs(profileSchemaFor(q), q), ...semanticErrors(q)];
    assert.deepEqual(errors(p), []);
    for (const change of [
      (q) => (q.scenarios[0].ops[0].repeat = { warmups: 0, measured: 2 }),
      (q) => (q.scenarios[0].ops[0].article = "article-00004"),
      (q) => (q.fixture.scale = 1000),
      (q) => (q.framework = "zfb"),
      (q) => (q.framework = "next"),
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

test.each([
  "success",
  "no-socket",
  "timeout",
  "stale",
  "late-notify",
  "late-document",
  "late-frame",
  "sheet-unloaded",
  "wrong-token",
  "same-detail",
])(
  "Astro CSS gate records %s without admitting a missing socket, late edit or reused document",
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
          return Promise.resolve({
            kind: "notify",
            t1: ns,
            payload: {
              token,
              docUUID: "doc-1",
              armedDocUUID: "doc-1",
              url: "http://localhost/",
              actual: [1, outcome === "wrong-token" ? "prefix" + token : token, "#f5f2e9"],
            },
          });
        },
        liveCheck: ({ path }) => ({
          ok: true,
          actual: [1, [1, "", "#f5f2e9"]],
          docUUID: outcome === "same-detail" ? "doc-1" : "fresh-" + path,
        }),
      });
    page.url = () => "http://localhost/";
    const prep = fs.prepareAttempt;
    fs.prepareAttempt = (op) => ({ ...prep(op), targetPath: "/" });
    fs.liveChecks = () =>
      [
        "/",
        "/articles/article-00001/",
        "/articles/article-00005/",
        "/archive/1/",
        "/archive/2/",
        "/category/architecture/",
      ].map((path) => ({ path, predicate: { kind: "count", selector: ":root", equals: 1 } }));
    page.peerSocketTracking = new Map(connected ? [[1, { firstFrameNs: "10" }]] : []);
    const oldArm = page.arm;
    page.arm = async (...args) => ({
      ...(await oldArm(...args)),
      actual: [1, "", outcome === "sheet-unloaded" ? "" : "#f5f2e9"],
    });
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
      bodyRevision: { matched: true, docUUID: "doc-1", actual: [1, "", "#f5f2e9"] },
    });
    const d = {
      ...deps({ fs, page, clock }),
      workDeadlineNs: 100000000000n,
      verifyBodyRevision: true,
      fixedAstroCss: true,
      beforeAstroWrite: () => {},
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
          id: "fn-managed-css",
          ops: [{ id: "managed-css", op: "edit-managed-css", article: "a", expect: "classify" }],
        },
      ],
    });
    assert.equal(fs.writes.length, connected && outcome !== "sheet-unloaded" ? 1 : 0);
    assert.equal(
      accounting.success,
      ["success", "late-frame", "same-detail"].includes(outcome) ? 1 : 0,
    );
    if (connected && !["sheet-unloaded", "wrong-token"].includes(outcome))
      assert.equal(accounting.restorationFailures, outcome === "same-detail" ? 1 : 0);
    if (["timeout", "late-document", "late-notify"].includes(outcome))
      assert.equal(accounting.timeout, 1);
    if (outcome === "stale") assert.equal(accounting.stale, 1);
    if (outcome === "late-frame")
      assert.equal(d.recorder.records.find((r) => r.mechanism).mechanism, "in-document-update");
  },
);

test("a CSS frame identifies the managed stylesheet only for its exact path and acceptedPath", () => {
  const socket = { name: "vite", typeKey: "type", events: { "update:css-update": "css" } };
  for (const path of ["/src/styles/site.imported.css", "/foreign.css"]) {
    const frame = updateSignalFrame(
      JSON.stringify({
        type: "update",
        updates: [{ type: "css-update", path, acceptedPath: path }],
      }),
      1n,
      "ws://127.0.0.1/",
      socket,
      true,
    );
    assert.equal(frame.astroManagedCss, path === "/src/styles/site.imported.css");
  }
});

test("Astro accepts only its exact root argument vector and original singleton terminal census", () => {
  const cfg = { framework: "astro", rootPid: 123, appDir: "/app", node: "/node", port: 1234 };
  const root = {
    pid: 123,
    ppid: 12,
    pgid: 123,
    bootId: "boot",
    startTicks: "123",
    birthVersion: "linux-proc-v1",
    argv: [
      "/node",
      "/app/node_modules/astro/bin/astro.mjs",
      "dev",
      "--ignore-lock",
      "--host",
      "127.0.0.1",
      "--port",
      "1234",
    ],
  };
  assert.equal(identifyPeerRoles([root], cfg)[0].role, "astro-cli");
  assert.throws(() => identifyPeerRoles([{ ...root, argv: [...root.argv, ""] }], cfg));
  assert.throws(() => identifyPeerRoles([root, { ...root, pid: 124 }], cfg));
  const exit = { exitCode: null, signalCode: "SIGINT" };
  const stop = {
    rootAlive: true,
    absent: true,
    remaining: [],
    ownedBefore: [root],
    known: [root],
    endings: [{ pid: 123, endedAfter: "SIGINT", exit }],
    steps: [{ signal: "SIGINT", target: "group:123", delivery: "sent" }],
    exit,
  };
  assert(verifyAstroPopulation(root, stop));
  for (const delta of [
    { known: [root, { ...root, pid: 124 }] },
    { ownedBefore: [{ ...root, startTicks: "124" }] },
    { steps: [] },
    { endings: [] },
    { remaining: [root] },
  ])
    assert(!verifyAstroPopulation(root, { ...stop, ...delta }));
});
