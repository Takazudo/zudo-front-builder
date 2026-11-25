import { setTimeout as delay } from "node:timers/promises";
import { pathAliases } from "./provenance.mjs";
import { interruptible } from "./session-deadline.mjs";
import {
  rendererMemoryAllocatorProtocol,
  RENDERER_MEMORY_LIMITS as limits,
  RENDERER_MEMORY_MAPPING_BOUNDARIES,
  RENDERER_MEMORY_MAPPING_LIMITS,
  RENDERER_MEMORY_POST_APPLY_PROTOCOL,
  RENDERER_MEMORY_PROTOCOL,
} from "./renderer-memory-protocol.mjs";
import { hrtimeNs, nsToMs } from "./util.mjs";

const deadlineNs = (ms) => BigInt(ms) * 1_000_000n;

export function createRendererMemoryStudy({
  append,
  signal,
  supervisorStartedMs,
  workDeadline,
  protocol = RENDERER_MEMORY_PROTOCOL,
  sample,
  inspect,
  captureMappings,
  clock = hrtimeNs,
  sleep = delay,
}) {
  const failureSignal = new AbortController();
  const workSignal = signal
    ? AbortSignal.any([signal, failureSignal.signal])
    : failureSignal.signal;
  const workEnd = deadlineNs(workDeadline);
  let phase = "startup";
  let server = null;
  let interval = null;
  let inflight = null;
  let sampleStarted = null;
  let failure = null;
  let evidenceFailure = null;
  let lastInspection = null;
  let idleMs = null;
  let overlapSkipped = 0;
  let samples = 0;
  let peakSummedRssKiB = null;
  let samplingStopped = false;
  const withAllocator = rendererMemoryAllocatorProtocol(protocol);
  const withPostApply = protocol === RENDERER_MEMORY_POST_APPLY_PROTOCOL;
  const mutations = [];
  const mappingResults = [];

  const record = (entry) =>
    append({
      phase,
      ...entry,
      label: "unmeasured",
      protocol,
      hrtimeNs: String(clock()),
    });
  const fail = (details) => {
    if (details.reason === "renderer-telemetry" || details.reason.startsWith("renderer-memory-"))
      evidenceFailure ??= details;
    if (failure) return;
    failure = details;
    failureSignal.abort(new Error(details.reason));
    record({ type: "renderer-memory-failure", failure });
  };
  const checkTrace = (options) => {
    const mark = server.provMark?.();
    if (
      !options.final &&
      !options.requireStart &&
      !options.requireAttributedPostApply &&
      !options.requireCompletePostApply &&
      mark != null &&
      lastInspection?.mark === mark &&
      lastInspection.mutationCount === mutations.length
    )
      return lastInspection.report;
    const report = inspect(server, { ...options, withAllocator, withPostApply, mutations });
    if (!options.requireAttributedPostApply && !options.requireCompletePostApply)
      lastInspection = { mark, mutationCount: mutations.length, report };
    if (report.failure) fail(report.failure);
    return report;
  };
  const take = (boundary = null) => {
    if (inflight) {
      if (boundary) return inflight.then(() => take(boundary));
      overlapSkipped += 1;
      if (nsToMs(clock() - sampleStarted) >= limits.limits.sampleEveryMs)
        fail({
          reason: "renderer-memory-safety-sample-stalled",
          startedHrtimeNs: String(sampleStarted),
        });
      return inflight;
    }
    if (samplingStopped) return Promise.resolve();
    const started = clock();
    sampleStarted = started;
    const phaseAtStart = phase;
    inflight = (async () => {
      let result;
      try {
        result = await sample(server);
      } catch (error) {
        if (!samplingStopped && phase !== "shutdown")
          fail({
            reason: "renderer-memory-sample-invalid",
            error: String(error?.message ?? error),
          });
        return;
      }
      if (samplingStopped) return;
      samples += 1;
      if (typeof result.summedRssKiB === "number")
        peakSummedRssKiB = Math.max(peakSummedRssKiB ?? 0, result.summedRssKiB);
      record({
        type: "renderer-memory-sample",
        phase: phaseAtStart,
        boundary,
        startedHrtimeNs: String(started),
        result,
      });
      if (result.summedRssKiB > limits.limits.rssCeilingKiB)
        fail({
          reason: "resource-limit",
          sample: result,
          rssCeilingKiB: limits.limits.rssCeilingKiB,
        });
      if (phase !== "shutdown") {
        checkTrace({ final: false });
        const alive = server.isAlive();
        if (!alive.alive) fail({ reason: "server-exit", exit: alive.exit });
      }
    })().finally(() => {
      inflight = null;
    });
    return inflight;
  };
  const takePeriodic = () => {
    Promise.resolve()
      .then(() => take())
      .catch((error) => {
        try {
          fail({ reason: "renderer-memory-record-failed", error: String(error?.message ?? error) });
        } catch {
          // The abort precedes the record write, including when the evidence sink fails.
        }
      });
  };
  const enter = (next) => {
    phase = next;
    record({ type: "renderer-memory-phase", step: "start" });
  };
  const workRemainingMs = () => Math.floor(nsToMs(workEnd - clock()));
  const clamp = (stage, maximum, stageStart = clock()) => {
    workSignal.throwIfAborted();
    const remaining = Math.min(
      workRemainingMs(),
      Math.floor(maximum - nsToMs(clock() - stageStart)),
    );
    if (remaining <= 0)
      throw new Error(`${stage}: no time remains within the stage and session work budgets`);
    return remaining;
  };
  const capture = async (boundary) => {
    if (!withAllocator) return;
    const startedHrtimeNs = String(clock());
    try {
      checkTrace({ final: false });
      const timeoutMs = clamp(`mappings: ${boundary}`, RENDERER_MEMORY_MAPPING_LIMITS.timeMs);
      const result = await captureMappings(server, boundary, { timeoutMs, signal: workSignal });
      workSignal.throwIfAborted();
      if (workRemainingMs() <= 0) throw new Error("mapping observation exceeded the work budget");
      mappingResults.push({ boundary, ok: true });
      record({ type: "renderer-memory-mapping", boundary, startedHrtimeNs, ok: true, result });
    } catch (error) {
      const details = {
        boundary,
        ok: false,
        error: String(error?.message ?? error),
        partial: error.mappingEvidence ?? null,
      };
      mappingResults.push(details);
      record({ type: "renderer-memory-mapping", startedHrtimeNs, ...details });
      fail({ reason: "renderer-memory-mapping-invalid", ...details });
      throw error;
    }
  };
  const runStage = async (stage, maximum, stageStart, operation) => {
    const timeoutMs = clamp(stage, maximum, stageStart);
    let timer;
    try {
      const result = await interruptible(
        Promise.race([
          operation(timeoutMs),
          new Promise((_, reject) => {
            timer = setTimeout(() => reject(new Error(`${stage} reached its deadline`)), timeoutMs);
          }),
        ]),
        workSignal,
      );
      clamp(stage, maximum, stageStart);
      return result;
    } finally {
      clearTimeout(timer);
    }
  };
  // Wait only for this already-issued write's trace to complete. The stage
  // and work clocks keep running, and no further mutation can start meanwhile.
  const observeMutation = async ({ kind, edit, path, t0Ns, witnessNs }, maximum, stageStart) => {
    const mutation = { kind, ...(edit && { edit }), paths: pathAliases(path), t0Ns, witnessNs };
    mutations.push(mutation);
    record({ type: "renderer-memory-mutation", mutation });
    try {
      while (true) {
        const report = checkTrace({ final: false });
        workSignal.throwIfAborted();
        const endpoint = report.postApply?.endpoints[mutations.length - 1];
        if (endpoint) {
          requireAttributed();
          clamp("post-apply observation", maximum, stageStart);
          break;
        }
        const remaining = clamp("post-apply observation", maximum, stageStart);
        await sleep(Math.min(25, remaining), undefined, { signal: workSignal });
      }
    } catch (error) {
      if (!workSignal.aborted)
        fail({
          reason: "renderer-memory-post-apply-incomplete",
          mutation,
          error: String(error?.message ?? error),
        });
      throw error;
    }
  };
  const requireAttributed = () => {
    checkTrace({ final: false, requireAttributedPostApply: true });
    workSignal.throwIfAborted();
  };
  record({ type: "renderer-memory-phase", step: "start", supervisorStartedMs, workDeadline });
  return {
    signal: workSignal,
    enter,
    clamp,
    runStage,
    startSampling(ownedServer) {
      server = ownedServer;
      interval = setInterval(takePeriodic, limits.limits.sampleEveryMs);
      takePeriodic();
    },
    async beforeEdits() {
      await interruptible(take("before-edits"), workSignal);
      // Readiness and the detached provenance drain use different pipes and
      // scheduling. Give the marker at most one safety interval to arrive.
      const end = clock() + deadlineNs(clamp("native trace startup", limits.limits.sampleEveryMs));
      while (!checkTrace({ final: false }).start && clock() < end) {
        workSignal.throwIfAborted();
        await sleep(Math.min(25, Math.ceil(nsToMs(end - clock()))), undefined, {
          signal: workSignal,
        });
      }
      checkTrace({
        final: false,
        requireStart: true,
        requireAttributedPostApply: withPostApply,
      });
      workSignal.throwIfAborted();
      await capture("before-edits");
      enter("edits");
    },
    async afterAttempt(record, stageStart) {
      // The bracket is a controller observation. Attribution of lifecycle
      // events stays with their native host/refresh IDs and occurrence clock.
      const work = take({
        scenario: record.scenario,
        opId: record.opId,
        iteration: record.iteration,
        t0Ns: record.t0Ns ?? null,
        t1Ns: record.t1Ns ?? null,
        outcome: record.outcome,
      });
      await interruptible(work, workSignal);
      if (withPostApply && record.outcome === "success")
        await observeMutation(
          {
            kind: "edit",
            edit: mutations.length + 1,
            path: record.write.path,
            t0Ns: record.t0Ns,
            witnessNs: record.t1Ns,
          },
          limits.timeouts.attemptMs,
          stageStart,
        );
    },
    ...(withPostApply && {
      beforeMutation: requireAttributed,
      async afterRestoration({ actions, mismatches, restoreNs, witnessNs }) {
        const edit = mutations.findLast((mutation) => mutation.kind === "edit");
        const action = actions?.[0];
        if (
          !edit ||
          mutations.length !== limits.edits ||
          mismatches.length !== 0 ||
          actions?.length !== 1 ||
          action.method !== "in-place" ||
          action.error ||
          !edit.paths.some((path) => pathAliases(action.path).includes(path))
        ) {
          fail({ reason: "renderer-memory-post-apply-invalid-restoration-write" });
          workSignal.throwIfAborted();
        }
        await observeMutation(
          {
            kind: "restoration",
            path: action.path,
            t0Ns: String(restoreNs),
            witnessNs: String(witnessNs),
          },
          limits.timeouts.restoreMs,
          restoreNs,
        );
        checkTrace({ final: false, requireCompletePostApply: true });
        clamp("post-apply restoration observation", limits.timeouts.restoreMs, restoreNs);
      },
    }),
    afterEdits: withAllocator
      ? async () => {
          if (withPostApply) requireAttributed();
          await capture("after-edits");
          if (withPostApply) requireAttributed();
        }
      : undefined,
    async finalIdle({ stopped, stopReason, cleanup }) {
      if (
        stopped ||
        failure ||
        !cleanup?.ok ||
        cleanup.live?.length !== limits.liveChecks ||
        !cleanup.live.every((check) => check.ok) ||
        cleanup.settle?.bodyRevision?.matched !== true
      ) {
        const reason = failure?.reason ?? stopReason ?? "incomplete-restoration-or-route-checks";
        record({ type: "renderer-memory-idle", step: "skipped", reason, plannedMs: limits.idleMs });
        if (!stopped && !failure) fail({ reason });
        return;
      }
      workSignal.throwIfAborted();
      await interruptible(take("before-idle"), workSignal);
      await capture("after-route-verification");
      if (workRemainingMs() < limits.idleMs) {
        fail({ reason: "insufficient-final-idle-budget", workRemainingMs: workRemainingMs() });
        record({ type: "renderer-memory-idle", step: "skipped", plannedMs: limits.idleMs });
        return;
      }
      enter("idle");
      const started = clock();
      const end = started + deadlineNs(limits.idleMs);
      record({ type: "renderer-memory-idle", step: "start", plannedMs: limits.idleMs });
      try {
        while (clock() < end)
          await sleep(Math.ceil(nsToMs(end - clock())), undefined, { signal: workSignal });
        workSignal.throwIfAborted();
        if (clock() > workEnd) throw new Error("idle completed after the work deadline");
        const alive = server.isAlive();
        if (!alive.alive) throw new Error("server exited before idle completed");
        idleMs = nsToMs(clock() - started);
        record({
          type: "renderer-memory-idle",
          step: "complete",
          plannedMs: limits.idleMs,
          actualMs: idleMs,
        });
      } catch (error) {
        record({
          type: "renderer-memory-idle",
          step: "interrupted",
          plannedMs: limits.idleMs,
          actualMs: nsToMs(clock() - started),
          error: String(error?.message ?? error),
        });
        throw error;
      }
      await capture("after-idle");
    },
    beginShutdown() {
      // No process scans during expected server exit. Keep the in-flight
      // sample if it finishes, with the phase captured when it started.
      clearInterval(interval);
      enter("shutdown");
    },
    finishSampling() {
      clearInterval(interval);
      samplingStopped = true;
    },
    finish({ logClose, completedSchedule, serverAbsent, cleanup }) {
      const lifecycle = server
        ? checkTrace({
            final: true,
            requireStart: true,
            minimumAdoptions: completedSchedule ? 14 : 0,
            requireCompletePostApply: withPostApply && completedSchedule,
          })
        : null;
      const logComplete =
        logClose?.pipes === "closed" && !logClose.droppedAfterEnd && !logClose.errors?.length;
      const mappingsComplete = RENDERER_MEMORY_MAPPING_BOUNDARIES.every(
        (boundary, index) =>
          mappingResults[index]?.boundary === boundary && mappingResults[index].ok,
      );
      return {
        diagnostic: true,
        protocol,
        scheduleComplete: completedSchedule,
        observationComplete:
          lifecycle?.complete === true &&
          logComplete &&
          serverAbsent &&
          !evidenceFailure &&
          (!withAllocator || mappingsComplete) &&
          (!withPostApply || lifecycle.postApply?.complete === true),
        ...(withAllocator && {
          mappings: {
            complete: mappingsComplete,
            planned: RENDERER_MEMORY_MAPPING_BOUNDARIES,
            results: mappingResults,
          },
        }),
        lifecycle,
        logComplete,
        failure,
        evidenceFailure,
        restoration: cleanup
          ? {
              outcome: cleanup.ok ? "verified" : "incomplete",
              diskMismatches: cleanup.mismatches ?? null,
              freshBodyWitness: cleanup.settle?.bodyRevision?.matched ?? null,
              liveChecks: {
                planned: limits.liveChecks,
                attempted: cleanup.live?.length ?? 0,
                passed: cleanup.live?.filter((check) => check.ok).length ?? 0,
                skipped: cleanup.liveSkipped ?? null,
              },
            }
          : null,
        idleMs,
        samples,
        peakSummedRssKiB,
        overlapSkipped,
      };
    },
    get failure() {
      return failure;
    },
    get idleMs() {
      return idleMs;
    },
  };
}
