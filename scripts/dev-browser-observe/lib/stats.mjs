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
  const successes = records.filter(
    (r) => r.type === "attempt" && r.outcome === "success" && typeof r.latencyMs === "number",
  );
  const byOp = {};
  for (const r of successes) (byOp[r.op] ??= []).push(r.latencyMs);
  return {
    accounting,
    endpoint: "predicate confirmed + 2 rAF + binding delivery",
    latencyConditionalOnSuccessMs: distribution(successes.map((r) => r.latencyMs)),
    byOpConditionalOnSuccessMs: Object.fromEntries(
      Object.entries(byOp).map(([k, v]) => [k, distribution(v)]),
    ),
    mechanisms: successes.reduce((acc, r) => {
      acc[r.mechanism] = (acc[r.mechanism] ?? 0) + 1;
      return acc;
    }, {}),
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
