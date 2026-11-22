// Parser for zfb's dev provenance trace (ZFB_DEV_PROVENANCE_TRACE=1). Line contract:
// `[zfb-prov] {"ev":..,"seq":..,"ms":..,"tick"?:..}` from crates/zfb-build/src/provenance_trace.rs.
import { realpathSync } from "node:fs";
import { basename, dirname, join } from "node:path";

export const PROV_PREFIX = "[zfb-prov] ";
export const PROV_ENV = "ZFB_DEV_PROVENANCE_TRACE";

/**
 * zfb dev's lazy-render switch as the server resolves it (crates/zfb/src/commands/dev.rs
 * resolve_lazy_dev_render): ZFB_LAZY_DEV_RENDER=1|true / 0|false wins, else ZFB_DEV_EAGER=1|true
 * turns it off, else the built-in default (LAZY_DEV_RENDER_DEFAULT = true).
 */
export function resolveLazyDevRender(env) {
  const norm = (v) => (typeof v === "string" ? v.trim().toLowerCase() : null);
  const lazyVar = norm(env?.ZFB_LAZY_DEV_RENDER);
  if (lazyVar === "1" || lazyVar === "true") return { lazy: true, source: "ZFB_LAZY_DEV_RENDER" };
  if (lazyVar === "0" || lazyVar === "false") return { lazy: false, source: "ZFB_LAZY_DEV_RENDER" };
  const eager = norm(env?.ZFB_DEV_EAGER);
  if (eager === "1" || eager === "true") return { lazy: false, source: "ZFB_DEV_EAGER" };
  return { lazy: true, source: "default (LAZY_DEV_RENDER_DEFAULT)" };
}

/** The Rust side's own gate: "1" or "true", case-insensitive, trimmed. */
export function traceEnabledIn(env) {
  const raw = env?.[PROV_ENV];
  if (typeof raw !== "string") return false;
  const t = raw.trim().toLowerCase();
  return t === "1" || t === "true";
}

/**
 * Parse trace lines ({ text, ns? }). A malformed line is kept as a parse
 * error, never dropped silently. `seq` must strictly increase; a violation is
 * recorded. Every list field cut at the source (`x_truncated: n`) is listed.
 */
export function parseProvLines(lines) {
  const events = [];
  const parseErrors = [];
  const seqViolations = [];
  const truncatedLists = [];
  let lastSeq = null;
  lines.forEach((line, index) => {
    const at = line.text.indexOf(PROV_PREFIX);
    if (at < 0) return;
    const body = line.text.slice(at + PROV_PREFIX.length);
    let ev;
    try {
      ev = JSON.parse(body);
    } catch (error) {
      parseErrors.push({ index, text: body.slice(0, 300), error: String(error?.message ?? error) });
      return;
    }
    if (!ev || typeof ev !== "object" || typeof ev.ev !== "string" || typeof ev.seq !== "number") {
      parseErrors.push({ index, text: body.slice(0, 300), error: "missing ev or seq" });
      return;
    }
    if (lastSeq !== null && !(ev.seq > lastSeq))
      seqViolations.push({ index, seq: ev.seq, previous: lastSeq });
    lastSeq = ev.seq;
    collectTruncation(ev, ev.ev, ev.seq, truncatedLists);
    events.push({ ...ev, _line: index, _ns: line.ns === undefined ? null : String(line.ns) });
  });
  return { events, parseErrors, seqViolations, truncatedLists };
}

function collectTruncation(value, ev, seq, out, path = "") {
  if (Array.isArray(value)) {
    value.forEach((v, i) => collectTruncation(v, ev, seq, out, `${path}[${i}]`));
    return;
  }
  if (!value || typeof value !== "object") return;
  for (const [k, v] of Object.entries(value)) {
    if (k.endsWith("_truncated") && typeof v === "number" && v > 0)
      out.push({
        ev,
        seq,
        field: `${path}${path ? "." : ""}${k.slice(0, -"_truncated".length)}`,
        omitted: v,
      });
    else collectTruncation(v, ev, seq, out, `${path}${path ? "." : ""}${k}`);
  }
}

const sameGen = (a, b) =>
  !!a && !!b && a.worker_gen === b.worker_gen && a.snapshot_gen === b.snapshot_gen;

const link = (observed, detail = {}) => ({ status: observed ? "observed" : "missing", ...detail });

/** URL forms a lazy publish may carry for a target path. */
function urlForms(url) {
  if (!url) return [];
  const trimmed = url.length > 1 ? url.replace(/\/$/, "") : url;
  return [...new Set([url, trimmed, url === "/" ? "/" : `${trimmed}/`])];
}

/**
 * Output paths zfb writes for `url`, RELATIVE to the dist root (zfb-build's
 * atomic writer rejects absolute ones): `/` -> `index.html`, `/a/` ->
 * `a/index.html` (and `a.html` for the non-trailing-slash form).
 */
export function outputsFor(url) {
  if (!url) return [];
  if (url === "/") return ["index.html"];
  const trimmed = url.replace(/^\/+/, "").replace(/\/$/, "");
  return [`${trimmed}/index.html`, `${trimmed}.html`];
}

/** Does an emitted output path name one of `expected`? A relative path must equal it; an absolute one must end in /<it>. */
export function outputMatches(output, expected) {
  if (typeof output !== "string") return false;
  const rel = output.replace(/^\.\//, "");
  if (!rel.startsWith("/")) return expected.includes(rel);
  return expected.some((e) => e !== "index.html" && rel.endsWith(`/${e}`));
}

const nsOf = (e) => (e?._ns === null || e?._ns === undefined ? null : BigInt(e._ns));

/**
 * The causal chain for one attempt, from parsed events only (never HTTP order
 * or controller labels):
 * intake(s) naming a written path -> select for that tick -> generation
 * (snapshot/worker/refresh adopted, or reject retained) -> render_plan and
 * stale -> the first drain of the adopted generation (observations) and the
 * edges event of that drain -> publish of the target with that generation ->
 * the DOM outcome the harness observed.
 *
 * The deciding tick is the last intake naming a written path that arrived at
 * or before t1 (when there is a t1). A publish counts only at or before t1.
 * Every link must be `observed` (not `unverified`, `failed` or `missing`) for
 * the events to be complete, and four links must also show evidence about the
 * target itself: select marked it dirty (or all pages), the render plan
 * covered it (lazy: its output went stale), its consumer read the written
 * entry (or the aggregate), and a written graph edge links it to that entry.
 * All events but no target evidence is `chainEventsOnly`, never complete.
 *
 * `writePaths`: alias lists, one per written/renamed path (raw and realpath forms).
 * `consumers`: alias list for the target page's source file (may be empty).
 */
export function buildCausalChain(
  events,
  { writePaths = [], consumers = [], targetUrl, dom, t1Ns = null, lazy = null },
) {
  const written = new Set(writePaths.flat());
  const consumerSet = new Set(consumers);
  const t1 = t1Ns === null || t1Ns === undefined ? null : BigInt(t1Ns);
  const beforeT1 = (e) => t1 === null || nsOf(e) === null || nsOf(e) <= t1;
  const byEv = (name) => events.filter((e) => e.ev === name);
  const intakes = byEv("intake").filter((e) => (e.paths ?? []).some((p) => written.has(p.path)));
  const timedIntakes = intakes.filter(beforeT1);
  const ticks = [...new Set(intakes.map((e) => e.tick))];
  const deciding = timedIntakes.at(-1) ?? null;
  const tick = deciding ? deciding.tick : null;
  const links = {};
  links.intake = link(!!deciding, {
    ticks,
    decidingSeq: deciding?.seq ?? null,
    afterT1: intakes.filter((e) => !beforeT1(e)).map((e) => e.seq),
    events: intakes.map((e) => ({
      seq: e.seq,
      tick: e.tick,
      intake_id: e.intake_id ?? null,
      matched: (e.paths ?? []).filter((p) => written.has(p.path)),
      suppressed: e.suppressed ?? null,
    })),
  });
  const forTick = (name) => byEv(name).filter((e) => tick !== null && e.tick === tick);
  const select = forTick("select")[0] ?? null;
  const targetDirty = select
    ? (select.changes ?? [])
        .filter((c) => written.has(c.path))
        .some(
          (c) =>
            c.dirty === "all" ||
            (Array.isArray(c.dirty) && c.dirty.some((p) => consumerSet.has(p))),
        )
    : null;
  links.select = link(!!select, {
    seq: select?.seq ?? null,
    selected: select?.selected ?? null,
    all_pages: select?.all_pages ?? null,
    all_reasons: select?.all_reasons ?? null,
    fan_out_safe: select?.fan_out_safe ?? null,
    noop: select?.noop ?? null,
    // Per-path fields come from the real plan and pass through unchanged (e.g. a
    // global_removed reason or an external_override marker).
    changes: (select?.changes ?? []).filter((c) => written.has(c.path)),
    externalOverride: (select?.changes ?? []).some(
      (c) => written.has(c.path) && c.external_override === true,
    ),
    // `dirty` is the fold's page list or the string "all"; `class` may be null (swallowed by a
    // global full rebuild).
    targetDirty,
  });

  const reject = forTick("reject")[0] ?? null;
  const snapshot = forTick("snapshot")[0] ?? null;
  const worker = forTick("worker")[0] ?? null;
  const refresh = forTick("refresh")[0] ?? null;
  let generation = null;
  let genOutcome = "none";
  if (reject) {
    genOutcome = "retained";
    generation = reject.retained ?? null;
  } else if (refresh?.outcome === "refreshed") {
    genOutcome = "adopted";
    generation = refresh.generation ?? null;
  } else if (worker) {
    genOutcome = "adopted";
    generation = { worker_gen: worker.worker_gen, snapshot_gen: worker.snapshot_gen };
  } else if (refresh?.outcome === "skipped") {
    genOutcome = "unchanged";
    generation = refresh.generation ?? null;
  }
  links.generation = link(genOutcome !== "none", {
    outcome: genOutcome,
    generation,
    snapshot: snapshot
      ? {
          seq: snapshot.seq,
          snapshot_gen: snapshot.snapshot_gen,
          outcome: snapshot.outcome,
          error: snapshot.error ?? null,
        }
      : null,
    worker: worker ? { seq: worker.seq, worker_gen: worker.worker_gen, phase: worker.phase } : null,
    refresh: refresh ? { seq: refresh.seq, outcome: refresh.outcome } : null,
    reject: reject
      ? { seq: reject.seq, stage: reject.stage, error: reject.error, retained: reject.retained }
      : null,
  });
  const expectedOutputs = outputsFor(targetUrl);
  const plan = forTick("render_plan")[0] ?? null;
  const stale = forTick("stale")[0] ?? null;
  const staleHasTarget = stale
    ? (stale.outputs ?? []).some((o) => outputMatches(o, expectedOutputs))
    : null;
  links.render_plan = link(!!plan, {
    seq: plan?.seq ?? null,
    pages: plan?.pages ?? null,
    targetPlanned: plan ? (plan.pages ?? []).some((p) => consumerSet.has(p)) : null,
    renderer_refreshed: plan?.renderer_refreshed ?? null,
    route_structure_moved: plan?.route_structure_moved ?? null,
  });
  links.stale = link(!!stale, {
    seq: stale?.seq ?? null,
    outputs: stale?.outputs ?? null,
    targetOutputStale: staleHasTarget,
  });

  // Downstream events are tied to this attempt by generation ids, not seq adjacency: a worker event
  // can be printed a few seq after events emitted under the renderer lock. The only order used is
  // the lower bound of the deciding intake (nothing for this tick precedes it).
  const intakeSeq = deciding?.seq ?? null;
  const after = (e) => genOutcome !== "none" && intakeSeq !== null && e.seq > intakeSeq;
  // `first_reader` is the generation's first reader only in a drain flagged
  // first_drain_of_generation: true. Absent (older binary) or null (no current
  // generation) is unverified, never observed; drains flagged false are skipped.
  const sameGenDrains =
    genOutcome === "adopted" || genOutcome === "unchanged"
      ? byEv("observations").filter((e) => after(e) && sameGen(e.generation, generation))
      : [];
  const firstFlagged = sameGenDrains.find((e) => e.first_drain_of_generation === true) ?? null;
  const unflagged = sameGenDrains.find((e) => typeof e.first_drain_of_generation !== "boolean");
  const obs = firstFlagged ?? unflagged ?? null;
  const notFirstDrains = sameGenDrains
    .filter((e) => e.first_drain_of_generation === false)
    .map((e) => e.drain);
  const obsStatus = firstFlagged ? "observed" : unflagged ? "unverified" : "missing";
  const targetObs = (obs?.observations ?? []).filter((o) => consumerSet.has(o.consumer));
  const targetConsumerRead = obs
    ? targetObs.some((o) => (o.reads ?? []).some((r) => written.has(r.entry)))
    : null;
  const targetConsumerAggregateRead = obs
    ? targetObs.some((o) => (o.reads ?? []).some((r) => r.aggregate === true))
    : null;
  links.observations = {
    status: obsStatus,
    reason: firstFlagged
      ? null
      : unflagged
        ? unflagged.first_drain_of_generation === null
          ? "no-current-generation"
          : "first_drain_of_generation absent (older binary)"
        : sameGenDrains.length
          ? genOutcome === "unchanged"
            ? "generation-not-adopted-this-tick"
            : "not-first-drain: every drain of this generation is flagged first_drain_of_generation false"
          : genOutcome === "unchanged"
            ? "generation-not-adopted-this-tick"
            : null,
    notFirstDrains,
    seq: obs?.seq ?? null,
    drain: obs?.drain ?? null,
    first_drain_of_generation: obs ? (obs.first_drain_of_generation ?? null) : null,
    first_reader: obs?.first_reader ?? null,
    targetConsumerRead,
    targetConsumerAggregateRead,
  };
  // Several edges events may share a drain id (applied:true then drain_failed on a
  // poisoned lock): the failure wins.
  const drainEdges = obs ? byEv("edges").filter((e) => e.drain === obs.drain) : [];
  const edges = drainEdges.find((e) => e.applied === false) ?? drainEdges[0] ?? null;
  const slots = (edges?.slots ?? []).filter((s) => consumerSet.has(s.consumer));
  const graphWritten = typeof edges?.graph_written === "boolean" ? edges.graph_written : null;
  const groupNamesEntry = edges
    ? (edges.groups ?? []).some(
        (g) => consumerSet.has(g.consumer) && (g.entries ?? []).some((p) => written.has(p)),
      )
    : null;
  let edgesStatus = "missing";
  let edgesReason = null;
  if (edges) {
    if (edges.applied === false) {
      edgesStatus = "failed";
      edgesReason = edges.reason ?? "applied false";
    } else if (edges.applied !== true) {
      edgesStatus = "unverified";
      edgesReason = "applied absent";
    } else if (graphWritten === null) {
      edgesStatus = "unverified";
      edgesReason = "graph_written absent (older binary)";
    } else if (graphWritten === false) {
      edgesStatus = "not-written";
      edgesReason = "graph_written false: the drain's groups were not written to the graph";
    } else edgesStatus = "observed";
  }
  links.edges = {
    status: edgesStatus,
    reason: edgesReason,
    sameDrainEvents: drainEdges.length,
    seq: edges?.seq ?? null,
    drain: edges?.drain ?? null,
    applied: edges?.applied ?? null,
    graph_written: graphWritten,
    first_drain_of_generation: edges ? (edges.first_drain_of_generation ?? null) : null,
    cleared: edges ? (edges.cleared ?? null) : null,
    error: edges?.error ?? null,
    targetSlotStatuses: edges ? slots.map((s) => ({ phase: s.phase, status: s.status })) : null,
    groupNamesTargetAndEntry: groupNamesEntry,
    groupLinksTargetToEntry: edges ? edgesStatus === "observed" && groupNamesEntry === true : null,
    groupEntriesTruncated: edges
      ? (edges.groups ?? []).some((g) => consumerSet.has(g.consumer) && g.entries_truncated > 0)
      : null,
  };
  const urls = urlForms(targetUrl);
  const candidates = byEv("publish").filter((e) => {
    if (!(after(e) && sameGen(e.generation, generation))) return false;
    // A lazy publish names its page source when known; prefer it over the url.
    if (e.mode === "lazy")
      return typeof e.page === "string" && consumerSet.size
        ? consumerSet.has(e.page) && urls.includes(e.url)
        : urls.includes(e.url);
    // Eager outputs are relative to the dist root.
    return consumerSet.has(e.page) && outputMatches(e.output, expectedOutputs);
  });
  const afterT1Pub = candidates.filter((e) => !beforeT1(e));
  const inTime = candidates.filter(beforeT1);
  // A lazy publish links to its drain only when drain_contains_page is true; a
  // missing flag (older binary) is unverified, false is not linked.
  const linked = (e) => e.mode !== "lazy" || e.drain_contains_page === true;
  const unverifiedPub = inTime.filter(
    (e) => e.mode === "lazy" && typeof e.drain_contains_page !== "boolean",
  );
  const publishes = inTime.filter(linked);
  const pubView = (e) => ({
    seq: e.seq,
    mode: e.mode,
    url: e.url ?? null,
    page: e.page ?? null,
    output: e.output,
    written: e.written,
    drain: e.drain ?? null,
    drain_contains_page: e.drain_contains_page ?? null,
    provenance_current: e.provenance_current ?? null,
  });
  links.publish = {
    status: publishes.length ? "observed" : unverifiedPub.length ? "unverified" : "missing",
    reason: publishes.length
      ? null
      : unverifiedPub.length
        ? "drain_contains_page absent (older binary)"
        : inTime.length
          ? "lazy publish with drain_contains_page false is not linked to the drain"
          : afterT1Pub.length
            ? "the only matching publish came after t1"
            : null,
    events: publishes.map(pubView),
    unlinked: inTime.filter((e) => !linked(e)).map(pubView),
    afterT1: afterT1Pub.map(pubView),
  };
  links.dom = link(dom?.outcome === "success" || dom?.outcome === "recovery-observed", {
    outcome: dom?.outcome ?? null,
    t1Ns: dom?.t1Ns ?? null,
    latencyMs: dom?.latencyMs ?? null,
  });

  const rejected = genOutcome === "retained";
  const required = rejected
    ? ["intake", "select", "generation"]
    : ["intake", "select", "generation", "render_plan", "observations", "edges", "publish", "dom"];
  // Only "observed" counts: unverified, failed, not-written and missing do not.
  const missing = required.filter((k) => links[k].status !== "observed");
  const unverified = required.filter((k) => links[k].status === "unverified");
  // Target evidence. Observations and edges are consumer-level and every article shares
  // [slug].tsx, so the evidence drain is tied to the target's own publish: a lazy publish's
  // drain (drain_contains_page true), or for an eager publish the first drain of this
  // generation after this tick's render_plan. Aggregate reads count only for the written
  // entry's collection.
  const writtenCollections = new Set();
  for (const e of events) {
    for (const o of e.observations ?? [])
      for (const r of o.reads ?? [])
        if (r.entry !== undefined && written.has(r.entry)) writtenCollections.add(r.collection);
    for (const g of e.groups ?? [])
      if ((g.entries ?? []).some((x) => written.has(x))) writtenCollections.add(g.collection);
  }
  const lazyPub = publishes.find((e) => e.mode === "lazy") ?? null;
  const eagerPub = publishes.find((e) => e.mode === "eager") ?? null;
  let evidenceDrain = null;
  let evidenceVia = null;
  if (lazyPub) {
    evidenceDrain =
      sameGenDrains.find(
        (e) => e.drain === lazyPub.drain && lazyPub.drain_contains_page === true,
      ) ?? null;
    evidenceVia = "lazy-publish-drain";
  } else if (eagerPub && plan) {
    const sameTick = sameGenDrains.find((e) => e.tick === plan.tick && e.seq > plan.seq);
    evidenceDrain = sameTick ?? sameGenDrains.find((e) => e.seq > plan.seq) ?? null;
    evidenceVia = sameTick ? "same-tick-drain-after-render-plan" : "first-drain-after-render-plan";
  }
  const evidenceObs = (evidenceDrain?.observations ?? []).filter((o) =>
    consumerSet.has(o.consumer),
  );
  const evEntryRead = evidenceObs.some((o) => (o.reads ?? []).some((r) => written.has(r.entry)));
  const evAggregateRead = evidenceObs.some((o) =>
    (o.reads ?? []).some((r) => r.aggregate === true && writtenCollections.has(r.collection)),
  );
  const evEdgesAll = evidenceDrain
    ? byEv("edges").filter((e) => e.drain === evidenceDrain.drain)
    : [];
  const evEdges = evEdgesAll.find((e) => e.applied === false) ?? evEdgesAll[0] ?? null;
  const evEdgeLink =
    !!evEdges &&
    evEdges.applied === true &&
    evEdges.graph_written === true &&
    (evEdges.groups ?? []).some(
      (g) => consumerSet.has(g.consumer) && (g.entries ?? []).some((x) => written.has(x)),
    );
  links.observations.evidence = {
    via: evidenceVia,
    drain: evidenceDrain?.drain ?? null,
    targetConsumerRead: evidenceDrain ? evEntryRead : null,
    targetConsumerAggregateRead: evidenceDrain ? evAggregateRead : null,
    writtenCollections: [...writtenCollections],
  };
  links.edges.evidence = {
    drain: evEdges?.drain ?? null,
    applied: evEdges?.applied ?? null,
    graph_written: evEdges?.graph_written ?? null,
    linksTargetToEntry: evidenceDrain ? evEdgeLink : null,
  };

  // Lazy ticks render the edited entries' own routes eagerly (publish mode "eager", never in
  // stale.outputs) and mark only the rest stale; `stale` is also emitted, usually empty, in eager
  // mode. So the mode comes from the server's effective env, not from the stale event.
  const inTickTargetPublish = byEv("publish").some(
    (e) =>
      sameGen(e.generation, generation) &&
      e.seq > (intakeSeq ?? Infinity) &&
      beforeT1(e) &&
      // A lazy publish is request-time, not in-tick; an untagged eager publish is the tick's render.
      (e.tick === tick || (e.tick === undefined && e.mode === "eager")) &&
      (e.mode === "lazy" ? urls.includes(e.url) : outputMatches(e.output, expectedOutputs)),
  );
  const lazyRule = lazy !== false;
  links.render_plan.targetOutputStaleOrPublished = staleHasTarget === true || inTickTargetPublish;
  const targetEvidence = rejected
    ? {}
    : {
        select: targetDirty === true || select?.all_pages === true,
        render_plan:
          links.render_plan.targetPlanned === true &&
          (!lazyRule || staleHasTarget === true || inTickTargetPublish),
        observations: evidenceDrain !== null && (evEntryRead || evAggregateRead),
        edges: evEdgeLink,
      };
  const lackingTarget = Object.entries(targetEvidence)
    .filter(([, ok]) => !ok)
    .map(([k]) => k);
  const eventsComplete = missing.length === 0;
  return {
    tick,
    rejected,
    lazy,
    lazyRuleApplied: lazyRule,
    links,
    required,
    missing,
    unverified,
    targetEvidence,
    lackingTarget,
    eventsComplete,
    chainEventsOnly: eventsComplete && lackingTarget.length > 0,
    chainComplete: eventsComplete && lackingTarget.length === 0,
  };
}

/** Raw and realpath forms of a path (a removed file resolves through its dir). */
export function pathAliases(p) {
  const out = new Set([p]);
  try {
    out.add(realpathSync(p));
  } catch {
    try {
      out.add(join(realpathSync(dirname(p)), basename(p)));
    } catch {
      // the directory is gone too: only the raw form
    }
  }
  return [...out];
}

/**
 * The absolute paths an attempt wrote or renamed (rename from and to), split
 * into the timed writes and the expected-negative recovery writes, which get
 * their own chain.
 */
export function attemptWritePaths(attempt, { recovery = false } = {}) {
  const writes = recovery
    ? (attempt.recovery ?? [])
    : [
        ...(attempt.writes ?? (attempt.write ? [attempt.write] : [])),
        ...(attempt.rejectWrites ?? []),
        ...(attempt.duringWrites ?? []),
        ...(attempt.repairWrites ?? []),
      ];
  const paths = new Set();
  for (const w of writes) {
    if (w.path) paths.add(w.path);
    if (w.to) paths.add(w.to);
  }
  return [...paths];
}

/** Every page string the trace emitted that may name the target: exact alias matches and same-basename others. */
function targetPageStrings(events, consumers) {
  const set = new Set(consumers);
  const base = consumers[0] ? basename(consumers[0]) : null;
  const seen = new Set();
  const add = (p) => typeof p === "string" && seen.add(p);
  for (const e of events) {
    for (const p of e.pages ?? []) add(p);
    for (const p of e.selected ?? []) add(p);
    for (const c of e.changes ?? []) if (Array.isArray(c.dirty)) c.dirty.forEach(add);
    for (const o of e.observations ?? []) add(o.consumer);
    for (const s of e.slots ?? []) add(s.consumer);
    for (const g of e.groups ?? []) add(g.consumer);
    add(e.page);
  }
  return {
    matched: [...seen].filter((p) => set.has(p)),
    sameBasenameUnmatched: base ? [...seen].filter((p) => !set.has(p) && basename(p) === base) : [],
  };
}

/**
 * The provenance block of one diagnostic attempt record: every trace line
 * since `mark` (from the server's dedicated, uncapped trace buffer), parsed,
 * with any loss recorded, and the causal chain.
 */
export function attemptProvenance({
  server,
  mark,
  attempt,
  rec,
  pageFor,
  traceEnabled,
  renderMode = null,
}) {
  const since = server.provSince ? server.provSince(mark) : { lines: [], droppedBeforeMark: null };
  const parsed = parseProvLines(since.lines);
  const consumer = pageFor ? pageFor(rec.targetPath ?? attempt.targetPath) : null;
  const consumers = consumer ? pathAliases(consumer) : [];
  const targetUrl = rec.targetPath ?? attempt.targetPath;
  const chain = buildCausalChain(parsed.events, {
    writePaths: attemptWritePaths(attempt).map(pathAliases),
    consumers,
    targetUrl,
    dom: rec,
    t1Ns: rec.t1Ns ?? null,
    lazy: renderMode?.lazy ?? null,
  });
  const recoveryPaths = attemptWritePaths(attempt, { recovery: true });
  const recoveryChain = recoveryPaths.length
    ? buildCausalChain(parsed.events, {
        writePaths: recoveryPaths.map(pathAliases),
        consumers,
        targetUrl,
        dom: null,
        lazy: renderMode?.lazy ?? null,
      })
    : null;
  return {
    traceEnabled,
    // From the server's effective env; null means unknown and the stricter lazy rule applies.
    renderMode,
    linesSeen: since.lines.length,
    traceMissing: traceEnabled === true && since.lines.length === 0,
    droppedBeforeMark: since.droppedBeforeMark ?? null,
    lossNote:
      since.droppedBeforeMark > 0
        ? `${since.droppedBeforeMark} trace lines were dropped from the buffer before this attempt's mark`
        : null,
    parseErrors: parsed.parseErrors,
    seqViolations: parsed.seqViolations,
    truncatedLists: parsed.truncatedLists,
    targetConsumer: consumer,
    // Raw page strings as emitted, so a native run can confirm the absolute PageId form.
    targetPageStrings: targetPageStrings(parsed.events, consumers),
    causalChain: chain,
    chainComplete: chain.chainComplete,
    chainEventsOnly: chain.chainEventsOnly,
    recoveryChain,
    events: parsed.events,
  };
}
