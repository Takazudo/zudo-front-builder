import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { semanticErrors } from "../lib/profile.mjs";
import {
  attemptWritePaths,
  buildCausalChain,
  parseProvLines,
  traceEnabledIn,
} from "../lib/provenance.mjs";
import { validateAs } from "../lib/schema.mjs";
import { runBatch } from "../lib/state-machine.mjs";
import { summarize } from "../lib/stats.mjs";
import { HARNESS_DIR } from "../lib/util.mjs";
import { deps, fakeFs, fakePage, fakeServer } from "./fakes.mjs";

const W = "/w/content/articles";
const A = `${W}/a.md`;
const PAGE = "/w/app/pages/articles/[slug].tsx";
const HOME = "/w/app/pages/index.tsx";
const GEN = { worker_gen: 2, snapshot_gen: 2 };

let seq = 0;
const line = (ev, fields) => ({
  text: `[zfb-prov] ${JSON.stringify({ ev, seq: ++seq, ms: seq, ...fields })}`,
});

/** One accepted tick for an edit of `paths`, lazily published at /articles/a/. */
function acceptedTick(tick, paths, { selected = [PAGE], allPages = false, gen = GEN } = {}) {
  return [
    line("intake", {
      tick,
      intake_id: tick,
      paths: paths.map((p) => ({ path: p.path ?? p, kind: p.kind ?? "Modified" })),
      suppressed: 0,
    }),
    line("select", {
      tick,
      changes: paths.map((p) => ({ path: p.path ?? p, kind: "Modified", class: "Content" })),
      all_pages: allPages,
      all_reasons: allPages ? [{ reason: "global_full_rebuild" }] : [],
      fan_out_safe: !allPages,
      selected: allPages ? null : selected,
      noop: false,
    }),
    line("snapshot", {
      tick,
      snapshot_gen: gen.snapshot_gen,
      outcome: "Built",
      collections: { articles: 2 },
    }),
    line("worker", {
      tick,
      worker_gen: gen.worker_gen,
      snapshot_gen: gen.snapshot_gen,
      phase: "refresh",
    }),
    line("refresh", { tick, outcome: "refreshed", generation: gen, changed: [], vanished: [] }),
    line("render_plan", {
      tick,
      pages: [PAGE, HOME],
      renderer_refreshed: true,
      route_structure_moved: false,
    }),
    line("stale", { tick, outputs: ["/w/app/dist/articles/a/index.html"], generation: gen }),
  ];
}

const afterAdoption = (gen = GEN, drain = 4, entry = A, flags = {}) => [
  line("observations", {
    drain,
    generation: gen,
    events: 3,
    observations: [
      {
        consumer: PAGE,
        phase: "Render",
        source: "x",
        visit: 1,
        raw_reads: 1,
        reads: [{ collection: "articles", entry }],
      },
    ],
    first_reader: { index: 0, consumer: PAGE, phase: "Render", collection: "articles" },
    ...("first" in flags
      ? flags.first === undefined
        ? {}
        : { first_drain_of_generation: flags.first }
      : { first_drain_of_generation: true }),
  }),
  line("edges", {
    drain,
    generation: gen,
    applied: true,
    slots: [
      { consumer: PAGE, phase: "Render", status: "replaced", reads_before: 1, reads_after: 1 },
    ],
    demoted: [],
    groups: [{ consumer: PAGE, collection: "articles", entries: [entry], entries_total: 1 }],
    ...("graph" in flags
      ? flags.graph === undefined
        ? {}
        : { graph_written: flags.graph }
      : { graph_written: true }),
  }),
  line("publish", {
    mode: "lazy",
    url: "/articles/a/",
    page: "page" in flags ? flags.page : PAGE,
    output: "/w/app/dist/articles/a/index.html",
    written: true,
    generation: gen,
    drain,
    provenance_current: true,
    ...("contains" in flags
      ? flags.contains === undefined
        ? {}
        : { drain_contains_page: flags.contains }
      : { drain_contains_page: true }),
  }),
];

const opts = (over = {}) => ({
  writePaths: [[A]],
  consumers: [PAGE],
  targetUrl: "/articles/a/",
  dom: { outcome: "success", t1Ns: "9", latencyMs: 12 },
  ...over,
});

describe("trace gate and parsing", () => {
  it("the gate matches the Rust side: 1 or true, case-insensitive", () => {
    expect(traceEnabledIn({ ZFB_DEV_PROVENANCE_TRACE: " TRUE " })).toBe(true);
    expect(traceEnabledIn({ ZFB_DEV_PROVENANCE_TRACE: "1" })).toBe(true);
    expect(traceEnabledIn({ ZFB_DEV_PROVENANCE_TRACE: "0" })).toBe(false);
    expect(traceEnabledIn({})).toBe(false);
  });

  it("records malformed lines, seq violations and list truncation instead of dropping them", () => {
    const p = parseProvLines([
      { text: '[zfb-prov] {"ev":"intake","seq":5,"ms":1,"tick":1,"paths":[],"paths_truncated":3}' },
      { text: "[zfb-prov] {not json" },
      { text: '[zfb-prov] {"seq":6}' },
      { text: '[zfb-prov] {"ev":"select","seq":5,"ms":2,"tick":1}' },
      { text: "unrelated line" },
      {
        text: '[zfb-prov] {"ev":"edges","seq":7,"ms":3,"groups":[{"consumer":"/p","entries":[],"entries_truncated":9}]}',
      },
    ]);
    expect(p.events.map((e) => e.ev)).toEqual(["intake", "select", "edges"]);
    expect(p.parseErrors).toHaveLength(2);
    expect(p.parseErrors[1].error).toBe("missing ev or seq");
    expect(p.seqViolations).toEqual([{ index: 3, seq: 5, previous: 5 }]);
    expect(p.truncatedLists).toEqual([
      { ev: "intake", seq: 5, field: "paths", omitted: 3 },
      { ev: "edges", seq: 7, field: "groups[0].entries", omitted: 9 },
    ]);
  });
});

describe("causal chain", () => {
  it("full chain: every link observed and chainComplete", () => {
    const { events } = parseProvLines([...acceptedTick(3, [A]), ...afterAdoption()]);
    const c = buildCausalChain(events, opts());
    expect(c.missing).toEqual([]);
    expect(c.chainComplete).toBe(true);
    expect(c.tick).toBe(3);
    expect(c.links.select).toMatchObject({
      selected: [PAGE],
      all_pages: false,
      fan_out_safe: true,
    });
    expect(c.links.generation).toMatchObject({ outcome: "adopted", generation: GEN });
    expect(c.links.render_plan.targetPlanned).toBe(true);
    expect(c.links.observations).toMatchObject({ drain: 4, targetConsumerRead: true });
    expect(c.links.edges).toMatchObject({
      groupLinksTargetToEntry: true,
      targetSlotStatuses: [{ phase: "Render", status: "replaced" }],
    });
    expect(c.links.publish.events).toHaveLength(1);
    expect(c.links.dom).toMatchObject({ status: "observed", latencyMs: 12 });
  });

  it("observations or publish of an older generation never complete the chain", () => {
    const old = { worker_gen: 1, snapshot_gen: 1 };
    const { events } = parseProvLines([...acceptedTick(3, [A]), ...afterAdoption(old)]);
    const c = buildCausalChain(events, opts());
    expect(c.chainComplete).toBe(false);
    expect(c.missing).toEqual(["observations", "edges", "publish"]);
  });

  it("an eager publish matches by page and output suffix", () => {
    const { events } = parseProvLines([
      ...acceptedTick(3, [A]),
      ...afterAdoption().slice(0, 2),
      line("publish", {
        mode: "eager",
        page: HOME,
        output: "/w/app/dist/index.html",
        written: true,
        generation: GEN,
      }),
    ]);
    expect(
      buildCausalChain(events, opts({ consumers: [HOME], targetUrl: "/" })).links.publish.status,
    ).toBe("observed");
    expect(buildCausalChain(events, opts()).links.publish.status).toBe("missing");
  });

  it("rejected generation: reject retains the last-good generation; the repair tick decides", () => {
    const rejected = [
      line("intake", { tick: 5, paths: [{ path: A, kind: "Modified" }], suppressed: 0 }),
      line("select", { tick: 5, changes: [], all_pages: false, selected: [PAGE], noop: false }),
      line("snapshot", { tick: 5, snapshot_gen: 3, outcome: "Failed", error: "bad yaml" }),
      line("reject", { tick: 5, stage: "refresh", error: "bad yaml", retained: GEN }),
    ];
    const onlyRejected = buildCausalChain(parseProvLines(rejected).events, opts({ dom: null }));
    expect(onlyRejected).toMatchObject({ tick: 5, rejected: true, chainComplete: true });
    expect(onlyRejected.links.generation).toMatchObject({
      outcome: "retained",
      generation: GEN,
      snapshot: { outcome: "Failed", error: "bad yaml" },
      reject: { stage: "refresh" },
    });
    const repaired = { worker_gen: 3, snapshot_gen: 4 };
    const both = buildCausalChain(
      parseProvLines([
        ...rejected,
        ...acceptedTick(6, [A], { gen: repaired }),
        ...afterAdoption(repaired, 5),
      ]).events,
      opts(),
    );
    expect(both.links.intake.ticks).toEqual([5, 6]);
    expect(both).toMatchObject({ tick: 6, rejected: false, chainComplete: true });
  });

  it("all-pages fallback: select carries all_pages and its reasons, selected is null", () => {
    const { events } = parseProvLines([
      ...acceptedTick(7, [A], { allPages: true }),
      ...afterAdoption(),
    ]);
    const c = buildCausalChain(events, opts());
    expect(c.links.select).toMatchObject({
      all_pages: true,
      all_reasons: [{ reason: "global_full_rebuild" }],
      selected: null,
      fan_out_safe: false,
    });
    expect(c.chainComplete).toBe(true);
  });

  it("rename paths: an intake naming either the source or the destination matches", () => {
    const to = `${W}/A.md`;
    const write = { method: "rename", path: A, to };
    expect(attemptWritePaths({ writes: [write] })).toEqual([A, to]);
    const { events } = parseProvLines([
      ...acceptedTick(8, [
        { path: A, kind: "Removed" },
        { path: to, kind: "Created" },
      ]),
      ...afterAdoption(GEN, 4, to),
    ]);
    const c = buildCausalChain(events, opts({ writePaths: [[A], [to]] }));
    expect(c.links.intake.events[0].matched.map((p) => p.kind)).toEqual(["Removed", "Created"]);
    expect(c.links.edges.groupLinksTargetToEntry).toBe(true);
    expect(c.chainComplete).toBe(true);
  });

  it("no intake for the written path: nothing downstream is attributed", () => {
    const { events } = parseProvLines([...acceptedTick(9, ["/w/other.md"]), ...afterAdoption()]);
    const c = buildCausalChain(events, opts());
    expect(c.tick).toBeNull();
    expect(c.missing).toEqual([
      "intake",
      "select",
      "generation",
      "render_plan",
      "observations",
      "edges",
      "publish",
    ]);
  });
});

function provServer(lines, droppedBeforeMark = 0) {
  const s = fakeServer();
  s.provMark = () => 0;
  s.provSince = () => ({ lines, droppedBeforeMark });
  return s;
}

async function runOne({ lines, traceEnabled = true, dropped = 0, outcome = "notify" }) {
  const fs = fakeFs();
  fs.prepareAttempt = () => ({
    token: "tok",
    targetPath: "/articles/a/",
    predicate: { kind: "text", selector: "x", includes: "tok" },
    supplemental: [],
    write: { method: "in-place", path: A, bytes: "x" },
  });
  fs.performWrite = (w, onT0) => onT0();
  fs.pageFor = () => PAGE;
  const page = fakePage({
    behaviors: [
      outcome === "notify" ? { kind: "notify", t1: 9_000_000_000_000n } : { kind: "timeout" },
    ],
  });
  const d = deps({ page, fs, server: provServer(lines, dropped) });
  d.provenance = { traceEnabled };
  await runBatch({
    scenarios: [{ id: "s", set: "workload", ops: [{ id: "o", op: "edit-title", article: "a" }] }],
    deps: d,
  });
  return d.recorder.records;
}

describe("diagnostic attempt records", () => {
  it("carry the parsed events, the chain and the DOM outcome; summary lists chainComplete", async () => {
    seq = 0;
    const records = await runOne({ lines: [...acceptedTick(3, [A]), ...afterAdoption()] });
    const rec = records.find((r) => r.type === "attempt");
    expect(rec.provenance).toMatchObject({
      traceEnabled: true,
      traceMissing: false,
      chainComplete: true,
      targetConsumer: PAGE,
    });
    expect(rec.provenance.linesSeen).toBe(10);
    const s = summarize(records, {});
    expect(s.provenance).toEqual([
      expect.objectContaining({ opId: "o", chainComplete: true, traceMissing: false, missing: [] }),
    ]);
  });

  it("gate on with zero lines is traceMissing; buffer loss is recorded", async () => {
    const rec = (await runOne({ lines: [], dropped: 4 })).find((r) => r.type === "attempt");
    expect(rec.provenance).toMatchObject({
      traceMissing: true,
      chainComplete: false,
      droppedBeforeMark: 4,
    });
    expect(rec.provenance.lossNote).toMatch(/4 trace lines were dropped/);
  });

  it("a timeout keeps the DOM link missing even when the trace chain is otherwise whole", async () => {
    seq = 0;
    const rec = (
      await runOne({ lines: [...acceptedTick(3, [A]), ...afterAdoption()], outcome: "timeout" })
    ).find((r) => r.type === "attempt");
    expect(rec.outcome).toBe("timeout");
    expect(rec.provenance.causalChain.missing).toEqual(["dom"]);
  });

  it("non-diagnostic batches carry no provenance block", async () => {
    const fs = fakeFs();
    const d = deps({
      page: fakePage({ behaviors: [{ kind: "notify", t1: 9_000_000_000_000n }] }),
      fs,
    });
    await runBatch({
      scenarios: [{ id: "s", set: "workload", ops: [{ id: "o", op: "edit-title", article: "a" }] }],
      deps: d,
    });
    expect(d.recorder.records.find((r) => r.type === "attempt").provenance).toBeUndefined();
  });
});

describe("provenance example profiles", () => {
  it("validate, are diagnostic and enable both env gates", () => {
    const names = readdirSync(join(HARNESS_DIR, "profiles")).filter((n) =>
      n.includes("provenance-diagnostic"),
    );
    expect(names.sort()).toEqual([
      "field-notes-original-memoized-provenance-diagnostic.example.json",
      "tiny-external-a1-provenance-diagnostic.example.json",
      "tiny-internal-a1-provenance-diagnostic.example.json",
    ]);
    for (const n of names) {
      const p = JSON.parse(readFileSync(join(HARNESS_DIR, "profiles", n), "utf8"));
      expect(validateAs("profile", p), n).toEqual([]);
      expect(semanticErrors(p), n).toEqual([]);
      expect(p.diagnostic).toBe(true);
      expect(p.server.env).toMatchObject({ ZFB_DEV_PROVENANCE_TRACE: "1", ZFB_DEV_TIMING: "1" });
      if (n.startsWith("tiny-")) {
        expect(p.fixture.adapter).toBe("A1");
        expect(new Set(p.scenarios.map((s) => s.warmOrder))).toEqual(
          new Set(["index-first", "detail-first", "none"]),
        );
      }
    }
  });
});

describe("contract update: first_drain_of_generation, drain_contains_page, select pass-through", () => {
  it("skips drains flagged not-first and uses the earliest flagged first", () => {
    const { events } = parseProvLines([
      ...acceptedTick(3, [A]),
      ...afterAdoption(GEN, 4, A, { first: false }).slice(0, 1),
      ...afterAdoption(GEN, 5, A),
    ]);
    const c = buildCausalChain(events, opts());
    expect(c.links.observations).toMatchObject({
      status: "observed",
      drain: 5,
      notFirstDrains: [4],
      first_drain_of_generation: true,
    });
    expect(c.links.edges.drain).toBe(5);
    expect(c.chainComplete).toBe(true);
  });

  it("every drain flagged false: observations missing with the not-first-drain reason", () => {
    const { events } = parseProvLines([
      ...acceptedTick(3, [A]),
      ...afterAdoption(GEN, 4, A, { first: false }),
    ]);
    const c = buildCausalChain(events, opts());
    expect(c.links.observations).toMatchObject({ status: "missing", notFirstDrains: [4] });
    expect(c.links.observations.reason).toMatch(/^not-first-drain/);
    expect(c.chainComplete).toBe(false);
  });

  it("an older binary without the flags is unverified, never observed, and the chain is incomplete", () => {
    const { events } = parseProvLines([
      ...acceptedTick(3, [A]),
      ...afterAdoption(GEN, 4, A, { first: undefined, contains: undefined, graph: undefined }),
    ]);
    const c = buildCausalChain(events, opts());
    expect(c.links.observations.status).toBe("unverified");
    expect(c.links.publish.status).toBe("unverified");
    expect(c.links.edges.status).toBe("unverified");
    expect(c.unverified).toEqual(["observations", "edges", "publish"]);
    expect(c.chainComplete).toBe(false);
  });

  it("a lazy publish with drain_contains_page false is not linked", () => {
    const { events } = parseProvLines([
      ...acceptedTick(3, [A]),
      ...afterAdoption(GEN, 4, A, { contains: false }),
    ]);
    const c = buildCausalChain(events, opts());
    expect(c.links.publish).toMatchObject({ status: "missing", events: [] });
    expect(c.links.publish.unlinked).toHaveLength(1);
    expect(c.chainComplete).toBe(false);
  });

  it("select per-path fields from the real plan pass through unchanged", () => {
    const { events } = parseProvLines([
      line("intake", { tick: 11, paths: [{ path: A, kind: "Removed" }], suppressed: 0 }),
      line("select", {
        tick: 11,
        changes: [
          {
            path: A,
            kind: "Removed",
            class: "Content",
            reason: "global_removed",
            external_override: true,
            future_field: 7,
          },
        ],
        all_pages: true,
        all_reasons: [{ reason: "global_removed" }],
        selected: null,
        noop: false,
      }),
    ]);
    const c = buildCausalChain(events, opts());
    expect(c.links.select.changes[0]).toMatchObject({
      reason: "global_removed",
      external_override: true,
      future_field: 7,
    });
    expect(c.links.select.externalOverride).toBe(true);
    expect(c.links.select.all_reasons).toEqual([{ reason: "global_removed" }]);
  });
});

describe("contract 1e493c9bc", () => {
  it("first_drain_of_generation null is unverified", () => {
    const { events } = parseProvLines([
      ...acceptedTick(3, [A]),
      ...afterAdoption(GEN, 4, A, { first: null }),
    ]);
    const c = buildCausalChain(events, opts());
    expect(c.links.observations.status).toBe("unverified");
    expect(c.chainComplete).toBe(false);
  });

  it("a group counts as a graph edge only when graph_written is true", () => {
    const { events } = parseProvLines([
      ...acceptedTick(3, [A]),
      ...afterAdoption(GEN, 4, A, { graph: false }),
    ]);
    const c = buildCausalChain(events, opts());
    expect(c.links.edges).toMatchObject({
      status: "observed",
      graph_written: false,
      groupNamesTargetAndEntry: true,
      groupLinksTargetToEntry: false,
    });
  });

  it("a lazy publish naming another page is not the target's, even at the same url", () => {
    const { events } = parseProvLines([
      ...acceptedTick(3, [A]),
      ...afterAdoption(GEN, 4, A, { page: HOME }),
    ]);
    expect(buildCausalChain(events, opts()).links.publish.status).toBe("missing");
    const nullPage = parseProvLines([
      ...acceptedTick(3, [A]),
      ...afterAdoption(GEN, 4, A, { page: null }),
    ]).events;
    expect(buildCausalChain(nullPage, opts()).links.publish.status).toBe("observed");
  });

  it("a worker event printed after the drain it enabled still links by generation ids", () => {
    const tick = acceptedTick(3, [A]);
    const worker = tick.find((l) => l.text.includes('"ev":"worker"'));
    const refresh = tick.find((l) => l.text.includes('"ev":"refresh"'));
    const rest = tick.filter((l) => l !== worker && l !== refresh);
    const drained = afterAdoption();
    // Re-sequence so the worker and refresh lines come after the observations and publish.
    seq = 0;
    const reseq = [...rest, ...drained, worker, refresh].map((l) => {
      const j = JSON.parse(l.text.slice("[zfb-prov] ".length));
      return { text: `[zfb-prov] ${JSON.stringify({ ...j, seq: ++seq })}` };
    });
    const c = buildCausalChain(parseProvLines(reseq).events, opts());
    expect(c.links.generation.outcome).toBe("adopted");
    expect(c.chainComplete).toBe(true);
  });

  it('select dirty may be a page list or "all"; class may be null; per-path fields pass through', () => {
    const sel = (dirty, extra = {}) =>
      parseProvLines([
        line("intake", { tick: 12, paths: [{ path: A, kind: "Modified" }], suppressed: 0 }),
        line("select", {
          tick: 12,
          changes: [{ path: A, kind: "Modified", class: null, dirty, ...extra }],
          all_pages: false,
          all_reasons: [],
          selected: [PAGE],
          noop: false,
        }),
      ]).events;
    expect(buildCausalChain(sel("all"), opts()).links.select.targetDirty).toBe(true);
    expect(buildCausalChain(sel([HOME]), opts()).links.select.targetDirty).toBe(false);
    const c = buildCausalChain(
      sel([PAGE], {
        external_override: true,
        ssr_module_dependency: "all",
        all_reasons: ["external", "unattributed"],
        graph_known: true,
      }),
      opts(),
    );
    expect(c.links.select.targetDirty).toBe(true);
    expect(c.links.select.externalOverride).toBe(true);
    expect(c.links.select.changes[0]).toMatchObject({
      class: null,
      ssr_module_dependency: "all",
      all_reasons: ["external", "unattributed"],
      graph_known: true,
    });
  });
});
