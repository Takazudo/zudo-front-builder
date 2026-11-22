// Parser for zfb's dev provenance trace (ZFB_DEV_PROVENANCE_TRACE=1). Line contract:
// `[zfb-prov] {"ev":..,"seq":..,"ms":..,"tick"?:..}` from crates/zfb-build/src/provenance_trace.rs.
import { realpathSync } from "node:fs";
import { basename, dirname, join } from "node:path";

export const PROV_PREFIX = "[zfb-prov] ";
export const PROV_ENV = "ZFB_DEV_PROVENANCE_TRACE";

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

/** Output-file suffixes an eager publish of `url` writes (`/a/` -> `/a/index.html`; `/a` -> also `/a.html`). */
function outputSuffixes(url) {
  if (!url) return [];
  if (url === "/") return ["/index.html"];
  const trimmed = url.replace(/\/$/, "");
  return [`${trimmed}/index.html`, `${trimmed}.html`];
}

/**
 * The causal chain for one attempt, from parsed events only (never HTTP order
 * or controller labels):
 * intake(s) naming a written path -> select for that tick -> generation
 * (snapshot/worker/refresh adopted, or reject retained) -> render_plan and
 * stale -> the first observations event for the adopted generation (after it
 * was adopted) and the edges event of the same drain -> publish of the
 * target with that generation -> the DOM outcome the harness observed.
 * The deciding tick is the LAST intake tick that names a written path.
 *
 * `writePaths`: alias lists, one per written/renamed path (raw and realpath forms).
 * `consumers`: alias list for the target page's source file (may be empty).
 */
export function buildCausalChain(events, { writePaths = [], consumers = [], targetUrl, dom }) {
  const written = new Set(writePaths.flat());
  const consumerSet = new Set(consumers);
  const byEv = (name) => events.filter((e) => e.ev === name);
  const intakes = byEv("intake").filter((e) => (e.paths ?? []).some((p) => written.has(p.path)));
  const ticks = [...new Set(intakes.map((e) => e.tick))];
  const tick = ticks.length ? ticks[ticks.length - 1] : null;
  const links = {};
  links.intake = link(intakes.length > 0, {
    ticks,
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
  links.select = link(!!select, {
    seq: select?.seq ?? null,
    selected: select?.selected ?? null,
    all_pages: select?.all_pages ?? null,
    all_reasons: select?.all_reasons ?? null,
    fan_out_safe: select?.fan_out_safe ?? null,
    noop: select?.noop ?? null,
    changes: (select?.changes ?? []).filter((c) => written.has(c.path)),
  });

  const reject = forTick("reject")[0] ?? null;
  const snapshot = forTick("snapshot")[0] ?? null;
  const worker = forTick("worker")[0] ?? null;
  const refresh = forTick("refresh")[0] ?? null;
  let generation = null;
  let adoptedAtSeq = null;
  let genOutcome = "none";
  if (reject) {
    genOutcome = "retained";
    generation = reject.retained ?? null;
  } else if (refresh?.outcome === "refreshed") {
    genOutcome = "adopted";
    generation = refresh.generation ?? null;
    adoptedAtSeq = refresh.seq;
  } else if (worker) {
    genOutcome = "adopted";
    generation = { worker_gen: worker.worker_gen, snapshot_gen: worker.snapshot_gen };
    adoptedAtSeq = worker.seq;
  } else if (refresh?.outcome === "skipped") {
    genOutcome = "unchanged";
    generation = refresh.generation ?? null;
    adoptedAtSeq = refresh.seq;
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
  const plan = forTick("render_plan")[0] ?? null;
  links.render_plan = link(!!plan, {
    seq: plan?.seq ?? null,
    pages: plan?.pages ?? null,
    targetPlanned: plan ? (plan.pages ?? []).some((p) => consumerSet.has(p)) : null,
    renderer_refreshed: plan?.renderer_refreshed ?? null,
    route_structure_moved: plan?.route_structure_moved ?? null,
  });
  const stale = forTick("stale")[0] ?? null;
  links.stale = link(!!stale, { seq: stale?.seq ?? null, outputs: stale?.outputs ?? null });

  const after = (e) => adoptedAtSeq !== null && e.seq > adoptedAtSeq;
  const obs =
    genOutcome === "adopted" || genOutcome === "unchanged"
      ? (byEv("observations").find((e) => after(e) && sameGen(e.generation, generation)) ?? null)
      : null;
  const targetObs = (obs?.observations ?? []).filter((o) => consumerSet.has(o.consumer));
  links.observations = link(!!obs, {
    seq: obs?.seq ?? null,
    drain: obs?.drain ?? null,
    first_reader: obs?.first_reader ?? null,
    targetConsumerRead: obs
      ? targetObs.some((o) => (o.reads ?? []).some((r) => written.has(r.entry)))
      : null,
    targetConsumerAggregateRead: obs
      ? targetObs.some((o) => (o.reads ?? []).some((r) => r.aggregate === true))
      : null,
  });
  const edges = obs ? (byEv("edges").find((e) => e.drain === obs.drain) ?? null) : null;
  const slots = (edges?.slots ?? []).filter((s) => consumerSet.has(s.consumer));
  links.edges = link(!!edges, {
    seq: edges?.seq ?? null,
    drain: edges?.drain ?? null,
    applied: edges?.applied ?? null,
    targetSlotStatuses: edges ? slots.map((s) => ({ phase: s.phase, status: s.status })) : null,
    groupLinksTargetToEntry: edges
      ? (edges.groups ?? []).some(
          (g) => consumerSet.has(g.consumer) && (g.entries ?? []).some((p) => written.has(p)),
        )
      : null,
    groupEntriesTruncated: edges
      ? (edges.groups ?? []).some((g) => consumerSet.has(g.consumer) && g.entries_truncated > 0)
      : null,
    reason: edges?.reason ?? null,
  });
  const urls = urlForms(targetUrl);
  const suffixes = outputSuffixes(targetUrl);
  const publishes = byEv("publish").filter((e) => {
    if (!(after(e) && sameGen(e.generation, generation))) return false;
    if (e.mode === "lazy") return urls.includes(e.url);
    return (
      consumerSet.has(e.page) &&
      typeof e.output === "string" &&
      suffixes.some((s) => e.output.endsWith(s))
    );
  });
  links.publish = link(publishes.length > 0, {
    events: publishes.map((e) => ({
      seq: e.seq,
      mode: e.mode,
      url: e.url ?? null,
      page: e.page ?? null,
      output: e.output,
      written: e.written,
      provenance_current: e.provenance_current ?? null,
    })),
  });
  links.dom = link(dom?.outcome === "success" || dom?.outcome === "recovery-observed", {
    outcome: dom?.outcome ?? null,
    t1Ns: dom?.t1Ns ?? null,
    latencyMs: dom?.latencyMs ?? null,
  });

  const rejected = genOutcome === "retained";
  const required = rejected
    ? ["intake", "select", "generation"]
    : ["intake", "select", "generation", "render_plan", "observations", "edges", "publish", "dom"];
  const missing = required.filter((k) => links[k].status !== "observed");
  return { tick, rejected, links, required, missing, chainComplete: missing.length === 0 };
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

/** The absolute paths an attempt wrote or renamed (rename from and to). */
export function attemptWritePaths(attempt) {
  const writes = [
    ...(attempt.writes ?? (attempt.write ? [attempt.write] : [])),
    ...(attempt.rejectWrites ?? []),
    ...(attempt.duringWrites ?? []),
    ...(attempt.repairWrites ?? []),
    ...(attempt.recovery ?? []),
  ];
  const paths = new Set();
  for (const w of writes) {
    if (w.path) paths.add(w.path);
    if (w.to) paths.add(w.to);
  }
  return [...paths];
}

/**
 * The provenance block of one diagnostic attempt record: every trace line
 * since `mark` (from the server's dedicated, uncapped trace buffer), parsed,
 * with any loss recorded, and the causal chain.
 */
export function attemptProvenance({ server, mark, attempt, rec, pageFor, traceEnabled }) {
  const since = server.provSince ? server.provSince(mark) : { lines: [], droppedBeforeMark: null };
  const parsed = parseProvLines(since.lines);
  const consumer = pageFor ? pageFor(rec.targetPath ?? attempt.targetPath) : null;
  const chain = buildCausalChain(parsed.events, {
    writePaths: attemptWritePaths(attempt).map(pathAliases),
    consumers: consumer ? pathAliases(consumer) : [],
    targetUrl: rec.targetPath ?? attempt.targetPath,
    dom: rec,
  });
  return {
    traceEnabled,
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
    causalChain: chain,
    chainComplete: chain.chainComplete,
    events: parsed.events,
  };
}
