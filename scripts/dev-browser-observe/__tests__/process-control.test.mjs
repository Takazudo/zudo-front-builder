import {
  chmodSync,
  cpSync,
  existsSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { OBSERVER_PATH } from "../lib/browser.mjs";
import {
  fixtureTreeHash,
  currentPins,
  BUILD_RECEIPT_SCHEMA,
  sourcePackagesAtSha,
} from "../lib/identity.mjs";
import { LockBusyError, acquireLock } from "../lib/lock.mjs";
import { sample } from "../lib/mac-process-adapter.mjs";
import {
  PidReuseError,
  birthIdentity,
  liveOwnedFromRegistry,
  spawnOwned,
  stopOwned,
} from "../lib/process-owner.mjs";
import { EXIT, execute, verifyIdentityOnly } from "../lib/run.mjs";
import { injectKillOwned } from "../lib/server.mjs";
import { lockContentionCheck } from "../lib/lock-check.mjs";
import { runBatch } from "../lib/state-machine.mjs";
import {
  HARNESS_DIR,
  delay,
  gitHead,
  gitToplevel,
  readJson,
  readJsonl,
  sha256File,
} from "../lib/util.mjs";
import { deps, fakeFs, fakePage } from "./fakes.mjs";

const FIXTURE = join(HARNESS_DIR, "fixtures", "tiny-two-article");
const NODE = process.execPath;
const tmps = [];
const children = [];
const tmp = () => {
  const d = mkdtempSync(join(tmpdir(), "dbo-proc-"));
  tmps.push(d);
  return d;
};
const own = (args) => {
  const h = spawnOwned(NODE, args);
  children.push(h);
  return h;
};

afterEach(async () => {
  for (const h of children.splice(0)) {
    if (h.exit === null) {
      try {
        process.kill(-h.native.pgid, "SIGKILL");
      } catch {
        // already gone
      }
      await Promise.race([h.exited, delay(2000)]);
    }
  }
  while (tmps.length) rmSync(tmps.pop(), { recursive: true, force: true });
});

const serverFrom = (h) => ({
  isAlive: () => ({ alive: h.exit === null, exit: h.exit }),
  pid: () => h.native.pid,
  waitExit: () => h.exited,
  sampleResources: () => ({ summedRssKiB: 1 }),
});

const ops = [
  {
    id: "s",
    ops: [
      { id: "o1", op: "edit-title", article: "a" },
      { id: "o2", op: "edit-body", article: "a" },
    ],
  },
];

describe("real child processes", () => {
  it("records a signalled exit as exitCode null + signalCode", async () => {
    const h = own(["-e", "process.kill(process.pid, 'SIGKILL')"]);
    const exit = await h.exited;
    expect(exit.exitCode).toBeNull();
    expect(exit.signalCode).toBe("SIGKILL");
    expect(h.launcher).toBe("none");
  });

  it("stops the batch when the server is SIGKILLed mid-attempt", async () => {
    const h = own(["-e", "setInterval(() => {}, 1000)"]);
    expect(h.native.pid).toBeGreaterThan(0);
    const page = fakePage({
      onWait: () => {
        setTimeout(() => process.kill(h.native.pid, "SIGKILL"), 50);
        return new Promise(() => {});
      },
    });
    const d = deps({ page, server: serverFrom(h), fs: fakeFs() });
    const { accounting } = await runBatch({ scenarios: ops, deps: d });
    expect(accounting).toMatchObject({
      stopReason: "server-exit",
      attempted: 1,
      notAttempted: 1,
      error: 1,
    });
    const rec = d.recorder.records.find((r) => r.type === "attempt");
    expect(rec.exit).toMatchObject({ exitCode: null, signalCode: "SIGKILL" });
    expect(accounting.lastArmedUnknownOperation).toMatchObject({ op: "o1", writeCompleted: true });
  });

  it("stops the batch when the server exits with a code mid-attempt", async () => {
    const h = own(["-e", "setTimeout(() => process.exit(3), 100)"]);
    const d = deps({
      page: fakePage({ behaviors: [{ kind: "never" }] }),
      server: serverFrom(h),
      fs: fakeFs(),
    });
    d.timeouts.attemptMs = 10_000;
    const { accounting } = await runBatch({ scenarios: ops, deps: d });
    expect(accounting.stopReason).toBe("server-exit");
    expect(d.recorder.records.find((r) => r.type === "attempt").exit).toMatchObject({
      exitCode: 3,
      signalCode: null,
    });
  });

  it("stops an owned group including a grandchild and verifies absence", async () => {
    const h = own([
      "-e",
      `require('child_process').spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'ignore' }); setInterval(() => {}, 1000)`,
    ]);
    await delay(300);
    const report = await stopOwned(h, { intWaitMs: 3000, termWaitMs: 2000, killWaitMs: 2000 });
    expect(report.rootAlive).toBe(true);
    expect(report.ownedBefore.length).toBeGreaterThanOrEqual(2);
    expect(report.absent).toBe(true);
    expect(report.steps.map((s) => s.signal)).toEqual(["SIGINT"]);
    expect(report.exit).toMatchObject({ exitCode: null, signalCode: "SIGINT" });
    expect(report.endings.every((e) => e.endedAfter === "SIGINT")).toBe(true);
    expect(report.endings.find((e) => e.pid === h.native.pid).exit).toMatchObject({
      signalCode: "SIGINT",
    });
  });

  it("sends SIGTERM after stopTermMs when SIGINT is ignored", async () => {
    const h = own(["-e", "process.on('SIGINT', () => {}); setInterval(() => {}, 1000)"]);
    await delay(300);
    const report = await stopOwned(h, { intWaitMs: 400, termWaitMs: 2000, killWaitMs: 2000 });
    expect(report.steps.map((s) => s.signal)).toEqual(["SIGINT", "SIGTERM"]);
    expect(report.exit).toMatchObject({ exitCode: null, signalCode: "SIGTERM" });
    expect(report.endings.find((e) => e.pid === h.native.pid)).toMatchObject({
      endedAfter: "SIGTERM",
    });
  });

  it("records a graceful SIGINT exit code", async () => {
    const h = own([
      "-e",
      "process.on('SIGINT', () => process.exit(0)); setInterval(() => {}, 1000)",
    ]);
    await delay(300);
    const report = await stopOwned(h, { intWaitMs: 3000, termWaitMs: 1000, killWaitMs: 1000 });
    expect(report.exit).toMatchObject({ exitCode: 0, signalCode: null });
    expect(report.endings.find((e) => e.pid === h.native.pid)).toMatchObject({
      endedAfter: "SIGINT",
    });
  });

  it("escalates to SIGKILL for a survivor that ignores SIGINT and SIGTERM", async () => {
    const h = own([
      "-e",
      "process.on('SIGINT', () => {}); process.on('SIGTERM', () => {}); setInterval(() => {}, 1000)",
    ]);
    await delay(300);
    const report = await stopOwned(h, { intWaitMs: 300, termWaitMs: 300, killWaitMs: 2000 });
    expect(report.steps.map((s) => s.signal)).toEqual(["SIGINT", "SIGTERM", "SIGKILL"]);
    expect(report.endings.find((e) => e.pid === h.native.pid)).toMatchObject({
      endedAfter: "SIGKILL",
    });
    expect(report.absent).toBe(true);
    expect(report.exit).toMatchObject({ exitCode: null, signalCode: "SIGKILL" });
  });

  it("refuses to signal when the birth identity no longer matches (PID reuse guard)", async () => {
    const h = own(["-e", "setInterval(() => {}, 1000)"]);
    const signals = [];
    h.native = { ...h.native, lstart: "Mon Jan 1 00:00:00 2001" };
    await expect(stopOwned(h, { killImpl: (...a) => signals.push(a) })).rejects.toBeInstanceOf(
      PidReuseError,
    );
    expect(signals).toEqual([]);
    expect(h.exit).toBeNull();
  });

  it("finds a still-alive owned server from the registry by birth identity", async () => {
    const h = own(["-e", "setInterval(() => {}, 1000)"]);
    const entry = birthIdentity(h.native.pid);
    expect(liveOwnedFromRegistry([entry])).toHaveLength(1);
    process.kill(h.native.pid, "SIGKILL");
    await h.exited;
    expect(liveOwnedFromRegistry([entry])).toHaveLength(0);
  });

  it("samples rss/vsz/threads for a live pid", () => {
    const s = sample([process.pid]);
    expect(typeof s.processes[0].rssKiB).toBe("number");
    expect(s.summedRssKiB).toBeGreaterThan(0);
    expect(sample([999_999]).processes[0]).toMatchObject({ present: false, rssKiB: "unavailable" });
  });
});

describe("kill-server-during-wait injection on a real child", () => {
  it("SIGKILLs the birth-verified owned process and the batch records server-exit/SIGKILL", async () => {
    const h = own(["-e", "setInterval(() => {}, 1000)"]);
    const server = { ...serverFrom(h), injectKill: () => injectKillOwned(h) };
    const d = deps({ page: fakePage({ behaviors: [{ kind: "never" }] }), server, fs: fakeFs() });
    d.timeouts.attemptMs = 10_000;
    const scen = [
      {
        id: "neg",
        set: "negative-controls",
        ops: [
          {
            id: "k",
            op: "edit-title",
            article: "a",
            writeMethod: "in-place",
            inject: "kill-server-during-wait",
          },
          { id: "n", op: "edit-body", article: "a", writeMethod: "in-place" },
        ],
      },
    ];
    const { accounting } = await runBatch({ scenarios: scen, deps: d });
    const rec = d.recorder.records.find((r) => r.type === "attempt");
    expect(rec.injectKill).toMatchObject({ ok: true, pid: h.native.pid, signal: "SIGKILL" });
    expect(rec).toMatchObject({
      outcome: "server-exit",
      exit: { exitCode: null, signalCode: "SIGKILL" },
      controlPass: true,
    });
    expect(accounting).toMatchObject({ stoppedByExpectedControl: true, notAttempted: 1 });
  });

  it("refuses to kill when the birth identity no longer matches", () => {
    const h = own(["-e", "setInterval(() => {}, 1000)"]);
    h.native = { ...h.native, lstart: "Mon Jan 1 00:00:00 2001" };
    const kills = [];
    expect(injectKillOwned(h, (...a) => kills.push(a))).toMatchObject({
      ok: false,
      reason: "birth-identity-mismatch-or-gone",
    });
    expect(kills).toEqual([]);
  });
});

describe("pgid scan after the root died (round 4 #5)", () => {
  it("finds, signals and verifies a grandchild that no snapshot ever captured", async () => {
    const h = own([
      "-e",
      `const c = require('child_process').spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'ignore' }); console.log('GC', c.pid); setInterval(() => {}, 1000)`,
    ]);
    const gcPid = await new Promise((resolve) =>
      h.child.stdout.on("data", (d) => {
        const m = String(d).match(/GC (\d+)/);
        if (m) resolve(Number(m[1]));
      }),
    );
    expect([...h.known.keys()]).toEqual([h.native.pid]);
    process.kill(h.native.pid, "SIGKILL");
    await h.exited;
    const alive = (pid) => {
      try {
        process.kill(pid, 0);
        return true;
      } catch {
        return false;
      }
    };
    expect(alive(gcPid)).toBe(true);
    const registered = [];
    h.onNewOwned = (p) => registered.push(p);
    const report = await stopOwned(h, { intWaitMs: 2000, termWaitMs: 1000, killWaitMs: 1000 });
    expect(report.rootAlive).toBe(false);
    expect(report.pgidScans[0].found).toContain(gcPid);
    expect(registered.map((p) => [p.pid, p.source])).toEqual([[gcPid, "pgid-scan"]]);
    expect(report.endings.find((e) => e.pid === gcPid)).toMatchObject({ endedAfter: "SIGINT" });
    expect(report.absent).toBe(true);
    expect(alive(gcPid)).toBe(false);
  });
});

describe("lock", () => {
  it("a second nonblocking acquirer fails while the first holds the lock", async () => {
    const path = join(tmp(), "x.lock");
    const first = await acquireLock(path);
    await expect(acquireLock(path)).rejects.toBeInstanceOf(LockBusyError);
    await first.release();
    const again = await acquireLock(path);
    await again.release();
  });
});

describe("execute (identity gate)", () => {
  function setup({ mode = "source-built" } = {}) {
    const root = tmp();
    const fixtureDir = join(root, "fixture");
    cpSync(FIXTURE, fixtureDir, { recursive: true });
    const exe = join(root, "fake-zfb");
    writeFileSync(exe, "not the recorded binary");
    writeFileSync(
      join(root, "build-receipt.json"),
      JSON.stringify({
        schema: BUILD_RECEIPT_SCHEMA,
        exe: { path: exe, sha256: "0".repeat(64), bytes: 1 },
        source: { repo: root, sha: "a".repeat(40), dirty: false, dirtyIdentitySha256: null },
        vendor: { chosen: null },
        embeddedBytes: { checked: 0, missing: [] },
      }),
    );
    const pins = currentPins({
      fixtureDir,
      observerPath: OBSERVER_PATH,
      adapterPath: join(fixtureDir, "adapters/A0/content.ts.adapter"),
    });
    const profile = JSON.parse(
      readFileSync(join(HARNESS_DIR, "profiles", "tiny-internal-a0.example.json"), "utf8"),
    );
    Object.assign(profile, {
      mode,
      productSha: "a".repeat(40),
      buildReceipt: "build-receipt.json",
      outDir: "out",
      pins,
    });
    profile.fixture.dir = "fixture";
    writeFileSync(join(root, "profile.json"), JSON.stringify(profile));
    return {
      root,
      fixtureDir,
      profilePath: join(root, "profile.json"),
      outDir: join(root, "out"),
      planned: profile.scenarios.reduce((n, s) => n + s.ops.length, 0),
    };
  }

  it("rejects a mismatched identity before any workload edit and keeps the fixture untouched", async () => {
    const s = setup();
    const before = fixtureTreeHash(s.fixtureDir);
    const msgs = [];
    const code = await execute({
      profilePath: s.profilePath,
      label: "L1",
      command: "run",
      stderr: (m) => msgs.push(m),
    });
    expect(code).toBe(EXIT.identityRejected);
    expect(fixtureTreeHash(s.fixtureDir)).toBe(before);
    expect(existsSync(join(s.outDir, "L1", "work"))).toBe(false);
    expect(existsSync(join(s.outDir, "L1", "server.log"))).toBe(false);
    const records = readJsonl(join(s.outDir, "L1", "records.jsonl"));
    const identity = records.find((r) => r.type === "identity");
    expect(identity.outcome).toBe("identity-rejected");
    expect(identity.checks.filter((c) => !c.ok).map((c) => c.name)).toEqual(
      expect.arrayContaining(["exe-sha256", "embedded-bytes-in-exe"]),
    );
    expect(records.map((r) => r.type)).not.toContain("attempt");
    const summary = readJson(join(s.outDir, "L1", "summary.json"));
    expect(summary.accounting).toMatchObject({
      planned: s.planned,
      attempted: 0,
      notAttempted: s.planned,
      stopReason: "identity-rejected",
    });
    const receipt = readJson(join(s.outDir, "L1", "receipt.json"));
    expect(receipt.harnessCapability.sha).toMatch(/^[0-9a-f]{40}$/);
    expect(receipt.productSha).toBe("a".repeat(40));
  });

  it("refuses an outDir inside a git work tree before creating anything", async () => {
    const s = setup();
    const p = JSON.parse(readFileSync(s.profilePath, "utf8"));
    const inside = join(HARNESS_DIR, "__tests__", "never-created-out");
    p.outDir = inside;
    writeFileSync(s.profilePath, JSON.stringify(p));
    const msgs = [];
    expect(
      await execute({
        profilePath: s.profilePath,
        label: "G1",
        command: "run",
        stderr: (m) => msgs.push(m),
      }),
    ).toBe(EXIT.usage);
    expect(msgs.join("\n")).toMatch(/inside the git work tree/);
    expect(existsSync(inside)).toBe(false);
    const v = verifyIdentityOnly(s.profilePath);
    expect(v.checks.find((c) => c.name === "outdir-outside-git-worktree").ok).toBe(false);
  });

  it("rejects A1 when its bytes no longer match the original input hash", async () => {
    const s = setup();
    const p = JSON.parse(readFileSync(s.profilePath, "utf8"));
    p.fixture.adapter = "A1";
    const a1 = join(s.fixtureDir, "adapters/A1/content.ts.adapter");
    chmodSync(a1, 0o644);
    writeFileSync(a1, `${readFileSync(a1, "utf8")}// tampered\n`);
    p.pins = currentPins({
      fixtureDir: s.fixtureDir,
      observerPath: OBSERVER_PATH,
      adapterPath: a1,
    });
    writeFileSync(s.profilePath, JSON.stringify(p));
    const v = verifyIdentityOnly(s.profilePath);
    expect(v.checks.find((c) => c.name === "adapter-provenance:A1")).toMatchObject({
      ok: false,
      expected: "10cb58dbc77234bc378397601c9b55b534c69b2a43c7e8b488bb32cbeebe38a6",
    });
    expect(v.checks.find((c) => c.name === "pin:adapterSha256").ok).toBe(true);
  });

  it("refuses to reuse a label", async () => {
    const s = setup();
    await execute({ profilePath: s.profilePath, label: "L1", command: "run", stderr: () => {} });
    const msgs = [];
    expect(
      await execute({
        profilePath: s.profilePath,
        label: "L1",
        command: "run",
        stderr: (m) => msgs.push(m),
      }),
    ).toBe(EXIT.usage);
    expect(msgs.join("\n")).toMatch(/label already used/);
  });

  it("rejects the original-shape ts config when the stored original no longer matches its hash", () => {
    const s = setup();
    const p = JSON.parse(readFileSync(s.profilePath, "utf8"));
    Object.assign(p.fixture, { placement: "external", trailingSlash: true, configFormat: "ts" });
    writeFileSync(s.profilePath, JSON.stringify(p));
    expect(
      verifyIdentityOnly(s.profilePath).checks.find((c) => c.name === "config-provenance:original"),
    ).toMatchObject({ ok: true });
    const cfg = join(s.fixtureDir, "configs/original.zfb.config.ts.src");
    chmodSync(cfg, 0o644);
    writeFileSync(cfg, `${readFileSync(cfg, "utf8")}// tampered\n`);
    expect(
      verifyIdentityOnly(s.profilePath).checks.find((c) => c.name === "config-provenance:original"),
    ).toMatchObject({
      ok: false,
      expected: "7839c840ae456b8b868463d2bc4cfb654ce490a5d80ace58cd71ab86c5c6f602",
    });
  });

  it("lock-contention-check: a second run exits 5 while hold-lock holds the lock", async () => {
    const s = setup();
    const r = await lockContentionCheck(s.profilePath, { holdMs: 30_000 });
    expect(r).toMatchObject({
      ok: true,
      secondRun: { exit: 5 },
      labelDirCreated: false,
      holder: { exit: { code: 0 } },
    });
    expect(r.secondRun.stderr).toMatch(/holds/);
  });

  it("a materialize failure after accepted identity still writes receipt, summary and run-end (round 4 #10)", async () => {
    const s = setup();
    const repo = gitToplevel(HARNESS_DIR);
    const sha = gitHead(repo);
    const exe = join(s.root, "fake-zfb");
    writeFileSync(
      join(s.root, "build-receipt.json"),
      JSON.stringify({
        schema: BUILD_RECEIPT_SCHEMA,
        exe: { path: exe, sha256: sha256File(exe), bytes: 1 },
        source: { repo, sha, dirty: false, dirtyIdentitySha256: null },
        vendor: { chosen: { treeSha256: sourcePackagesAtSha(repo, sha).sha256 } },
        embeddedBytes: { checked: 1, missing: [] },
      }),
    );
    rmSync(join(s.fixtureDir, "content"), { recursive: true });
    const p = JSON.parse(readFileSync(s.profilePath, "utf8"));
    p.productSha = sha;
    p.pins = currentPins({
      fixtureDir: s.fixtureDir,
      observerPath: OBSERVER_PATH,
      adapterPath: join(s.fixtureDir, "adapters/A0/content.ts.adapter"),
    });
    writeFileSync(s.profilePath, JSON.stringify(p));
    const code = await execute({
      profilePath: s.profilePath,
      label: "M1",
      command: "run",
      stderr: () => {},
    });
    expect(code).toBe(EXIT.controllerError);
    const records = readJsonl(join(s.outDir, "M1", "records.jsonl"));
    expect(records.find((r) => r.type === "identity").ok).toBe(true);
    expect(records.find((r) => r.type === "controller-error")).toMatchObject({
      phase: "materialize",
      outcome: "materialize-error",
    });
    expect(records.at(-1)).toMatchObject({ type: "run-end", exitCode: EXIT.controllerError });
    expect(records.map((r) => r.type)).not.toContain("server-ready");
    const summary = readJson(join(s.outDir, "M1", "summary.json"));
    expect(summary).toMatchObject({
      accountingSource: "planned",
      ownedServerAbsentAfterStop: true,
      accounting: { stopReason: "materialize-error", error: 1, attempted: 0 },
    });
    expect(existsSync(join(s.outDir, "M1", "receipt.json"))).toBe(true);
  });

  it("rejects fixed-distribution mode as not implemented", async () => {
    const s = setup({ mode: "fixed-distribution" });
    const msgs = [];
    expect(
      await execute({
        profilePath: s.profilePath,
        label: "L2",
        command: "run",
        stderr: (m) => msgs.push(m),
      }),
    ).toBe(EXIT.notImplemented);
    expect(JSON.parse(msgs[0]).outcome).toBe("not-implemented");
    expect(existsSync(join(s.outDir, "L2"))).toBe(false);
  });
});
