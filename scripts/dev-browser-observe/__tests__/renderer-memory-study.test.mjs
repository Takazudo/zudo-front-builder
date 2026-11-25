import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { aggregate } from "../lib/aggregate.mjs";
import { evaluateIdentity } from "../lib/identity.mjs";
import { semanticErrors } from "../lib/profile.mjs";
import {
  RENDERER_MEMORY_FEATURE as feature,
  RENDERER_MEMORY_LIMITS as limits,
  RENDERER_MEMORY_PROTOCOL as protocol,
} from "../lib/renderer-memory-protocol.mjs";
import { createRendererMemoryStudy } from "../lib/renderer-memory-study.mjs";
import { expandSchedule } from "../lib/schedule.mjs";
import { validateAs } from "../lib/schema.mjs";
import { runBatch } from "../lib/state-machine.mjs";
import { HARNESS_DIR } from "../lib/util.mjs";
import { deps, fakeFs, fakePage, fakeServer } from "./fakes.mjs";

function profile() {
  const input = JSON.parse(
    readFileSync(join(HARNESS_DIR, "profiles", "field-notes-direct-read.example.json")),
  );
  const [scenario] = input.scenarios;
  scenario.ops[0].repeat = { warmups: 2, measured: 10 };
  return {
    ...input,
    diagnostic: true,
    rendererMemoryStudy: {
      protocol,
      browserExecutable: "/chromium/headless_shell",
      browserExecutableSha256: "a".repeat(64),
    },
    fixture: { ...input.fixture, scale: 100 },
    server: {
      ...input.server,
      env: { ZFB_DEV_PROVENANCE_TRACE: "1" },
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

  it("never pools diagnostic latency rows even if a diagnostic build has an ordinary profile", () => {
    const root = mkdtempSync(join(tmpdir(), "zfb-memory-aggregate-"));
    try {
      const dir = join(root, "diagnostic");
      mkdirSync(dir);
      writeFileSync(
        join(dir, "receipt.json"),
        JSON.stringify({
          buildReceipt: { build: { features: [`zfb/${feature}`] } },
          profile: {},
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
  });
});

async function simulate({ failPoll = null, workDeadline = 240_000, routeFailure = false } = {}) {
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
    return { ...output, actions: [{ path: "article-00005.md" }] };
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
  const arm = page.arm;
  page.arm = async (...args) => ({ ...(await arm(...args)), actual: fs.files["article-00005.md"] });
  page.settle = async ({ bodyRevision }) => {
    expect(bodyRevision.predicate.equals).toBe("original body");
    return { settled: true, nativeUpdateObserved: true, bodyRevision: { matched: true } };
  };
  const d = deps({ fs, page, server, clock: { now: clock } });
  const study = createRendererMemoryStudy({
    append: (r) => d.recorder.append(r),
    supervisorStartedMs: 0,
    workDeadline,
    clock,
    sleep,
    sample: async () => ({ summedRssKiB: 1000 }),
    inspect: () => ({ failure: null, start: {} }),
  });
  try {
    study.startSampling(server);
    await study.beforeEdits();
    const { accounting, stopped } = await runBatch({
      scenarios: expandSchedule(profile()),
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
    return { accounting, records: d.recorder.records, fs, liveChecks, study, sleep };
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
