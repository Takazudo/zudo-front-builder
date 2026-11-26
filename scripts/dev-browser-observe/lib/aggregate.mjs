import { existsSync, readdirSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { rendererMemoryBuild } from "./renderer-memory-protocol.mjs";
import { distribution } from "./stats.mjs";
import { readJson, readJsonl } from "./util.mjs";

export const AGGREGATE_SCHEMA = "dev-browser-observe.aggregate.v1";

/** Facets a variant may declare as different from the other variant. Every other facet must match. */
export const DECLARABLE_FACETS = ["build", "toolchain", "framework"];

/**
 * Declaring `framework` (fixed-distribution cohorts only, e.g. fixed ZFB vs Astro) also lets the
 * facets each framework's own app defines differ: its helper (adapter), config and installed
 * executables (build). Corpus, browser, harness, observer, host, schedule and node must still match.
 */
export const FRAMEWORK_IMPLIED_FACETS = ["adapter", "config", "build"];

/** Facets that may differ given a declaration list. */
export function allowedDifferences(declared) {
  return declared.includes("framework")
    ? [...new Set([...declared, ...FRAMEWORK_IMPLIED_FACETS, "profile"])]
    : declared;
}

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

const without = (obj, path) => {
  if (!obj || typeof obj !== "object") return obj;
  const [head, ...rest] = path;
  if (!(head in obj)) return obj;
  if (rest.length === 0) return omit(obj, [head]);
  return { ...obj, [head]: without(obj[head], rest) };
};

/**
 * A profile without the keys that select the build a variant runs; with `framework`, also
 * without the keys that select the framework (framework, its helper and its helper pin).
 */
export function stripVariantKeys(profile, { framework = false } = {}) {
  let p = omit(profile, [...PROFILE_VARIANT_KEYS, "outDir", "lockPath"]);
  p = without(p, ["distribution", "installDir"]);
  if (framework) {
    p = omit(p, ["framework"]);
    p = without(p, ["fixture", "adapter"]);
    p = without(p, ["pins", "adapterSha256"]);
  }
  return p;
}

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
    framework: {
      mode: receipt.mode ?? "source-built",
      framework: receipt.framework ?? "zfb-source-built",
    },
    toolchain:
      receipt.mode === "fixed-distribution"
        ? {
            node: receipt.distribution?.node
              ? {
                  version: receipt.distribution.node.version,
                  sha256: receipt.distribution.node.sha256,
                }
              : null,
          }
        : (b.toolchain ?? null),
    build:
      receipt.mode === "fixed-distribution"
        ? {
            pinned: f.pinned ?? null,
            lockfileSha256: receipt.distribution?.install?.lockfileSha256 ?? null,
            appTreeSha256: f.appTreeSha256 ?? null,
            nodeModulesSha256: f.nodeModules?.sha256 ?? null,
            executables: (receipt.distribution?.install?.executables ?? []).map((x) => ({
              role: x.role,
              sha256: x.sha256 ?? null,
            })),
            launch: receipt.server?.launch?.kind ?? null,
          }
        : {
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
    profile: stripVariantKeys(profile),
    profileFrameworkNeutral: stripVariantKeys(profile, { framework: true }),
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
 * Compare the facets of sessions that ran (a label dir exists; unrun sessions have no identity): within a variant every facet must match; across variants every facet
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
    const declared = allowedDifferences(declaredDifferences).includes(facet);
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
  if (declaredDifferences.includes("framework") && withReceipt.length) {
    const across = checks.find((c) => c.name === "across-variants:framework");
    checks.push({
      name: "framework-declared-and-differs",
      ok: across?.differs === true,
      expected: "the variants run different frameworks",
      actual: across?.differs ?? null,
    });
  }
  if (declaredDifferences.includes("framework")) {
    const notFixed = withReceipt.filter((s) => s.facets.framework.mode !== "fixed-distribution");
    checks.push({
      name: "framework-difference-only-for-fixed-distribution",
      ok: notFixed.length === 0,
      expected: "every session fixed-distribution",
      actual: notFixed.map((s) => s.label),
    });
  }
  for (const s of sessions)
    if (s.ran && !s.facets)
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

/** notAttempted is unknown (null) without a planned count, and never negative: more attempts than planned is flagged instead. */
function withNotAttempted(c) {
  const diff = c.planned === null ? null : c.planned - c.attempted;
  return {
    ...c,
    notAttempted: diff !== null && diff >= 0 ? diff : null,
    ...(diff !== null && diff < 0 ? { countsInconsistent: "attempted exceeds planned" } : {}),
  };
}

function finishCounts(counts, latencies) {
  const c = withNotAttempted(counts);
  const { values, ...d } = distribution(latencies);
  const observed = c.stale + c.timeout + c.error > 0 || (c.notAttempted ?? 0) > 0;
  return {
    ...c,
    latencyMs: {
      ...d,
      values,
      conditionalOnSuccess: true,
      // null: no observed failure, but without a planned count missing attempts cannot be ruled out.
      failuresPresent: observed ? true : c.notAttempted === null ? null : false,
    },
  };
}

/** Per-op block statistics of one session label dir; warmups are counted apart and never enter stats. */
export function sessionBlocks(labelDir) {
  const records = readJsonl(join(labelDir, "records.jsonl"));
  const runStart = records.find((r) => r.type === "run-start") ?? {};
  return blocksFrom(records, runStart.schedule ?? null);
}

/** Blocks from records against a schedule; a session that never ran passes no records. */
export function blocksFrom(records, schedule) {
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
      warmup: { ...withNotAttempted(b.warmup), excludedFromStats: true },
    })),
  };
}

// A worker can report success before its process exits. Look for contrary outer
// evidence even for --sessions, which supplies paths without a series record.
function terminalEvidence(dir, receipt, summary, records, seriesStatus, seriesExitCode) {
  const label = basename(dir);
  const outDir = dirname(dir);
  const problems = [];
  if (seriesStatus !== null && (seriesStatus !== "completed" || seriesExitCode !== 0))
    problems.push("series session did not complete with exit 0");
  if (summary?.ownedServerAbsentAfterStop !== true || summary.stopFailure)
    problems.push("summary lacks verified server cleanup");
  const end = records.at(-1);
  if (end?.type !== "run-end" || end.exitCode !== 0)
    problems.push("missing successful final run-end");
  const stops = records.filter((r) => r.type === "server-stop");
  if (stops.length !== 1 || stops[0].report?.absent !== true || stops[0].report?.error)
    problems.push("missing verified server-stop");
  if (receipt?.supervisorId != null) {
    const completion = readJson(join(dir, "supervisor-completion.json"));
    if (
      completion.schema !== "dev-browser-observe.supervisor-completion.v1" ||
      completion.supervisorId !== receipt.supervisorId ||
      completion.exitCode !== 0
    )
      problems.push("supervisor completion does not match this session");
  }
  // A series label is generated from its prefix, session index and variant.
  if (seriesStatus === null) {
    const match = /^(.*)-s[1-9][0-9]*-[A-Za-z0-9]+$/.exec(label);
    if (match) {
      const path = join(outDir, match[1] + ".series.json");
      if (existsSync(path)) {
        const record = readJson(path);
        const row = record.sessions.find((x) => x.label === label);
        if (!row || row.status !== "completed" || row.exitCode !== 0)
          problems.push("series record lacks completed session with exit 0");
      }
    }
  }
  const prefix = label + ".supervisor-";
  for (const name of readdirSync(outDir)) {
    if (!name.startsWith(prefix) || !name.endsWith(".json")) continue;
    const failure = readJson(join(outDir, name));
    if (
      !receipt?.supervisorId ||
      !failure.supervisorId ||
      failure.supervisorId === receipt.supervisorId
    )
      problems.push("supervisor failure: " + name);
  }
  return problems;
}

/** Read one session: receipt facets, summary, blocks, and whether it ran to completion. */
export function readSession({
  label,
  variant,
  dir,
  index = null,
  seriesStatus = null,
  seriesExitCode = null,
  plannedSchedule = null,
  plannedError = null,
}) {
  const s = { index, label, variant, dir, ran: existsSync(dir) };
  if (!s.ran) {
    // Never ran: no identity to compare; its planned attempts all count as not attempted.
    const { blocks, schedulePresent } = plannedSchedule
      ? blocksFrom([], plannedSchedule)
      : { blocks: [], schedulePresent: false };
    return {
      ...s,
      status: seriesStatus === "not-attempted" ? "not-attempted" : "missing",
      seriesStatus,
      facets: null,
      schedulePresent,
      plannedError,
      blocks,
    };
  }
  let receipt = null;
  try {
    receipt = readJson(join(dir, "receipt.json"));
  } catch (error) {
    s.error = String(error?.message ?? error);
  }
  // Do not read or pool observer timings from a renderer-memory run, even if its
  // declared profile and build are inconsistent or the session itself failed.
  if (
    receipt?.profile?.tinyAcceptance ||
    receipt?.tinyAcceptance ||
    receipt?.command === "tiny-acceptance" ||
    receipt?.profile?.startupAcceptance ||
    receipt?.startupAcceptance ||
    receipt?.command === "startup-acceptance" ||
    receipt?.profile?.rendererMemoryStudy ||
    receipt?.rendererMemoryStudy ||
    receipt?.buildReceipt?.diagnostic === true ||
    rendererMemoryBuild(receipt?.buildReceipt)
  )
    return {
      ...s,
      status: "incomplete",
      seriesStatus,
      terminalProblems: ["renderer-memory diagnostic excluded from timing aggregation"],
      facets: receipt ? sessionFacets(receipt) : null,
      blocks: [],
      timingExcluded: true,
    };
  let summary = null;
  try {
    summary = readJson(join(dir, "summary.json"));
  } catch {
    summary = null;
  }
  let records = [];
  let problems;
  try {
    records = readJsonl(join(dir, "records.jsonl"));
    const start = records.find((r) => r.type === "run-start");
    if (
      start?.rendererMemoryStudy ||
      start?.tinyAcceptance ||
      start?.command === "tiny-acceptance" ||
      start?.startupAcceptance ||
      start?.command === "startup-acceptance"
    )
      return { ...s, status: "incomplete", facets: null, blocks: [], timingExcluded: true };
    problems = terminalEvidence(dir, receipt, summary, records, seriesStatus, seriesExitCode);
  } catch (error) {
    problems = ["unreadable terminal evidence: " + error.message];
  }
  const schedule = records.find((r) => r.type === "run-start")?.schedule ?? null;
  const { blocks, schedulePresent } = blocksFrom(records, schedule);
  const acc = summary?.accounting ?? null;
  const complete =
    problems.length === 0 &&
    schedulePresent &&
    summary?.exitCode === 0 &&
    acc !== null &&
    acc.stopReason === null &&
    acc.notAttempted === 0 &&
    receipt?.identity?.ok === true;
  return {
    ...s,
    status: complete ? "complete" : "incomplete",
    exitCode: summary?.exitCode ?? null,
    seriesStatus,
    terminalProblems: problems,
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
      // One block without a planned count makes the pooled planned (and notAttempted) unknown.
      p.counts.planned =
        p.counts.planned === null || b.measured.planned === null
          ? null
          : p.counts.planned + b.measured.planned;
      for (const k of ["attempted", "success", "stale", "timeout", "error"])
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
 * and the session completeness. Exit: 3 identity mismatch among the sessions that ran, 1 any session
 * incomplete, missing or not attempted, else 0.
 */
export function aggregate({ sessions, declaredDifferences = [], protocol = null, series = null }) {
  const read = sessions.map((s) => readSession(s));
  const identity = compareFacets(
    read.filter((s) => s.ran),
    declaredDifferences,
  );
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
    facets: Object.fromEntries(read.filter((s) => s.ran).map((s) => [s.label, s.facets])),
    pooledComparable: incomplete.length === 0 && identity.ok,
    pooled: poolVariants(read.filter((s) => !s.timingExcluded)),
  };
  const exitCode = !identity.ok ? 3 : incomplete.length > 0 ? 1 : 0;
  return { report, exitCode };
}
