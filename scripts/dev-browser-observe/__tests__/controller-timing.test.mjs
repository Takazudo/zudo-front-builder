import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import { ControllerTiming } from "../lib/controller-timing.mjs";
import { sample } from "../lib/mac-process-adapter.mjs";
import { snapshotOwned } from "../lib/process-owner.mjs";
import { semanticErrors } from "../lib/profile.mjs";
import { serverEnv } from "../lib/server.mjs";
import { HARNESS_DIR } from "../lib/util.mjs";
import { Recorder } from "../lib/recorder.mjs";
import { inspectRendererLifecycle } from "../lib/renderer-memory-evidence.mjs";
import { createRendererMemoryStudy } from "../lib/renderer-memory-study.mjs";
import { TINY_ACCEPTANCE } from "../lib/startup-acceptance.mjs";

const io = vi.hoisted(() => ({ onFsync: null }));
vi.mock("node:fs", async (original) => {
  const fs = await original();
  return {
    ...fs,
    fsyncSync(fd) {
      io.onFsync?.();
      return fs.fsyncSync(fd);
    },
  };
});

afterEach(() => {
  io.onFsync = null;
  vi.useRealTimers();
});

function harness({ sampleImpl, server, inspect = inspectRendererLifecycle } = {}) {
  let ns = 0n;
  const clock = () => ns;
  const advance = (ms) => {
    ns += BigInt(ms) * 1_000_000n;
  };
  const timing = new ControllerTiming(clock);
  const records = [];
  const study = createRendererMemoryStudy({
    protocol: TINY_ACCEPTANCE,
    tinyProbe: "B",
    timing,
    clock,
    supervisorStartedMs: 0,
    workDeadline: 120_000,
    append: (row) => records.push(row),
    inspect,
    sample:
      sampleImpl ??
      (async () => ({
        hrtimeNs: String(ns),
        helper: { pid: 123, spawnHrtimeNs: String(ns) },
        timing: { parent: { closeNs: String(ns) } },
        summedRssKiB: 1,
      })),
  });
  const owned = server ?? { isAlive: () => ({ alive: true }), provSince: () => ({ lines: [] }) };
  return { study, timing, records, clock, advance, owned };
}

it("distinguishes a missed dispatch with no inflight helper from helper elapsed time", async () => {
  vi.useFakeTimers();
  const { study, timing, advance, owned } = harness();
  await study.startSampling(owned);
  study.enter("edits");
  study.setActiveEdit(8);
  advance(1000);
  await vi.advanceTimersByTimeAsync(1000);
  advance(15_558);
  await vi.advanceTimersByTimeAsync(1000);
  await study.beginShutdown();
  const [, first, late, cancelled] = timing.report.dispatches;
  expect(first).toMatchObject({ intervalCallbackOrdinal: 1, inFlightAtRequest: false });
  expect(late).toMatchObject({
    source: "interval",
    intervalOriginNs: "0",
    intervalCallbackOrdinal: 2,
    activeEdit: 8,
    nominalDueNs: "2000000000",
    callbackEntryNs: "16558000000",
    samplerEntryNs: "16558000000",
    coreSamplerEntryNs: "16558000000",
    previousCallbackEntryNs: "1000000000",
    previousCoreEntryNs: "1000000000",
    previousHelperCloseNs: "1000000000",
    previousPeriodicSamplerEntryNs: "1000000000",
    skippedIntervals: 14,
    inFlightAtRequest: false,
  });
  expect(cancelled).toMatchObject({
    outcome: "cancelled",
    intervalCallbackOrdinal: null,
    callbackEntryNs: null,
    samplerEntryNs: null,
  });
});

it("records a queued boundary separately from an overlapping periodic callback", async () => {
  vi.useFakeTimers();
  let release;
  const { study, timing, owned } = harness({
    sampleImpl: vi
      .fn()
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            release = () => resolve({ summedRssKiB: 10 });
          }),
      )
      .mockResolvedValue({ summedRssKiB: 10 }),
  });
  const pending = study.startSampling(owned);
  const boundary = study.afterAttempt(
    { scenario: "edits", iteration: { index: 1 }, outcome: "success" },
    0n,
  );
  await vi.advanceTimersByTimeAsync(1000);
  release();
  await Promise.all([pending, boundary]);
  await study.beginShutdown();
  expect(timing.report.dispatches.map((row) => [row.source, row.outcome])).toEqual([
    ["initial", "started"],
    ["boundary", "started"],
    ["interval", "overlap"],
    ["interval", "cancelled"],
  ]);
  expect(timing.report.dispatches[1]).toMatchObject({
    waitedForInflight: true,
    boundary: { scenario: "edits" },
  });
});

it("refuses the 161st sampler invocation at the shared dispatch gate", async () => {
  const sampler = vi.fn().mockResolvedValue({ summedRssKiB: 1, helper: { pid: 1 } });
  const { study, timing, owned } = harness({ sampleImpl: sampler });
  try {
    await study.startSampling(owned);
    for (let i = 1; i < 160; i++) await study.afterAttempt({}, 0n);
    await expect(study.afterAttempt({}, 0n)).rejects.toThrow("renderer-memory-helper-cap");
    expect(sampler).toHaveBeenCalledTimes(160);
    expect(study.failure).toMatchObject({ sampleInvocations: 160, helperStarts: 160 });
    expect(timing.report.dispatches.at(-1).outcome).toBe("cap");
  } finally {
    await study.beginShutdown();
  }
});

it("brackets fsync, validated input and synchronous ps and birth reads including controlled delays", async () => {
  const dir = mkdtempSync(join(tmpdir(), "dbo-controller-timing-"));
  const { timing, study, advance, owned } = harness();
  const duration = (row) => Number(BigInt(row.endNs) - BigInt(row.startNs)) / 1e6;
  try {
    io.onFsync = () => advance(17);
    const path = join(dir, "records.jsonl");
    new Recorder(path, timing).append({ type: "identity" });
    expect(JSON.parse(readFileSync(path, "utf8")).seq).toBe(0);
    const [append, open, write, fsync, close] = timing.report.stages;
    expect([append, open, write, fsync, close].map((row) => [row.name, duration(row)])).toEqual([
      ["append", 17],
      ["open", 0],
      ["write", 0],
      ["fsync", 17],
      ["close", 0],
    ]);
    expect([open, write, fsync, close].every((row) => row.parentId === append.id)).toBe(true);
    owned.provSince = () => {
      advance(31);
      return { lines: [{ text: "ignored", ns: "0" }], droppedBeforeMark: 0 };
    };
    await study.startSampling(owned);
    const inspection = timing.report.stages.find((row) => row.name === "checkTrace.inspect");
    expect(inspection).toMatchObject({ inputLines: 1, inputBytes: 7, mutationCount: 0 });
    expect(duration(inspection)).toBe(31);
    const native = {
      pid: 12,
      ppid: 1,
      pgid: 12,
      birthVersion: "linux-proc-v1",
      startTicks: "5",
      bootId: "boot",
    };
    const handle = { native, known: new Map([[12, native]]) };
    const rows = snapshotOwned(
      handle,
      () => {
        advance(53);
        return "12 1 12 Mon Jan 1 00:00:00 2024 node";
      },
      "linux",
      () => {
        advance(7);
        return native;
      },
      timing,
    );
    expect(rows).toHaveLength(1);
    const sampled = sample(rows, {
      timing,
      runImpl: () => {
        advance(13);
        return "12 144 200";
      },
    });
    expect(sampled.summedRssKiB).toBe(144);
    const stages = timing.report.stages;
    expect(duration(stages.find((row) => row.name === "process-table ps"))).toBe(53);
    expect(stages.filter((row) => row.name === "proc/birth").map(duration)).toEqual([7, 7, 7]);
    expect(duration(stages.find((row) => row.name === "RSS ps"))).toBe(13);
    expect(stages.find((row) => row.name === "thread reads")).toMatchObject({ pid: 12 });
  } finally {
    await study.beginShutdown();
    rmSync(dir, { recursive: true, force: true });
  }
});

it("limits the tiny command to the pinned fixture, declared trace and standalone edit schedule", () => {
  const profile = JSON.parse(
    readFileSync(join(HARNESS_DIR, "profiles/tiny-internal-a0.example.json")),
  );
  profile.diagnostic = true;
  delete profile.calibration;
  profile.server.readyTimeoutMs = 90_000;
  profile.server.env = { ZFB_DEV_PROVENANCE_TRACE: "1" };
  profile.limits.wallCeilingMs = 180_000;
  profile.scenarios = [];
  profile.tinyAcceptance = { protocol: TINY_ACCEPTANCE, probe: "A" };
  expect(semanticErrors(profile)).toEqual([]);
  const inherited = {
    ZFB_DEV_TIMING: "1",
    ZFB_DEV_RENDERER_MEMORY_ALLOCATOR: "4",
    ZFB_TEST: "enabled",
  };
  expect(serverEnv(inherited, profile.server.env, true, true).zfbEnv).toEqual(profile.server.env);
  profile.server.env.ZFB_DEV_RENDERER_MEMORY_ALLOCATOR = "4";
  expect(semanticErrors(profile).join(" ")).toMatch(/provenance tracing only/);
  delete profile.server.env.ZFB_DEV_RENDERER_MEMORY_ALLOCATOR;
  profile.tinyAcceptance.probe = "B";
  expect(semanticErrors(profile).join(" ")).toMatch(/12 serial/);
  Object.assign(profile.tinyAcceptance, {
    browserExecutable: "/pinned",
    browserExecutableSha256: "b".repeat(64),
  });
  const op = {
    op: "edit-body",
    article: "a",
    writeMethod: "in-place",
    repeat: { warmups: 0, measured: 12 },
  };
  profile.scenarios = [{ id: "body", ops: [op] }];
  expect(semanticErrors(profile)).toEqual([]);
  op.predicate = { kind: "count", selector: "div", equals: 0 };
  expect(semanticErrors(profile).join(" ")).toMatch(/12 serial/);
  delete op.predicate;
  profile.limits.wallCeilingMs++;
  expect(semanticErrors(profile).join(" ")).toMatch(/180 s total/);
  profile.limits.wallCeilingMs--;
  profile.fixture.adapter = "A1";
  expect(semanticErrors(profile).join(" ")).toMatch(/two-article A0/);
});
