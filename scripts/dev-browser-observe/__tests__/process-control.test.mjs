import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { OBSERVER_PATH } from "../lib/browser.mjs";
import { fixtureTreeHash, currentPins, BUILD_RECEIPT_SCHEMA } from "../lib/identity.mjs";
import { LockBusyError, acquireLock } from "../lib/lock.mjs";
import { sample } from "../lib/mac-process-adapter.mjs";
import {
  PidReuseError,
  birthIdentity,
  liveOwnedFromRegistry,
  spawnOwned,
  stopOwned,
} from "../lib/process-owner.mjs";
import { EXIT, execute } from "../lib/run.mjs";
import { runBatch } from "../lib/state-machine.mjs";
import { HARNESS_DIR, delay, readJson, readJsonl } from "../lib/util.mjs";
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
    const report = await stopOwned(h, { termWaitMs: 3000, killWaitMs: 2000 });
    expect(report.rootAlive).toBe(true);
    expect(report.ownedBefore.length).toBeGreaterThanOrEqual(2);
    expect(report.absent).toBe(true);
    expect(report.exit).toMatchObject({ exitCode: null, signalCode: "SIGTERM" });
  });

  it("escalates to SIGKILL for a survivor that ignores SIGTERM", async () => {
    const h = own(["-e", "process.on('SIGTERM', () => {}); setInterval(() => {}, 1000)"]);
    await delay(300);
    const report = await stopOwned(h, { termWaitMs: 400, killWaitMs: 2000 });
    expect(report.steps.map((s) => s.signal)).toEqual(["SIGTERM", "SIGKILL"]);
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
      adapterPath: join(fixtureDir, "adapters/A0/content.ts"),
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
