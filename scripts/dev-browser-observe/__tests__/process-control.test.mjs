import {
  chmodSync,
  cpSync,
  existsSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { spawn } from "node:child_process";
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
  shouldRegisterOwned,
  snapshotOwned,
  spawnOwned,
  stopOwned,
} from "../lib/process-owner.mjs";
import { EXIT, execute, verifyIdentityOnly } from "../lib/run.mjs";
import { loadFieldNotesFixture, sourceTreeHashes } from "../lib/fieldnotes.mjs";
import { makeFakeSource } from "./fieldnotes-fake.mjs";
import { SERVER_LINES_KEPT, injectKillOwned, startDevServer } from "../lib/server.mjs";
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

  it("returns never-spawned, without signalling, when the server was never spawned", () => {
    const kills = [];
    expect(injectKillOwned({ native: null, exit: null }, (...a) => kills.push(a))).toEqual({
      ok: false,
      reason: "never-spawned",
    });
    expect(kills).toEqual([]);
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

function shellServer(dir, body) {
  const exe = join(dir, "fake-zfb.sh");
  writeFileSync(exe, `#!/bin/sh\n${body}\n`);
  chmodSync(exe, 0o755);
  return exe;
}

describe("round 5: dev-server stream handling on real processes", () => {
  it("#4 a ready URL split across writes resolves to the complete URL", async () => {
    const dir = tmp();
    const exe = shellServer(
      dir,
      `printf 'info listening on 127.0.0.1:1234 (routes pending)\\n'; printf '→ ready on http://127.0.0.1:12'; sleep 0.3; printf '34\\n'; sleep 30`,
    );
    const server = startDevServer({
      exe,
      appDir: dir,
      port: { mode: "ephemeral" },
      readyTimeoutMs: 5000,
      logPath: join(dir, "server.log"),
    });
    children.push(server.handle);
    const ready = await server.ready;
    expect(ready.baseUrl).toBe("http://127.0.0.1:1234");
    await stopOwned(server.handle, { intWaitMs: 1000, termWaitMs: 1000, killWaitMs: 1000 });
    await server.closeLog();
  });

  it("#1/#10 late output after stop never crashes, the log closes, and kept lines are capped", async () => {
    const dir = tmp();
    const exe = shellServer(
      dir,
      `trap '' INT; printf '→ ready on http://127.0.0.1:9\\n'; seq 1 ${SERVER_LINES_KEPT + 500}; while true; do echo tick; sleep 0.01; done`,
    );
    const server = startDevServer({
      exe,
      appDir: dir,
      port: { mode: "ephemeral" },
      readyTimeoutMs: 5000,
      logPath: join(dir, "server.log"),
    });
    children.push(server.handle);
    await server.ready;
    const mark0 = 0;
    await delay(300);
    const report = await stopOwned(server.handle, {
      intWaitMs: 300,
      termWaitMs: 1000,
      killWaitMs: 1000,
    });
    expect(report.absent).toBe(true);
    const closed = await server.closeLog(2000);
    expect(closed.errors).toEqual([]);
    server.log.write("late line after close\n");
    expect(server.log.droppedAfterEnd).toBeGreaterThanOrEqual(1);
    const since = server.logSince(mark0, 0n);
    expect(since.droppedBeforeMark).toBeGreaterThan(0);
    expect(readFileSync(join(dir, "server.log"), "utf8")).toContain("ready on http://127.0.0.1:9");
  });
});

describe("round 5 #5: unverifiable spawn birth", () => {
  it("records the birth as unverifiable and still stops the process within the spawn-time pgid scope", async () => {
    const h = spawnOwned(NODE, ["-e", "setInterval(() => {}, 1000)"], { birthImpl: () => null });
    children.push(h);
    expect(h.native).toMatchObject({
      birthUnverifiable: true,
      lstart: "unavailable",
      birthAttempts: 5,
    });
    await delay(1100);
    const report = await stopOwned(h, { intWaitMs: 2000, termWaitMs: 1000, killWaitMs: 1000 });
    expect(report.mode).toBe("unverifiable-birth-pgid-scope");
    expect(report.endings.find((e) => e.pid === h.native.pid)).toMatchObject({
      endedAfter: "SIGINT",
    });
    expect(report.absent).toBe(true);
    expect(h.exit).toMatchObject({ signalCode: "SIGINT" });
  });
});

describe("round 7 #1: an unverifiable root is registered with its real birth", () => {
  it("snapshot registers the root once ps shows it, and the registry row then guards the next batch", async () => {
    const h = spawnOwned(NODE, ["-e", "setInterval(() => {}, 1000)"], { birthImpl: () => null });
    children.push(h);
    const registered = [];
    h.onNewOwned = (p) => {
      if (shouldRegisterOwned(h.native, p)) registered.push(p);
    };
    snapshotOwned(h);
    expect(registered.map((p) => p.pid)).toEqual([h.native.pid]);
    expect(registered[0].lstart).not.toBe("unavailable");
    const spawnRow = { type: "spawn", ...h.native };
    expect(liveOwnedFromRegistry([spawnRow])).toHaveLength(1);
    expect(liveOwnedFromRegistry([{ type: "owned", ...registered[0] }])).toHaveLength(1);
    process.kill(h.native.pid, "SIGKILL");
    await h.exited;
    expect(liveOwnedFromRegistry([spawnRow, { type: "owned", ...registered[0] }])).toHaveLength(0);
  });
});

describe("round 5 #3: hold-lock exits after --ms even with stdin left open", () => {
  it("exits 0 on its own", async () => {
    const root = tmp();
    const profile = JSON.parse(
      readFileSync(join(HARNESS_DIR, "profiles", "tiny-internal-a0.example.json"), "utf8"),
    );
    Object.assign(profile, { outDir: join(root, "out"), buildReceipt: join(root, "none.json") });
    profile.fixture.dir = FIXTURE;
    writeFileSync(join(root, "p.json"), JSON.stringify(profile));
    const child = spawn(
      NODE,
      [join(HARNESS_DIR, "cli.mjs"), "hold-lock", "--profile", join(root, "p.json"), "--ms", "300"],
      { stdio: ["pipe", "pipe", "pipe"] },
    );
    const exit = await Promise.race([
      new Promise((r) => child.once("exit", (code) => r(code))),
      delay(8000).then(() => "hung"),
    ]);
    if (exit === "hung") child.kill("SIGKILL");
    expect(exit).toBe(0);
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

  describe("lock-contention-check deadline", () => {
    const hanging =
      (lines = "") =>
      (args) => {
        const child = spawn(
          process.execPath,
          [
            "-e",
            `process.stdout.write(${JSON.stringify(args[0] === "hold-lock" ? lines : "")}); setInterval(() => {}, 1000)`,
          ],
          { stdio: ["pipe", "pipe", "pipe"] },
        );
        return {
          child,
          done: new Promise((resolve) =>
            child.once("exit", (code, signal) => resolve({ code, signal, stdout: "", stderr: "" })),
          ),
        };
      };
    const withOnLine =
      (impl) =>
      (args, opts = {}) => {
        const started = impl(args);
        let text = "";
        started.child.stdout.on("data", (d) => {
          text += d.toString();
          opts.onLine?.(text);
        });
        return started;
      };

    it("on expiry kills the birth-verified holder and second run and reports timedOut", async () => {
      const s = setup();
      const kills = [];
      const pids = [];
      const runNodeImpl = withOnLine((args) => {
        const started = hanging("LOCKED fake\n")(args);
        pids.push(started.child.pid);
        return started;
      });
      const t0 = Date.now();
      const r = await lockContentionCheck(s.profilePath, {
        deadlineMs: 1500,
        runNodeImpl,
        killImpl: (pid, signal) => {
          kills.push([pid, signal]);
          process.kill(pid, signal);
        },
      });
      expect(Date.now() - t0).toBeLessThan(10_000);
      expect(r).toMatchObject({ ok: false, timedOut: true, deadlineMs: 1500 });
      expect(r.children.map((c) => [c.role, c.killed, c.exitSeen])).toEqual([
        ["holder", true, true],
        ["second-run", true, true],
      ]);
      expect(kills).toEqual(pids.map((pid) => [pid, "SIGKILL"]));
      for (const pid of pids) expect(birthIdentity(pid)).toBeNull();
    });

    it("does not kill a child whose birth identity no longer matches", async () => {
      const s = setup();
      const kills = [];
      let holderPid = null;
      const runNodeImpl = withOnLine((args) => {
        const started = hanging("LOCKED fake\n")(args);
        if (args[0] === "hold-lock") holderPid = started.child.pid;
        return started;
      });
      let checks = 0;
      const birthImpl = (pid) => {
        const b = birthIdentity(pid);
        if (pid === holderPid && b && ++checks > 1)
          return { ...b, lstart: "Mon Jan 1 00:00:00 2001" };
        return b;
      };
      try {
        const r = await lockContentionCheck(s.profilePath, {
          deadlineMs: 1000,
          runNodeImpl,
          birthImpl,
          killImpl: (pid, signal) => {
            kills.push([pid, signal]);
            process.kill(pid, signal);
          },
        });
        expect(r).toMatchObject({ ok: false, timedOut: true });
        expect(r.children.find((c) => c.role === "holder")).toMatchObject({
          killed: false,
          reason: "birth-unverified",
        });
        expect(kills.some(([pid]) => pid === holderPid)).toBe(false);
        expect(r.children.find((c) => c.role === "second-run")).toMatchObject({ killed: true });
      } finally {
        if (holderPid && birthIdentity(holderPid)) process.kill(holderPid, "SIGKILL");
      }
    });
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

  function acceptedIdentity(s, exe) {
    const repo = gitToplevel(HARNESS_DIR);
    const sha = gitHead(repo);
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
    const p = JSON.parse(readFileSync(s.profilePath, "utf8"));
    p.productSha = sha;
    p.server.readyTimeoutMs = 300;
    writeFileSync(s.profilePath, JSON.stringify(p));
  }

  it("round 5 #2: a stop that cannot verify absence exits 6 and records stopFailure", async () => {
    const s = setup();
    const exe = shellServer(s.root, "sleep 30");
    acceptedIdentity(s, exe);
    const code = await execute({
      profilePath: s.profilePath,
      label: "S6",
      command: "run",
      stderr: () => {},
      hooks: {
        stopOwned: async (handle, opts) => {
          const real = await stopOwned(handle, opts);
          return { ...real, absent: false, simulated: "survivor" };
        },
      },
    });
    expect(code).toBe(EXIT.ownedServerAlive);
    const summary = readJson(join(s.outDir, "S6", "summary.json"));
    expect(summary).toMatchObject({
      exitCode: EXIT.ownedServerAlive,
      ownedServerAbsentAfterStop: false,
      stopFailure: { absent: false, exitBefore: EXIT.serverNotReady },
    });
    expect(readJsonl(join(s.outDir, "S6", "records.jsonl")).at(-1)).toMatchObject({
      type: "run-end",
      exitCode: EXIT.ownedServerAlive,
    });
  });

  it("round 5 #2: a throwing stop also exits 6", async () => {
    const s = setup();
    const exe = shellServer(s.root, "sleep 30");
    acceptedIdentity(s, exe);
    const code = await execute({
      profilePath: s.profilePath,
      label: "S7",
      command: "run",
      stderr: () => {},
      hooks: {
        stopOwned: async (handle, opts) => {
          await stopOwned(handle, opts);
          throw Object.assign(new Error("birth identity mismatch"), { code: "PID_REUSE" });
        },
      },
    });
    expect(code).toBe(EXIT.ownedServerAlive);
    expect(readJson(join(s.outDir, "S7", "summary.json")).stopFailure).toMatchObject({
      error: "birth identity mismatch",
      code: "PID_REUSE",
    });
  });

  it("round 7 #3: the server is stopped before the browser closes, and a failing record write skips neither stop nor summary", async () => {
    const s = setup();
    const exe = shellServer(s.root, `printf '→ ready on http://127.0.0.1:9\\n'; sleep 30`);
    acceptedIdentity(s, exe);
    const p = JSON.parse(readFileSync(s.profilePath, "utf8"));
    p.server.readyTimeoutMs = 5000;
    p.calibration.cells = [];
    writeFileSync(s.profilePath, JSON.stringify(p));
    const order = [];
    const msgs = [];
    const code = await execute({
      profilePath: s.profilePath,
      label: "C7",
      command: "calibrate",
      stderr: (m) => msgs.push(m),
      hooks: {
        launchOwnedBrowser: async () => ({
          browser: { close: async () => order.push("browser.close") },
          context: { close: async () => order.push("context.close") },
          observer: {},
          identity: { engine: "fake" },
        }),
        stopOwned: async (handle, opts) => {
          order.push("server-stop");
          chmodSync(join(s.outDir, "C7", "records.jsonl"), 0o444);
          return stopOwned(handle, opts);
        },
      },
    });
    expect(order).toEqual(["server-stop", "context.close", "browser.close"]);
    expect(code).toBe(EXIT.ok);
    const summary = readJson(join(s.outDir, "C7", "summary.json"));
    expect(summary).toMatchObject({ exitCode: EXIT.ok, ownedServerAbsentAfterStop: true });
    expect(existsSync(join(s.outDir, "C7", "receipt.json"))).toBe(true);
    expect(msgs.join("\n")).toMatch(/record write failed \(server-stop\)/);
    expect(msgs.join("\n")).toMatch(/record write failed \(run-end\)/);
    chmodSync(join(s.outDir, "C7", "records.jsonl"), 0o644);
  });

  it("round 5 #8: a lock failure that is not contention exits controllerError, not 5", async () => {
    const s = setup();
    const p = JSON.parse(readFileSync(s.profilePath, "utf8"));
    p.lockPath = join(s.root, "no-such-dir", "x.lock");
    writeFileSync(s.profilePath, JSON.stringify(p));
    const msgs = [];
    expect(
      await execute({
        profilePath: s.profilePath,
        label: "L8",
        command: "run",
        stderr: (m) => msgs.push(m),
      }),
    ).toBe(EXIT.controllerError);
    expect(msgs.join("\n")).toMatch(/not contention/);
  });

  it("--scenario runs only the named scenarios and records the selection; an unknown id is a usage error", async () => {
    const s = setup();
    const msgs = [];
    expect(
      await execute({
        profilePath: s.profilePath,
        label: "SC0",
        command: "run",
        stderr: (m) => msgs.push(m),
        scenarios: ["nope"],
      }),
    ).toBe(EXIT.usage);
    expect(msgs.join("\n")).toMatch(/unknown --scenario/);
    const p = JSON.parse(readFileSync(s.profilePath, "utf8"));
    const first = p.scenarios[0].id;
    const code = await execute({
      profilePath: s.profilePath,
      label: "SC1",
      command: "run",
      stderr: () => {},
      scenarios: [first],
    });
    expect(code).toBe(EXIT.identityRejected);
    const records = readJsonl(join(s.outDir, "SC1", "records.jsonl"));
    expect(records[0]).toMatchObject({ type: "run-start", scenarioSelection: [first] });
    expect(readJson(join(s.outDir, "SC1", "summary.json")).accounting.planned).toBe(
      p.scenarios[0].ops.length,
    );
    expect(readJson(join(s.outDir, "SC1", "receipt.json")).scenarioSelection).toEqual([first]);
  });

  it("field-notes verify-identity checks the source pins, the dataset declaration and adapter provenance", async () => {
    const root = tmp();
    const src = join(root, "src");
    makeFakeSource(src);
    const fixtureDir = join(HARNESS_DIR, "fixtures", "field-notes");
    const manifest = loadFieldNotesFixture(fixtureDir);
    const profile = JSON.parse(
      readFileSync(
        join(HARNESS_DIR, "profiles", "field-notes-original-memoized.example.json"),
        "utf8",
      ),
    );
    profile.fixture.dir = fixtureDir;
    profile.fixture.source = { dir: src, ...sourceTreeHashes(src, manifest) };
    Object.assign(profile, { buildReceipt: join(root, "none.json"), outDir: join(root, "out") });
    profile.pins = currentPins({
      fixtureDir,
      observerPath: OBSERVER_PATH,
      adapterPath: join(fixtureDir, manifest.adapters.variants["original-memoized"].file),
    });
    writeFileSync(join(root, "p.json"), JSON.stringify(profile));
    const v = verifyIdentityOnly(join(root, "p.json"));
    const byName = Object.fromEntries(v.checks.map((c) => [c.name, c]));
    for (const k of ["appsZfbTreeSha256", "contentTreeSha256", "sharedTreeSha256", "datasetSha256"])
      expect(byName[`source-pin:${k}`].ok, k).toBe(true);
    expect(byName["adapter-provenance:original-memoized"].ok).toBe(true);
    expect(byName["managed-css-addition"].ok).toBe(true);
    expect(byName["source-dataset-matches-manifest-declaration"]).toMatchObject({
      ok: false,
      expected: manifest.source.datasetSha256,
    });
    writeFileSync(join(src, "content", "articles", "article-00001.md"), "tampered\n");
    const v2 = verifyIdentityOnly(join(root, "p.json"));
    expect(v2.checks.find((c) => c.name === "source-pin:contentTreeSha256").ok).toBe(false);
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
