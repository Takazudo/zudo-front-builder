import { identifyPeerRoles, verifyAstroPopulation } from "../lib/fixed-peer-roles.mjs";
import assert from "node:assert/strict";
import { ObserverPage, updateSignalFrame } from "../lib/browser.mjs";
import { deps, fakePage } from "./fakes.mjs";
import { astroCssPredicate, runBatch } from "../lib/state-machine.mjs";
import { createModel } from "../lib/fieldnotes-model.mjs";
import { fieldNotesOps, liveRestoreChecks } from "../lib/fieldnotes.mjs";
import { hashOf, performWrite } from "../lib/fixture.mjs";
import { SPEC, articleRow } from "./fieldnotes-fake.mjs";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
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

const CSS_SOURCE = readFileSync(
  new URL("../fixtures/field-notes/distributions/astro/site.imported.css.src", import.meta.url),
);
const LAYOUT = readFileSync(
  new URL("../fixtures/field-notes/distributions/astro/Site.astro.src", import.meta.url),
);
const CSS_OP = {
  id: "managed-css",
  op: "edit-managed-css",
  targetPath: "/",
  writeMethod: "in-place",
  expect: "classify",
};
const CSS_SCENARIO = { id: "fn-managed-css", set: "workload", warmOrder: "none", ops: [CSS_OP] };
// The original fxq2 fn-managed-css receipt (exit 0) restored exactly these routes, in this order.
const ORIGINAL_ROUTES = [
  "/",
  "/articles/article-00001/",
  "/archive/1/",
  "/archive/2/",
  "/category/architecture/",
];
const ORIGINAL_CSS = [1, "", "#f5f2e9"];

function cssFixture() {
  const dir = mkdtempSync(join(tmpdir(), "astro-css-"));
  const contentDir = join(dir, "content"),
    layoutPath = join(dir, "src", "layouts", "Site.astro"),
    publicCssPath = join(dir, "public", "site.css"),
    managedCssPath = join(dir, "src", "styles", "site.imported.css");
  for (const d of [
    contentDir,
    dirname(layoutPath),
    dirname(publicCssPath),
    dirname(managedCssPath),
  ])
    mkdirSync(d, { recursive: true });
  writeFileSync(layoutPath, LAYOUT);
  writeFileSync(publicCssPath, "");
  writeFileSync(managedCssPath, CSS_SOURCE);
  const model = createModel(
    { articles: Array.from({ length: 100 }, (_, i) => articleRow(i + 1)) },
    SPEC,
  );
  const m = {
    model,
    contentDir,
    layoutPath,
    publicCssPath,
    managedCssPath,
    managedCssEdit: { mode: "insert-root", customProperty: "--fn-managed-marker" },
    manifest: {},
  };
  return { dir, m, model, managedCssPath };
}

test("the real managed-CSS planner schedules exactly the five original ordered restoration routes", () => {
  const { dir, m, model, managedCssPath } = cssFixture();
  try {
    const ops = fieldNotesOps(m);
    const plan = ops.prepareAttempt(CSS_OP);
    assert.equal(CSS_SOURCE.length, 6156);
    assert.equal(plan.targetPath, "/");
    assert.deepEqual(plan.touched, []);
    assert.deepEqual(plan.supplemental, []);
    assert.equal(plan.write.path, managedCssPath);
    assert.equal(
      plan.write.bytes.toString(),
      CSS_SOURCE.toString().replace(":root{", ":root{--fn-managed-marker:" + plan.token + ";"),
    );
    assert.deepEqual(plan.predicate, {
      kind: "computed-style",
      selector: ":root",
      property: "--fn-managed-marker",
      includes: plan.token,
    });
    const checks = ops.liveChecks();
    assert.deepEqual(
      checks.map((c) => c.path),
      ORIGINAL_ROUTES,
    );
    assert.deepEqual(checks, liveRestoreChecks(model, []));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

const ROUTE_REJECTIONS = [
  "sixth-route",
  "reordered",
  "missing-route",
  "duplicate-doc",
  "reused-home",
  "fresh-home-edited",
  "bad-inode",
];
test.each([
  "success",
  "foreign-css-frame",
  ...ROUTE_REJECTIONS,
  "no-socket",
  "sheet-unloaded",
  "unexpected-plan",
  "timeout",
  "stale",
  "late-notify",
  "early-notify",
  "wrong-token",
  "foreign-frame",
])("real Astro managed-CSS planner, update and ordered restoration: %s", async (scenarioCase) => {
  const { dir, m, model, managedCssPath } = cssFixture();
  try {
    const ops = fieldNotesOps(m);
    const fs = { ...ops, hashOf, performWrite };
    const clock = {
      ns: 1000000000n,
      now() {
        return (this.ns += 10000n);
      },
    };
    let token,
      deadline,
      writes = 0;
    fs.prepareAttempt = (op) => {
      const plan = ops.prepareAttempt(op);
      token = plan.token;
      if (scenarioCase === "unexpected-plan")
        plan.supplemental = [{ path: "/archive/1/", predicate: plan.predicate }];
      return plan;
    };
    fs.performWrite = (w, now) => {
      writes++;
      return performWrite(w, now);
    };
    fs.restore = (snapshot) => {
      const result = ops.restore(snapshot);
      if (scenarioCase === "bad-inode" && result.actions.length)
        result.actions[0].evidence.inodeAfter++;
      return result;
    };
    fs.liveChecks = () => {
      const checks = ops.liveChecks();
      assert.deepEqual(
        checks.map((c) => c.path),
        ORIGINAL_ROUTES,
      );
      if (scenarioCase === "sixth-route") {
        const detail = liveRestoreChecks(model, ["article-00005"]).find(
          (c) => c.path === "/articles/article-00005/",
        );
        checks.splice(2, 0, detail);
      }
      if (scenarioCase === "reordered") [checks[2], checks[3]] = [checks[3], checks[2]];
      if (scenarioCase === "missing-route") checks.pop();
      return checks;
    };
    const page = fakePage({
      onWait: (tok, _late, p) => {
        p.evidence.docInits = [];
        p.evidence.navigations = [];
        p.evidence.wsFrames = [];
        const now = clock.now();
        if (scenarioCase === "foreign-css-frame")
          p.evidence.wsFrames.push({ event: "css", astroManagedCss: false, ns: String(now) });
        if (["timeout", "stale"].includes(scenarioCase)) {
          if (scenarioCase === "stale")
            p.evidence.docInits.push({ ns: String(now), url: page.url("/"), docUUID: "reload" });
          p.evidence.newDocuments = p.evidence.docInits.length;
          return Promise.resolve({ kind: "timeout" });
        }
        const t1 =
          scenarioCase === "late-notify"
            ? deadline + 1n
            : scenarioCase === "early-notify"
              ? 1n
              : clock.now();
        const url = page.url("/");
        return Promise.resolve({
          kind: "notify",
          t1,
          payload: {
            token: tok,
            docUUID: "home-edited",
            armedDocUUID: "home-edited",
            url,
            producer: {
              designatedMain: scenarioCase !== "foreign-frame",
              frameUrl: url,
              latestDocUUID: "home-edited",
              latestDocNs: "100",
              receivedNs: String(t1),
            },
            actual: [1, scenarioCase === "wrong-token" ? "prefix" + tok : tok, "#f5f2e9"],
          },
        });
      },
    });
    page.url = (path) => "http://localhost:1234" + path;
    page.prepare = async (path) => ({ docUUID: "home-edited", url: page.url(path) });
    page.peerSocketTracking = new Map(
      scenarioCase === "no-socket" ? [] : [[1, { firstFrameNs: "1" }]],
    );
    page.arm = async () => ({
      armed: true,
      docUUID: "home-edited",
      observedUrl: page.url("/"),
      actual: [1, "", scenarioCase === "sheet-unloaded" ? "" : "#f5f2e9"],
    });
    page.settle = async (opts) => {
      assert(readFileSync(managedCssPath).equals(CSS_SOURCE));
      assert.equal(opts.bodyRevision?.path, "/");
      return {
        settled: true,
        nativeUpdateObserved: true,
        bodyRevision: { matched: true, docUUID: "home-edited", actual: ORIGINAL_CSS },
      };
    };
    page.liveCheck = async (c) => {
      if (c.path === "/") assert.deepEqual(c.predicate.of[1], astroCssPredicate(""));
      return {
        ok: true,
        docUUID:
          scenarioCase === "reused-home" && c.path === "/"
            ? "home-edited"
            : scenarioCase === "duplicate-doc" && c.path === "/archive/2/"
              ? "fresh/archive/1/"
              : "fresh" + c.path,
        actual: [[], scenarioCase === "fresh-home-edited" ? [1, token, "#f5f2e9"] : ORIGINAL_CSS],
      };
    };
    const d = {
      ...deps({ fs, page, clock }),
      fixedAstroCss: true,
      workDeadlineNs: 99000000000n,
      beforeAstroWrite: () => {},
      stageBudget: (ms, ns) => {
        deadline = ns + BigInt(ms) * 1000000n;
        return ms;
      },
    };
    d.timeouts = { ...d.timeouts, quietMs: 3000 };
    const { accounting } = await runBatch({ deps: d, scenarios: [CSS_SCENARIO] });
    const cleanup = d.recorder.records.find((r) => r.type === "cleanup");
    assert.equal(
      writes,
      ["no-socket", "sheet-unloaded", "unexpected-plan"].includes(scenarioCase) ? 0 : 1,
    );
    assert(readFileSync(managedCssPath).equals(CSS_SOURCE));
    if (["success", "foreign-css-frame"].includes(scenarioCase)) {
      assert.equal(accounting.success, 1);
      assert.equal(accounting.restorationFailures, 0);
      assert.equal(accounting.stopReason, null);
      assert.equal(cleanup.ok, true);
      assert.deepEqual(
        cleanup.live.map((r) => r.path),
        ORIGINAL_ROUTES,
      );
      assert.deepEqual(page.quietWindows, [3000]);
      const attempt = d.recorder.records.find((r) => r.type === "attempt" && r.peerDecision);
      assert.deepEqual(
        attempt.peerDecision.window.livereloadEvents,
        scenarioCase === "foreign-css-frame" ? ["other-css"] : [],
      );
    } else if (ROUTE_REJECTIONS.includes(scenarioCase)) {
      assert.equal(accounting.success, 1);
      assert.equal(accounting.restorationFailures, 1);
      assert.equal(cleanup.ok, false);
      assert.deepEqual(page.quietWindows, []);
    } else {
      assert.equal(accounting.success, 0);
      if (["timeout", "late-notify"].includes(scenarioCase)) assert.equal(accounting.timeout, 1);
      if (scenarioCase === "stale") assert.equal(accounting.stale, 1);
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

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

test.each(["same", "arm-changed", "notify-changed", "wrong-url"])(
  "fresh route binds CSS to its actual read document: %s",
  async (variant) => {
    const p = new ObserverPage(null, "", () => 1000000000n);
    p.baseUrl = "http://127.0.0.1:1234";
    p.peerSocketTracking = new Map();
    const url = p.url("/");
    p.prepare = async () => ({ docUUID: "fresh", url, httpStatus: 200 });
    p.arm = async () => ({
      armed: true,
      docUUID: variant === "arm-changed" ? "other" : "fresh",
      observedUrl: variant === "wrong-url" ? url + "else" : url,
      actual: [1, "", "#f5f2e9"],
    });
    p.waitNotify = async (token) => ({
      kind: "notify",
      t1: 1000000001n,
      payload: {
        token,
        docUUID: variant === "notify-changed" ? "other" : "fresh",
        armedDocUUID: "fresh",
        url,
        producer: {
          designatedMain: true,
          frameUrl: url,
          latestDocUUID: "fresh",
          latestDocNs: "1000000000",
          receivedNs: "1000000001",
        },
      },
    });
    p.failureSnapshot = async () => ({ notInCache: false });
    p.disarm = async () => {};
    const r = await p.navigateAndAwait("/", {}, 100, "fresh");
    assert.equal(r.ok, variant === "same");
  },
);
test("pre-write module frames invalidate only the strict Astro preparation", () => {
  const p = new ObserverPage(null, "", () => 10n);
  p.markPre();
  p.wsFrames.push({ event: "module", ns: "10" });
  assert.equal(p.preWriteInstability(), null);
  p.peerSocketTracking = new Map();
  assert.equal(p.preWriteInstability().wsFrames[0].event, "module");
});
