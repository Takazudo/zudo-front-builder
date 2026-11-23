import { execFileSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
  aggregate,
  blocksFrom,
  compareFacets,
  poolVariants,
  sessionBlocks,
  sessionFacets,
} from "../lib/aggregate.mjs";
import {
  FN_SCALE_PIN_KEYS,
  fnScale,
  loadFieldNotesFixture,
  loadFieldNotesScales,
  makeFieldNotesSource,
  materializeFieldNotes,
  sourceTreeHashes,
} from "../lib/fieldnotes.mjs";
import { FIXTURE_KINDS } from "../lib/fixture-kinds.mjs";
import { freezeHarness, unfreezeForRemoval, verifyFrozen } from "../lib/frozen.mjs";
import { frozenPlaywrightLink, harnessCapability, harnessTreeHash } from "../lib/identity.mjs";
import { sampleAsync, threadCountAsync } from "../lib/mac-process-adapter.mjs";
import { semanticErrors } from "../lib/profile.mjs";
import { createResourceStudy, summarizeResourceStudy } from "../lib/resource-study.mjs";
import { EXIT } from "../lib/run.mjs";
import {
  PROTOCOLS,
  RESOURCE_STUDY_PROTOCOL,
  TIMING_ABBA_PROTOCOL,
  expandSchedule,
  scheduleErrors,
  scheduleOf,
} from "../lib/schedule.mjs";
import { validateAs } from "../lib/schema.mjs";
import {
  aggregateInputFromRecord,
  isAbba,
  loadSeriesPlan,
  runSeries,
  sessionsFromRecord,
} from "../lib/series.mjs";
import { runBatch } from "../lib/state-machine.mjs";
import { summarize } from "../lib/stats.mjs";
import { HARNESS_DIR, readJson, readJsonl, sha256File } from "../lib/util.mjs";
import { deps, fakeClock, fakePage, fakeRecorder } from "./fakes.mjs";
import { makeFakeSource } from "./fieldnotes-fake.mjs";

const FIXTURE = join(HARNESS_DIR, "fixtures", "field-notes");
const example = (n) => JSON.parse(readFileSync(join(HARNESS_DIR, "profiles", n), "utf8"));
const tmps = [];
const tmp = () => {
  const d = mkdtempSync(join(tmpdir(), "dbo-m1-"));
  tmps.push(d);
  return d;
};
afterEach(() => {
  while (tmps.length) {
    const d = tmps.pop();
    try {
      unfreezeForRemoval(d);
    } catch {
      // not frozen
    }
    rmSync(d, { recursive: true, force: true });
  }
});

describe("versioned Field Notes scales", () => {
  it("scales.json validates, declares 100/1000/5000, and scale 100 is the existing pinned dataset", () => {
    const manifest = loadFieldNotesFixture(FIXTURE);
    const scales = loadFieldNotesScales(FIXTURE, manifest);
    expect(validateAs("field-notes-scales", scales)).toEqual([]);
    expect(validateAs("field-notes-fixture", manifest)).toEqual([]);
    expect(Object.keys(scales.byCount)).toEqual(["100", "1000", "5000"]);
    expect(scales.default).toBe(100);
    expect(fnScale(FIXTURE, manifest).datasetSha256).toBe(manifest.source.datasetSha256);
    expect(scales.generator.sha256).toBe(manifest.source.generator.sha256);
    const existing = example("field-notes-direct-read.example.json").fixture.source;
    for (const k of FN_SCALE_PIN_KEYS) expect(scales.byCount["100"][k], k).toBe(existing[k]);
    expect(scales.byCount["5000"].archiveDataset.equal).toBe(true);
    expect(scales.byCount["1000"].archiveDataset.equal).toBe(false);
    expect(() => fnScale(FIXTURE, manifest, 200)).toThrow(/unknown field-notes scale 200/);
  });

  it("fixture.scale is a field-notes enum; example timing/resource profiles validate", () => {
    const fn = example("field-notes-direct-read.example.json");
    expect(validateAs("profile", { ...fn, fixture: { ...fn.fixture, scale: 1000 } })).toEqual([]);
    expect(validateAs("profile", { ...fn, fixture: { ...fn.fixture, scale: 200 } })[0]).toMatch(
      /not in/,
    );
    const tiny = example("tiny-internal-a0.example.json");
    expect(semanticErrors({ ...tiny, fixture: { ...tiny.fixture, scale: 100 } })).toContain(
      "$.fixture.scale is field-notes only",
    );
    for (const n of [
      "field-notes-direct-read-timing-abba-5000.example.json",
      "field-notes-direct-read-resource-study-5000.example.json",
    ]) {
      expect(validateAs("profile", example(n)), n).toEqual([]);
      expect(semanticErrors(example(n)), n).toEqual([]);
    }
    for (const n of ["series-timing-abba.example.json", "series-resource-study.example.json"])
      expect(validateAs("series", example(n)), n).toEqual([]);
  });

  function scaledFixture(count) {
    const root = tmp();
    const src = join(root, "src");
    const fake = makeFakeSource(src, count);
    const fixtureDir = join(root, "fixture");
    cpSync(FIXTURE, fixtureDir, { recursive: true });
    const manifest = loadFieldNotesFixture(fixtureDir);
    const scales = loadFieldNotesScales(fixtureDir, manifest);
    const hashes = sourceTreeHashes(src, manifest);
    scales.byCount["1000"] = {
      ...scales.byCount["1000"],
      ...hashes,
      articleCount: count,
      contentManifestSha256: sha256File(join(src, "content", "manifest.json")),
    };
    writeFileSync(join(fixtureDir, manifest.scales), JSON.stringify(scales));
    return { root, src, fake, fixtureDir, manifest, hashes };
  }

  const fnChecks = (s, scale, pins = s.hashes) =>
    FIXTURE_KINDS["field-notes"].extraChecks(
      { fixture: { kind: "field-notes", scale, source: { dir: s.src, ...pins } } },
      { fixtureDir: s.fixtureDir, sourceDir: s.src },
      s.manifest,
    );

  it("verify-identity checks pass for the declared scale and fail for a wrong scale or wrong pins", () => {
    const s = scaledFixture(40);
    const ok = fnChecks(s, 1000);
    expect(ok.filter((c) => !c.ok)).toEqual([]);
    expect(ok.map((c) => c.name)).toEqual(
      expect.arrayContaining([
        "source-dataset-matches-manifest-declaration",
        "source-article-count-matches-scale",
        "source-pin-matches-scale-declaration:datasetSha256",
      ]),
    );
    const wrongScale = fnChecks(s, 5000)
      .filter((c) => !c.ok)
      .map((c) => c.name);
    expect(wrongScale).toEqual(
      expect.arrayContaining([
        "source-dataset-matches-manifest-declaration",
        "source-article-count-matches-scale",
        "source-pin-matches-scale-declaration:contentTreeSha256",
      ]),
    );
    const defaultScale = fnChecks(s, undefined)
      .filter((c) => !c.ok)
      .map((c) => c.name);
    expect(defaultScale).toContain("source-dataset-matches-manifest-declaration");
    const wrongPins = fnChecks(s, 1000, { ...s.hashes, datasetSha256: "0".repeat(64) });
    expect(wrongPins.filter((c) => !c.ok).map((c) => c.name)).toEqual([
      "source-pin:datasetSha256",
      "source-pin-matches-scale-declaration:datasetSha256",
    ]);
  });

  it("materialization uses the scale's dataset and refuses another scale before copying", () => {
    const s = scaledFixture(40);
    const m = materializeFieldNotes({
      fixtureDir: s.fixtureDir,
      sourceDir: s.src,
      workDir: join(s.root, "work"),
      adapter: "direct-read",
      scale: 1000,
    });
    expect(m).toMatchObject({ scale: 1000, scaleId: "field-notes-1000" });
    expect(m.materializedDatasetSha256).toBe(s.fake.datasetSha256);
    expect(() =>
      materializeFieldNotes({
        fixtureDir: s.fixtureDir,
        sourceDir: s.src,
        workDir: join(s.root, "work-5000"),
        adapter: "direct-read",
        scale: 5000,
      }),
    ).toThrow(/is not the pinned dataset .*nothing materialized/);
    expect(existsSync(join(s.root, "work-5000"))).toBe(false);
  });

  function fakeArchive() {
    const root = tmp();
    const archive = join(root, "archive");
    makeFakeSource(archive, 40);
    rmSync(join(archive, "content"), { recursive: true });
    mkdirSync(join(archive, "scripts"));
    const gen = [
      'import fs from "node:fs";',
      "const n = Number(process.argv[2]);",
      'fs.mkdirSync("content/articles", { recursive: true });',
      "const rows = [];",
      "for (let i = 1; i <= n; i++) {",
      '  const slug = `article-${String(i).padStart(5, "0")}`;',
      "  fs.writeFileSync(`content/articles/${slug}.md`, `# ${slug}\\n`);",
      "  rows.push({ slug });",
      "}",
      'fs.writeFileSync("content/manifest.json", JSON.stringify({ articleCount: n, articles: rows }));',
      "console.log(JSON.stringify({ articleCount: n }));",
      "",
    ].join("\n");
    writeFileSync(join(archive, "scripts", "generate.mjs"), gen);
    const manifest = loadFieldNotesFixture(FIXTURE);
    const scales = structuredClone(loadFieldNotesScales(FIXTURE, manifest));
    scales.generator.sha256 = sha256File(join(archive, "scripts", "generate.mjs"));
    return { root, archive, manifest, scales };
  }

  it("make-source copies apps/shared/scripts, runs the pinned generator at the scale and checks every hash", () => {
    const a = fakeArchive();
    a.scales.byCount["100"].generatorArgs = "40";
    const probe = join(a.root, "probe");
    const first = makeFieldNotesSource({
      fixtureDir: FIXTURE,
      archiveDir: a.archive,
      scale: 100,
      outDir: probe,
      scales: a.scales,
    });
    expect(first.generator.status).toBe(0);
    expect(first.ok).toBe(false);
    const decl = Object.fromEntries(first.checks.map((c) => [c.name.split(":")[1], c.actual]));
    a.scales.byCount["100"] = { ...a.scales.byCount["100"], ...decl };
    const out = join(a.root, "src-100");
    const r = makeFieldNotesSource({
      fixtureDir: FIXTURE,
      archiveDir: a.archive,
      scale: 100,
      outDir: out,
      scales: a.scales,
    });
    expect(r.ok).toBe(true);
    expect(r.scale).toBe(100);
    expect(readJson(join(out, "content", "manifest.json")).articleCount).toBe(40);
    expect(r.source.datasetSha256).toBe(sourceTreeHashes(out, a.manifest).datasetSha256);
    expect(existsSync(join(a.archive, "content"))).toBe(false);
    expect(() =>
      makeFieldNotesSource({
        fixtureDir: FIXTURE,
        archiveDir: a.archive,
        scale: 100,
        outDir: out,
        scales: a.scales,
      }),
    ).toThrow(/already exists/);
  });

  it("make-source refuses a generator whose sha256 is not declared, before copying anything", () => {
    const a = fakeArchive();
    const out = join(a.root, "src");
    expect(() =>
      makeFieldNotesSource({ fixtureDir: FIXTURE, archiveDir: a.archive, scale: 100, outDir: out }),
    ).toThrow(/is not the declared f2eaa2fe/);
    expect(existsSync(out)).toBe(false);
  });
});

const fnProfile = () => example("field-notes-direct-read.example.json");
const withOps = (p, ops, extra = {}) => ({
  ...p,
  ...extra,
  scenarios: [{ id: "s", set: "workload", warmOrder: "none", ops }],
});
const body = (extra = {}) => ({
  id: "b",
  op: "edit-body",
  article: "article-00005",
  writeMethod: "in-place",
  ...extra,
});

describe("op repeat and resource-study schedule", () => {
  it("expands repeat into warmups then measured attempts with ids and iteration", () => {
    const p = withOps(fnProfile(), [body({ repeat: { warmups: 2, measured: 3 } })]);
    expect(semanticErrors(p)).toEqual([]);
    const [s] = expandSchedule(p);
    expect(s.ops.map((o) => o.id)).toEqual(["b#w1", "b#w2", "b#m1", "b#m2", "b#m3"]);
    expect(s.ops[0].iteration).toEqual({ opId: "b", kind: "warmup", index: 1, of: 2 });
    expect(s.ops[4].iteration).toEqual({ opId: "b", kind: "measured", index: 3, of: 3 });
    expect(s.ops.some((o) => "repeat" in o)).toBe(false);
    expect(scheduleOf([s]).filter((e) => e.kind === "measured")).toHaveLength(3);
    expect(expandSchedule(withOps(fnProfile(), [body()]))[0].ops).toEqual([body()]);
  });

  it("rejects repeat on non-repeatable ops, outside workloads, on controls and on tiny profiles", () => {
    const rep = { repeat: { warmups: 1, measured: 1 } };
    const errs = (p) => scheduleErrors(p).join("\n");
    expect(
      errs(
        withOps(fnProfile(), [
          {
            id: "d",
            op: "delete-article",
            article: "article-00042",
            writeMethod: "unlink",
            ...rep,
          },
        ]),
      ),
    ).toMatch(/delete-article cannot repeat/);
    const neg = withOps(fnProfile(), [body(rep)]);
    neg.scenarios[0].set = "expected-negative";
    expect(errs(neg)).toMatch(/only allowed in a workload/);
    const tiny = example("tiny-internal-a0.example.json");
    expect(
      errs(withOps(tiny, [{ op: "edit-title", article: "a", writeMethod: "in-place", ...rep }])),
    ).toMatch(/field-notes only/);
  });

  it("resourceStudy must carry the protocol constants and a low sample rate; it cycles one scenario", () => {
    const rs = { ...PROTOCOLS[RESOURCE_STUDY_PROTOCOL] };
    const study = {
      protocol: RESOURCE_STUDY_PROTOCOL,
      cycles: rs.cycles,
      idleAfterRestoreMs: rs.idleAfterRestoreMs,
      finalIdleMs: rs.finalIdleMs,
      sampleEveryMs: 5000,
    };
    const p = withOps(fnProfile(), [body()], { resourceStudy: study });
    expect(validateAs("profile", p)).toEqual([]);
    expect(scheduleErrors(p)).toEqual([]);
    const cycles = expandSchedule(p);
    expect(cycles).toHaveLength(30);
    expect(cycles[0].id).toBe("s@c01");
    expect(cycles[29].cycle).toEqual({ scenario: "s", index: 30, of: 30 });
    expect(cycles[29].ops[0].iteration.kind).toBe("cycle");
    expect(scheduleErrors({ ...p, resourceStudy: { ...study, cycles: 3 } }).join("\n")).toMatch(
      /cycles must be 30/,
    );
    expect(
      validateAs("profile", { ...p, resourceStudy: { ...study, sampleEveryMs: 100 } })[0],
    ).toMatch(/below 1000/);
    expect(
      scheduleErrors({
        ...p,
        scenarios: [{ ...p.scenarios[0], ops: [body({ repeat: { warmups: 1, measured: 1 } })] }],
      }).join("\n"),
    ).toMatch(/cycles its scenario instead of repeat/);
    const two = { ...p, scenarios: [p.scenarios[0], { ...p.scenarios[0], id: "t" }] };
    expect(() => expandSchedule(two)).toThrow(/exactly one scenario/);
  });
});

function repeatFs() {
  let n = 0;
  return {
    files: {},
    prepareAttempt(op) {
      n += 1;
      const token = `tok-${op.id}-${n}`;
      return {
        token,
        targetPath: "/articles/a",
        predicate: { kind: "text", selector: "[x]", includes: token },
        supplemental: [],
        write: { method: "in-place", path: "a.md", bytes: token },
      };
    },
    performWrite: (_w, onT0) => onT0(),
    snapshot: () => ({}),
    restore: () => ({ restored: [], mismatches: [] }),
    warmPaths: () => [],
    liveChecks: () => [],
  };
}

describe("repeated attempts in the batch runner", () => {
  const scenarios = (w, m) =>
    expandSchedule(withOps(fnProfile(), [body({ repeat: { warmups: w, measured: m } })]));

  it("records warmups with label warmup, counts them in accounting and keeps them out of stats", async () => {
    const page = fakePage({
      behaviors: Array.from({ length: 4 }, (_, i) => ({
        kind: "notify",
        t1: 2_000_000_000n + BigInt(i) * 1_000_000n,
      })),
    });
    const d = deps({ page, fs: repeatFs() });
    const { accounting, stopped } = await runBatch({ scenarios: scenarios(2, 2), deps: d });
    expect(stopped).toBe(false);
    expect(accounting).toMatchObject({ planned: 4, attempted: 4, success: 4, notAttempted: 0 });
    const attempts = d.recorder.records.filter((r) => r.type === "attempt");
    expect(attempts.map((r) => r.label)).toEqual(["warmup", "warmup", "measured", "measured"]);
    expect(attempts.map((r) => r.iteration.kind)).toEqual([
      "warmup",
      "warmup",
      "measured",
      "measured",
    ]);
    const s = summarize(d.recorder.records, accounting);
    expect(s.latencyConditionalOnSuccessMs.n).toBe(2);
    expect(s.warmupsExcludedFromStats).toEqual({ attempts: 2, success: 2 });
  });

  it("a failing warmup stops the session before the next edit; the rest is not attempted", async () => {
    const page = fakePage({ behaviors: [{ kind: "timeout" }] });
    const fs = repeatFs();
    const d = deps({ page, fs });
    const { accounting, stopped } = await runBatch({ scenarios: scenarios(2, 10), deps: d });
    expect(stopped).toBe(true);
    expect(accounting).toMatchObject({
      planned: 12,
      attempted: 1,
      timeout: 1,
      notAttempted: 11,
      stopReason: "timeout",
      stoppedAfter: { scenario: "s", op: "b#w1" },
    });
    expect(d.recorder.records.filter((r) => r.type === "attempt")).toHaveLength(1);
  });
});

function writeSession(
  dir,
  {
    variant = "A",
    exe = "e".repeat(64),
    adapterSha = "a".repeat(64),
    latencies = [10, 20, 30],
    warm = [99, 98],
    failAt = null,
    measured = 3,
  } = {},
) {
  mkdirSync(dir, { recursive: true });
  const schedule = [
    ...warm.map((_, i) => ({
      scenario: "s",
      scenarioBase: "s",
      opId: `b#w${i + 1}`,
      baseOpId: "b",
      op: "edit-body",
      kind: "warmup",
      index: i + 1,
    })),
    ...Array.from({ length: measured }, (_, i) => ({
      scenario: "s",
      scenarioBase: "s",
      opId: `b#m${i + 1}`,
      baseOpId: "b",
      op: "edit-body",
      kind: "measured",
      index: i + 1,
    })),
  ];
  const recs = [{ type: "run-start", schedule }];
  warm.forEach((l, i) =>
    recs.push({
      type: "attempt",
      scenario: "s",
      opId: `b#w${i + 1}`,
      op: "edit-body",
      iteration: { opId: "b", kind: "warmup", index: i + 1 },
      outcome: "success",
      latencyMs: l,
    }),
  );
  let attempted = warm.length;
  for (let i = 0; i < measured; i += 1) {
    if (failAt !== null && i > failAt) break;
    const fail = failAt === i;
    attempted += 1;
    recs.push({
      type: "attempt",
      scenario: "s",
      opId: `b#m${i + 1}`,
      op: "edit-body",
      iteration: { opId: "b", kind: "measured", index: i + 1 },
      outcome: fail ? "timeout" : "success",
      ...(fail ? {} : { latencyMs: latencies[i] }),
    });
  }
  writeFileSync(join(dir, "records.jsonl"), recs.map((r) => JSON.stringify(r)).join("\n") + "\n");
  const planned = schedule.length;
  writeFileSync(
    join(dir, "summary.json"),
    JSON.stringify({
      exitCode: failAt === null ? 0 : 1,
      accounting: {
        planned,
        attempted,
        notAttempted: planned - attempted,
        stopReason: failAt === null ? null : "timeout",
      },
    }),
  );
  writeFileSync(
    join(dir, "receipt.json"),
    JSON.stringify({
      identity: { ok: true },
      schedule,
      fixture: {
        kind: "field-notes",
        scale: 5000,
        adapter: "direct-read",
        adapterSha256: adapterSha,
        sourcePins: { dir: `/x/${variant}`, datasetSha256: "d" },
      },
      buildReceipt: {
        exe: { sha256: exe },
        source: { sha: "1".repeat(40) },
        toolchain: { rustc: "1.99.1" },
      },
      browser: { version: "1", executableSha256: "b" },
      harnessCapability: { sha: "h" },
      host: { hwModel: "Mac", freememBytes: Math.random(), loadavg: [Math.random()] },
      profile: {
        buildReceipt: `/r/${variant}.json`,
        productSha: "x",
        pins: { harnessTreeSha256: "h", observerSha256: "o" },
      },
    }),
  );
}

describe("aggregate", () => {
  it("per-session blocks: measured counts and nearest-rank stats, warmups counted apart", () => {
    const dir = join(tmp(), "s1-A");
    writeSession(dir, { latencies: [30, 10, 20] });
    const { blocks } = sessionBlocks(dir);
    expect(blocks).toHaveLength(1);
    expect(blocks[0].measured).toMatchObject({
      planned: 3,
      attempted: 3,
      success: 3,
      stale: 0,
      timeout: 0,
      error: 0,
      notAttempted: 0,
    });
    expect(blocks[0].measured.latencyMs).toMatchObject({
      n: 3,
      min: 10,
      median: 20,
      p95NearestRank: 30,
      max: 30,
      conditionalOnSuccess: true,
      failuresPresent: false,
    });
    expect(blocks[0].warmup).toMatchObject({
      planned: 2,
      attempted: 2,
      success: 2,
      excludedFromStats: true,
    });
  });

  it("ABBA: pools per variant, passes identity with a declared build difference, reports a stopped session", () => {
    const root = tmp();
    const s = [
      ["s1-A", "A", { exe: "1".repeat(64), latencies: [10, 11, 12] }],
      ["s2-B", "B", { exe: "2".repeat(64), latencies: [20, 21, 22] }],
      ["s3-B", "B", { exe: "2".repeat(64), latencies: [23, 24, 25], failAt: 1 }],
      ["s4-A", "A", { exe: "1".repeat(64), latencies: [13, 14, 15] }],
    ].map(([label, variant, o], index) => {
      writeSession(join(root, label), { variant, ...o });
      return { index, label, variant, dir: join(root, label) };
    });
    const { report, exitCode } = aggregate({ sessions: s, declaredDifferences: ["build"] });
    expect(validateAs("aggregate", report)).toEqual([]);
    expect(report.identity.ok).toBe(true);
    expect(report.identity.checks.find((c) => c.name === "across-variants:build")).toMatchObject({
      ok: true,
      declaredDifference: true,
      differs: true,
    });
    expect(report.incompleteSessions).toEqual(["s3-B"]);
    expect(exitCode).toBe(1);
    const a = report.pooled.A.ops["s/b"].measured;
    expect(a).toMatchObject({ planned: 6, attempted: 6, success: 6, notAttempted: 0 });
    expect(a.latencyMs).toMatchObject({ n: 6, min: 10, max: 15, median: 12.5, p95NearestRank: 15 });
    const b = report.pooled.B.ops["s/b"].measured;
    expect(b).toMatchObject({ planned: 6, attempted: 5, success: 4, timeout: 1, notAttempted: 1 });
    expect(b.latencyMs).toMatchObject({ n: 4, failuresPresent: true, conditionalOnSuccess: true });
  });

  it("identity: an undeclared build difference or a differing adapter within a variant is a mismatch (exit 3)", () => {
    const root = tmp();
    const mk = (label, variant, o) => {
      writeSession(join(root, label), { variant, ...o });
      return { label, variant, dir: join(root, label) };
    };
    const ab = [mk("a1", "A", { exe: "1".repeat(64) }), mk("b1", "B", { exe: "2".repeat(64) })];
    const r = aggregate({ sessions: ab, declaredDifferences: [] });
    expect(r.exitCode).toBe(3);
    expect(r.report.identity.checks.filter((c) => !c.ok).map((c) => c.name)).toEqual([
      "across-variants:build",
    ]);
    const aa = [mk("a2", "A", {}), mk("a3", "A", { adapterSha: "f".repeat(64) })];
    expect(
      compareFacets(
        aa.map((x) => ({ ...x, facets: sessionFacets(readJson(join(x.dir, "receipt.json"))) })),
      )
        .checks.filter((c) => !c.ok)
        .map((c) => c.name),
    ).toEqual(["within-variant:A:adapter"]);
  });

  it("a session that ran but whose receipt.json is missing or unreadable fails receipt-readable (exit 3)", () => {
    const root = tmp();
    for (const l of ["a1", "a2", "a3"]) writeSession(join(root, l));
    rmSync(join(root, "a2", "receipt.json"));
    writeFileSync(join(root, "a3", "receipt.json"), "{ not json");
    const { report, exitCode } = aggregate({
      sessions: ["a1", "a2", "a3"].map((label) => ({
        label,
        variant: "A",
        dir: join(root, label),
      })),
    });
    expect(exitCode).toBe(3);
    expect(report.identity.ok).toBe(false);
    const failed = report.identity.checks.filter((c) => !c.ok);
    expect(failed.map((c) => c.name)).toEqual(["receipt-readable:a2", "receipt-readable:a3"]);
    for (const c of failed) expect(c.actual).toEqual(expect.any(String));
    expect(report.sessions.map((x) => [x.label, x.ran, x.status])).toEqual([
      ["a1", true, "complete"],
      ["a2", true, "incomplete"],
      ["a3", true, "incomplete"],
    ]);
  });

  it("a missing session dir is reported missing and incomplete", () => {
    const root = tmp();
    writeSession(join(root, "a1"));
    const { report, exitCode } = aggregate({
      sessions: [
        { label: "a1", variant: "A", dir: join(root, "a1") },
        { label: "a2", variant: "A", dir: join(root, "a2") },
      ],
    });
    expect(report.sessions[1].status).toBe("missing");
    expect(report.identity.ok).toBe(true);
    expect(report.identity.checks.some((c) => c.name.includes("a2"))).toBe(false);
    expect(Object.keys(report.facets)).toEqual(["a1"]);
    expect(exitCode).toBe(1);
  });
});

function seriesSetup({
  protocol = TIMING_ABBA_PROTOCOL,
  exes = ["1", "2"],
  toolchains = ["t", "t"],
} = {}) {
  const root = tmp();
  const timing = example("field-notes-direct-read-timing-abba-5000.example.json");
  const paths = {};
  ["A", "B"].forEach((v, i) => {
    writeFileSync(
      join(root, `r${v}.json`),
      JSON.stringify({
        exe: { sha256: exes[i].repeat(64) },
        source: { sha: "s" },
        toolchain: { rustc: toolchains[i] },
      }),
    );
    const p = { ...timing, buildReceipt: `r${v}.json`, outDir: "out" };
    if (protocol === RESOURCE_STUDY_PROTOCOL)
      Object.assign(p, example("field-notes-direct-read-resource-study-5000.example.json"), {
        buildReceipt: `r${v}.json`,
        outDir: "out",
      });
    paths[v] = join(root, `${v}.json`);
    writeFileSync(paths[v], JSON.stringify(p));
  });
  const plan = {
    schema: "dev-browser-observe.series.v1",
    id: "t",
    protocol,
    variants: { A: { profile: "A.json" }, B: { profile: "B.json" } },
    declaredDifferences: ["build"],
    order: ["A", "B", "B", "A"],
    labelPrefix: "ser1",
  };
  const planPath = join(root, "plan.json");
  writeFileSync(planPath, JSON.stringify(plan));
  return { root, plan, planPath, paths };
}

describe("series plans", () => {
  it("accepts an A,B,B,A timing plan whose profiles differ only in the build", () => {
    const s = seriesSetup();
    const l = loadSeriesPlan(s.planPath);
    expect(l.errors).toEqual([]);
    expect(l.sessions.map((x) => x.label)).toEqual([
      "ser1-s1-A",
      "ser1-s2-B",
      "ser1-s3-B",
      "ser1-s4-A",
    ]);
    expect(isAbba(["A", "B", "A", "B"])).toBe(false);
  });

  it("rejects a non-ABBA order, an undeclared build or toolchain difference, a profile difference and wrong repeats", () => {
    const write = (s, plan) => writeFileSync(s.planPath, JSON.stringify(plan));
    let s = seriesSetup();
    write(s, { ...s.plan, order: ["A", "B", "A", "B"] });
    expect(loadSeriesPlan(s.planPath).errors.join("\n")).toMatch(/X,Y,Y,X/);
    s = seriesSetup();
    write(s, { ...s.plan, declaredDifferences: [] });
    expect(loadSeriesPlan(s.planPath).errors.join("\n")).toMatch(/differ in build/);
    s = seriesSetup({ exes: ["1", "1"], toolchains: ["t1", "t2"] });
    expect(loadSeriesPlan(s.planPath).errors.join("\n")).toMatch(/differ in toolchain/);
    s = seriesSetup();
    const b = JSON.parse(readFileSync(s.paths.B, "utf8"));
    b.fixture.adapter = "original-memoized";
    writeFileSync(s.paths.B, JSON.stringify(b));
    expect(loadSeriesPlan(s.planPath).errors.join("\n")).toMatch(/profile differs from A/);
    s = seriesSetup();
    for (const v of ["A", "B"]) {
      const p = JSON.parse(readFileSync(s.paths[v], "utf8"));
      p.scenarios[0].ops[0].repeat = { warmups: 1, measured: 10 };
      writeFileSync(s.paths[v], JSON.stringify(p));
    }
    expect(loadSeriesPlan(s.planPath).errors.join("\n")).toMatch(
      /repeat must be \{ warmups: 2, measured: 10 \}/,
    );
    s = seriesSetup();
    write(s, { ...s.plan, declaredDifferences: ["corpus"] });
    expect(loadSeriesPlan(s.planPath).errors.join("\n")).toMatch(/not in/);
  });

  it("a resource-study plan needs one scenario per profile and resourceStudy", () => {
    const s = seriesSetup({ protocol: RESOURCE_STUDY_PROTOCOL });
    expect(loadSeriesPlan(s.planPath).errors).toEqual([]);
  });

  it("runs sessions in order with fresh labels; the first failing session stops the series", async () => {
    const s = seriesSetup();
    const calls = [];
    const codes = [EXIT.ok, EXIT.ok, EXIT.stopped];
    const { exitCode, record } = await runSeries({
      planPath: s.planPath,
      stderr: () => {},
      verifyImpl: () => ({ ok: true, checks: [] }),
      frozenCheck: () => null,
      executeImpl: async (a) => {
        calls.push(a);
        return codes.shift();
      },
    });
    expect(exitCode).toBe(EXIT.stopped);
    expect(calls.map((c) => [c.label, c.profilePath, c.command])).toEqual([
      ["ser1-s1-A", s.paths.A, "run"],
      ["ser1-s2-B", s.paths.B, "run"],
      ["ser1-s3-B", s.paths.B, "run"],
    ]);
    expect(record.sessions.map((x) => x.status)).toEqual([
      "completed",
      "completed",
      "stopped",
      "not-attempted",
    ]);
    const recPath = join(s.root, "out", "ser1.series.json");
    expect(readJson(recPath).stoppedAt).toEqual({ session: "ser1-s3-B", exitCode: EXIT.stopped });
    expect(sessionsFromRecord(recPath).sessions.map((x) => x.dir)).toEqual(
      record.sessions.map((x) => join(s.root, "out", x.label)),
    );
    const again = await runSeries({
      planPath: s.planPath,
      stderr: () => {},
      verifyImpl: () => ({ ok: true, checks: [] }),
      frozenCheck: () => null,
      executeImpl: async () => EXIT.ok,
    });
    expect(again.exitCode).toBe(EXIT.usage);
  });

  it("a preflight identity rejection runs no session", async () => {
    const s = seriesSetup();
    let ran = 0;
    const { exitCode, record } = await runSeries({
      planPath: s.planPath,
      stderr: () => {},
      frozenCheck: () => null,
      verifyImpl: (p) => ({
        ok: p !== s.paths.B,
        checks: [{ name: "exe-sha256", ok: p !== s.paths.B }],
      }),
      executeImpl: async () => {
        ran += 1;
        return EXIT.ok;
      },
    });
    expect(exitCode).toBe(EXIT.identityRejected);
    expect(ran).toBe(0);
    expect(record.preflight.B).toEqual({ ok: false, failed: ["exe-sha256"] });
    expect(record.sessions.every((x) => x.status === "not-attempted")).toBe(true);
  });
});

describe("resource study", () => {
  const study = {
    protocol: RESOURCE_STUDY_PROTOCOL,
    cycles: 2,
    idleAfterRestoreMs: 10000,
    finalIdleMs: 60000,
    sampleEveryMs: 1000,
  };

  it("idles after each completed restoration, then a final idle; samples carry phase and cost", async () => {
    const rec = fakeRecorder();
    const sleeps = [];
    let k = 0;
    const rs = createResourceStudy({
      study,
      clock: fakeClock(),
      append: (r) => rec.append(r),
      sleep: async (ms) => sleeps.push(ms),
      sampleNow: async () => {
        k += 1;
        return {
          summedRssKiB: 1000 + k,
          summedThreads: 10,
          processes: [{ pid: 1, present: true, rssKiB: 1000 + k, threads: 10 }],
        };
      },
    });
    await rs.start();
    const scenarios = expandSchedule(
      withOps(fnProfile(), [body()], { resourceStudy: { ...study, cycles: 30 } }),
    ).slice(0, 2);
    const page = fakePage({
      behaviors: [
        { kind: "notify", t1: 5_000_000_000n },
        { kind: "notify", t1: 6_000_000_000n },
      ],
    });
    const d = deps({
      page,
      fs: repeatFs(),
      recorder: {
        append: (r) => {
          rs.onRecord(r);
          return rec.append(r);
        },
      },
    });
    d.afterScenario = rs.afterScenario;
    const { stopped } = await runBatch({ scenarios, deps: d });
    await rs.finalIdle({ stopped, stopReason: null });
    await rs.stop();
    expect(sleeps).toEqual([10000, 10000, 60000]);
    const idles = rec.records.filter((r) => r.type === "idle");
    expect(idles.map((r) => `${r.phase}:${r.step}:${r.cycle}`)).toEqual([
      "after-restore:start:1",
      "after-restore:end:1",
      "after-restore:start:2",
      "after-restore:end:2",
      "final:start:null",
      "final:end:null",
    ]);
    const samples = rec.records.filter((r) => r.type === "resource-sample");
    expect(samples.length).toBeGreaterThanOrEqual(2);
    for (const s of samples) expect(typeof s.costMs).toBe("number");
    const sum = summarizeResourceStudy(rec.records, study);
    expect(sum).toMatchObject({
      cyclesPlanned: 2,
      cyclesCompletedWithIdle: 2,
      finalIdleCompleted: true,
      unavailable: { rss: 0, threads: 0, sampleErrors: 0 },
    });
    expect(sum.labels.inference).toMatch(/no leak/);
  });

  it("a stopped batch skips the final idle and records why", async () => {
    const rec = fakeRecorder();
    const rs = createResourceStudy({
      study,
      clock: fakeClock(),
      append: (r) => rec.append(r),
      sleep: async () => {},
      sampleNow: async () => ({ summedRssKiB: 1, processes: [] }),
    });
    await rs.start();
    await rs.finalIdle({ stopped: true, stopReason: "timeout" });
    await rs.stop();
    const sum = summarizeResourceStudy(rec.records, study);
    expect(sum.finalIdleCompleted).toBe(false);
    expect(sum.finalIdleSkipped).toBe("batch stopped: timeout");
  });

  it("async sampling with threads marks an unreadable thread count unavailable", async () => {
    const exec = async (cmd, args) => {
      if (args[0] === "-o") return { stdout: "11 100 200\n12 300 400\n" };
      if (args[2] === "11") return { stdout: "USER PID\nu 11\nu 11\nu 11\n" };
      throw new Error("ps failed");
    };
    expect(await threadCountAsync(11, exec)).toBe(3);
    const s = await sampleAsync([11, 12], { withThreads: true, execImpl: exec });
    expect(s.processes.map((p) => p.threads)).toEqual([3, "unavailable"]);
    expect(s.summedRssKiB).toBe(400);
    expect(s.summedThreads).toBe("unavailable");
    const plain = await sampleAsync([11], { execImpl: exec });
    expect(plain.processes[0].threads).toBe("unavailable");
    expect(plain.threadsNote).toMatch(/not sampled/);
  });
});

describe("frozen harness copy", () => {
  it("freezes a read-only, hash-stamped copy that resolves playwright and reports itself frozen", () => {
    const out = join(tmp(), "frozen");
    const r = freezeHarness({ outDir: out, allowDirty: true });
    expect(r.stamp.harnessTreeSha256).toBe(harnessTreeHash(HARNESS_DIR));
    const v = verifyFrozen(r.dir);
    expect(v.checks.filter((c) => !c.ok)).toEqual([]);
    const cap = harnessCapability(r.dir);
    expect(cap).toMatchObject({ frozen: true, intact: true, sha: r.stamp.source.sha });
    expect(
      createRequire(join(r.dir, "cli.mjs")).resolve("@playwright/test/package.json"),
    ).toBeTruthy();
    unfreezeForRemoval(r.dir);
    writeFileSync(join(r.dir, "lib", "stats.mjs"), "// changed\n");
    expect(harnessCapability(r.dir).intact).toBe(false);
    expect(
      verifyFrozen(r.dir)
        .checks.filter((c) => !c.ok)
        .map((c) => c.name),
    ).toEqual(["harness-tree", "full-tree", "read-only"]);
    expect(() => freezeHarness({ outDir: out })).toThrow(/already exists/);
  });

  it("refuses to freeze a harness dir with uncommitted changes", () => {
    const repo = tmp();
    const git = (...a) =>
      execFileSync("git", ["-c", "user.name=t", "-c", "user.email=t@example.invalid", ...a], {
        cwd: repo,
      });
    git("init", "-q");
    mkdirSync(join(repo, "h"));
    writeFileSync(join(repo, "h", "cli.mjs"), "");
    git("add", ".");
    git("commit", "-qm", "x");
    writeFileSync(join(repo, "h", "new.mjs"), "");
    expect(() =>
      freezeHarness({ outDir: join(repo, "..", "never"), harnessDir: join(repo, "h") }),
    ).toThrow(/uncommitted changes/);
  });
});

describe("records written by a run carry the schedule", () => {
  it("record schema accepts the new record types and the warmup label", () => {
    for (const r of [
      { seq: 0, at: "x", type: "idle" },
      { seq: 0, at: "x", type: "resource-sample", label: "unmeasured" },
      { seq: 0, at: "x", type: "resource-study" },
      { seq: 0, at: "x", type: "attempt", label: "warmup" },
    ])
      expect(validateAs("record", r)).toEqual([]);
    expect(readJsonl(join(tmp(), "none.jsonl"))).toEqual([]);
  });
});

describe("review fixes", () => {
  it("pooling a schedule-less block keeps planned and notAttempted unknown, never negative", () => {
    const old = blocksFrom(
      [
        {
          type: "attempt",
          scenario: "s",
          opId: "b",
          op: "edit-body",
          outcome: "success",
          latencyMs: 5,
        },
        {
          type: "attempt",
          scenario: "s",
          opId: "b",
          op: "edit-body",
          outcome: "success",
          latencyMs: 6,
        },
      ],
      null,
    ).blocks;
    expect(old[0].measured).toMatchObject({ planned: null, attempted: 2, notAttempted: null });
    expect(old[0].measured.latencyMs.failuresPresent).toBe(null);
    const fresh = blocksFrom(
      [],
      [
        {
          scenario: "s",
          scenarioBase: "s",
          opId: "b#m1",
          baseOpId: "b",
          op: "edit-body",
          kind: "measured",
        },
      ],
    ).blocks;
    const pooled = poolVariants([
      { variant: "A", label: "old", blocks: old },
      { variant: "A", label: "new", blocks: fresh },
    ]).A.ops["s/b"].measured;
    expect(pooled).toMatchObject({ planned: null, attempted: 2, notAttempted: null });
    expect(pooled.latencyMs.failuresPresent).toBe(null);
    const over = blocksFrom(
      [
        { type: "attempt", scenario: "s", opId: "b#m1", outcome: "success", latencyMs: 1 },
        { type: "attempt", scenario: "s", opId: "b#m1", outcome: "success", latencyMs: 1 },
      ],
      [
        {
          scenario: "s",
          scenarioBase: "s",
          opId: "b#m1",
          baseOpId: "b",
          op: "edit-body",
          kind: "measured",
        },
      ],
    ).blocks[0].measured;
    expect(over).toMatchObject({ planned: 1, attempted: 2, notAttempted: null });
    expect(over.countsInconsistent).toMatch(/exceeds/);
  });

  it("a stopped series aggregates as incomplete (exit 1); unrun sessions add their planned attempts as not attempted", async () => {
    const s = seriesSetup();
    const outDir = join(s.root, "out");
    const codes = [EXIT.ok, EXIT.stopped];
    await runSeries({
      planPath: s.planPath,
      stderr: () => {},
      verifyImpl: () => ({ ok: true, checks: [] }),
      frozenCheck: () => null,
      executeImpl: async ({ label }) => {
        const variant = label.endsWith("-A") ? "A" : "B";
        const code = codes.shift();
        writeSession(join(outDir, label), {
          variant,
          exe: variant === "A" ? "1".repeat(64) : "2".repeat(64),
          ...(code === EXIT.ok ? {} : { failAt: 0 }),
        });
        return code;
      },
    });
    const input = aggregateInputFromRecord(join(outDir, "ser1.series.json"));
    const { report, exitCode } = aggregate(input);
    expect(validateAs("aggregate", report)).toEqual([]);
    expect(exitCode).toBe(1);
    expect(report.identity.ok).toBe(true);
    expect(Object.keys(report.facets)).toEqual(["ser1-s1-A", "ser1-s2-B"]);
    expect(report.sessions.map((x) => x.status)).toEqual([
      "complete",
      "incomplete",
      "not-attempted",
      "not-attempted",
    ]);
    expect(report.series.stoppedAt).toEqual({ session: "ser1-s2-B", exitCode: EXIT.stopped });
    const body = report.pooled.B.ops["fn-edit-body/body-05"].measured;
    expect(body).toMatchObject({ planned: 10, attempted: 0, notAttempted: 10 });
    expect(body.latencyMs.failuresPresent).toBe(true);
    expect(report.pooled.A.ops["fn-category/category-15"].measured.notAttempted).toBe(10);
    expect(report.pooled.B.ops["s/b"].measured).toMatchObject({
      planned: 3,
      attempted: 1,
      timeout: 1,
      notAttempted: 2,
    });
  });

  it("an unrun session whose profile changed since the series reports the planned schedule as unknown", async () => {
    const s = seriesSetup();
    await runSeries({
      planPath: s.planPath,
      stderr: () => {},
      verifyImpl: () => ({ ok: true, checks: [] }),
      frozenCheck: () => null,
      executeImpl: async () => EXIT.stopped,
    });
    writeFileSync(
      s.paths.B,
      readFileSync(s.paths.B, "utf8").replace("direct-read", "direct-read "),
    );
    const { sessions } = sessionsFromRecord(join(s.root, "out", "ser1.series.json"));
    expect(sessions[1]).toMatchObject({ seriesStatus: "not-attempted", plannedSchedule: null });
    expect(sessions[1].plannedError).toMatch(/changed since the series ran/);
    expect(sessions[3].plannedSchedule).toHaveLength(60);
  });

  it("timing-abba.v1 needs all five ops unless a declared opSubset (recorded) names fewer", async () => {
    const trim = (s, keep) => {
      for (const v of ["A", "B"]) {
        const p = JSON.parse(readFileSync(s.paths[v], "utf8"));
        p.scenarios = p.scenarios.filter((x) => keep.includes(x.ops[0].op));
        writeFileSync(s.paths[v], JSON.stringify(p));
      }
    };
    const three = ["edit-body", "edit-title", "edit-category"];
    let s = seriesSetup();
    trim(s, three);
    expect(loadSeriesPlan(s.planPath).errors.join("\n")).toMatch(
      /needs every protocol op at least once; missing edit-shared-component, edit-managed-css/,
    );
    writeFileSync(
      s.planPath,
      JSON.stringify({ ...s.plan, opSubset: { ops: three, reason: "content ops only" } }),
    );
    expect(loadSeriesPlan(s.planPath).errors).toEqual([]);
    const { record } = await runSeries({
      planPath: s.planPath,
      stderr: () => {},
      verifyImpl: () => ({ ok: true, checks: [] }),
      frozenCheck: () => null,
      executeImpl: async () => EXIT.ok,
    });
    expect(record.opSubset).toEqual({ ops: three, reason: "content ops only" });
    expect(
      aggregateInputFromRecord(join(s.root, "out", "ser1.series.json")).series.opSubset,
    ).toEqual(record.opSubset);
    s = seriesSetup();
    writeFileSync(
      s.planPath,
      JSON.stringify({ ...s.plan, opSubset: { ops: ["edit-body"], reason: "x" } }),
    );
    expect(loadSeriesPlan(s.planPath).errors.join("\n")).toMatch(/outside the declared opSubset/);
    writeFileSync(
      s.planPath,
      JSON.stringify({
        ...s.plan,
        opSubset: { ops: PROTOCOLS[TIMING_ABBA_PROTOCOL].ops, reason: "x" },
      }),
    );
    expect(loadSeriesPlan(s.planPath).errors.join("\n")).toMatch(/fewer ops than the protocol/);
  });

  it("resource samples taken during a restoration are tagged restore from the start of cleanup", async () => {
    const order = [];
    const fs = repeatFs();
    fs.restore = () => {
      order.push("restore");
      return { restored: [], mismatches: [] };
    };
    const page = fakePage({ behaviors: [{ kind: "notify", t1: 5_000_000_000n }] });
    const d = deps({ page, fs });
    d.beforeCleanup = ({ reason }) => order.push(`before:${reason}`);
    await runBatch({ scenarios: expandSchedule(withOps(fnProfile(), [body()])), deps: d });
    expect(order).toEqual(["before:scenario-end", "restore"]);

    const rec = fakeRecorder();
    const rs = createResourceStudy({
      study: {
        protocol: RESOURCE_STUDY_PROTOCOL,
        cycles: 30,
        idleAfterRestoreMs: 0,
        finalIdleMs: 0,
        sampleEveryMs: 1000,
      },
      clock: fakeClock(),
      append: (r) => rec.append(r),
      sleep: async () => {},
      sampleNow: async () => ({ summedRssKiB: 1, processes: [] }),
    });
    rs.onRecord({ type: "scenario-start", cycle: { index: 4 } });
    await rs.start();
    rs.beforeCleanup({ reason: "scenario-end" });
    await rs.stop();
    const phases = rec.records.filter((r) => r.type === "resource-sample").map((r) => r.phase);
    expect(phases).toEqual([
      { step: "edit", cycle: 4 },
      { step: "end", cycle: null },
    ]);
    const rec2 = fakeRecorder();
    const rs2 = createResourceStudy({
      study: {
        protocol: RESOURCE_STUDY_PROTOCOL,
        cycles: 30,
        idleAfterRestoreMs: 0,
        finalIdleMs: 0,
        sampleEveryMs: 1000,
      },
      clock: fakeClock(),
      append: (r) => rec2.append(r),
      sleep: async () => {},
      sampleNow: async () => ({ summedRssKiB: 1, processes: [] }),
    });
    rs2.onRecord({ type: "scenario-start", cycle: { index: 4 } });
    rs2.beforeCleanup({ reason: "scenario-end" });
    await rs2.start();
    rs2.onRecord({ type: "cleanup" });
    await rs2.stop();
    expect(rec2.records.find((r) => r.type === "resource-sample").phase).toEqual({
      step: "restore",
      cycle: 4,
    });
  });

  it("a sampler tick whose record write throws halts the batch before the next edit, without an unhandled rejection", async () => {
    let failSamples = false;
    const rec = fakeRecorder();
    const rs = createResourceStudy({
      study: {
        protocol: RESOURCE_STUDY_PROTOCOL,
        cycles: 30,
        idleAfterRestoreMs: 0,
        finalIdleMs: 0,
        sampleEveryMs: 1000,
      },
      clock: fakeClock(),
      append: (r) => {
        if (failSamples && r.type === "resource-sample") throw new Error("EIO: disk full");
        return rec.append(r);
      },
      sleep: async () => {},
      sampleNow: async () => ({ summedRssKiB: 1, processes: [] }),
    });
    await rs.start();
    failSamples = true;
    await rs.stop().catch(() => {});
    expect(rs.haltReason()).toMatchObject({
      reason: "resource-sample-record-failed",
      error: "EIO: disk full",
    });
    await expect(rs.afterScenario({ scenario: { cycle: { index: 1 } } })).rejects.toThrow(/halted/);
    const page = fakePage({ behaviors: [{ kind: "notify", t1: 5_000_000_000n }] });
    const d = deps({ page, fs: repeatFs() });
    d.haltReason = rs.haltReason;
    const scenarios = expandSchedule(
      withOps(fnProfile(), [body({ repeat: { warmups: 0, measured: 3 } })]),
    );
    const { accounting, stopped } = await runBatch({ scenarios, deps: d });
    expect(stopped).toBe(true);
    expect(accounting).toMatchObject({
      attempted: 0,
      notAttempted: 3,
      error: 1,
      stopReason: "controller-error",
    });
    expect(d.recorder.records.find((r) => r.type === "precheck")).toMatchObject({
      outcome: "controller-error",
      halt: { reason: "resource-sample-record-failed" },
    });
    await rs.finalIdle({ stopped: false, stopReason: null }).catch(() => {});
    expect(rec.records.find((r) => r.type === "idle" && r.phase === "final").reason).toMatch(
      /halted/,
    );
  });

  it("a frozen copy's broken playwright link fails verify-frozen and the run identity check; series preflight refuses it", async () => {
    const out = join(tmp(), "frozen");
    const r = freezeHarness({ outDir: out, allowDirty: true });
    expect(frozenPlaywrightLink(r.stamp).ok).toBe(true);
    const link = r.stamp.playwright.link;
    rmSync(link);
    symlinkSync(tmpdir(), link, "dir");
    expect(frozenPlaywrightLink(r.stamp)).toMatchObject({ ok: false, actual: tmpdir() });
    expect(
      verifyFrozen(r.dir)
        .checks.filter((c) => !c.ok)
        .map((c) => c.name),
    ).toEqual(["playwright-link"]);
    const profile = example("tiny-internal-a0.example.json");
    profile.fixture.dir = join(r.dir, "fixtures", "tiny-two-article");
    profile.outDir = join(tmp(), "out");
    const pPath = join(tmp(), "p.json");
    writeFileSync(pPath, JSON.stringify(profile));
    let res;
    try {
      execFileSync(
        process.execPath,
        [join(r.dir, "cli.mjs"), "verify-identity", "--profile", pPath],
        {
          encoding: "utf8",
        },
      );
    } catch (error) {
      res = JSON.parse(error.stdout);
    }
    expect(res.checks.find((c) => c.name === "harness-frozen-copy-intact").ok).toBe(true);
    expect(res.checks.find((c) => c.name === "harness-frozen-playwright-link")).toMatchObject({
      ok: false,
      actual: tmpdir(),
    });
    const s = seriesSetup();
    let ran = 0;
    const { exitCode, record } = await runSeries({
      planPath: s.planPath,
      stderr: () => {},
      verifyImpl: () => ({ ok: true, checks: [] }),
      frozenCheck: () => verifyFrozen(r.dir),
      executeImpl: async () => {
        ran += 1;
        return EXIT.ok;
      },
    });
    expect(exitCode).toBe(EXIT.identityRejected);
    expect(ran).toBe(0);
    expect(record.preflightFrozen).toMatchObject({ ok: false, failed: ["playwright-link"] });
  });
});
