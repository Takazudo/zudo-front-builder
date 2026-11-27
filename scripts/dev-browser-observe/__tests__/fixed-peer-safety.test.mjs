import assert from "node:assert/strict";
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
