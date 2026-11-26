export const TIMING_ABBA_PROTOCOL = "dev-browser-observe.timing-abba.v1";
export const RESOURCE_STUDY_PROTOCOL = "dev-browser-observe.resource-study.v1";

/** Constants each protocol id stands for; a profile or series that names the id must use exactly these. */
export const PROTOCOLS = {
  [TIMING_ABBA_PROTOCOL]: {
    warmups: 2,
    measured: 10,
    orderPattern: "XYYX",
    ops: ["edit-body", "edit-shared-component", "edit-managed-css", "edit-title", "edit-category"],
    session: "one run per session: a fresh server process and a fresh browser",
  },
  [RESOURCE_STUDY_PROTOCOL]: {
    cycles: 30,
    idleAfterRestoreMs: 10000,
    finalIdleMs: 60000,
    minSampleEveryMs: 1000,
    sampleMethod:
      "async ps -o pid=,rss=,vsz= over the birth-verified owned pids, plus async Linux /proc/PID/stat num_threads or macOS ps -M for threads",
    labels: {
      rss: "instantaneous RSS in KiB; summed across owned processes it is not unique memory",
      peak: "sampled values over discrete samples, not a lifetime high-water mark",
      inference: "no leak, growth trend or OOM is inferred from these samples",
    },
  },
};

/** Field-notes ops that may repeat in one scenario: each run is a fresh token on the current model. */
export const REPEATABLE_OPS = new Set([
  "edit-body",
  "edit-shared-component",
  "edit-managed-css",
  "edit-title",
  "edit-category",
  "edit-prop",
]);

/** Profile rules for op.repeat and resourceStudy. */
export function scheduleErrors(profile) {
  const errors = [];
  const fieldNotes = (profile.fixture?.kind ?? "tiny") === "field-notes";
  for (const s of profile.scenarios ?? [])
    for (const [i, op] of (s.ops ?? []).entries()) {
      if (!op.repeat) continue;
      const at = `$.scenarios[${s.id}].ops[${i}]`;
      if (!fieldNotes && profile.tinyAcceptance?.probe !== "B")
        errors.push(`${at}: repeat is field-notes only`);
      if (!REPEATABLE_OPS.has(op.op))
        errors.push(
          `${at}: ${op.op} cannot repeat (repeatable: ${[...REPEATABLE_OPS].join(", ")})`,
        );
      if ((s.set ?? "workload") !== "workload")
        errors.push(`${at}: repeat is only allowed in a workload scenario`);
      if (op.expect === "timeout") errors.push(`${at}: a declared negative control cannot repeat`);
      if (op.inject) errors.push(`${at}: an injected op cannot repeat`);
      if (profile.resourceStudy)
        errors.push(`${at}: a resource-study profile cycles its scenario instead of repeat`);
    }
  const rs = profile.resourceStudy;
  if (rs) {
    const c = PROTOCOLS[RESOURCE_STUDY_PROTOCOL];
    if (!fieldNotes) errors.push("$.resourceStudy is field-notes only");
    for (const k of ["cycles", "idleAfterRestoreMs", "finalIdleMs"])
      if (rs[k] !== c[k])
        errors.push(`$.resourceStudy.${k} must be ${c[k]} for ${RESOURCE_STUDY_PROTOCOL}`);
    if (!(rs.sampleEveryMs >= c.minSampleEveryMs))
      errors.push(`$.resourceStudy.sampleEveryMs must be at least ${c.minSampleEveryMs}`);
    for (const s of profile.scenarios ?? []) {
      if ((s.set ?? "workload") !== "workload")
        errors.push(`$.scenarios[${s.id}]: a resource-study scenario must be a workload`);
      for (const [i, op] of (s.ops ?? []).entries())
        if (!REPEATABLE_OPS.has(op.op))
          errors.push(
            `$.scenarios[${s.id}].ops[${i}]: ${op.op} is not an edit that a restore cycle can repeat`,
          );
    }
  }
  return errors;
}

const pad = (n, w) => String(n).padStart(w, "0");

/**
 * Expand op.repeat into consecutive ops (warmups first) and a resource study into one scenario copy
 * per cycle. Every expanded op carries `iteration`; the expansion is pure and recorded verbatim.
 */
export function expandSchedule(profile) {
  const rs = profile.resourceStudy ?? null;
  if (rs && (profile.scenarios ?? []).length !== 1)
    throw new Error(
      `a resource-study run needs exactly one scenario (got ${(profile.scenarios ?? []).length}); select one with --scenario`,
    );
  if (rs) {
    const [s] = profile.scenarios;
    const w = String(rs.cycles).length;
    return Array.from({ length: rs.cycles }, (_, c) => ({
      ...s,
      id: `${s.id}@c${pad(c + 1, w)}`,
      cycle: { scenario: s.id, index: c + 1, of: rs.cycles },
      ops: s.ops.map((op) => ({
        ...op,
        iteration: {
          opId: op.id ?? op.op,
          kind: "cycle",
          index: c + 1,
          of: rs.cycles,
        },
      })),
    }));
  }
  return (profile.scenarios ?? []).map((s) => ({
    ...s,
    ops: s.ops.flatMap((op) => {
      if (!op.repeat) return [op];
      const base = op.id ?? op.op;
      const { repeat, ...rest } = op;
      const out = [];
      for (let k = 1; k <= repeat.warmups; k += 1)
        out.push({
          ...rest,
          id: `${base}#w${k}`,
          iteration: { opId: base, kind: "warmup", index: k, of: repeat.warmups },
        });
      for (let k = 1; k <= repeat.measured; k += 1)
        out.push({
          ...rest,
          id: `${base}#m${k}`,
          iteration: { opId: base, kind: "measured", index: k, of: repeat.measured },
        });
      return out;
    }),
  }));
}

/** Compact list of what will run, in order: scenario, op id, base op id, iteration kind and index. */
export function scheduleOf(scenarios) {
  return scenarios.flatMap((s) =>
    s.ops.map((op) => ({
      scenario: s.id,
      scenarioBase: s.cycle?.scenario ?? s.id,
      opId: op.id ?? op.op,
      baseOpId: op.iteration?.opId ?? op.id ?? op.op,
      op: op.op,
      kind: op.iteration?.kind ?? "measured",
      index: op.iteration?.index ?? 1,
    })),
  );
}
