export function nearestRank(sorted, p) {
  if (sorted.length === 0) return null;
  const rank = Math.ceil((p / 100) * sorted.length);
  return sorted[Math.min(sorted.length, Math.max(1, rank)) - 1];
}

export function median(sorted) {
  if (sorted.length === 0) return null;
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

export function distribution(values) {
  const sorted = [...values].sort((a, b) => a - b);
  return {
    n: sorted.length,
    min: sorted.length ? sorted[0] : null,
    median: median(sorted),
    p95NearestRank: nearestRank(sorted, 95),
    max: sorted.length ? sorted[sorted.length - 1] : null,
    values: sorted,
  };
}

/** Latency stats are conditional on success: failed attempts never contribute a duration. */
export function summarize(records, accounting) {
  // hidden-edit and final-state successes carry no update latency, so they are left out of the distribution.
  // Warmup attempts (op.repeat) are recorded and counted in accounting but never enter a distribution.
  const isWarmup = (r) => r.iteration?.kind === "warmup";
  const tinyDiagnostic = records.some((r) => r.type === "run-start" && r.tinyAcceptance);
  const successes = records.filter(
    (r) =>
      r.type === "attempt" &&
      r.outcome === "success" &&
      typeof r.latencyMs === "number" &&
      !tinyDiagnostic &&
      !isWarmup(r),
  );
  const warmups = records.filter((r) => r.type === "attempt" && isWarmup(r));
  const byOp = {};
  for (const r of successes) (byOp[r.op] ??= []).push(r.latencyMs);
  return {
    accounting,
    endpoint: "predicate confirmed + 2 rAF + binding delivery",
    warmupsExcludedFromStats: {
      attempts: warmups.length,
      success: warmups.filter((r) => r.outcome === "success").length,
    },
    latencyConditionalOnSuccessMs: distribution(successes.map((r) => r.latencyMs)),
    byOpConditionalOnSuccessMs: Object.fromEntries(
      Object.entries(byOp).map(([k, v]) => [k, distribution(v)]),
    ),
    mechanisms: successes.reduce((acc, r) => {
      acc[r.mechanism] = (acc[r.mechanism] ?? 0) + 1;
      return acc;
    }, {}),
    // Diagnostic attempts: whether the provenance trace proved the causal chain, and what is missing.
    provenance: records
      .filter((r) => r.type === "attempt" && r.provenance)
      .map((r) => ({
        scenario: r.scenario,
        opId: r.opId,
        outcome: r.outcome,
        traceEnabled: r.provenance.traceEnabled ?? null,
        traceMissing: r.provenance.traceMissing ?? null,
        chainComplete: r.provenance.chainComplete ?? false,
        chainEventsOnly: r.provenance.chainEventsOnly ?? false,
        chainConsumerLevel: r.provenance.chainConsumerLevel ?? false,
        evidenceVia: r.provenance.evidenceVia ?? null,
        reasons: r.provenance.causalChain?.reasons ?? null,
        integrity: r.provenance.causalChain?.integrity?.status ?? null,
        lackingTarget: r.provenance.causalChain?.lackingTarget ?? null,
        missing: r.provenance.causalChain?.missing ?? null,
        parseErrors: r.provenance.parseErrors?.length ?? null,
      })),
    // Counts over the diagnostic attempts; chainEventsOnly and incomplete are never complete.
    provenanceCounts: (() => {
      const ps = records
        .filter((r) => r.type === "attempt" && r.provenance)
        .map((r) => r.provenance);
      return {
        attempts: ps.length,
        chainComplete: ps.filter((p) => p.chainComplete === true).length,
        chainEventsOnly: ps.filter((p) => p.chainEventsOnly === true).length,
        chainConsumerLevel: ps.filter((p) => p.chainConsumerLevel === true).length,
        incomplete: ps.filter(
          (p) =>
            p.chainComplete !== true && p.chainEventsOnly !== true && p.chainConsumerLevel !== true,
        ).length,
        traceIntegrityUnverified: ps.filter(
          (p) => p.causalChain?.integrity?.status === "unverified",
        ).length,
        byEvidenceVia: ps.reduce((acc, p) => {
          const k = p.evidenceVia ?? "none";
          acc[k] = (acc[k] ?? 0) + 1;
          return acc;
        }, {}),
        traceMissing: ps.filter((p) => p.traceMissing === true).length,
      };
    })(),
    // Recovery attempts, each with whether its same tick was proven (true), disproven (false) or neither (null).
    recovery: records
      .filter((r) => r.type === "attempt" && r.mode === "recovery")
      .map((r) => ({
        scenario: r.scenario,
        opId: r.opId,
        op: r.op,
        outcome: r.outcome,
        sameTickProven: r.sameTickProven ?? null,
        latencyMs: typeof r.latencyMs === "number" ? r.latencyMs : null,
      })),
  };
}
