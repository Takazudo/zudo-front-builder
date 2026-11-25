import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { aggregate } from "../lib/aggregate.mjs";
import { evaluateIdentity } from "../lib/identity.mjs";
import { semanticErrors } from "../lib/profile.mjs";
import {
  RENDERER_MEMORY_ALLOCATOR_ENV as allocatorEnv,
  RENDERER_MEMORY_ALLOCATOR_PROTOCOL as allocatorProtocol,
  RENDERER_MEMORY_FEATURE as feature,
  RENDERER_MEMORY_LIMITS as limits,
  RENDERER_MEMORY_POST_APPLY_PROTOCOL as postApplyProtocol,
  RENDERER_MEMORY_PROTOCOL as protocol,
} from "../lib/renderer-memory-protocol.mjs";
import { createRendererMemoryStudy } from "../lib/renderer-memory-study.mjs";
import { expandSchedule } from "../lib/schedule.mjs";
import { validateAs } from "../lib/schema.mjs";
import { serverEnv } from "../lib/server.mjs";
import { runBatch } from "../lib/state-machine.mjs";
import { HARNESS_DIR } from "../lib/util.mjs";
import { deps, fakeFs, fakePage, fakeServer } from "./fakes.mjs";

function profile(studyProtocol = protocol) {
  const input = JSON.parse(
    readFileSync(join(HARNESS_DIR, "profiles", "field-notes-direct-read.example.json")),
  );
  const [scenario] = input.scenarios;
  const allocator = studyProtocol !== protocol;
  scenario.ops[0].repeat = allocator ? { warmups: 0, measured: 12 } : { warmups: 2, measured: 10 };
  return {
    ...input,
    diagnostic: true,
    rendererMemoryStudy: {
      protocol: studyProtocol,
      browserExecutable: "/chromium/headless_shell",
      browserExecutableSha256: "a".repeat(64),
    },
    fixture: { ...input.fixture, scale: 100 },
    server: {
      ...input.server,
      env: {
        ZFB_DEV_PROVENANCE_TRACE: "1",
        ...(allocator && { [allocatorEnv]: studyProtocol === postApplyProtocol ? "2" : "1" }),
      },
      readyTimeoutMs: limits.readyTimeoutMs,
    },
    limits: limits.limits,
    timeouts: limits.timeouts,
    scenarios: [scenario],
  };
}

describe("renderer-memory profile and build identity", () => {
  it("admits only the bounded, source-built 100-article body sequence", () => {
    const accepted = profile();
    expect(validateAs("profile", accepted)).toEqual([]);
    expect(semanticErrors(accepted)).toEqual([]);
    expect(semanticErrors({ ...accepted, diagnostic: false })).toContain(
      "$.rendererMemoryStudy: requires diagnostic: true",
    );
    expect(
      semanticErrors({ ...accepted, limits: { ...limits.limits, wallCeilingMs: 301000 } }),
    ).toContain("$.rendererMemoryStudy: limits.wallCeilingMs must be 300000");
    accepted.scenarios[0].ops[0].repeat.measured += 1;
    expect(semanticErrors(accepted).join("\n")).toContain("12 consecutive");
  });

  it("requires explicit v2 opt-in and strips the inherited allocator switch", () => {
    const accepted = profile(allocatorProtocol);
    expect(validateAs("profile", accepted)).toEqual([]);
    expect(semanticErrors(accepted)).toEqual([]);
    expect(serverEnv({ [allocatorEnv]: "1" }, {}, true).removedDiagnosticEnv).toEqual([
      allocatorEnv,
    ]);
    expect(serverEnv({}, { [allocatorEnv]: "1" }, true).zfbEnv[allocatorEnv]).toBe("1");
    expect(serverEnv({}, { [allocatorEnv]: "1" }, false).zfbEnv[allocatorEnv]).toBeUndefined();
    accepted.scenarios[0].ops[0].repeat = { warmups: 2, measured: 10 };
    expect(semanticErrors(accepted).join("\n")).toContain("v2 requires zero warmup attempts");
    delete accepted.server.env[allocatorEnv];
    expect(semanticErrors(accepted).join("\n")).toContain(allocatorEnv);
    const old = profile();
    old.server.env[allocatorEnv] = "1";
    expect(semanticErrors(old).join("\n")).toContain("requires renderer-memory.v2");
  });

  it("requires the separate post-apply mode and preserves strict v2 profile admission", () => {
    const postApply = profile(postApplyProtocol);
    const locked = profile(allocatorProtocol);
    expect(validateAs("profile", postApply)).toEqual([]);
    expect(semanticErrors(postApply)).toEqual([]);
    expect(serverEnv({ [allocatorEnv]: "2" }, {}, true).zfbEnv[allocatorEnv]).toBeUndefined();
    expect(serverEnv({}, { [allocatorEnv]: "2" }, true).zfbEnv[allocatorEnv]).toBe("2");
    postApply.server.env[allocatorEnv] = "1";
    expect(semanticErrors(postApply).join("\n")).toContain(`${allocatorEnv}: "2"`);
    locked.server.env[allocatorEnv] = "2";
    expect(semanticErrors(locked).join("\n")).toContain(`${allocatorEnv}: "1"`);
  });

  it("matches the diagnostic build only to its declared study", () => {
    const diagnosticBuild = {
      diagnostic: true,
      build: { features: [feature], profile: "dev", noDefaultFeatures: false },
    };
    const facts = { localTakazudo: [], pins: {} };
    const check = (p, receipt) =>
      evaluateIdentity(p, receipt, facts).checks.find(
        (c) => c.name === "renderer-memory-diagnostic-build",
      );
    expect(check(profile(), diagnosticBuild)?.ok).toBe(true);
    expect(check({}, diagnosticBuild)?.ok).toBe(false);
    expect(check(profile(), { build: { features: [] } })?.ok).toBe(false);
    expect(check({}, { build: { features: [] } })).toBeUndefined();
  });

  it.each([null, protocol, allocatorProtocol, postApplyProtocol])(
    "excludes diagnostic receipt %s from timing",
    (studyProtocol) => {
      const root = mkdtempSync(join(tmpdir(), "zfb-memory-aggregate-"));
      try {
        const dir = join(root, "diagnostic");
        mkdirSync(dir);
        writeFileSync(
          join(dir, "receipt.json"),
          JSON.stringify({
            ...(studyProtocol
              ? { profile: { rendererMemoryStudy: { protocol: studyProtocol } } }
              : { buildReceipt: { build: { features: [`zfb/${feature}`] } }, profile: {} }),
          }),
        );
        writeFileSync(
          join(dir, "records.jsonl"),
          JSON.stringify({
            type: "attempt",
            op: "edit-body",
            scenario: "body",
            outcome: "success",
            latencyMs: 10,
          }),
        );
        const { report } = aggregate({ sessions: [{ dir, variant: "A", label: "diagnostic" }] });
        expect(report.sessions[0]).toMatchObject({ timingExcluded: true, blocks: [] });
        expect(report.pooled).toEqual({});
        expect(report.pooledComparable).toBe(false);
      } finally {
        rmSync(root, { recursive: true });
      }
    },
  );
});

async function simulate({
  failPoll = null,
  workDeadline = 240_000,
  routeFailure = false,
  studyProtocol = protocol,
  mappingFailure = null,
  missingPostApply = null,
  collectFailure = null,
} = {}) {
  let now = 0n;
  const clock = () => now;
  const sleep = vi.fn(async (ms) => {
    now += BigInt(ms) * 1_000_000n;
  });
  const server = fakeServer();
  const fs = fakeFs();
  const restore = fs.restore;
  fs.files["article-00005.md"] = "original body";
  fs.restore = (snapshot) => {
    const output = restore(snapshot);
    expect(fs.files["article-00005.md"]).toBe("original body");
    return { ...output, actions: [{ path: "article-00005.md", method: "in-place" }] };
  };
  fs.liveChecks = () =>
    [
      "/",
      "/articles/article-00001",
      "/articles/article-00005",
      "/archive/1",
      "/archive/2",
      "/category/notes",
    ].map((path) => ({ path, predicate: { kind: "present" } }));
  let liveChecks = 0;
  let edits = 0;
  const page = fakePage({
    liveCheck(check) {
      liveChecks += 1;
      if (check.path === "/articles/article-00005")
        expect(check.predicate.of.at(-1).equals).toBe("original body");
      return { ok: !routeFailure };
    },
    onWait(token) {
      edits += 1;
      return Promise.resolve(
        edits === failPoll
          ? { kind: "timeout" }
          : {
              kind: "notify",
              payload: { token, docUUID: `revision-${edits}` },
              t1: clock() + 1_000_000n,
            },
      );
    },
  });
  const collectAttempt = page.collectAttempt;
  page.collectAttempt = (...args) => {
    if (edits === collectFailure) throw new Error("browser evidence collection failed");
    return collectAttempt(...args);
  };
  const arm = page.arm;
  page.arm = async (...args) => ({ ...(await arm(...args)), actual: fs.files["article-00005.md"] });
  page.settle = async ({ bodyRevision }) => {
    expect(bodyRevision.predicate.equals).toBe("original body");
    return { settled: true, nativeUpdateObserved: true, bodyRevision: { matched: true } };
  };
  const d = deps({ fs, page, server, clock: { now: clock } });
  const study = createRendererMemoryStudy({
    append: (r) => d.recorder.append(r),
    protocol: studyProtocol,
    captureMappings: async (_server, boundary) => {
      if (boundary === mappingFailure) throw new Error("mapping input exceeded cap");
      return { boundary, edits, liveChecks, restores: fs.restores, elapsedNs: String(now) };
    },
    supervisorStartedMs: 0,
    workDeadline,
    clock,
    sleep,
    sample: async () => ({ summedRssKiB: 1000 }),
    inspect: (_server, { mutations, final, withPostApply }) => ({
      failure: null,
      start: {},
      complete: final,
      ...(withPostApply && {
        postApply: {
          endpoints:
            missingPostApply !== null ? mutations.slice(0, missingPostApply - 1) : mutations,
          complete: missingPostApply === null && mutations.length === 13,
        },
      }),
    }),
  });
  try {
    study.startSampling(server);
    await study.beforeEdits();
    const { accounting, stopped } = await runBatch({
      scenarios: expandSchedule(profile(studyProtocol)),
      deps: {
        ...d,
        timeouts: limits.timeouts,
        rendererMemory: study,
        signal: study.signal,
        verifyBodyRevision: true,
        beforeCleanup: () => study.enter("restoration"),
      },
    });
    await study.finalIdle({
      stopped,
      stopReason: accounting.stopReason,
      cleanup: d.recorder.records.findLast((r) => r.type === "cleanup"),
    });
    study.beginShutdown();
    const report = study.finish({
      logClose: { pipes: "closed" },
      completedSchedule: study.idleMs !== null,
      serverAbsent: true,
    });
    return { accounting, records: d.recorder.records, fs, liveChecks, study, sleep, report };
  } finally {
    study.finishSampling();
  }
}

describe("bounded renderer-memory schedule", () => {
  it("records all twelve edits, restores once, verifies six routes, then idles with the server alive", async () => {
    const { accounting, records, fs, liveChecks, study, sleep } = await simulate();
    expect(accounting).toMatchObject({ planned: 12, attempted: 12, success: 12, notAttempted: 0 });
    expect(fs.writes).toHaveLength(12);
    expect(fs.restores).toBe(1);
    expect(liveChecks).toBe(6);
    const boundaries = records.filter(
      (r) => r.type === "renderer-memory-sample" && r.boundary?.opId,
    );
    expect(boundaries).toHaveLength(12);
    expect(boundaries.map((r) => r.boundary.iteration.kind)).toEqual([
      "warmup",
      "warmup",
      ...Array(10).fill("measured"),
    ]);
    expect(records.filter((r) => r.type === "renderer-memory-phase").map((r) => r.phase)).toEqual([
      "startup",
      "edits",
      "restoration",
      "route-verification",
      "idle",
      "shutdown",
    ]);
    expect(sleep.mock.calls.map(([ms]) => ms)).toEqual([60_000]);
    expect(study.idleMs).toBe(60_000);
  });

  it("stops on first edit failure, restores, and omits route checks and idle", async () => {
    const { accounting, fs, liveChecks, sleep } = await simulate({ failPoll: 3 });
    expect(accounting).toMatchObject({ attempted: 3, success: 2, timeout: 1, notAttempted: 9 });
    expect(fs.restores).toBe(1);
    expect(liveChecks).toBe(0);
    expect(sleep).not.toHaveBeenCalled();
  });

  it("stops a failed route check and omits idle", async () => {
    const { fs, liveChecks, sleep } = await simulate({ routeFailure: true });
    expect(fs.restores).toBe(1);
    expect(liveChecks).toBe(1);
    expect(sleep).not.toHaveBeenCalled();
  });

  it("refuses partial idle when the full duration exceeds the remaining work budget", async () => {
    const { study, sleep, records } = await simulate({ workDeadline: 59_999 });
    expect(study.failure?.reason).toBe("insufficient-final-idle-budget");
    expect(study.idleMs).toBeNull();
    expect(sleep).not.toHaveBeenCalled();
    expect(records.find((r) => r.type === "renderer-memory-idle")).toMatchObject({
      step: "skipped",
    });
  });
});

describe("v2 mapping boundaries", () => {
  it("takes exactly four observations in workload order with twelve edits and an intact idle", async () => {
    const result = await simulate({ studyProtocol: allocatorProtocol });
    expect(result.accounting).toMatchObject({ attempted: 12, success: 12, notAttempted: 0 });
    const mapping = result.records.filter((row) => row.type === "renderer-memory-mapping");
    expect(
      mapping.map((row) => [
        row.boundary,
        row.phase,
        row.result.edits,
        row.result.liveChecks,
        row.result.restores,
        row.result.elapsedNs,
      ]),
    ).toEqual([
      ["before-edits", "startup", 0, 0, 0, "0"],
      ["after-edits", "edits", 12, 0, 0, "0"],
      ["after-route-verification", "route-verification", 12, 6, 1, "0"],
      ["after-idle", "idle", 12, 6, 1, "60000000000"],
    ]);
    expect(mapping.every((row) => row.ok && row.protocol === allocatorProtocol)).toBe(true);
    expect(result.report.mappings.complete).toBe(true);
  });

  it("takes no later mapping after a failed attempt or mapping read and restores the source", async () => {
    for (const options of [{ failPoll: 3 }, { mappingFailure: "after-edits" }]) {
      const result = await simulate({ studyProtocol: allocatorProtocol, ...options });
      const mapping = result.records.filter((row) => row.type === "renderer-memory-mapping");
      expect(mapping.map((row) => [row.boundary, row.ok])).toEqual([
        ["before-edits", true],
        ...(options.mappingFailure ? [["after-edits", false]] : []),
      ]);
      expect(result.fs.restores).toBe(1);
      expect(result.liveChecks).toBe(0);
      expect(result.study.idleMs).toBeNull();
      expect(result.report.mappings.complete).toBe(false);
    }
  });

  it("keeps the one-second safety sampling cadence during a permitted mapping read", async () => {
    vi.useFakeTimers();
    const sampler = vi.fn(async () => ({ summedRssKiB: 100 }));
    const started = Date.now();
    let resolveMapping;
    const captureMappings = vi.fn(
      () =>
        new Promise((resolve) => {
          resolveMapping = resolve;
        }),
    );
    const study = createRendererMemoryStudy({
      append: () => {},
      protocol: allocatorProtocol,
      supervisorStartedMs: 0,
      workDeadline: 240_000,
      sample: sampler,
      inspect: () => ({ start: {} }),
      captureMappings,
      clock: () => BigInt(Date.now() - started) * 1_000_000n,
    });
    try {
      study.startSampling(fakeServer());
      const ready = study.beforeEdits();
      await vi.advanceTimersByTimeAsync(0);
      expect(captureMappings).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(1500);
      expect(sampler.mock.calls.length).toBeGreaterThan(1);
      expect(study.failure).toBeNull();
      resolveMapping({});
      await ready;
    } finally {
      study.finishSampling();
      vi.useRealTimers();
    }
  });
});

describe("post-apply workload admission", () => {
  it("matches each edit and the verified restoration before the existing live checks", async () => {
    const { records, sleep, report } = await simulate({ studyProtocol: postApplyProtocol });
    const rows = records.filter((row) => row.type === "renderer-memory-mutation");
    expect(rows.slice(0, -1).map((row) => row.mutation.edit)).toEqual(
      Array.from({ length: 12 }, (_, index) => index + 1),
    );
    expect(rows.at(-1)).toMatchObject({
      phase: "restoration",
      mutation: { kind: "restoration", paths: expect.arrayContaining(["article-00005.md"]) },
    });
    expect(report.observationComplete).toBe(true);
    expect(sleep.mock.calls.map(([ms]) => ms)).toEqual([60_000]);
  });

  it("retains an unknown written slot when browser evidence fails after notification", async () => {
    const { accounting, fs } = await simulate({
      studyProtocol: postApplyProtocol,
      collectFailure: 3,
    });
    expect(accounting).toMatchObject({
      attempted: 3,
      success: 2,
      notAttempted: 9,
      stopReason: expect.stringContaining("controller-error"),
      lastArmedUnknownOperation: { writeCompleted: true },
    });
    expect(fs.restores).toBe(1);
  });

  it("stops before the next write on an incomplete getter", async () => {
    const { accounting, report, liveChecks, fs, records } = await simulate({
      studyProtocol: postApplyProtocol,
      missingPostApply: 3,
    });
    expect(accounting).toMatchObject({ attempted: 3, success: 3, notAttempted: 9 });
    expect(accounting.lastArmedUnknownOperation).toBeNull();
    expect(report.failure.reason).toBe("renderer-memory-post-apply-incomplete");
    expect(report.scheduleComplete).toBe(false);
    expect(report.observationComplete).toBe(false);
    expect(fs.restores).toBe(1);
    expect(liveChecks).toBe(0);
    expect(records.filter((row) => row.type === "renderer-memory-mapping")).toHaveLength(1);
  });
});
