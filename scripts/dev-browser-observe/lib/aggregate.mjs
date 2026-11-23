import { existsSync } from "node:fs";
import { join } from "node:path";
import { distribution } from "./stats.mjs";
import { readJson, readJsonl } from "./util.mjs";

export const AGGREGATE_SCHEMA = "dev-browser-observe.aggregate.v1";

/** Facets a variant may declare as different from the other variant. Every other facet must match. */
export const DECLARABLE_FACETS = ["build", "toolchain"];

const omit = (obj, keys) =>
  obj && typeof obj === "object"
    ? Object.fromEntries(Object.entries(obj).filter(([k]) => !keys.includes(k)))
    : (obj ?? null);

/** Profile keys that may differ between the variants of one series (the build each runs). */
export const PROFILE_VARIANT_KEYS = [
  "buildReceipt",
  "productSha",
  "productDirtyIdentity",
  "description",
];

/** Identity facets of one session, read from its receipt.json. */
export function sessionFacets(receipt) {
  const f = receipt.fixture ?? {};
  const b = receipt.buildReceipt ?? {};
  const profile = receipt.profile ?? {};
  return {
    corpus: {
      kind: f.kind ?? null,
      scale: f.scale ?? null,
      scaleId: f.scaleId ?? null,
      sourcePins: omit(f.sourcePins, ["dir"]),
      materializedDatasetSha256: f.materializedDatasetSha256 ?? null,
      placement: f.placement ?? null,
    },
    adapter: {
      adapter: f.adapter ?? null,
      adapterSha256: f.adapterSha256 ?? null,
      adapterSourceSha256: f.adapterSourceSha256 ?? null,
    },
    config: {
      configSha256: f.configSha256 ?? null,
      configBaseSha256: f.configBaseSha256 ?? null,
      managedCss: f.managedCss ?? null,
      trailingSlash: f.trailingSlash ?? null,
    },
    toolchain: b.toolchain ?? null,
    build: {
      exeSha256: b.exe?.sha256 ?? null,
      sourceSha: b.source?.sha ?? null,
      dirtyIdentitySha256: b.source?.dirtyIdentitySha256 ?? null,
      cargoLockSha256: b.cargoLockSha256 ?? null,
      build: omit(b.build, ["targetDir", "command"]),
      vendorTreeSha256: b.vendor?.chosen?.treeSha256 ?? null,
    },
    browser: omit(receipt.browser, []),
    harness: {
      pins: omit(profile.pins, ["observerSha256", "adapterSha256"]),
      capability: omit(receipt.harnessCapability, []),
    },
    observer: {
      pin: profile.pins?.observerSha256 ?? null,
      browserObserverSha256: receipt.browser?.observerSha256 ?? null,
    },
    host: omit(receipt.host, ["freememBytes", "loadavg"]),
    profile: omit(profile, [...PROFILE_VARIANT_KEYS, "outDir", "lockPath"]),
    schedule: receipt.schedule ?? null,
  };
}

const canon = (v) =>
  JSON.stringify(v, (_k, x) =>
    x && typeof x === "object" && !Array.isArray(x)
      ? Object.fromEntries(
          Object.keys(x)
            .sort()
            .map((k) => [k, x[k]]),
        )
      : x,
  );

/**
 * Compare sessions' facets: within a variant every facet must match; across variants every facet
 * except the declared ones must match. Returns { ok, checks }.
 */
export function compareFacets(sessions, declaredDifferences = []) {
  const checks = [];
  const withReceipt = sessions.filter((s) => s.facets);
  const facetNames = withReceipt.length ? Object.keys(withReceipt[0].facets) : [];
  const variants = [...new Set(withReceipt.map((s) => s.variant))];
  for (const facet of facetNames) {
    for (const v of variants) {
      const vals = withReceipt.filter((s) => s.variant === v).map((s) => canon(s.facets[facet]));
      checks.push({
        name: `within-variant:${v}:${facet}`,
        ok: new Set(vals).size <= 1,
        sessions: withReceipt.filter((s) => s.variant === v).map((s) => s.label),
      });
    }
    const declared = declaredDifferences.includes(facet);
    const firsts = variants.map((v) =>
      canon(withReceipt.find((s) => s.variant === v).facets[facet]),
    );
    const same = new Set(firsts).size <= 1;
    checks.push({
      name: `across-variants:${facet}`,
      ok: declared || same,
      declaredDifference: declared,
      differs: !same,
      values: same
        ? undefined
        : Object.fromEntries(variants.map((v, i) => [v, JSON.parse(firsts[i])])),
    });
  }
  for (const s of sessions)
    if (!s.facets)
      checks.push({ name: `receipt-readable:${s.label}`, ok: false, actual: s.error ?? null });
  return { ok: checks.every((c) => c.ok), checks };
}

const OUTCOME_BUCKET = (o) =>
  o === "success" ? "success" : o === "stale" ? "stale" : o === "timeout" ? "timeout" : "error";

function emptyCounts(planned) {
  return {
    planned,
    attempted: 0,
    success: 0,
    stale: 0,
    timeout: 0,
    error: 0,
    notAttempted: planned,
  };
}

function finishCounts(c, latencies) {
  c.notAttempted = c.planned === null ? null : c.planned - c.attempted;
  const { values, ...d } = distribution(latencies);
  return {
    ...c,
    latencyMs: {
      ...d,
      values,
      conditionalOnSuccess: true,
      failuresPresent: c.stale + c.timeout + c.error > 0 || (c.notAttempted ?? 0) > 0,
    },
  };
}

/** Per-op block statistics of one session label dir; warmups are counted apart and never enter stats. */
export function sessionBlocks(labelDir) {
  const records = readJsonl(join(labelDir, "records.jsonl"));
  const runStart = records.find((r) => r.type === "run-start") ?? {};
  const schedule = runStart.schedule ?? null;
  const byScenarioOp = new Map();
  for (const e of schedule ?? []) byScenarioOp.set(`${e.scenario}\0${e.opId}`, e);
  const keyOf = (r) => {
    const e = byScenarioOp.get(`${r.scenario}\0${r.opId}`);
    const base = e?.scenarioBase ?? r.scenario;
    const op = e?.baseOpId ?? r.iteration?.opId ?? r.opId;
    return { key: `${base}/${op}`, scenario: base, opId: op, op: r.op ?? e?.op ?? null };
  };
  const blocks = new Map();
  const blockFor = (k) => {
    if (!blocks.has(k.key))
      blocks.set(k.key, {
        scenario: k.scenario,
        opId: k.opId,
        op: k.op,
        measured: emptyCounts(schedule ? 0 : null),
        warmup: emptyCounts(schedule ? 0 : null),
        latencies: [],
      });
    return blocks.get(k.key);
  };
  for (const e of schedule ?? []) {
    const b = blockFor({
      key: `${e.scenarioBase}/${e.baseOpId}`,
      scenario: e.scenarioBase,
      opId: e.baseOpId,
      op: e.op,
    });
    b[e.kind === "warmup" ? "warmup" : "measured"].planned += 1;
  }
  for (const r of records) {
    if (r.type !== "attempt" && r.type !== "precheck") continue;
    const b = blockFor(keyOf(r));
    const c = r.iteration?.kind === "warmup" ? b.warmup : b.measured;
    if (r.type === "precheck") {
      c.error += 1;
      continue;
    }
    c.attempted += 1;
    c[OUTCOME_BUCKET(r.outcome)] += 1;
    if (
      r.iteration?.kind !== "warmup" &&
      r.outcome === "success" &&
      typeof r.latencyMs === "number"
    )
      b.latencies.push(r.latencyMs);
  }
  return {
    schedulePresent: schedule !== null,
    blocks: [...blocks.values()].map((b) => ({
      scenario: b.scenario,
      opId: b.opId,
      op: b.op,
      measured: finishCounts(b.measured, b.latencies),
      warmup: (() => {
        const w = { ...b.warmup };
        w.notAttempted = w.planned === null ? null : w.planned - w.attempted;
        return { ...w, excludedFromStats: true };
      })(),
    })),
  };
}

/** Read one session: receipt facets, summary, blocks, and whether it ran to completion. */
export function readSession({ label, variant, dir, index = null }) {
  const s = { index, label, variant, dir };
  if (!existsSync(dir))
    return { ...s, status: "missing", facets: null, blocks: [], error: "no label dir" };
  let receipt = null;
  try {
    receipt = readJson(join(dir, "receipt.json"));
  } catch (error) {
    s.error = String(error?.message ?? error);
  }
  let summary = null;
  try {
    summary = readJson(join(dir, "summary.json"));
  } catch {
    summary = null;
  }
  const { blocks, schedulePresent } = sessionBlocks(dir);
  const acc = summary?.accounting ?? null;
  const complete =
    summary?.exitCode === 0 &&
    acc !== null &&
    acc.stopReason === null &&
    acc.notAttempted === 0 &&
    receipt?.identity?.ok === true;
  return {
    ...s,
    status: complete ? "complete" : "incomplete",
    exitCode: summary?.exitCode ?? null,
    stopReason: acc?.stopReason ?? null,
    stoppedAfter: acc?.stoppedAfter ?? null,
    identityOk: receipt?.identity?.ok ?? null,
    accounting: acc,
    schedulePresent,
    facets: receipt ? sessionFacets(receipt) : null,
    blocks,
    resourceStudy: summary?.resourceStudy ?? null,
  };
}

/** Pool each variant's measured blocks across its sessions (counts summed, latencies concatenated). */
export function poolVariants(sessions) {
  const pooled = {};
  for (const s of sessions) {
    const v = (pooled[s.variant] ??= { sessions: [], ops: {} });
    v.sessions.push(s.label);
    for (const b of s.blocks ?? []) {
      const key = `${b.scenario}/${b.opId}`;
      const p = (v.ops[key] ??= {
        scenario: b.scenario,
        opId: b.opId,
        op: b.op,
        counts: emptyCounts(0),
        latencies: [],
        sessions: [],
      });
      p.sessions.push(s.label);
      for (const k of ["planned", "attempted", "success", "stale", "timeout", "error"])
        p.counts[k] += b.measured[k] ?? 0;
      p.latencies.push(...b.measured.latencyMs.values);
    }
  }
  for (const v of Object.values(pooled))
    for (const [key, p] of Object.entries(v.ops)) {
      const { latencies, counts, ...rest } = p;
      v.ops[key] = { ...rest, measured: finishCounts(counts, latencies) };
    }
  return pooled;
}

/**
 * Aggregate the given sessions: per-session blocks, pooled per-variant stats, the identity comparison
 * and the session completeness. Exit: 3 identity mismatch, 1 any session incomplete, else 0.
 */
export function aggregate({ sessions, declaredDifferences = [], protocol = null, series = null }) {
  const read = sessions.map((s) => readSession(s));
  const identity = compareFacets(read, declaredDifferences);
  const incomplete = read.filter((s) => s.status !== "complete").map((s) => s.label);
  const report = {
    schema: AGGREGATE_SCHEMA,
    protocol,
    series,
    declaredDifferences,
    endpoint: "predicate confirmed + 2 rAF + binding delivery",
    statsNote:
      "latency stats are conditional on success, over measured attempts only (warmups excluded); p95 is nearest-rank",
    identity,
    complete: incomplete.length === 0,
    incompleteSessions: incomplete,
    sessions: read.map(({ facets: _f, ...rest }) => rest),
    facets: Object.fromEntries(read.map((s) => [s.label, s.facets])),
    pooled: poolVariants(read),
  };
  const exitCode = !identity.ok ? 3 : incomplete.length > 0 ? 1 : 0;
  return { report, exitCode };
}
