import { setTimeout as delay } from "node:timers/promises";
import { PROTOCOLS, RESOURCE_STUDY_PROTOCOL } from "./schedule.mjs";
import { distribution } from "./stats.mjs";
import { nsToMs } from "./util.mjs";

/**
 * Resource-study sampler and idle phases for one session. Samples owned processes every
 * sampleEveryMs from start() to stop(), each tagged with the phase it was taken in and its own
 * wall cost. A sample is never started while the previous one is still running; such a tick is
 * counted in overlappingSkipped.
 */
export function createResourceStudy({ study, sampleNow, append, clock, sleep = delay }) {
  const constants = PROTOCOLS[RESOURCE_STUDY_PROTOCOL];
  const state = {
    phase: { step: "start", cycle: null },
    timer: null,
    inflight: null,
    overlappingSkipped: 0,
    failure: null,
  };

  const take = () => {
    if (state.inflight) {
      state.overlappingSkipped += 1;
      return state.inflight;
    }
    const phase = { ...state.phase };
    const t = clock.now();
    state.inflight = (async () => {
      let sample;
      let error = null;
      try {
        sample = await sampleNow();
      } catch (e) {
        error = String(e?.message ?? e);
      }
      try {
        append({
          type: "resource-sample",
          label: "unmeasured",
          protocol: study.protocol,
          phase,
          costMs: nsToMs(clock.now() - t),
          sample: sample ?? null,
          error,
        });
      } catch (e) {
        // A sample that cannot be recorded halts the batch before its next edit (haltReason).
        state.failure ??= {
          reason: "resource-sample-record-failed",
          phase,
          error: String(e?.message ?? e),
        };
      }
    })().finally(() => {
      state.inflight = null;
    });
    return state.inflight;
  };

  const idle = async (kind, cycle, ms) => {
    state.phase = { step: kind, cycle };
    const t = clock.now();
    append({ type: "idle", label: "unmeasured", phase: kind, cycle, plannedMs: ms, step: "start" });
    await sleep(ms);
    append({
      type: "idle",
      label: "unmeasured",
      phase: kind,
      cycle,
      plannedMs: ms,
      step: "end",
      actualMs: nsToMs(clock.now() - t),
    });
  };

  return {
    start() {
      append({
        type: "resource-study",
        label: "unmeasured",
        step: "start",
        protocol: study.protocol,
        declared: { ...study },
        constants,
      });
      state.timer = setInterval(take, study.sampleEveryMs);
      return take();
    },
    onRecord(r) {
      if (r.type === "scenario-start")
        state.phase = { step: "edit", cycle: r.cycle?.index ?? null };
    },
    beforeCleanup() {
      state.phase = { step: "restore", cycle: state.phase.cycle };
    },
    haltReason: () => state.failure,
    async afterScenario({ scenario }) {
      if (state.failure)
        throw new Error(`resource study halted: ${state.failure.reason}: ${state.failure.error}`);
      await idle("after-restore", scenario.cycle?.index ?? null, study.idleAfterRestoreMs);
    },
    async finalIdle({ stopped, stopReason }) {
      if (stopped || state.failure) {
        append({
          type: "idle",
          label: "unmeasured",
          phase: "final",
          step: "skipped",
          reason: stopped
            ? `batch stopped: ${stopReason}`
            : `resource study halted: ${state.failure.reason}`,
          plannedMs: study.finalIdleMs,
        });
        return;
      }
      await idle("final", null, study.finalIdleMs);
    },
    async stop() {
      clearInterval(state.timer);
      state.timer = null;
      if (state.inflight) await state.inflight;
      state.phase = { step: "end", cycle: null };
      await take();
      append({
        type: "resource-study",
        label: "unmeasured",
        step: "stop",
        overlappingSkipped: state.overlappingSkipped,
      });
    },
    get failure() {
      return state.failure;
    },
    get overlappingSkipped() {
      return state.overlappingSkipped;
    },
  };
}

const num = (v) => (typeof v === "number" ? v : null);

/** Descriptive summary of a resource-study session's records. Never infers a leak, trend or OOM. */
export function summarizeResourceStudy(records, study) {
  const constants = PROTOCOLS[RESOURCE_STUDY_PROTOCOL];
  const samples = records.filter((r) => r.type === "resource-sample");
  const idles = records.filter((r) => r.type === "idle");
  const ends = idles.filter((r) => r.phase === "after-restore" && r.step === "end");
  const finalEnd = idles.find((r) => r.phase === "final" && r.step === "end") ?? null;
  const finalSkipped = idles.find((r) => r.phase === "final" && r.step === "skipped") ?? null;
  const stopRec = records.find((r) => r.type === "resource-study" && r.step === "stop");
  const lastIn = (pred) => {
    const s = samples.filter(pred).at(-1);
    return s
      ? {
          cycle: s.phase.cycle,
          summedRssKiB: num(s.sample?.summedRssKiB),
          summedThreads: s.sample?.summedThreads ?? "unavailable",
          processes: s.sample?.processes?.length ?? 0,
        }
      : null;
  };
  const unavailable = { rss: 0, threads: 0, sampleErrors: 0 };
  for (const s of samples) {
    if (s.error) unavailable.sampleErrors += 1;
    const ps = s.sample?.processes ?? [];
    if (ps.some((p) => p.present && p.rssKiB === "unavailable")) unavailable.rss += 1;
    if (s.sample && s.sample.summedThreads === "unavailable") unavailable.threads += 1;
  }
  const byPhase = samples.reduce((acc, s) => {
    acc[s.phase.step] = (acc[s.phase.step] ?? 0) + 1;
    return acc;
  }, {});
  const rss = samples.map((s) => num(s.sample?.summedRssKiB)).filter((v) => v !== null);
  const threads = samples.map((s) => num(s.sample?.summedThreads)).filter((v) => v !== null);
  const { values: _c, ...cost } = distribution(samples.map((s) => s.costMs));
  return {
    protocol: study.protocol,
    declared: study,
    constants,
    cyclesPlanned: study.cycles,
    cyclesCompletedWithIdle: ends.length,
    finalIdleCompleted: finalEnd !== null,
    finalIdleSkipped: finalSkipped?.reason ?? null,
    samples: samples.length,
    overlappingSkipped: stopRec?.overlappingSkipped ?? null,
    sampleCostMs: cost,
    unavailable,
    byPhase,
    endOfIdle: ends.map((e) =>
      lastIn((s) => s.phase.step === "after-restore" && s.phase.cycle === e.cycle),
    ),
    endOfFinalIdle: finalEnd ? lastIn((s) => s.phase.step === "final") : null,
    sampledRange: {
      summedRssKiB: rss.length ? { min: Math.min(...rss), max: Math.max(...rss) } : null,
      summedThreads: threads.length
        ? { min: Math.min(...threads), max: Math.max(...threads) }
        : null,
    },
    labels: constants.labels,
    timingNote:
      "attempt latencies in a resource-study session include sampling load and are not timing-protocol measurements",
  };
}
