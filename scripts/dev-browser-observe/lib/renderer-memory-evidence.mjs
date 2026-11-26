import { parseProvLines, PROV_PREFIX } from "./provenance.mjs";
import { inspectReclamation, reclamationDisposition } from "./renderer-memory-reclamation.mjs";
import {
  RENDERER_MEMORY_ALLOCATOR_DOMAIN as ALLOCATOR_DOMAIN,
  RENDERER_MEMORY_ALLOCATOR_LIMIT as ALLOCATOR_LIMIT,
  RENDERER_MEMORY_LIMITS as LIMITS,
  RENDERER_MEMORY_RECLAMATION_LIMITS as RECLAMATION_LIMITS,
} from "./renderer-memory-protocol.mjs";

export { sampleRendererMemory } from "./linux-rss-sample.mjs";

export function parseMemoryKiB(text, fields) {
  const values = Object.fromEntries(fields.map((field) => [field, null]));
  for (const line of text.split("\n")) {
    const row = /^(\w+):\s+(\d+)\s+kB\s*$/.exec(line);
    if (row && Object.hasOwn(values, row[1]) && Number.isSafeInteger(Number(row[2]))) {
      values[row[1]] = Number(row[2]);
    }
  }
  return values;
}

const LIFECYCLE = "renderer_lifecycle";
const START = "renderer_lifecycle_start";
const TERMINAL = "renderer_lifecycle_terminal";
const LOSS = "renderer_lifecycle_loss";
const positive = (n) => Number.isSafeInteger(n) && n > 0;
const counter = (n) => Number.isSafeInteger(n) && n >= 0;
const ALLOCATOR_START = "allocator_start";
const ALLOCATOR_COMPLETE = "allocator_complete";
const APPLY_RETURN = "apply_return";
const POST_APPLY = "post_apply_return";
const APPLY_FIELDS = ["apply_id", "apply_outcome", "refresh_outcome"];
const ALLOCATOR_PHASES = [
  "entry",
  "snapshot_before",
  "snapshot_after",
  "bundle_before",
  "bundle_after",
  "retirement_after",
  "refresh_return",
];
const ALLOCATOR_COMPLETION_FIELDS = [
  "allocator_started_us",
  "allocator_completed_us",
  "allocator_duration_us",
  "allocator_outcome",
  "allocator_unavailable_reason",
  "allocator",
];
const ALLOCATOR_FIELDS = [
  "arena_bytes",
  "uordblks_bytes",
  "fordblks_bytes",
  "hblkhd_bytes",
  "hblks",
  "fsmblks_bytes",
];
const HEAP_FIELDS = [
  "used_heap_size",
  "total_heap_size",
  "total_physical_size",
  "external_memory",
  "malloced_memory",
  "peak_malloced_memory",
  "total_allocated_bytes",
  "total_global_handles_size",
  "used_global_handles_size",
  "number_of_native_contexts",
  "number_of_detached_contexts",
];

const NATIVE_FAILURES = new Set([
  "snapshot_failed",
  "refresh_failed",
  "host_boot_failed",
  "validation_rejected",
  "join_panicked",
  "join_timeout",
  "join_disconnected",
]);

const REFRESH_PHASES = new Set([
  "refresh_start",
  "refresh_complete",
  "snapshot_start",
  "snapshot_ready",
  "snapshot_failed",
  "bundle_start",
  "bundle_ready",
  "refresh_skipped",
  "refresh_failed",
]);

const HOST_PHASES = new Set([
  "host_create",
  "host_initialized",
  "host_boot",
  "host_boot_failed",
  "validation_ok",
  "validation_rejected",
  "adopted",
  "retire_requested",
  "host_final_heap",
  "host_destroyed",
  "join_completed",
  "join_panicked",
  "join_timeout",
  "join_disconnected",
]);

function checkHost(events, final) {
  const phases = new Map();
  for (const e of events) {
    if (phases.has(e.phase)) return `duplicate ${e.phase}`;
    phases.set(e.phase, e);
  }
  const initialized = phases.get("host_initialized");
  const boot = phases.get("host_boot");
  const bootFailed = phases.get("host_boot_failed");
  const validation = phases.get("validation_ok");
  const adoption = phases.get("adopted");
  const retirement = phases.get("retire_requested");
  const lastHeap = phases.get("host_final_heap");
  const destroyed = phases.get("host_destroyed");
  const create = phases.get("host_create");
  const joins = events.filter((e) => e.phase.startsWith("join_"));
  const join = joins[0];
  for (const [earlier, later] of [
    [create, initialized],
    [initialized, boot],
    [boot, validation],
    [validation, adoption],
    [boot, adoption],
    [adoption, retirement],
    [retirement, lastHeap],
    [lastHeap, destroyed],
    [destroyed, join?.phase === "join_completed" ? join : null],
  ]) {
    if (earlier && later && earlier.occurrence >= later.occurrence) {
      return `${later.phase} precedes ${earlier.phase}`;
    }
  }
  if (
    [boot, lastHeap, destroyed].some(
      (e) =>
        initialized &&
        e &&
        (e.tid !== initialized.tid || e.tid_start_ticks !== initialized.tid_start_ticks),
    )
  ) {
    return "owner thread changed within a host";
  }
  if (joins.length > 1) return "multiple join outcomes";
  if (boot && bootFailed) return "both boot success and boot failure";
  if (adoption && phases.has("validation_rejected")) return "adopted a rejected host";
  if (
    create &&
    events.some((e) => e.refresh_id !== create.refresh_id || e.snapshot_gen !== create.snapshot_gen)
  ) {
    return "host context changed across its lifecycle";
  }
  if (retirement && join && retirement.retired_by_refresh_id !== join.retired_by_refresh_id) {
    return "retirement context changed at join";
  }
  if (!final) return null;
  if (!create) return "missing host creation";
  if (!boot && !bootFailed) return "missing boot outcome";
  if (boot && !initialized) return "boot without owner initialization";
  if (bootFailed && initialized && join?.phase === "join_completed" && !destroyed)
    return "boot failure joined without completed destruction of initialized host";
  if (boot && (!retirement || !join)) return "missing retirement request or join outcome";
  if (boot && join?.phase === "join_completed" && (!lastHeap || !destroyed)) {
    return "successful join without final owner heap snapshot and completed destruction";
  }
  if (boot && !adoption && !phases.has("validation_rejected")) return "missing adoption outcome";
  if (adoption && !validation) return "adoption without a successful validation";
  if (adoption && (!retirement || !join)) return "adopted host missing shutdown records";
  return null;
}

/**
 * Check only occurrence order. Emission can be reordered by producer contention.
 * A pending getter is normal during a periodic check. At termination it is
 * incomplete evidence, even if the server needed forced shutdown.
 */
function inspectAllocator(rows, byRefresh, lifecycle, final, withPostApply, withReclamation) {
  const starts = new Map();
  const completed = new Map();
  const byCallPhase = new Map();
  let lastCheckpoint = 0;
  const unavailable = new Set(["application_write_exclusion", "unsupported_target"]);
  const report = {
    domain: ALLOCATOR_DOMAIN,
    limit: ALLOCATOR_LIMIT,
    started: 0,
    completed: 0,
    unfinished: [],
    unavailable: [],
    endIdle: "unavailable: no end-idle GNU allocator call",
    failure: null,
  };
  const fail = (detail) => ({ ...report, failure: detail });
  for (const e of rows) {
    const call = e.allocator_call;
    const postApply =
      withPostApply &&
      (e.allocator_phase === POST_APPLY ||
        (withReclamation && e.allocator_phase === "reclamation_after"));
    if (
      e.metric_domain !== ALLOCATOR_DOMAIN ||
      (!postApply && !ALLOCATOR_PHASES.includes(e.allocator_phase)) ||
      (postApply
        ? !positive(e.apply_id) ||
          e.apply_outcome !== "ok" ||
          e.refresh_outcome !== "refreshed" ||
          e.allocator_boundary_outcome !== "returned"
        : APPLY_FIELDS.some((field) => Object.hasOwn(e, field))) ||
      !positive(e.refresh_id) ||
      [e.tick, e.host_id, e.snapshot_gen].some((value) => value !== undefined && !positive(value))
    )
      return fail("allocator metric identity or phase mismatch");
    const startFields = e.phase === ALLOCATOR_START;
    if (startFields && ALLOCATOR_COMPLETION_FIELDS.some((field) => Object.hasOwn(e, field)))
      return fail("allocator start contains completion fields");
    if (e.allocator_outcome === "limit") {
      if (
        call !== ALLOCATOR_LIMIT + 1 ||
        lastCheckpoint !== ALLOCATOR_LIMIT ||
        Object.hasOwn(e, "allocator") ||
        Object.hasOwn(e, "allocator_unavailable_reason") ||
        e.allocator_started_us !== e.occurred_us ||
        e.allocator_completed_us !== e.occurred_us ||
        e.allocator_duration_us !== 0
      )
        return fail("invalid native allocator ceiling record");
      return fail("allocator native invocation ceiling reached");
    }
    if (!positive(call) || call > ALLOCATOR_LIMIT)
      return fail("invalid native allocator invocation number");
    if (e.phase === ALLOCATOR_START || e.allocator_outcome === "unavailable") {
      if (call !== lastCheckpoint + 1) return fail("missing or repeated allocator checkpoint");
      lastCheckpoint = call;
      if (byCallPhase.has(`${e.refresh_id}/${e.allocator_phase}`))
        return fail("duplicate allocator phase within refresh");
      byCallPhase.set(`${e.refresh_id}/${e.allocator_phase}`, e);
      if (e.allocator_outcome === "unavailable") {
        if (
          !unavailable.has(e.allocator_unavailable_reason) ||
          Object.hasOwn(e, "allocator") ||
          e.allocator_started_us !== e.occurred_us ||
          e.allocator_completed_us !== e.occurred_us ||
          e.allocator_duration_us !== 0
        )
          return fail("invalid unavailable allocator checkpoint");
        report.unavailable.push({
          call,
          refreshId: e.refresh_id,
          phase: e.allocator_phase,
          reason: e.allocator_unavailable_reason,
        });
        continue;
      }
      starts.set(call, e);
      report.started += 1;
      continue;
    }
    const start = starts.get(call);
    if (
      !start ||
      completed.has(call) ||
      start.refresh_id !== e.refresh_id ||
      start.allocator_phase !== e.allocator_phase ||
      start.snapshot_gen !== e.snapshot_gen ||
      start.host_id !== e.host_id ||
      start.tick !== e.tick ||
      start.allocator_boundary_outcome !== e.allocator_boundary_outcome ||
      APPLY_FIELDS.some((field) => start[field] !== e[field])
    )
      return fail("allocator completion lacks its matching start");
    if (
      !counter(e.allocator_started_us) ||
      e.allocator_started_us < start.occurred_us ||
      !counter(e.allocator_completed_us) ||
      e.allocator_completed_us !== e.occurred_us ||
      !counter(e.allocator_duration_us) ||
      e.allocator_completed_us - e.allocator_started_us !== e.allocator_duration_us
    )
      return fail("invalid allocator getter clock or duration");
    completed.set(call, e);
    report.completed += 1;
    if (
      e.allocator_outcome !== "ok" ||
      Object.hasOwn(e, "allocator_unavailable_reason") ||
      Object.keys(e.allocator ?? {}).length !== ALLOCATOR_FIELDS.length ||
      !ALLOCATOR_FIELDS.every((field) => counter(e.allocator?.[field]))
    )
      return fail("invalid allocator completion or counters");
  }
  report.unfinished = [...starts.entries()]
    .filter(([call]) => !completed.has(call))
    .map(([call, row]) => ({
      call,
      refreshId: row.refresh_id,
      phase: row.allocator_phase,
      startedUs: row.occurred_us,
    }));
  const joins = new Map(
    lifecycle
      .filter((row) => row.phase.startsWith("join_") && row.retired_by_refresh_id)
      .map((row) => [row.retired_by_refresh_id, row]),
  );
  const hostCreates = new Map();
  for (const row of lifecycle) {
    if (row.phase !== "host_create") continue;
    if (!hostCreates.has(row.refresh_id)) hostCreates.set(row.refresh_id, []);
    hostCreates.get(row.refresh_id).push(row);
  }
  for (const row of byCallPhase.values()) {
    const refresh = byRefresh.get(row.refresh_id);
    const context = refresh?.find((candidate) => candidate.phase === "refresh_start");
    if (!context) return fail("allocator checkpoint attached to unknown refresh");
    const host = hostCreates
      .get(row.refresh_id)
      ?.findLast((candidate) => candidate.occurrence < row.occurrence);
    const snapshot = refresh.findLast(
      (candidate) =>
        candidate.occurrence < row.occurrence &&
        ["snapshot_ready", "snapshot_failed"].includes(candidate.phase),
    );
    if (
      row.tick !== context.tick ||
      row.host_id !== host?.host_id ||
      row.snapshot_gen !== snapshot?.snapshot_gen
    )
      return fail(
        `allocator checkpoint has foreign tick, host or snapshot for refresh ${row.refresh_id}`,
      );
  }
  const unexpectedUnavailable = report.unavailable.find(
    (row) =>
      !withPostApply ||
      [POST_APPLY, "reclamation_after"].includes(row.phase) ||
      row.reason !== "application_write_exclusion",
  );
  if (unexpectedUnavailable)
    return fail(`GNU allocator getter unavailable: ${unexpectedUnavailable.reason}`);
  if (final && report.unfinished.length)
    return fail("allocator getter started but did not complete");
  if (final && (!byRefresh.size || !report.started))
    return fail("mandatory allocator refresh observations missing");
  for (const [id, refresh] of byRefresh) {
    const phase = (name) => refresh.find((e) => e.phase === name);
    const started = phase("refresh_start");
    const finished =
      phase("refresh_complete") ?? phase("refresh_skipped") ?? phase("refresh_failed");
    // RefreshComplete/Skipped/Failed is still an inner-function boundary. A
    // periodic observation can validate the preceding phases once this marker
    // is in a contiguous occurrence prefix. The post-return getter can still
    // be legitimately absent or in flight at that point.
    if (!finished && !final) continue;
    const bundle = phase("bundle_start");
    const snapshot = phase("snapshot_start");
    const snapshotEnd = phase("snapshot_ready") ?? phase("snapshot_failed");
    const join = joins.get(id);
    if (!started || typeof started.boot !== "boolean" || !finished || (snapshot && !snapshotEnd))
      return fail(`refresh ${id} lacks boot or finish context for allocator phases`);
    const returned = byCallPhase.get(`${id}/refresh_return`);
    const resolved = (start) =>
      completed.get(start.allocator_call) ??
      (start.allocator_outcome === "unavailable" ? start : null);
    const returnComplete = returned && resolved(returned);
    const expectReturn = !started.boot && (final || returnComplete);
    const expected = [
      ["entry", started, snapshot ?? finished],
      ...(snapshot
        ? [
            ["snapshot_before", snapshot, snapshotEnd],
            [
              "snapshot_after",
              snapshotEnd,
              bundle ?? finished,
              snapshotEnd?.phase === "snapshot_failed" ? "rejected" : "ok",
            ],
          ]
        : []),
      ...(bundle
        ? [
            ["bundle_before", bundle, phase("bundle_ready") ?? finished],
            [
              "bundle_after",
              null,
              phase("bundle_ready") ?? finished,
              phase("bundle_ready") ? "ok" : "failed",
            ],
          ]
        : []),
      ...(join ? [["retirement_after", join, finished, ["returned", "failed"]]] : []),
      ...(expectReturn
        ? [
            [
              "refresh_return",
              finished,
              null,
              {
                refresh_complete: "refreshed",
                refresh_failed: "failed",
                refresh_skipped: "skipped",
              }[finished.phase],
            ],
          ]
        : []),
    ];
    const got = ALLOCATOR_PHASES.filter(
      (name) => name !== "refresh_return" || expectReturn || started.boot,
    )
      .map((name) => byCallPhase.get(`${id}/${name}`))
      .filter(Boolean);
    if (got.length !== expected.length)
      return fail(`allocator schedule incomplete for refresh ${id}`);
    let previous = started.occurrence;
    for (const [index, [name, after, before, outcome]] of expected.entries()) {
      const start = got[index];
      const finish = resolved(start);
      if (
        !finish ||
        start.allocator_phase !== name ||
        start.occurrence <= previous ||
        (after && start.occurrence <= after.occurrence) ||
        (before && finish.occurrence >= before.occurrence) ||
        ![outcome].flat().includes(finish.allocator_boundary_outcome)
      )
        return fail(`invalid allocator ${name} boundary in refresh ${id}`);
      previous = finish.occurrence;
    }
  }
  return {
    ...report,
    ...(withPostApply && {
      postApplyCheckpoints: [...byCallPhase.values()]
        .filter((row) => row.allocator_phase === POST_APPLY)
        .map((start) => ({ start, completion: completed.get(start.allocator_call) ?? null })),
    }),
  };
}

const sameRefresh = (a, b) =>
  ["refresh_id", "tick", "snapshot_gen", "host_id"].every((field) => a[field] === b[field]);

// Both sides of the anchor are floored to microseconds. Take the wider
// epoch interval, then allow for the timestamp's own rounding (intake logs
// floor to ms; lifecycle occurrences floor to us). Stderr arrival is unused.
function nativeIntervalNs(anchor, offsetUs, precisionUs) {
  const sampledUs = BigInt(anchor.clock_monotonic_us);
  return [
    (sampledUs - BigInt(anchor.anchor_after_us) - 1n + offsetUs) * 1000n,
    (sampledUs + 1n - BigInt(anchor.anchor_before_us) + offsetUs + precisionUs) * 1000n,
  ];
}

const intakeIntervalNs = (intake, anchor) =>
  nativeIntervalNs(anchor, BigInt(intake.ms) * 1000n, 1000n);

const refreshEnd = {
  refreshed: "refresh_complete",
  skipped: "refresh_skipped",
  failed: "refresh_failed",
};

function inspectPostApply({
  anchor,
  applications,
  allocatorRows,
  checkpoints,
  byRefresh,
  intakes,
  mutations,
  final,
  requireComplete,
  requireAttributed,
  withReclamation,
}) {
  const endpoints = [];
  const report = { endpoints, failure: null };
  const fail = (failure) => ({ ...report, failure });
  const byId = new Map();
  const associatedRefresh = new Map();
  const returns = new Map(
    allocatorRows
      .filter((row) => row.allocator_phase === "refresh_return")
      .map((row) => [row.refresh_id, row]),
  );
  const lockedRefreshes = new Set(
    allocatorRows
      .filter((row) => row.allocator_unavailable_reason === "application_write_exclusion")
      .map((row) => row.refresh_id),
  );
  for (const row of applications) {
    if (
      !positive(row.apply_id) ||
      byId.has(row.apply_id) ||
      !["ok", "error"].includes(row.apply_outcome) ||
      (row.tick !== undefined && !positive(row.tick))
    )
      return fail("invalid or duplicate post-apply return identity");
    byId.set(row.apply_id, row);
    if (row.apply_association === "no_refresh") {
      if (
        row.refresh_id !== 0 ||
        ["snapshot_gen", "host_id", "refresh_outcome"].some((key) => Object.hasOwn(row, key))
      )
        return fail("no-refresh apply carries a refresh token");
      continue;
    }
    if (row.apply_association !== "associated")
      return fail(`invalid post-apply association: ${row.apply_association}`);
    const refresh = byRefresh.get(row.refresh_id);
    const finished = refresh?.find(
      (e) => e.phase.startsWith("refresh_") && e.phase !== "refresh_start",
    );
    const returned = returns.get(row.refresh_id);
    if (
      !positive(row.refresh_id) ||
      associatedRefresh.has(row.refresh_id) ||
      !finished ||
      !returned ||
      returned.allocator_unavailable_reason !== "application_write_exclusion" ||
      !sameRefresh(row, returned) ||
      row.refresh_outcome !== returned.allocator_boundary_outcome ||
      refreshEnd[row.refresh_outcome] !== finished.phase ||
      finished.occurrence >= returned.occurrence ||
      returned.occurrence >= row.occurrence ||
      returned.occurred_us > row.occurred_us
    )
      return fail("post-apply token does not match its inner refresh return");
    if (row.refresh_outcome === "refreshed") {
      const adopted = refresh.filter((event) => event.phase === "adopted");
      if (
        adopted.length !== 1 ||
        !positive(row.host_id) ||
        !positive(row.snapshot_gen) ||
        !sameRefresh(row, adopted[0]) ||
        adopted[0].occurrence >= returned.occurrence
      )
        return fail("post-apply token has no matching native host adoption");
    }
    associatedRefresh.set(row.refresh_id, row);
  }
  for (const id of lockedRefreshes) {
    if (final && !associatedRefresh.has(id))
      return fail("locked refresh is missing its apply return");
    if (
      associatedRefresh.has(id) &&
      allocatorRows.some(
        (row) =>
          row.refresh_id === id &&
          row.allocator_phase !== POST_APPLY &&
          !(withReclamation && row.allocator_phase === "reclamation_after") &&
          row.allocator_unavailable_reason !== "application_write_exclusion",
      )
    )
      return fail("associated refresh collected an inner allocator checkpoint under exclusion");
  }
  const getters = new Map();
  for (const { start, completion } of checkpoints) {
    const row = byId.get(start.apply_id);
    if (
      !row ||
      getters.has(start.apply_id) ||
      row.apply_association !== "associated" ||
      row.apply_outcome !== "ok" ||
      row.refresh_outcome !== "refreshed" ||
      !sameRefresh(start, row) ||
      start.occurrence <= row.occurrence ||
      start.occurred_us < row.occurred_us
    )
      return fail("post-apply getter is missing its matching successful apply return");
    getters.set(start.apply_id, { start, completion });
  }
  if (
    final &&
    applications.some(
      (row) =>
        row.apply_outcome === "ok" &&
        row.refresh_outcome === "refreshed" &&
        !getters.get(row.apply_id)?.completion,
    )
  )
    return fail("successful post-apply return is missing its completed getter");

  const usedApplies = new Set();
  const usedRefreshes = new Set();
  for (const mutation of mutations) {
    if (
      !anchor ||
      !["edit", "restoration"].includes(mutation.kind) ||
      !Array.isArray(mutation.paths) ||
      !mutation.paths.length ||
      !/^[0-9]+$/.test(mutation.t0Ns) ||
      !/^[0-9]+$/.test(mutation.witnessNs)
    )
      return fail("invalid post-apply mutation witness");
    const from = BigInt(mutation.t0Ns);
    const until = BigInt(mutation.witnessNs);
    const previous = endpoints.at(-1);
    if (
      from >= until ||
      (previous &&
        (from <= BigInt(previous.mutation.witnessNs) ||
          from <= nativeIntervalNs(anchor, BigInt(previous.getterCompletedUs), 1n)[1]))
    )
      return fail("post-apply mutation windows overlap");
    const related = intakes.filter((intake) => {
      if (!Array.isArray(intake.paths) || !counter(intake.ms)) return false;
      if (!intake.paths.some((path) => mutation.paths.includes(path?.path))) return false;
      const [first, last] = intakeIntervalNs(intake, anchor);
      return last >= from && first <= until;
    });
    const intake = related[0];
    if (!intake) {
      if (final) return fail("post-apply mutation is missing its native intake");
      break;
    }
    const [first, last] = intakeIntervalNs(intake, anchor);
    if (
      related.length !== 1 ||
      !positive(intake.tick) ||
      intake.paths.length !== 1 ||
      intake.paths_truncated ||
      intake.paths[0].kind !== "Modified" ||
      intakes.filter((event) => event.tick === intake.tick).length !== 1 ||
      first < from ||
      last > until
    )
      return fail("post-apply mutation has an ambiguous or coalesced native intake");
    const matching = applications.filter((row) => row.tick === intake.tick);
    if (!matching.length && !final) break;
    const row = matching[0];
    const refreshed = row && byRefresh.get(row.refresh_id)?.[0];
    if (
      matching.length !== 1 ||
      row.apply_outcome !== "ok" ||
      row.refresh_outcome !== "refreshed" ||
      !refreshed ||
      refreshed.seq <= intake.seq ||
      // A real tick can enter the refresh in the same native millisecond.
      // Reject a refresh timestamp before even the floor of intake's clock.
      refreshed.occurred_us < intake.ms * 1000 ||
      refreshed.tick !== intake.tick ||
      usedApplies.has(row.apply_id) ||
      usedRefreshes.has(row.refresh_id)
    )
      return fail("post-apply mutation lacks a unique successful apply for its intake");
    const { start, completion } = getters.get(row.apply_id) ?? {};
    if (!completion) {
      if (final) return fail("post-apply mutation lacks a completed getter");
      break;
    }
    usedApplies.add(row.apply_id);
    usedRefreshes.add(row.refresh_id);
    endpoints.push({
      mutation,
      intake: {
        seq: intake.seq,
        tick: intake.tick,
        path: intake.paths[0].path,
        kind: intake.paths[0].kind,
        earliestNs: String(first),
        latestNs: String(last),
      },
      applyId: row.apply_id,
      applyOutcome: row.apply_outcome,
      refreshOutcome: row.refresh_outcome,
      refreshId: row.refresh_id,
      refreshStartedUs: refreshed.occurred_us,
      snapshotGen: row.snapshot_gen,
      hostId: row.host_id,
      applyReturnedUs: row.occurred_us,
      applyOccurrence: row.occurrence,
      getterOccurrence: { start: start.occurrence, completion: completion.occurrence },
      getterStartedUs: completion.allocator_started_us,
      getterCompletedUs: completion.allocator_completed_us,
      getterDurationUs: completion.allocator_duration_us,
      allocatorCall: completion.allocator_call,
      counters: completion.allocator,
    });
  }
  const edits = endpoints.filter((row) => row.mutation.kind === "edit");
  const restoration = endpoints.find((row) => row.mutation.kind === "restoration") ?? null;
  const transitions = edits.slice(1).map((to, index) => {
    const from = edits[index];
    return {
      fromApply: from.applyId,
      toApply: to.applyId,
      delta: Object.fromEntries(
        ALLOCATOR_FIELDS.map((field) => [field, to.counters[field] - from.counters[field]]),
      ),
    };
  });
  const unassociated = [...byRefresh.entries()].filter(([id]) => !usedRefreshes.has(id));
  const onlyStartup =
    unassociated.length === 1 &&
    unassociated[0][1][0]?.tick === undefined &&
    unassociated[0][1].filter((row) => row.phase === "adopted").length === 1;
  const complete =
    edits.length === LIMITS.edits &&
    edits.every((row, index) => row.mutation.edit === index + 1) &&
    endpoints.length === LIMITS.edits + 1 &&
    restoration === endpoints.at(-1) &&
    endpoints.every((row) => row.intake.path === endpoints[0].intake.path) &&
    onlyStartup;
  if (
    (requireComplete || requireAttributed) &&
    (usedApplies.size !== getters.size ||
      unassociated.length > 1 ||
      unassociated.some(([, rows]) => rows[0]?.tick !== undefined))
  )
    return fail("unrelated post-apply observation or refresh");
  if (requireComplete && !complete)
    return fail("post-apply run requires twelve body endpoints and one restoration");
  return { ...report, complete, transitions, restoration };
}

/**
 * Validate the recorded stream independent of controller edit labels. Events can reach
 * stderr in a different order than they occurred. Only a terminal watermark can close gaps.
 */
export function inspectRendererLifecycle(
  server,
  {
    final = false,
    requireStart = false,
    minimumAdoptions = 0,
    withAllocator = false,
    withPostApply = false,
    reclamationArm = null,
    restorationWitness = false,
    mutations = [],
    requireCompletePostApply = false,
    requireAttributedPostApply = false,
  } = {},
) {
  const { lines, droppedBeforeMark } = server.provSince(0);
  const parsed = parseProvLines(lines);
  const events = parsed.events.filter((e) => e.ev === LIFECYCLE);
  const starts = parsed.events.filter((e) => e.ev === START);
  const start = starts[0];
  const terminal = parsed.events.filter((e) => e.ev === TERMINAL);
  const loss = parsed.events.filter((e) => e.ev === LOSS);
  const last = terminal.at(-1);
  const result = {
    failure: null,
    start: start ?? null,
    complete: false,
    eventCount: events.length,
    adoptions: events.filter((e) => e.phase === "adopted").length,
    terminal: last ?? null,
    nativeFailures: [],
    ...(reclamationArm && { reclamation: reclamationDisposition(events, reclamationArm) }),
  };
  const rawReclamation = reclamationArm ? reclamationDisposition(events, reclamationArm) : null;
  const fail = (detail) => ({
    ...result,
    failure: { reason: "renderer-telemetry", detail },
    ...(rawReclamation && {
      reclamation: {
        ...result.reclamation,
        ...reclamationDisposition(events, reclamationArm, { uncertain: true }),
        failure: result.reclamation?.failure ?? detail,
      },
    }),
  });
  if (
    droppedBeforeMark ||
    parsed.parseErrors.length ||
    parsed.seqViolations.length ||
    parsed.seqGaps.length
  ) {
    return fail("missing, out-of-order or malformed provenance lines");
  }
  if (parsed.events.length && parsed.events[0].seq !== 1)
    return fail("missing initial provenance lines");
  if (
    loss.length ||
    (last && ["contention", "overflow", "output_dropped"].some((key) => last[key] !== 0))
  ) {
    return fail("native producer or sink reported telemetry loss");
  }
  if (
    reclamationArm &&
    parsed.events.some((event) => {
      if (![LIFECYCLE, START, TERMINAL, LOSS].includes(event.ev)) return false;
      const text = lines[event._line].text;
      // This bound belongs to the renderer's NonblockingTraceSink. Other
      // provenance producers have their own record shapes and output paths.
      const nativeLine = text.slice(text.indexOf(PROV_PREFIX));
      return Buffer.byteLength(nativeLine, "utf8") + 1 > 1024;
    })
  )
    return fail("reclamation trace line exceeded 1024 bytes");
  if (starts.length > 1) return fail("multiple native startup anchors");
  if ((requireStart || final) && !start) return fail("native diagnostic startup anchor missing");
  if (
    start &&
    (start.schema_version !== 1 ||
      start.buffer_capacity !== 128 ||
      start.drain_capacity !== 128 ||
      start.attempt_limit !== 4096 ||
      start.line_limit_bytes !== 1024 ||
      (withAllocator
        ? start.allocator_metric_domain !== ALLOCATOR_DOMAIN ||
          start.allocator_call_limit !== ALLOCATOR_LIMIT
        : start.allocator_metric_domain !== undefined) ||
      (withPostApply
        ? start.allocator_post_apply !== true
        : start.allocator_post_apply !== undefined) ||
      (reclamationArm
        ? start.allocator_reclamation !== reclamationArm ||
          start.reclamation_target !== RECLAMATION_LIMITS.target ||
          start.reclamation_rollup_bytes !== RECLAMATION_LIMITS.rollupBytes ||
          start.reclamation_read_limit !== RECLAMATION_LIMITS.readAttempts ||
          start.reclamation_bracket_us !== RECLAMATION_LIMITS.bracketUs ||
          start.reclamation_chunk_bytes !== RECLAMATION_LIMITS.chunkBytes
        : start.allocator_reclamation !== undefined) ||
      !positive(start.pid) ||
      (server.handle?.native?.pid != null && start.pid !== server.handle.native.pid) ||
      !positive(start.clock_monotonic_us) ||
      !counter(start.anchor_before_us) ||
      !counter(start.anchor_after_us) ||
      start.anchor_after_us < start.anchor_before_us)
  ) {
    return fail("native startup identity, contract or clock anchor mismatch");
  }
  if (terminal.length > 1) return fail("multiple native terminal watermarks");
  const seen = new Set();
  const byHost = new Map();
  const byRefresh = new Map();
  const allocatorRows = [];
  const applications = [];
  const reclamationRows = [];
  let contiguous = 0;
  for (const e of events.sort((a, b) => a.occurrence - b.occurrence)) {
    if (!positive(e.occurrence) || seen.has(e.occurrence) || !counter(e.occurred_us)) {
      return fail("invalid native occurrence sequence or timestamp");
    }
    seen.add(e.occurrence);
    if (e.occurrence === contiguous + 1) contiguous += 1;
    if (e.phase === ALLOCATOR_START || e.phase === ALLOCATOR_COMPLETE) {
      if (!withAllocator) return fail("unexpected allocator telemetry in renderer-memory.v1");
      allocatorRows.push(e);
      continue;
    }
    if (e.phase === APPLY_RETURN) {
      if (!withPostApply) return fail("unexpected post-apply telemetry");
      applications.push(e);
      continue;
    }
    if (e.phase?.startsWith("reclamation_")) {
      if (!reclamationArm) return fail("unexpected reclamation telemetry");
      reclamationRows.push(e);
      continue;
    }
    const refreshPhase = REFRESH_PHASES.has(e.phase);
    if (!refreshPhase && !HOST_PHASES.has(e.phase)) return fail("unknown lifecycle phase");
    if (refreshPhase || e.phase === "adopted") {
      if (!positive(e.refresh_id)) return fail(`missing refresh identity at ${e.phase}`);
      if (!byRefresh.has(e.refresh_id)) byRefresh.set(e.refresh_id, []);
      byRefresh.get(e.refresh_id).push(e);
    }
    if (NATIVE_FAILURES.has(e.phase)) result.nativeFailures.push(e);
    if (HOST_PHASES.has(e.phase)) {
      if (!positive(e.host_id)) return fail(`missing host identity at ${e.phase}`);
      if (!byHost.has(e.host_id)) byHost.set(e.host_id, []);
      byHost.get(e.host_id).push(e);
    }
    if (
      ["host_initialized", "host_boot", "host_final_heap", "host_destroyed"].includes(e.phase) &&
      (!positive(e.tid) || !positive(e.tid_start_ticks))
    ) {
      return fail(`missing owner TID birth identity at ${e.phase}`);
    }
    if (["host_boot", "host_final_heap"].includes(e.phase)) {
      if (!HEAP_FIELDS.every((key) => counter(e.heap?.[key]))) {
        return fail(`missing supported V8 heap fields at ${e.phase}`);
      }
    }
  }
  if (withAllocator) {
    // Stderr can emit a later occurrence before an earlier producer. A missing
    // event is actionable only inside the proven prefix or at finalization.
    const established = (row) => final || row.occurrence <= contiguous;
    const knownRefreshes = new Map(
      [...byRefresh]
        .map(([id, rows]) => [id, rows.filter(established)])
        .filter(([, rows]) => rows.length),
    );
    const knownAllocator = allocatorRows.filter(established);
    const { postApplyCheckpoints, ...allocator } = inspectAllocator(
      knownAllocator,
      knownRefreshes,
      events.filter(established),
      final,
      withPostApply,
      Boolean(reclamationArm),
    );
    result.allocator = allocator;
    if (withPostApply && !allocator.failure) {
      result.postApply = inspectPostApply({
        anchor: start,
        applications: applications.filter(established),
        allocatorRows: knownAllocator,
        checkpoints: postApplyCheckpoints,
        byRefresh: knownRefreshes,
        intakes: parsed.events.filter((event) => event.ev === "intake"),
        mutations,
        final,
        requireComplete: requireCompletePostApply,
        requireAttributed: requireAttributedPostApply,
        withReclamation: Boolean(reclamationArm),
      });
    }
    if (reclamationArm && !allocator.failure) {
      result.reclamation = inspectReclamation({
        rows: reclamationRows.filter(established),
        allocatorRows: knownAllocator,
        postApply: result.postApply,
        arm: reclamationArm,
        final,
        restorationWitness,
        anchor: start,
      });
      Object.assign(result.reclamation, {
        performed: rawReclamation.performed,
        executionUnknown: rawReclamation.executionUnknown,
        actionReceipts: rawReclamation.actionReceipts,
        terminalReceipts: rawReclamation.terminalReceipts,
      });
    }
    const observerFailure =
      allocator.failure ?? result.postApply?.failure ?? result.reclamation?.failure;
    if (!final && observerFailure) return fail(observerFailure);
  }
  if (final) {
    if (
      !last ||
      !positive(last.attempted) ||
      last.attempted > 4096 ||
      events.some((row) => row.seq >= last.seq) ||
      last.attempted !== seen.size ||
      last.emitted !== seen.size
    ) {
      return fail(
        result.allocator?.failure ??
          result.postApply?.failure ??
          result.reclamation?.failure ??
          "missing native terminal watermark or occurrence records",
      );
    }
    for (let occurrence = 1; occurrence <= last.attempted; occurrence++)
      if (!seen.has(occurrence)) return fail(`missing occurrence ${occurrence}`);
    if (byHost.size === 0 && !result.nativeFailures.length)
      return fail("no observed native renderer host");
    if (result.adoptions < minimumAdoptions)
      return fail("too few distinct native adoptions for the completed schedule");
    for (const [id, refresh] of byRefresh) {
      const started = refresh[0]?.phase === "refresh_start";
      if (!started) return fail(`refresh ${id} missing start`);
      const adoption = refresh.filter((e) => e.phase === "adopted");
      if (adoption.length > 1) return fail(`refresh ${id} adopted more than one host`);
      if (adoption.length) {
        const bundle = refresh.find((e) => e.phase === "bundle_ready");
        const snapshot = refresh.find((e) => e.phase === "snapshot_ready");
        if (
          !bundle ||
          !snapshot ||
          !positive(adoption[0].snapshot_gen) ||
          !positive(adoption[0].worker_gen) ||
          snapshot.snapshot_gen !== adoption[0].snapshot_gen ||
          bundle.snapshot_gen !== adoption[0].snapshot_gen ||
          snapshot.occurrence >= bundle.occurrence ||
          bundle.occurrence >= adoption[0].occurrence
        ) {
          return fail(`refresh ${id} has no matching snapshot/bundle/adoption context`);
        }
      }
    }
  }
  for (const [host, rows] of byHost) {
    const problem = checkHost(rows, final);
    if (problem) return fail(`host ${host}: ${problem}`);
  }
  if (final) {
    const adopted = events.filter((e) => e.phase === "adopted");
    for (let index = 1; index < adopted.length; index++) {
      const next = adopted[index];
      const previous = adopted[index - 1];
      const retirement = byHost.get(previous.host_id).find((e) => e.phase === "retire_requested");
      if (
        retirement?.retired_by_refresh_id !== next.refresh_id ||
        retirement.occurrence <= next.occurrence
      ) {
        return fail(
          `host ${previous.host_id} missing post-publication retirement by refresh ${next.refresh_id}`,
        );
      }
    }
  }
  const observerFailure =
    result.allocator?.failure ?? result.postApply?.failure ?? result.reclamation?.failure;
  if (observerFailure) return { ...fail(observerFailure), lifecycleComplete: final };
  return {
    ...result,
    complete: final,
    failure: result.nativeFailures.length
      ? { reason: "renderer-native-failure", first: result.nativeFailures[0] }
      : null,
  };
}
