import { RENDERER_MEMORY_RECLAMATION_LIMITS as LIMITS } from "./renderer-memory-protocol.mjs";

const counter = (value) => Number.isSafeInteger(value) && value >= 0;
const positive = (value) => counter(value) && value > 0;
const IDENTITY = ["apply_id", "refresh_id", "tick", "snapshot_gen", "host_id"];
const FIELDS = {
  Rss: "rss_kib",
  Anonymous: "anonymous_kib",
  Private_Clean: "private_clean_kib",
  Private_Dirty: "private_dirty_kib",
  Swap: "swap_kib",
  AnonHugePages: "anon_huge_pages_kib",
};
const sameIdentity = (a, b) => IDENTITY.every((field) => a[field] === b[field]);
const interval = (record) =>
  counter(record.started_us) &&
  counter(record.completed_us) &&
  counter(record.duration_us) &&
  record.completed_us - record.started_us === record.duration_us;

/** Reparse the exact bounded native bytes independently of the numeric native summary. */
export function parseReclamationRollup(bytes) {
  if (!Buffer.isBuffer(bytes) || bytes.length === 0 || bytes.length > LIMITS.rollupBytes)
    throw new Error("invalid reclamation rollup byte length");
  const text = bytes.toString("utf8");
  if (!Buffer.from(text).equals(bytes) || !text.endsWith("\n"))
    throw new Error("malformed or truncated reclamation rollup");
  const [header, ...lines] = text.slice(0, -1).split("\n");
  const range = /^([0-9a-fA-F]+)-([0-9a-fA-F]+)\s+---p\s+00000000\s+00:00\s+0\s+\[rollup\]$/.exec(
    header,
  );
  if (
    !range ||
    BigInt(`0x${range[1]}`) >= BigInt(`0x${range[2]}`) ||
    BigInt(`0x${range[2]}`) > 0xffffffffffffffffn
  )
    throw new Error("invalid reclamation rollup header");
  const seen = new Set();
  const values = {};
  for (const line of lines) {
    const match = /^([A-Za-z0-9_]+):\s+(\d+)\s+kB\s*$/.exec(line);
    if (!match || seen.size >= 64 || seen.has(match[1]) || !counter(Number(match[2])))
      throw new Error("malformed, duplicate or overflowed reclamation rollup field");
    seen.add(match[1]);
    if (Object.hasOwn(FIELDS, match[1])) values[FIELDS[match[1]]] = Number(match[2]);
  }
  if (Object.keys(values).length !== Object.keys(FIELDS).length)
    throw new Error("missing mandatory reclamation rollup field");
  return { text, values };
}

/** Retain performed evidence even when identity, telemetry or later validation fails. */
export function reclamationDisposition(events, arm) {
  const completions = events.filter((event) => event.phase === "reclamation_action_complete");
  const terminals = events.filter((event) => event.phase === "reclamation_complete");
  const starts = events.filter((event) => event.phase === "reclamation_action_start");
  const performed = [...completions, ...terminals].some(
    (event) => event.reclamation?.performed === true,
  )
    ? true
    : arm === "trim" && starts.length && !completions.length && !terminals.length
      ? null
      : false;
  return {
    arm,
    complete: false,
    targetValid: false,
    performed,
    executionUnknown: performed === null,
    actionReceipts: completions,
    terminalReceipts: terminals,
    failure: null,
  };
}

export function inspectReclamation({
  rows,
  allocatorRows,
  postApply,
  arm,
  final,
  anchor,
  restorationWitness = false,
}) {
  const report = reclamationDisposition(rows, arm);
  const fail = (detail) => ({ ...report, failure: detail });
  const allowed = new Set([
    "reclamation_rollup_start",
    "reclamation_rollup_complete",
    "reclamation_rollup_chunk",
    "reclamation_action_start",
    "reclamation_action_complete",
    "reclamation_complete",
    "reclamation_disarmed",
  ]);
  for (const row of rows) {
    if (!allowed.has(row.phase) || !row.reclamation || typeof row.reclamation !== "object")
      return fail("unknown or malformed reclamation record");
    if (row.phase === "reclamation_disarmed")
      return fail(`native reclamation permanently disarmed: ${row.reclamation.reason}`);
    if (!IDENTITY.every((field) => positive(row[field])))
      return fail("missing reclamation action identity");
  }
  const selected = rows[0];
  if (selected && rows.some((row) => !sameIdentity(row, selected)))
    return fail("foreign or duplicate reclamation action identity");
  const one = (phase) => rows.filter((row) => row.phase === phase);
  const actionStarts = one("reclamation_action_start");
  const actions = one("reclamation_action_complete");
  const terminals = one("reclamation_complete");
  if ([actionStarts, actions, terminals].some((group) => group.length > 1))
    return fail("duplicate one-shot reclamation action record");
  const actionStart = actionStarts[0];
  const action = actions[0];
  const terminal = terminals[0];
  for (const row of [actionStart, action, terminal].filter(Boolean))
    if (row.reclamation.arm !== arm || row.reclamation.eligible !== LIMITS.target)
      return fail("wrong reclamation arm or eligible endpoint");
  if (terminal && terminal.reclamation.outcome !== "ok")
    return fail(`native reclamation observation incomplete: ${terminal.reclamation.outcome}`);
  if (action) {
    const value = action.reclamation;
    if (
      !actionStart ||
      !interval(value) ||
      actionStart.reclamation.started_us !== value.started_us ||
      actionStart.occurrence >= action.occurrence ||
      actionStart.occurred_us > value.started_us ||
      action.occurred_us !== value.completed_us ||
      (arm === "trim"
        ? value.performed !== true || ![0, 1].includes(value.trim_return)
        : value.performed !== false || value.trim_return !== null)
    )
      return fail("invalid reclamation action completion or trim count");
    report.action = { ...value, occurrence: action.occurrence };
  }
  const readings = {};
  for (const side of ["before", "after"]) {
    const sideRows = rows.filter(
      (row) => row.phase.startsWith("reclamation_rollup_") && row.reclamation.side === side,
    );
    const starts = sideRows.filter((row) => row.phase === "reclamation_rollup_start");
    const completes = sideRows.filter((row) => row.phase === "reclamation_rollup_complete");
    if (starts.length > 1 || completes.length > 1) return fail("duplicate reclamation rollup read");
    const start = starts[0];
    const complete = completes[0];
    const chunks = sideRows.filter((row) => row.phase === "reclamation_rollup_chunk");
    if (complete) {
      const read = complete.reclamation;
      if (
        !start ||
        !interval(read) ||
        start.reclamation.started_us !== read.started_us ||
        start.occurred_us > read.started_us ||
        complete.occurred_us !== read.completed_us ||
        start.occurrence >= complete.occurrence ||
        !positive(read.bytes) ||
        read.bytes > LIMITS.rollupBytes ||
        !positive(read.reads) ||
        read.reads > LIMITS.readAttempts ||
        read.eof !== true ||
        read.outcome !== "ok"
      )
        return fail("invalid, unavailable or incomplete reclamation rollup read");
      let length = 0;
      const raw = [];
      for (const chunk of chunks) {
        const value = chunk.reclamation;
        if (
          chunk.occurrence <= complete.occurrence ||
          value.offset !== length ||
          typeof value.hex !== "string" ||
          !/^(?:[0-9a-f]{2})+$/.test(value.hex) ||
          value.hex.length > LIMITS.chunkBytes * 2
        )
          return fail("malformed, duplicate or oversized reclamation rollup chunk");
        const bytes = Buffer.from(value.hex, "hex");
        if (bytes.length !== Math.min(LIMITS.chunkBytes, read.bytes - length))
          return fail("reclamation rollup chunk does not match its fixed-size schedule");
        length += bytes.length;
        if (length > read.bytes || length > LIMITS.rollupBytes)
          return fail("reclamation raw input exceeds its declared bound");
        raw.push(bytes);
      }
      if (length === read.bytes) {
        let parsed;
        try {
          parsed = parseReclamationRollup(Buffer.concat(raw));
        } catch (error) {
          return fail(error.message);
        }
        if (
          Object.keys(read.rollup ?? {}).length !== Object.keys(FIELDS).length ||
          !Object.values(FIELDS).every((field) => read.rollup[field] === parsed.values[field])
        )
          return fail("reclamation rollup numeric summary disagrees with raw bytes");
        readings[side] = {
          ...read,
          raw: parsed.text,
          occurrence: complete.occurrence,
          startOccurrence: start.occurrence,
          lastChunkOccurrence: chunks.at(-1).occurrence,
        };
      } else if (final || terminal) return fail("missing reclamation rollup raw input");
    } else if (chunks.length || (final && start))
      return fail("reclamation rollup read did not complete");
  }
  if (
    rows.some(
      (row) =>
        row.phase.startsWith("reclamation_rollup_") &&
        !["before", "after"].includes(row.reclamation.side),
    )
  )
    return fail("invalid reclamation rollup side");
  report.rollups = readings;
  const postStarts = allocatorRows.filter(
    (row) => row.allocator_phase === "reclamation_after" && row.phase === "allocator_start",
  );
  const postCompletes = allocatorRows.filter(
    (row) => row.allocator_phase === "reclamation_after" && row.phase === "allocator_complete",
  );
  if (postStarts.length > 1 || postCompletes.length > 1)
    return fail("duplicate reclamation post-action GNU getter");
  const postStart = postStarts[0];
  const post = postCompletes[0];
  if (postStart && (!selected || !sameIdentity(postStart, selected)))
    return fail("foreign reclamation post-action GNU getter");
  if (post) report.postAllocator = post;
  const endpoints = postApply?.endpoints ?? [];
  const endpoint = endpoints[LIMITS.target - 1];
  const actedIndex = selected && endpoints.findIndex((row) => row.applyId === selected.apply_id);
  if (actedIndex >= 0 && actedIndex !== LIMITS.target - 1)
    return fail("action occurred at the wrong workload endpoint");
  const completedBracket = terminal && action && readings.before && readings.after && post;
  const identity = endpoint && {
    apply_id: endpoint.applyId,
    refresh_id: endpoint.refreshId,
    tick: endpoint.intake.tick,
    snapshot_gen: endpoint.snapshotGen,
    host_id: endpoint.hostId,
  };
  if (
    endpoint &&
    selected &&
    (!sameIdentity(selected, identity) || endpoint.mutation.kind !== "restoration")
  )
    return fail("acted endpoint does not match the declared restoration");
  if (completedBracket) {
    const end = terminal.reclamation;
    const pre = readings.before;
    const after = readings.after;
    const act = action.reclamation;
    const preAllocator = allocatorRows.find(
      (row) =>
        row.phase === "allocator_complete" &&
        row.allocator_phase === "post_apply_return" &&
        sameIdentity(row, selected),
    );
    if (
      !preAllocator ||
      !interval(end) ||
      end.started_us !== preAllocator.allocator_started_us ||
      end.completed_us < post.allocator_completed_us ||
      end.duration_us > LIMITS.bracketUs ||
      terminal.occurred_us !== end.completed_us ||
      end.performed !== act.performed ||
      preAllocator.occurrence >= pre.startOccurrence ||
      preAllocator.allocator_completed_us > pre.started_us ||
      pre.lastChunkOccurrence >= actionStart.occurrence ||
      pre.completed_us > act.started_us ||
      action.occurrence >= after.startOccurrence ||
      act.completed_us > after.started_us ||
      after.occurrence >= postStart.occurrence ||
      after.completed_us > post.allocator_started_us ||
      Math.max(post.occurrence, after.lastChunkOccurrence) >= terminal.occurrence
    )
      return fail("reclamation bracket order, duration or pre-action evidence mismatch");
    report.preAllocator = preAllocator;
    report.bracket = end;
    report.bracketComplete = true;
    report.identity =
      identity ?? Object.fromEntries(IDENTITY.map((field) => [field, selected[field]]));
    report.associationValid = Boolean(endpoint && postApply.complete);
    const bracketLatestNs =
      anchor &&
      (BigInt(anchor.clock_monotonic_us) +
        2n -
        BigInt(anchor.anchor_before_us) +
        BigInt(end.completed_us)) *
        1000n;
    const witnessAfterBracket =
      endpoint &&
      bracketLatestNs !== undefined &&
      BigInt(endpoint.mutation.witnessNs) > bracketLatestNs;
    if (restorationWitness && endpoint && !witnessAfterBracket)
      return fail("restoration body witness precedes completion of the action bracket");
    report.targetValid =
      report.associationValid && restorationWitness && Boolean(witnessAfterBracket);
    report.complete = report.targetValid;
    report.deltaKiB = Object.fromEntries(
      Object.values(FIELDS).map((field) => [field, after.rollup[field] - pre.rollup[field]]),
    );
    report.allocatorDelta = Object.fromEntries(
      Object.keys(preAllocator.allocator).map((field) => [
        field,
        post.allocator[field] - preAllocator.allocator[field],
      ]),
    );
  }
  if (final && (!completedBracket || !report.targetValid))
    return fail("missing complete reclamation bracket or restoration attribution");
  return report;
}
