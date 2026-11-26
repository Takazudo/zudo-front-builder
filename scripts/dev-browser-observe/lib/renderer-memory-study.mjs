import { setTimeout as delay } from "node:timers/promises";
import { timed } from "./controller-timing.mjs";
import { TINY_HELPER_CAP } from "./startup-acceptance.mjs";
import { pathAliases } from "./provenance.mjs";
import { interruptible } from "./session-deadline.mjs";
import {
  rendererMemoryAllocatorProtocol,
  RENDERER_MEMORY_LIMITS as limits,
  RENDERER_MEMORY_MAPPING_BOUNDARIES,
  RENDERER_MEMORY_MAPPING_LIMITS,
  RENDERER_MEMORY_POST_APPLY_PROTOCOL,
  RENDERER_MEMORY_PROTOCOL,
  RENDERER_MEMORY_RECLAMATION_PROTOCOL,
} from "./renderer-memory-protocol.mjs";
import { hrtimeNs, nsToMs } from "./util.mjs";

const deadlineNs = (ms) => BigInt(ms) * 1_000_000n;

export function createRendererMemoryStudy({
  append,
  signal,
  supervisorStartedMs,
  workDeadline,
  protocol = RENDERER_MEMORY_PROTOCOL,
  reclamationArm = null,
  sample,
  tinyProbe,
  timing,
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
  let nextDue = null;
  let intervalOriginNs = null;
  let intervalCallbacks = 0;
  let previousCoreEntryNs = null;
  let previousCallbackEntryNs = null;
  let previousHelperCloseNs = null;
  let previousSamplerEntryNs = null;
  let previousPeriodicSamplerEntryNs = null;
  let previousDispatchEndNs = null;
  let sampleInvocations = 0;
  let helperStarts = 0;
  let helperLaunchAttempts = 0;
  const plannedIdleMs = tinyProbe === "A" ? 30_000 : tinyProbe === "B" ? 10_000 : limits.idleMs;
  const liveChecks = tinyProbe ? 4 : limits.liveChecks;
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
  let ending = false;
  let samplerFailure = null;
  let unresolvedHelper = null;
  const withAllocator = rendererMemoryAllocatorProtocol(protocol);
  const withReclamation = protocol === RENDERER_MEMORY_RECLAMATION_PROTOCOL;
  const withPostApply = protocol === RENDERER_MEMORY_POST_APPLY_PROTOCOL || withReclamation;
  let pendingRestoration = null;
  let restorationWitness = false;
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
  const checkTrace = (options) =>
    timed(timing, "checkTrace", options, (traceRow) => {
      const mark = server.provMark?.();
      if (
        !pendingRestoration &&
        !options.final &&
        !options.requireStart &&
        !options.requireAttributedPostApply &&
        !options.requireCompletePostApply &&
        mark != null &&
        lastInspection?.mark === mark &&
        lastInspection.mutationCount === mutations.length
      ) {
        if (traceRow) traceRow.cacheHit = true;
        return lastInspection.report;
      }
      if (traceRow) traceRow.cacheHit = false;
      const observedMutations = pendingRestoration
        ? [...mutations, { ...pendingRestoration, witnessNs: String(clock()) }]
        : mutations;
      let inputLines;
      let validatorRow;
      const report = timed(
        timing,
        "checkTrace.inspect",
        {
          mark: mark ?? null,
          mutationCount: observedMutations.length,
          ...options,
        },
        (row) => {
          validatorRow = row;
          return inspect(server, {
            ...options,
            ...(row && {
              onInput: (lines) => {
                inputLines = lines;
              },
            }),
            countReclamation: Boolean(tinyProbe && options.final),
            withAllocator,
            withPostApply,
            reclamationArm: withReclamation ? reclamationArm : null,
            restorationWitness,
            mutations: observedMutations,
          });
        },
      );
      if (inputLines) {
        validatorRow.inputLines = inputLines.length;
        validatorRow.inputBytes = timed(timing, "trace input sizing", { mark }, () =>
          inputLines.reduce((sum, line) => sum + Buffer.byteLength(line.text), 0),
        );
      }
      if (
        !pendingRestoration &&
        !options.requireAttributedPostApply &&
        !options.requireCompletePostApply
      )
        lastInspection = { mark, mutationCount: mutations.length, report };
      if (report.failure) fail(report.failure);
      return report;
    });
  const newDispatch = (source, boundary, callbackEntry = null) => {
    if (!timing) return null;
    const periodic = source === "interval";
    const row = timing.dispatch({
      source,
      boundary,
      outcome: "requested",
      intervalOriginNs,
      intervalCallbackOrdinal: periodic && callbackEntry !== null ? ++intervalCallbacks : null,
      ordinalDueNs:
        periodic && callbackEntry !== null && intervalOriginNs !== null
          ? String(
              BigInt(intervalOriginNs) +
                deadlineNs(intervalCallbacks * limits.limits.sampleEveryMs),
            )
          : null,
      requestedNs: String(clock()),
      nominalDueNs: periodic && nextDue !== null ? String(nextDue) : null,
      callbackEntryNs: callbackEntry === null ? null : String(callbackEntry),
      previousCallbackEntryNs,
      previousHelperCloseNs,
      previousSamplerEntryNs,
      previousPeriodicSamplerEntryNs,
      previousDispatchEndNs,
      previousCoreEntryNs,
      samplerEntryNs: null,
      inFlightAtRequest: Boolean(inflight),
      // Missed nominal 1 Hz opportunities, not evidence that Node queued callbacks.
      skippedIntervals:
        periodic && nextDue !== null && callbackEntry > nextDue
          ? Number((callbackEntry - nextDue) / deadlineNs(limits.limits.sampleEveryMs))
          : 0,
    });
    if (periodic && callbackEntry !== null) {
      previousCallbackEntryNs = String(callbackEntry);
      nextDue = callbackEntry + deadlineNs(limits.limits.sampleEveryMs);
    }
    return row;
  };
  const take = (boundary = null, dispatch = newDispatch("boundary", boundary)) => {
    if (ending || samplingStopped || samplerFailure || (tinyProbe && failure)) {
      if (dispatch) dispatch.outcome = "cancelled";
      return Promise.resolve();
    }
    if (inflight) {
      if (boundary) {
        if (dispatch) dispatch.waitedForInflight = true;
        return inflight.then(() => take(boundary, dispatch));
      }
      if (dispatch) dispatch.outcome = "overlap";
      overlapSkipped += 1;
      if (nsToMs(clock() - sampleStarted) >= limits.limits.sampleEveryMs)
        fail({
          reason: "renderer-memory-safety-sample-stalled",
          startedHrtimeNs: String(sampleStarted),
        });
      return inflight;
    }
    if (tinyProbe && sampleInvocations >= TINY_HELPER_CAP) {
      if (dispatch) dispatch.outcome = "cap";
      fail({ reason: "renderer-memory-helper-cap", sampleInvocations, helperStarts });
      return Promise.reject(workSignal.reason);
    }
    const started = clock();
    if (dispatch) {
      dispatch.samplerEntryNs = String(started);
      dispatch.previousSamplerEntryNs = previousSamplerEntryNs;
      dispatch.previousPeriodicSamplerEntryNs = previousPeriodicSamplerEntryNs;
      dispatch.previousHelperCloseNs = previousHelperCloseNs;
      dispatch.outcome = "started";
      previousSamplerEntryNs = String(started);
      if (dispatch.source === "interval") previousPeriodicSamplerEntryNs = String(started);
    }
    sampleInvocations++;
    sampleStarted = started;
    const phaseAtStart = phase;
    inflight = (async () => {
      let result;
      try {
        result = await sample(server);
        if (dispatch) {
          dispatch.sampleReturnedNs = String(clock());
          dispatch.sampleAccepted = true;
        }
        observeHelper(result, dispatch);
      } catch (error) {
        const details = error?.details ?? null;
        if (dispatch) {
          dispatch.sampleReturnedNs = String(clock());
          dispatch.sampleAccepted = false;
        }
        observeHelper(details, dispatch);
        const partial = details?.summedRssKiB;
        if (Number.isSafeInteger(partial) && partial >= 0)
          peakSummedRssKiB = Math.max(peakSummedRssKiB ?? 0, partial);
        samplerFailure ??= {
          reason: "renderer-memory-sample-invalid",
          error: String(error?.message ?? error),
          details,
          startedHrtimeNs: String(started),
          phase: phaseAtStart,
        };
        if (details?.helper?.closed === false || details?.helper?.pipesClosed === false)
          unresolvedHelper = details.helper;
        evidenceFailure ??= samplerFailure;
        // An invalid final row cannot erase RSS already observed by the helper.
        if (partial > limits.limits.rssCeilingKiB)
          fail({
            reason: "resource-limit",
            rssCeilingKiB: limits.limits.rssCeilingKiB,
            partialSample: samplerFailure,
          });
        else fail(samplerFailure);
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
      if (dispatch) {
        dispatch.endNs = String(clock());
        previousDispatchEndNs = dispatch.endNs;
      }
      inflight = null;
    });
    return inflight;
  };
  const observeHelper = (result, dispatch) => {
    const spawnCallNs =
      result?.timing?.parent?.spawnCallNs ?? result?.helper?.spawnHrtimeNs ?? null;
    if (spawnCallNs !== null) helperLaunchAttempts++;
    if (dispatch) {
      dispatch.helperSpawnCallNs = spawnCallNs;
      dispatch.coreSamplerEntryNs = result?.timing?.parent?.entryNs ?? result?.hrtimeNs ?? null;
      previousCoreEntryNs = dispatch.coreSamplerEntryNs;
    }
    if (!result?.helper?.pid) return;
    helperStarts++;
    if (dispatch) {
      dispatch.helperPid = result.helper.pid;
      dispatch.helperSpawnNs = result.helper.spawnHrtimeNs;
      dispatch.helperCloseNs = result.timing?.parent?.closeNs ?? null;
      previousHelperCloseNs = dispatch.helperCloseNs;
    }
  };
  const takePeriodic = async (source = "interval") => {
    const callbackEntry = timing ? clock() : null;
    try {
      await take(null, newDispatch(source, null, callbackEntry));
    } catch (error) {
      try {
        fail({ reason: "renderer-memory-record-failed", error: String(error?.message ?? error) });
      } catch {
        // The abort precedes the record write, including when the evidence sink fails.
      }
    }
  };
  const enter = (next) => {
    phase = next;
    if (timing) {
      timing.phase = next;
      timing.activeEdit = null;
    }
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
          requireAttributed(kind === "restoration");
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
  const requireAttributed = (requireCompletePostApply = false) => {
    checkTrace({ final: false, requireAttributedPostApply: true, requireCompletePostApply });
    workSignal.throwIfAborted();
  };
  const idle = async () => {
    workSignal.throwIfAborted();
    if (tinyProbe !== "A") await interruptible(take("before-idle"), workSignal);
    await capture("after-route-verification");
    if (workRemainingMs() < plannedIdleMs) {
      fail({ reason: "insufficient-final-idle-budget", workRemainingMs: workRemainingMs() });
      record({ type: "renderer-memory-idle", step: "skipped", plannedMs: plannedIdleMs });
      return;
    }
    enter("idle");
    const started = clock();
    const end = started + deadlineNs(plannedIdleMs);
    record({ type: "renderer-memory-idle", step: "start", plannedMs: plannedIdleMs });
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
        plannedMs: plannedIdleMs,
        actualMs: idleMs,
      });
    } catch (error) {
      record({
        type: "renderer-memory-idle",
        step: "interrupted",
        plannedMs: plannedIdleMs,
        actualMs: nsToMs(clock() - started),
        error: String(error?.message ?? error),
      });
      throw error;
    }
    await capture("after-idle");
  };
  record({ type: "renderer-memory-phase", step: "start", supervisorStartedMs, workDeadline });
  return {
    signal: workSignal,
    enter,
    ...(timing && {
      setActiveEdit(edit) {
        timing.activeEdit = edit;
      },
    }),
    ...(tinyProbe === "A" && { passiveIdle: idle }),
    clamp,
    runStage,
    startSampling(ownedServer) {
      server = ownedServer;
      if (timing) {
        const origin = clock();
        intervalOriginNs = String(origin);
        nextDue = origin + deadlineNs(limits.limits.sampleEveryMs);
      }
      interval = setInterval(takePeriodic, limits.limits.sampleEveryMs);
      return takePeriodic("initial");
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
    ...(withReclamation && {
      async beforeRestorationWitness({ actions, mismatches, restoreNs }) {
        const action = actions?.[0];
        const edit = mutations.at(-1);
        if (
          mutations.length !== limits.edits ||
          mismatches.length !== 0 ||
          actions?.length !== 1 ||
          action.method !== "in-place" ||
          action.error ||
          !edit.paths.some((path) => pathAliases(action.path).includes(path))
        ) {
          fail({ reason: "renderer-memory-reclamation-invalid-restoration-write" });
          workSignal.throwIfAborted();
        }
        pendingRestoration = {
          kind: "restoration",
          paths: pathAliases(action.path),
          t0Ns: String(restoreNs),
        };
        record({
          type: "renderer-memory-reclamation-gate",
          step: "start",
          mutation: pendingRestoration,
        });
        try {
          while (true) {
            const report = checkTrace({ final: false });
            workSignal.throwIfAborted();
            const remaining = clamp("reclamation bracket", limits.timeouts.restoreMs, restoreNs);
            if (report.reclamation?.bracketComplete && report.reclamation.associationValid) {
              record({
                type: "renderer-memory-reclamation-gate",
                step: "complete",
                reclamation: report.reclamation,
                originalBodyWitnessPending: true,
              });
              return;
            }
            await sleep(Math.min(25, remaining), undefined, { signal: workSignal });
          }
        } catch (error) {
          if (!workSignal.aborted)
            fail({
              reason: "renderer-memory-reclamation-incomplete",
              error: String(error?.message ?? error),
            });
          throw error;
        } finally {
          pendingRestoration = null;
          lastInspection = null;
        }
      },
    }),
    ...(withPostApply && {
      requiresPostApply: true,
      beforeMutation: () => requireAttributed(),
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
        restorationWitness = true;
        lastInspection = null;
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
      },
    }),
    afterEdits: withAllocator
      ? async () => {
          if (withPostApply) requireAttributed();
          await capture("after-edits");
          if (withPostApply) requireAttributed();
        }
      : undefined,
    async finalIdle({ stopped, stopReason, cleanup, expectedRestorationPath }) {
      if (
        stopped ||
        failure ||
        !cleanup?.ok ||
        cleanup.live?.length !== liveChecks ||
        !cleanup.live.every((check) => check.ok) ||
        cleanup.settle?.bodyRevision?.matched !== true ||
        (tinyProbe === "B" &&
          (cleanup.actions?.length !== 1 ||
            cleanup.actions[0].path !== expectedRestorationPath ||
            cleanup.actions[0].method !== "in-place" ||
            cleanup.actions[0].error ||
            !cleanup.settle.bodyRevision.docUUID ||
            cleanup.settle.bodyRevision.observedDocUUID !== cleanup.settle.bodyRevision.docUUID ||
            cleanup.mismatches?.length !== 0))
      ) {
        const reason = failure?.reason ?? stopReason ?? "incomplete-restoration-or-route-checks";
        record({ type: "renderer-memory-idle", step: "skipped", reason, plannedMs: plannedIdleMs });
        if (!stopped && !failure) fail({ reason });
        return;
      }
      await idle();
    },
    beginShutdown() {
      // No process scans during expected server exit. Keep the in-flight
      // sample if it finishes, with the phase captured when it started.
      if (timing && interval && !ending) {
        const row = newDispatch("interval", null);
        if (row) row.outcome = "cancelled";
      }
      clearInterval(interval);
      ending = true;
      enter("shutdown");
      return inflight;
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
            minimumAdoptions: completedSchedule && !tinyProbe ? 14 : 0,
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
        ...(withReclamation && { reclamationArm }),
        scheduleComplete: completedSchedule,
        observationComplete:
          lifecycle?.complete === true &&
          logComplete &&
          serverAbsent &&
          !evidenceFailure &&
          (!withAllocator || mappingsComplete) &&
          (!withPostApply || lifecycle.postApply?.complete === true) &&
          (!withReclamation || lifecycle.reclamation?.complete === true),
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
        samplerFailure,
        restoration: cleanup
          ? {
              outcome: cleanup.ok ? "verified" : "incomplete",
              diskMismatches: cleanup.mismatches ?? null,
              freshBodyWitness: cleanup.settle?.bodyRevision?.matched ?? null,
              liveChecks: {
                planned: liveChecks,
                attempted: cleanup.live?.length ?? 0,
                passed: cleanup.live?.filter((check) => check.ok).length ?? 0,
                skipped: cleanup.liveSkipped ?? null,
              },
            }
          : null,
        idleMs,
        ...(tinyProbe && { sampleInvocations, helperStarts, helperLaunchAttempts }),
        samples,
        peakSummedRssKiB,
        overlapSkipped,
      };
    },
    get unresolvedHelper() {
      return unresolvedHelper;
    },
    get samplerFailure() {
      return samplerFailure;
    },
    get failure() {
      return failure;
    },
    get idleMs() {
      return idleMs;
    },
  };
}
