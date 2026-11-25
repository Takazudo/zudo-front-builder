import { readFile, readdir, readlink, stat } from "node:fs/promises";
import { linuxBirth, parseLinuxStat, sameLinuxBirth } from "./linux-birth.mjs";
import { snapshotOwned } from "./process-owner.mjs";
import { parseProvLines } from "./provenance.mjs";
import {
  RENDERER_MEMORY_ALLOCATOR_DOMAIN as ALLOCATOR_DOMAIN,
  RENDERER_MEMORY_ALLOCATOR_LIMIT as ALLOCATOR_LIMIT,
} from "./renderer-memory-protocol.mjs";

const STATUS_FIELDS = ["VmRSS", "RssAnon", "RssFile", "RssShmem", "VmSwap"];
const PRIVATE_FIELDS = ["Rss", "Pss", "Private_Clean", "Private_Dirty", "Private_Hugetlb", "Swap"];
const TASK_LIMIT = 256;

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

const vanished = (error) => ["ENOENT", "ESRCH"].includes(error.code);
const inode = (info) => ({ device: String(info.dev), inode: String(info.ino) });
const sameInode = (a, b) => a.device === b.device && a.inode === b.inode;

async function observeThreads(root, bootId) {
  const path = `${root}/task`;
  const tids = (await readdir(path)).filter((entry) => /^[1-9]\d*$/.test(entry));
  if (tids.length > TASK_LIMIT) throw new Error(`native task sample exceeds ${TASK_LIMIT} TIDs`);
  const observations = await Promise.all(
    tids.map(async (entry) => {
      const tid = Number(entry);
      let text;
      try {
        text = await readFile(`${path}/${entry}/stat`, "utf8");
      } catch (error) {
        if (vanished(error)) return null;
        throw error;
      }
      const { startTicks, comm } = parseLinuxStat(text, tid);
      return { tid, startTicks, bootId, comm };
    }),
  );
  return {
    listed: tids.length,
    vanished: observations.filter((entry) => entry === null).length,
    rendererHosts: observations
      .filter((entry) => entry?.comm === "zfb-v8-host")
      .sort((a, b) => a.tid - b.tid),
  };
}

async function observeProcess(expected, executable) {
  const root = `/proc/${expected.pid}`;
  const before = linuxBirth(expected.pid);
  const verified = (found) =>
    sameLinuxBirth(expected, found) && expected.pgid === found.pgid && found.state !== "Z";
  if (!verified(before))
    throw new Error(`process birth changed before sample for PID ${expected.pid}`);
  const [exePath, exeInode, status, rollup, threads] = await Promise.all([
    readlink(`${root}/exe`),
    stat(`${root}/exe`, { bigint: true }).then(inode),
    readFile(`${root}/status`, "utf8").then((text) => parseMemoryKiB(text, STATUS_FIELDS)),
    readFile(`${root}/smaps_rollup`, "utf8").then(
      (text) => ({ values: parseMemoryKiB(text, PRIVATE_FIELDS), unavailable: null }),
      (error) => ({ values: null, unavailable: error.code ?? String(error.message) }),
    ),
    observeThreads(root, before.bootId),
  ]);
  if (!sameInode(executable, exeInode)) {
    throw new Error(`unexpected owned executable for PID ${expected.pid}: ${exePath}`);
  }
  const [afterPath, afterInode] = await Promise.all([
    readlink(`${root}/exe`),
    stat(`${root}/exe`, { bigint: true }).then(inode),
  ]);
  if (
    !verified(linuxBirth(expected.pid)) ||
    afterPath !== exePath ||
    !sameInode(exeInode, afterInode)
  ) {
    throw new Error(`process identity changed during sample for PID ${expected.pid}`);
  }
  if (status.VmRSS === null) throw new Error(`VmRSS unavailable for PID ${expected.pid}`);
  return {
    pid: before.pid,
    pgid: before.pgid,
    bootId: before.bootId,
    startTicks: before.startTicks,
    executable: { path: exePath, ...exeInode },
    rssKiB: status.VmRSS,
    statusKiB: status,
    smapsRollupKiB: rollup.values,
    smapsRollupUnavailable: rollup.unavailable,
    threads,
  };
}

/** The ownership snapshot discovers new descendants; birth and exe checks bracket every /proc sample. */
export async function sampleRendererMemory(server, expectedExecutablePath) {
  const at = new Date().toISOString();
  const hrtimeNs = process.hrtime.bigint().toString();
  const owned = server.isAlive().alive ? snapshotOwned(server.handle) : [];
  if (owned.length === 0 && server.isAlive().alive) {
    throw new Error("no birth-verified server process while the handle reports alive");
  }
  const expected = await stat(expectedExecutablePath, { bigint: true }).then(inode);
  const processes = await Promise.all(owned.map((entry) => observeProcess(entry, expected)));
  return {
    at,
    hrtimeNs,
    endHrtimeNs: process.hrtime.bigint().toString(),
    processes,
    summedRssKiB: processes.reduce((sum, p) => sum + p.rssKiB, 0),
  };
}

const LIFECYCLE = "renderer_lifecycle";
const START = "renderer_lifecycle_start";
const TERMINAL = "renderer_lifecycle_terminal";
const LOSS = "renderer_lifecycle_loss";
const positive = (n) => Number.isSafeInteger(n) && n > 0;
const counter = (n) => Number.isSafeInteger(n) && n >= 0;
const ALLOCATOR_START = "allocator_start";
const ALLOCATOR_COMPLETE = "allocator_complete";
const ALLOCATOR_PHASES = [
  "entry",
  "snapshot_before",
  "snapshot_after",
  "bundle_before",
  "bundle_after",
  "retirement_after",
  "refresh_return",
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
function inspectAllocator(rows, byRefresh, lifecycle, final) {
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
    if (
      e.metric_domain !== ALLOCATOR_DOMAIN ||
      !ALLOCATOR_PHASES.includes(e.allocator_phase) ||
      !positive(e.refresh_id)
    )
      return fail("allocator metric identity or phase mismatch");
    if (e.allocator_outcome === "limit" && e.phase === ALLOCATOR_COMPLETE)
      return fail("allocator native invocation ceiling reached");
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
      start.tick !== e.tick ||
      start.allocator_boundary_outcome !== e.allocator_boundary_outcome
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
  if (report.unavailable.length)
    return fail(`GNU allocator getter unavailable: ${report.unavailable[0].reason}`);
  if (!final) return report;
  if (report.unfinished.length) return fail("allocator getter started but did not complete");
  if (!byRefresh.size || !report.started)
    return fail("mandatory allocator refresh observations missing");
  const joins = new Map(
    lifecycle
      .filter((row) => row.phase.startsWith("join_") && row.retired_by_refresh_id)
      .map((row) => [row.retired_by_refresh_id, row]),
  );
  for (const [id, refresh] of byRefresh) {
    const phase = (name) => refresh.find((e) => e.phase === name);
    const started = phase("refresh_start");
    const finished =
      phase("refresh_complete") ?? phase("refresh_skipped") ?? phase("refresh_failed");
    const bundle = phase("bundle_start");
    const snapshot = phase("snapshot_start");
    const snapshotEnd = phase("snapshot_ready") ?? phase("snapshot_failed");
    const join = joins.get(id);
    if (!started || typeof started.boot !== "boolean" || !finished || (snapshot && !snapshotEnd))
      return fail(`refresh ${id} lacks boot or finish context for allocator phases`);
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
      ...(!started.boot
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
    const got = ALLOCATOR_PHASES.map((name) => byCallPhase.get(`${id}/${name}`)).filter(Boolean);
    if (got.length !== expected.length)
      return fail(`allocator schedule incomplete for refresh ${id}`);
    let previous = started.occurrence;
    for (const [index, [name, after, before, outcome]] of expected.entries()) {
      const start = got[index];
      const finish = completed.get(start.allocator_call);
      if (
        start.allocator_phase !== name ||
        start.occurrence <= previous ||
        (snapshotEnd &&
          [
            "snapshot_after",
            "bundle_before",
            "bundle_after",
            "retirement_after",
            "refresh_return",
          ].includes(name) &&
          start.snapshot_gen !== snapshotEnd.snapshot_gen) ||
        (after && start.occurrence <= after.occurrence) ||
        (before && finish.occurrence >= before.occurrence) ||
        (outcome && ![outcome].flat().includes(finish.allocator_boundary_outcome))
      )
        return fail(`invalid allocator ${name} boundary in refresh ${id}`);
      previous = finish.occurrence;
    }
  }
  if ([...starts.values()].some((row) => !byRefresh.has(row.refresh_id)))
    return fail("allocator getter attached to unknown refresh");
  return report;
}

/**
 * Validate the recorded stream independent of controller edit labels. Events can reach
 * stderr in a different order than they occurred. Only a terminal watermark can close gaps.
 */
export function inspectRendererLifecycle(
  server,
  { final = false, requireStart = false, minimumAdoptions = 0, withAllocator = false } = {},
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
  };
  const fail = (detail) => ({ ...result, failure: { reason: "renderer-telemetry", detail } });
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
  for (const e of events.sort((a, b) => a.occurrence - b.occurrence)) {
    if (!positive(e.occurrence) || seen.has(e.occurrence) || !counter(e.occurred_us)) {
      return fail("invalid native occurrence sequence or timestamp");
    }
    seen.add(e.occurrence);
    if (e.phase === ALLOCATOR_START || e.phase === ALLOCATOR_COMPLETE) {
      if (!withAllocator) return fail("unexpected allocator telemetry in renderer-memory.v1");
      allocatorRows.push(e);
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
    result.allocator = inspectAllocator(allocatorRows, byRefresh, events, final);
    if (!final && result.allocator.failure) return fail(result.allocator.failure);
  }
  if (final) {
    if (
      !last ||
      !positive(last.attempted) ||
      last.attempted > 4096 ||
      last.attempted !== seen.size ||
      last.emitted !== seen.size
    ) {
      return fail(
        result.allocator?.failure ?? "missing native terminal watermark or occurrence records",
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
  if (result.allocator?.failure)
    return { ...fail(result.allocator.failure), lifecycleComplete: final };
  return {
    ...result,
    complete: final,
    failure: result.nativeFailures.length
      ? { reason: "renderer-native-failure", first: result.nativeFailures[0] }
      : null,
  };
}
