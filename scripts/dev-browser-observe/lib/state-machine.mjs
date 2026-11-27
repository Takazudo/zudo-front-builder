import { interruptible } from "./session-deadline.mjs";
import { runSpecialMode } from "./special-modes.mjs";
import { RENAME_NOOP } from "./fixture.mjs";
import { attemptProvenance } from "./provenance.mjs";
import { NO_WRITE_STEPS, noWriteStepsError, nsToMs, sha256 } from "./util.mjs";

export const OUTCOMES = [
  "success",
  "timeout",
  "stale",
  "precondition-failed",
  "arm-error",
  "prepare-error",
  "precondition-unstable",
  "controller-error",
  "expected-negative-observed",
  "expected-negative-mismatch",
  "recovery-failed",
  "final-state-mismatch",
  "transient-not-rendered",
  "hidden-precondition-visited",
  "hidden-check-failed",
  "hidden-error",
  "final-state-error",
  "expected-negative-error",
  "write-error",
  "server-exit",
  "resource-limit",
  "supplemental-failed",
  "restoration-failed",
  "identity-rejected",
  "rename-noop",
  "recovery-observed",
  "recovery-state-mismatch",
  "not-same-tick",
  "same-tick-unknown",
  "recovery-error",
  "rejection-not-observed",
  "open-page-replaced",
];

const PASS_OUTCOMES = new Set(["success", "expected-negative-observed", "recovery-observed"]);

const ERROR_OUTCOMES = new Set(
  OUTCOMES.filter((o) => !PASS_OUTCOMES.has(o) && !["timeout", "stale"].includes(o)),
);

/**
 * Which settle rule a cleanup needs: if it wrote any file that has an
 * auto-update contract, its own native update must be observed before the
 * quiet window counts. Quiet alone ends the settle only when nothing was
 * written, or every written path is under a declared no-auto-update root.
 */
export function settleRule(actions, noAutoUpdatePaths = []) {
  const written = actions.filter((a) => !a.error).flatMap((a) => [a.path, a.from].filter(Boolean));
  if (written.length === 0) return { requireNativeUpdate: false, rule: "quiet-only:no-writes" };
  const covered = (p) =>
    noAutoUpdatePaths.some(
      (root) => p === root || p.startsWith(root.endsWith("/") ? root : `${root}/`),
    );
  if (written.every(covered))
    return { requireNativeUpdate: false, rule: "quiet-only:no-auto-update-contract" };
  return { requireNativeUpdate: true, rule: "require-native-update" };
}

/** Declared outcome for each negative-control injection (allowed only in set "negative-controls"). */
export const INJECT_EXPECT = {
  "kill-server-during-wait": "server-exit",
  "write-error": "write-error",
  "precondition-true": "precondition-failed",
  "restore-blocked": "restoration-failed",
};

export const ALWAYS_TRUE_PREDICATE = { kind: "count", selector: "html", equals: 1 };

function injectPass(kind, outcome, rec) {
  if (INJECT_EXPECT[kind] !== outcome) return false;
  if (kind === "kill-server-during-wait")
    return rec.exit?.exitCode === null && rec.exit?.signalCode === "SIGKILL";
  if (kind === "precondition-true")
    return rec.targetSha256Before !== null && rec.targetSha256Before === rec.targetSha256After;
  return true;
}

export function newAccounting(planned) {
  return {
    planned,
    attempted: 0,
    success: 0,
    stale: 0,
    timeout: 0,
    error: 0,
    notAttempted: planned,
    lateTokens: 0,
    expectedControlPasses: 0,
    expectedErrors: 0,
    expectedTimeouts: 0,
    controlsNotFired: 0,
    expectedNegativePasses: 0,
    recoveryPasses: 0,
    recoverySameTickProvenPasses: 0,
    recoveryNoTickPremisePasses: 0,
    recoveryUnprovenTick: 0,
    restorationFailures: 0,
    supplementalFailures: 0,
    lastArmedUnknownOperation: null,
    stoppedByExpectedControl: false,
    stoppedByClassifyTimeout: false,
    stopReason: null,
    stoppedAfter: null,
  };
}

export function classifyMechanism({
  preDocUUID,
  notifyDocUUID,
  mainFrameNavigations,
  serverPidBefore,
  serverPidAfter,
  livereloadEvents = [],
}) {
  if (serverPidBefore !== serverPidAfter) return "server-restart";
  const page = livereloadEvents.includes("page");
  const css = livereloadEvents.includes("css");
  const newDoc = notifyDocUUID !== preDocUUID;
  if (newDoc && mainFrameNavigations > 0)
    return page ? "native-full-reload" : "full-reload-without-page-frame";
  if (!newDoc && mainFrameNavigations === 0) {
    if (css) return "css-replacement";
    // Declared framework HMR events (fixed distributions only; zfb livereload has neither).
    if (livereloadEvents.includes("rsc")) return "rsc-update";
    if (livereloadEvents.includes("module")) return "hmr-module-update";
    return "in-document-update";
  }
  return "unclassified";
}

/** Which ceiling a sample trips, or null. */
export function overLimits(limits, sample, wallMs) {
  if (
    limits.rssCeilingKiB &&
    typeof sample?.summedRssKiB === "number" &&
    sample.summedRssKiB > limits.rssCeilingKiB
  )
    return "rss";
  if (limits.wallCeilingMs && wallMs > limits.wallCeilingMs) return "wall";
  return null;
}

/**
 * Exit disposition for a finished batch: ok when it never stopped, or when it
 * stopped only by declared controls (negative-control timeout/injection or a
 * classify timeout) with no undeclared error, timeout or stale left over.
 */
export function batchExitOk(acc, stopped) {
  if (!stopped) return true;
  if (!(acc.stoppedByExpectedControl || acc.stoppedByClassifyTimeout)) return false;
  if (acc.error - acc.expectedErrors !== 0) return false;
  return acc.timeout + acc.stale - acc.expectedTimeouts === 0;
}

function count(acc, outcome, rec = null) {
  if (outcome === "success") acc.success += 1;
  else if (outcome === "timeout") acc.timeout += 1;
  else if (outcome === "stale") acc.stale += 1;
  else if (outcome === "expected-negative-observed") acc.expectedNegativePasses += 1;
  else if (outcome === "recovery-observed") {
    // A recovery whose same-tick premise was declared but not proven is kept apart from the passes.
    if (rec?.sameTick && rec.sameTickProven !== true) acc.recoveryUnprovenTick += 1;
    else {
      acc.recoveryPasses += 1;
      if (rec?.sameTickProven === true) acc.recoverySameTickProvenPasses += 1;
      else acc.recoveryNoTickPremisePasses += 1;
    }
  } else if (ERROR_OUTCOMES.has(outcome)) acc.error += 1;
}

/**
 * Fail-stop batch runner. Every non-success outcome is appended (and fsynced
 * by the recorder) BEFORE the batch stops; no further workload edit follows.
 * Cleanup restoration always runs afterwards and is never a measured trial.
 *
 * deps: { recorder, clock, server, page, fs, limits, timeouts, startedNs, afterScenario?, beforeCleanup?, haltReason? }
 * afterScenario runs only after a scenario's restoration succeeded (resource-study idle);
 * beforeCleanup runs as each restoration starts; a non-null haltReason() stops before the next op.
 */
export async function runBatch({ scenarios, deps }) {
  const { recorder, clock, server, page, fs, limits = {}, timeouts, signal } = deps;
  const wait = (work) => interruptible(work, signal);
  const diagnostic = deps.rendererMemory;
  const stage = async (name, maximum, started, action) => {
    deps.safetySignal?.throwIfAborted();
    if (diagnostic) return diagnostic.runStage(name, maximum, started, action);
    if (!deps.stageBudget) return wait(action(maximum));
    const ms = deps.stageBudget(maximum, started);
    let timer;
    try {
      const result = await wait(
        Promise.race([
          action(ms),
          new Promise((_, reject) => {
            timer = setTimeout(() => reject(new Error(name + " exceeded its budget")), ms);
          }),
        ]),
      );
      deps.stageBudget(maximum, started);
      return result;
    } finally {
      clearTimeout(timer);
    }
  };
  const planned = scenarios.reduce((n, s) => n + s.ops.length, 0);
  const acc = newAccounting(planned);
  if (deps.accountingRef) deps.accountingRef.current = acc;
  const startedNs = deps.startedNs ?? clock.now();
  let activeSnapshot = null;
  let bodyRevision = null;
  let peerWrite = null;
  let stopped = false;

  const append = (rec) => recorder.append(rec);

  const stop = (reason, where) => {
    acc.stopReason = reason;
    acc.stoppedAfter = where;
    stopped = true;
  };

  let lastCleanupControlPass = null;
  const notFired = (where, kind, detail) => {
    acc.controlsNotFired += 1;
    acc.error += 1;
    acc.stoppedByExpectedControl = false;
    append({ type: "control-not-fired", label: "not-success", ...where, inject: kind, detail });
    stop("negative-control-not-fired", where);
  };

  const cleanup = async (snapshot, reason, blocked = [], { retry = false } = {}) => {
    // A refused safety observation leaves this disposable work copy intact.
    // Restoring it while its server is live would trigger another rebuild.
    if (deps.safetySignal?.aborted) {
      append({
        type: "cleanup",
        label: "cleanup",
        measured: false,
        reason,
        ok: false,
        liveSkipped: "fixed-rss-safety-stop",
        restored: false,
      });
      acc.restorationFailures++;
      return false;
    }
    deps.beforeCleanup?.({ reason });
    const blocks = [];
    for (const path of blocked) {
      try {
        blocks.push(fs.blockWrite(path));
      } catch (error) {
        blocks.push({ path, error: String(error?.message ?? error) });
      }
    }
    if (blocks.length)
      append({
        type: "inject",
        label: "cleanup",
        inject: "restore-blocked",
        step: "apply",
        blocks,
      });
    const restoreNs = clock.now();
    const mark = page.mark?.() ?? null;
    const restore = fs.restore(snapshot);
    let peerRestoration;
    if (peerWrite) {
      const action = restore.actions?.find((a) => a.path === peerWrite.path);
      const restored = fs.hashOf(peerWrite.path);
      peerRestoration = {
        path: peerWrite.path,
        originalSha256: peerWrite.originalSha256,
        restoredSha256: restored,
        ok:
          restore.actions?.length === 1 &&
          action?.method === "in-place" &&
          restored === peerWrite.originalSha256 &&
          Number.isSafeInteger(peerWrite.inode) &&
          action.evidence?.inodeBefore === peerWrite.inode &&
          action.evidence?.inodeAfter === peerWrite.inode,
      };
    }
    const alive = server.isAlive();
    const live = [];
    let liveSkipped = null;
    let settle = null;
    const rule = settleRule(restore.actions ?? [], fs.noAutoUpdatePaths ?? []);
    if (signal?.aborted) liveSkipped = "session-deadline";
    else if (restore.mismatches.length > 0) liveSkipped = "hash-mismatch";
    else if (!alive.alive) liveSkipped = "server-not-alive";
    else {
      const witness = rule.requireNativeUpdate ? bodyRevision : null;
      if (diagnostic?.beforeRestorationWitness && reason === "scenario-end") {
        try {
          await diagnostic.beforeRestorationWitness({
            actions: restore.actions,
            mismatches: restore.mismatches,
            restoreNs,
          });
        } catch (error) {
          liveSkipped = "restoration-observation-incomplete";
          settle = { settled: false, observerError: String(error?.message ?? error) };
        }
      }
      if (!liveSkipped && page.settle && mark)
        try {
          settle = await stage("restorative update", timeouts.restoreMs, restoreNs, (boundMs) =>
            page.settle({
              mark,
              startNs: restoreNs,
              quietMs: timeouts.quietMs ?? 1500,
              boundMs,
              ...rule,
              ...(witness ? { bodyRevision: witness } : {}),
            }),
          );
        } catch (error) {
          if (!diagnostic) throw error;
          settle = { settled: false, error: String(error?.message ?? error) };
        }
      if (
        diagnostic?.afterRestoration &&
        reason === "scenario-end" &&
        settle?.settled === true &&
        settle.bodyRevision?.matched === true
      )
        try {
          await diagnostic.afterRestoration({
            actions: restore.actions,
            mismatches: restore.mismatches,
            restoreNs,
            witnessNs: clock.now(),
          });
        } catch (error) {
          liveSkipped = "restoration-observation-incomplete";
          settle = { ...settle, observerError: String(error?.message ?? error) };
        }
      if (diagnostic && (reason !== "scenario-end" || settle?.settled !== true)) {
        liveSkipped = reason === "scenario-end" ? "restoration-unverified" : "fail-stop-cleanup";
      }
      if (diagnostic && !liveSkipped) diagnostic.enter("route-verification");
      for (const originalCheck of liveSkipped ? [] : fs.liveChecks(snapshot)) {
        // Recheck the exact body during its existing detail visit, preserving the navigation order.
        const check =
          bodyRevision && originalCheck.path === bodyRevision.path
            ? {
                ...originalCheck,
                predicate: { kind: "all", of: [originalCheck.predicate, bodyRevision.predicate] },
              }
            : originalCheck;
        if (!diagnostic) signal?.throwIfAborted();
        try {
          live.push({
            ...check,
            ...(await stage("route verification", timeouts.restoreMs, restoreNs, (timeoutMs) =>
              page.liveCheck(check, timeoutMs, {
                quietMs: timeouts.quietMs ?? 1500,
                nativeUpdateConfirmed:
                  settle?.settled === true && settle?.nativeUpdateObserved === true,
              }),
            )),
          });
        } catch (error) {
          live.push({ ...check, ok: false, error: String(error?.message ?? error) });
        }
        if (diagnostic && !live.at(-1).ok) break;
      }
    }
    // The declared kill-server control forbids restarting its server. Only that control
    // can verify disk-only cleanup after server death; an unexpected exit leaves live state unknown.
    const diskOnlyControl =
      liveSkipped === "server-not-alive" &&
      acc.stopReason === "server-exit" &&
      acc.stoppedByExpectedControl;
    const settled = settle ? settle.settled === true : !rule.requireNativeUpdate;
    const bodyVerified =
      !bodyRevision ||
      (liveSkipped === null &&
        (!rule.requireNativeUpdate || (settle?.settled && settle.bodyRevision?.matched === true)) &&
        live.some(
          (check) =>
            check.path === bodyRevision.path &&
            check.ok &&
            (!deps.fixedPeer || (check.docUUID && check.docUUID !== settle?.bodyRevision?.docUUID)),
        ));
    let ok =
      !signal?.aborted &&
      restore.mismatches.length === 0 &&
      (diskOnlyControl || (liveSkipped === null && settled)) &&
      (!peerRestoration || peerRestoration.ok) &&
      bodyVerified &&
      live.every((l) => l.ok);
    let quiet = null;
    if (ok && liveSkipped === null && page.quietWindow)
      try {
        quiet = await stage("restoration quiet window", timeouts.restoreMs, restoreNs, () =>
          page.quietWindow(timeouts.quietMs ?? 1500),
        );
      } catch (error) {
        if (!diagnostic) throw error;
        quiet = { error: String(error?.message ?? error) };
        ok = false;
      }
    const cleanupRec = {
      ...(peerRestoration && { peerRestoration }),
      type: "cleanup",
      label: "cleanup",
      measured: false,
      reason,
      ok,
      restored: restore.restored,
      restorationWriteMethod: restore.writeMethod ?? null,
      actions: restore.actions ?? null,
      mismatches: restore.mismatches,
      settle,
      quiet,
      live,
      liveSkipped,
      ...(diskOnlyControl ? { verification: "disk-only:declared-server-kill" } : {}),
      retry,
    };
    if (blocked.length) {
      cleanupRec.inject = { kind: "restore-blocked", expectedOutcome: "restoration-failed" };
      cleanupRec.controlPass = !ok && restore.mismatches.length > 0 && !stopped;
      if (stopped) cleanupRec.controlNotCredited = `batch already stopped by ${acc.stopReason}`;
    }
    append(cleanupRec);
    if (blocks.length) {
      const reverts = blocks
        .filter((b) => !b.error)
        .map((b) => {
          try {
            return fs.unblockWrite(b);
          } catch (error) {
            return { path: b.path, error: String(error?.message ?? error) };
          }
        });
      append({
        type: "inject",
        label: "cleanup",
        inject: "restore-blocked",
        step: "revert-mode",
        reverts,
      });
    }
    if (!ok) {
      acc.restorationFailures += 1;
      acc.error += 1;
      if (cleanupRec.controlPass && !stopped) {
        acc.expectedErrors += 1;
        acc.expectedControlPasses += 1;
        acc.stoppedByExpectedControl = true;
      }
    }
    if (blocked.length) lastCleanupControlPass = cleanupRec.controlPass === true;
    if (blocks.length && !ok) {
      // The blocked restore is the control; the work tree must not stay mutated, so retry once unblocked.
      await cleanup(snapshot, `${reason}:retry-unblocked`, [], { retry: true });
    }
    return ok;
  };

  let current = { where: null, armedToken: null, writeDone: false, resolved: false };
  try {
    outer: for (const scenario of scenarios) {
      signal?.throwIfAborted();
      activeSnapshot = fs.snapshot();
      bodyRevision = null;
      const verifyBodyRevision =
        deps.verifyBodyRevision &&
        scenario.ops.every(
          (op) =>
            !op.inject &&
            op.op === "edit-body" &&
            op.article === scenario.ops[0].article &&
            op.targetPath === scenario.ops[0].targetPath,
        );
      append({
        type: "scenario-start",
        scenario: scenario.id,
        ops: scenario.ops.map((o) => o.id ?? o.op),
        warmOrder: scenario.warmOrder ?? "none",
        ...(scenario.cycle ? { cycle: scenario.cycle } : {}),
      });
      if (scenario.warmOrder && scenario.warmOrder !== "none") {
        const paths = fs.warmPaths(scenario);
        const visits = [];
        try {
          for (const path of paths) {
            const t = clock.now();
            const state = await wait(page.prepare(path));
            visits.push({ path, docUUID: state.docUUID, ms: nsToMs(clock.now() - t) });
          }
        } catch (error) {
          append({
            type: "warm",
            label: "unmeasured",
            scenario: scenario.id,
            warmOrder: scenario.warmOrder,
            visits,
            ok: false,
            error: String(error?.message ?? error),
          });
          acc.error += 1;
          stop("prepare-error", { scenario: scenario.id, op: null });
          break outer;
        }
        append({
          type: "warm",
          label: "unmeasured",
          scenario: scenario.id,
          warmOrder: scenario.warmOrder,
          visits,
          ok: true,
        });
      }
      for (const op of scenario.ops) {
        const where = { scenario: scenario.id, op: op.id ?? op.op };
        diagnostic?.setActiveEdit?.(op.iteration?.index ?? null);
        current = { where, armedToken: null, writeDone: false, resolved: false };
        const base = {
          type: "attempt",
          label: op.iteration?.kind === "warmup" ? "warmup" : "measured",
          scenario: scenario.id,
          opId: op.id ?? op.op,
          op: op.op,
          article: op.article ?? null,
          expect: op.expect ?? "success",
          ...(op.iteration ? { iteration: op.iteration } : {}),
        };

        const halt = deps.haltReason?.() ?? null;
        if (halt) {
          const haltOutcome = halt.outcome ?? "controller-error";
          append({ ...base, type: "precheck", outcome: haltOutcome, halt });
          acc.error += 1;
          stop(haltOutcome, where);
          break outer;
        }
        const alive = server.isAlive();
        if (!alive.alive) {
          append({ ...base, type: "precheck", outcome: "server-exit", exit: alive.exit });
          acc.error += 1;
          stop("server-exit", where);
          break outer;
        }
        const sample = server.sampleResources();
        const wallMs = nsToMs(clock.now() - startedNs);
        const tripped = overLimits(limits, sample, wallMs);
        if (tripped) {
          append({
            ...base,
            type: "precheck",
            outcome: "resource-limit",
            sample,
            wallMs,
            limits,
            tripped,
          });
          acc.error += 1;
          stop("resource-limit", where);
          break outer;
        }

        const attemptStart = diagnostic || deps.stageBudget ? clock.now() : null;
        let attempt;
        let preparedState = null;
        try {
          attempt = fs.prepareAttempt(op);
          if (
            (Array.isArray(attempt.writes) && attempt.writes.length === 0) ||
            (!attempt.writes && !attempt.write)
          )
            throw noWriteStepsError();
          if (op.inject === "precondition-true") attempt.predicate = ALWAYS_TRUE_PREDICATE;
          if (op.inject === "write-error") {
            attempt.injectedFrom = attempt.write.path;
            attempt.write = { ...attempt.write, path: fs.missingSibling(attempt.write.path) };
            if (attempt.writes) attempt.writes = [attempt.write, ...attempt.writes.slice(1)];
          }
          preparedState =
            diagnostic || deps.stageBudget
              ? await stage("attempt prepare", timeouts.attemptMs, attemptStart, (ms) =>
                  page.prepare(attempt.targetPath, Math.min(ms, 30000)),
                )
              : await wait(page.prepare(attempt.targetPath));
          page.markPre?.();
        } catch (error) {
          append({
            ...base,
            type: "precheck",
            outcome: "prepare-error",
            errorKind: error?.code === NO_WRITE_STEPS ? NO_WRITE_STEPS : null,
            writePerformed: false,
            error: String(error?.message ?? error),
          });
          acc.error += 1;
          stop("prepare-error", where);
          break outer;
        }

        acc.attempted += 1;
        acc.notAttempted -= 1;
        // Only the v2 singleton in-place body edit can prove that this gate
        // precedes every possible mutation in the cell.
        const targetPath = attempt.write.path;
        const original = activeSnapshot.files?.[targetPath];
        const zeroWriteTarget =
          deps.beforePeerMutation &&
          planned === 1 &&
          !op.inject &&
          op.op === "edit-body" &&
          attempt.write.method === "in-place" &&
          (attempt.writes?.length ?? 1) === 1 &&
          original
            ? fs.readPeerTarget(targetPath, original)
            : null;
        const recBase = {
          ...base,
          token: attempt.token,
          targetPath: attempt.targetPath,
          predicate: attempt.predicate,
          write: {
            method: attempt.write.method,
            path: attempt.write.path,
            to: attempt.write.to ?? null,
          },
          sampleBefore: sample,
          inject: op.inject
            ? {
                kind: op.inject,
                expectedOutcome: INJECT_EXPECT[op.inject],
                injectedFrom: attempt.injectedFrom ?? null,
              }
            : null,
          targetSha256Before:
            zeroWriteTarget?.sha256 ?? fs.hashOf?.(attempt.injectedFrom ?? targetPath) ?? null,
          writeMethods: attempt.writes
            ? attempt.writes.map((w) => w.method)
            : [attempt.write.method],
          declared: attempt.declared ?? null,
        };

        // Diagnostic profiles only: trace lines from here on belong to this attempt's causal chain.
        const provMark = deps.provenance && server.provMark ? server.provMark() : null;
        const withProvenance = (rec) => {
          if (provMark === null) return;
          try {
            rec.provenance = attemptProvenance({
              server,
              mark: provMark,
              attempt,
              rec,
              pageFor: deps.provenance.pageFor ?? fs.pageFor ?? null,
              traceEnabled: deps.provenance.traceEnabled === true,
              renderMode: deps.provenance.renderMode ?? null,
            });
          } catch (error) {
            rec.provenance = { error: String(error?.stack ?? error) };
          }
        };

        if (attempt.mode && attempt.mode !== "standard") {
          current.armedToken = attempt.token;
          const specialCtx = {
            attempt,
            op,
            recBase,
            where,
            page,
            server,
            fs,
            clock,
            timeouts,
            limits,
            preparedState,
            signal,
          };
          let special;
          try {
            signal?.throwIfAborted();
            special = await runSpecialMode(attempt.mode, specialCtx);
          } catch (error) {
            // A throw after (some) writes must not lose that the workload changed the files.
            const steps = error?.steps ?? specialCtx.writeSteps ?? null;
            const noop = error?.code === RENAME_NOOP;
            const wrote =
              specialCtx.writeDone === true ||
              noop ||
              (Array.isArray(error?.steps) && error.steps.length > 0);
            const outcome = noop ? "rename-noop" : `${attempt.mode}-error`;
            special = {
              rec: {
                ...recBase,
                mode: attempt.mode,
                outcome,
                error: String(error?.stack ?? error),
                writePerformed: wrote,
                writeSteps: steps,
                ...(noop
                  ? { renameNoop: error.evidence ?? null, failedStep: error.failedStep ?? null }
                  : {}),
              },
              outcome,
              writeDone: wrote,
            };
          }
          current.writeDone = special.writeDone;
          const rec = special.rec;
          const outcome = special.outcome;
          if (rec.expect === "classify" && (outcome === "timeout" || outcome === "stale"))
            acc.expectedTimeouts += 1;
          rec.sampleAfter = server.sampleResources();
          withProvenance(rec);
          append(rec);
          count(acc, outcome, rec);
          if (PASS_OUTCOMES.has(outcome)) {
            acc.lastArmedUnknownOperation = null;
            continue;
          }
          acc.lastArmedUnknownOperation = {
            ...where,
            token: attempt.token,
            writeCompleted: special.writeDone === true,
          };
          acc.stoppedByClassifyTimeout =
            rec.expect === "classify" && (outcome === "timeout" || outcome === "stale");
          stop(outcome, where);
          break outer;
        }

        current.armedToken = attempt.token;
        let armed;
        try {
          armed = await stage("attempt arm", timeouts.attemptMs, attemptStart, () =>
            page.arm(attempt.token, attempt.predicate, attempt.watch ?? null),
          );
        } catch (error) {
          armed = { armed: false, error: String(error?.message ?? error), threw: true };
        }
        if (!armed.armed) {
          const outcome = armed.precondition ? "precondition-failed" : "arm-error";
          const r = {
            ...recBase,
            outcome,
            arm: armed,
            writePerformed: false,
            targetSha256After: fs.hashOf ? fs.hashOf(attempt.write.path) : null,
          };
          if (op.inject) r.controlPass = injectPass(op.inject, outcome, r);
          append(r);
          count(acc, outcome);
          if (r.controlPass) {
            acc.expectedControlPasses += 1;
            acc.expectedErrors += 1;
            acc.stoppedByExpectedControl = true;
          }
          if (op.inject && r.controlPass !== true) notFired(where, op.inject, `outcome ${outcome}`);
          else stop(outcome, where);
          break outer;
        }
        // arm already read this value before the first edit. Reuse it across consecutive
        // edits so a previous tokenized revision cannot become the cleanup baseline.
        if (verifyBodyRevision && !bodyRevision) {
          if (
            typeof armed.actual !== "string" ||
            !armed.actual.trim() ||
            (deps.requiredBodyBaseline && armed.actual !== deps.requiredBodyBaseline)
          )
            throw new Error("missing pre-edit rendered body baseline");
          if (deps.fixedPeer) {
            const sockets = [...page.peerSocketTracking.values()].filter(
              (row) => row.firstFrameNs !== null,
            );
            append({
              type: "fixed-peer-prewrite",
              docUUID: armed.docUUID,
              originalBody: armed.actual,
              armObservedNs: String(clock.now()),
              sockets,
            });
            if (!armed.docUUID || !sockets.length)
              throw new Error("peer update socket or original document unverified");
          }
          bodyRevision = {
            ...(deps.fixedPeer && { allowSameDocument: true }),
            path: attempt.targetPath,
            predicate: { kind: "text", selector: attempt.predicate.selector, equals: armed.actual },
          };
        }
        const preDocUUID = armed.docUUID;
        const logMark = server.logMark?.() ?? null;
        const serverPidBefore = server.pid();
        page.markAttempt();

        const unstable =
          page.preWriteInstability?.() ??
          (page.hasEarly?.(attempt.token)
            ? { reason: "notified-before-write", token: attempt.token }
            : null);
        if (unstable) {
          if (!signal?.aborted) await wait(page.disarm());
          append({
            ...recBase,
            outcome: "precondition-unstable",
            unstable,
            arm: armed,
            writePerformed: false,
            observerDisarmed: true,
          });
          count(acc, "precondition-unstable");
          stop("precondition-unstable", where);
          break outer;
        }

        signal?.throwIfAborted();
        deps.stageBudget?.(timeouts.attemptMs, attemptStart);
        diagnostic?.beforeMutation?.();
        diagnostic?.clamp("attempt write", timeouts.attemptMs, attemptStart);
        let t0;
        let tLast = null;
        let writeSteps = null;
        if (deps.beforePeerMutation) {
          try {
            if (deps.beforePeerDecision)
              await deps.beforePeerDecision({ attemptStart, attemptMs: timeouts.attemptMs });
            deps.beforePeerMutation();
            signal?.throwIfAborted();
            deps.stageBudget?.(timeouts.attemptMs, attemptStart);
          } catch (error) {
            let observerDisarmed = false,
              disarmError = null;
            if (!signal?.aborted) {
              try {
                await wait(page.disarm());
                observerDisarmed = true;
              } catch (e) {
                disarmError = String(e.message ?? e);
              }
            }
            append({
              ...recBase,
              outcome: "identity-rejected",
              phase: "before-mutation",
              error: String(error.message ?? error),
              writePerformed: false,
              observerDisarmed,
              disarmError,
            });
            acc.lastArmedUnknownOperation = null;
            count(acc, "identity-rejected");
            stop("identity-rejected", where);
            if (zeroWriteTarget) {
              let after, auditError;
              try {
                after = fs.readPeerTarget(targetPath, original);
              } catch (e) {
                auditError = String(e.message ?? e);
              }
              const unchanged =
                after &&
                original.sha256 === zeroWriteTarget.sha256 &&
                after.sha256 === original.sha256 &&
                after.device === zeroWriteTarget.device &&
                after.inode === zeroWriteTarget.inode;
              append({
                type: "fixed-peer-refused-disk-audit",
                path: targetPath,
                expectedSha256: original.sha256,
                before: zeroWriteTarget,
                after: after ?? null,
                error: auditError ?? null,
                unchanged: Boolean(unchanged),
              });
              if (unchanged) {
                append({
                  type: "cleanup",
                  label: "cleanup",
                  measured: false,
                  reason: "after-stop:identity-rejected",
                  ok: false,
                  restored: false,
                  live: [],
                  liveSkipped: "before-mutation-no-write",
                  writePerformed: false,
                  actions: [],
                  mismatches: [],
                });
                activeSnapshot = null;
              }
            }
            break outer;
          }
        }
        try {
          if (attempt.writes && attempt.writes.length > 1 && fs.performWrites) {
            const r = await fs.performWrites(attempt.writes, () => clock.now(), signal);
            t0 = r.t0;
            tLast = r.tLast;
            writeSteps = r.steps;
          } else t0 = fs.performWrite(attempt.write, () => clock.now());
          current.writeDone = true;
          attempt.applied?.();
        } catch (error) {
          if (error?.steps?.length) {
            current.writeDone = true;
            recBase.writeSteps = error.steps;
            recBase.failedStep = error.failedStep;
          }
          if (!signal?.aborted) await wait(page.disarm());
          acc.lastArmedUnknownOperation = {
            ...where,
            token: attempt.token,
            writeCompleted: "unknown",
          };
          // A rename whose directory entry did not change is a no-op, never a write that happened.
          const writeOutcome = error?.code === RENAME_NOOP ? "rename-noop" : "write-error";
          const wr = { ...recBase, outcome: writeOutcome };
          if (writeOutcome === "rename-noop") {
            wr.renameNoop = error.evidence ?? null;
            wr.failedStep = error.failedStep ?? null;
          }
          if (op.inject) wr.controlPass = injectPass(op.inject, "write-error", wr);
          if (wr.controlPass) {
            acc.expectedControlPasses += 1;
            acc.expectedErrors += 1;
            acc.stoppedByExpectedControl = true;
          }
          append({
            ...wr,
            error: String(error?.message ?? error),
            writePerformed: "unknown",
            observerDisarmed: true,
            nativeWorkCancelled: false,
          });
          count(acc, writeOutcome);
          if (op.inject && wr.controlPass !== true)
            notFired(where, op.inject, `outcome ${writeOutcome}`);
          else stop(writeOutcome, where);
          break outer;
        }

        let injectKill = null;
        if (op.inject === "kill-server-during-wait") injectKill = server.injectKill();
        const late = [];
        const waitSamples = {
          count: 0,
          peakSummedRssKiB: 0,
          everyMs: limits.sampleEveryMs ?? 1000,
        };
        let sampler = null;
        const resourceTrip = diagnostic
          ? null
          : new Promise((resolve) => {
              sampler = setInterval(async () => {
                const s = server.sampleResourcesAsync
                  ? await server.sampleResourcesAsync()
                  : server.sampleResources();
                waitSamples.count += 1;
                if (typeof s?.summedRssKiB === "number")
                  waitSamples.peakSummedRssKiB = Math.max(
                    waitSamples.peakSummedRssKiB,
                    s.summedRssKiB,
                  );
                const wall = nsToMs(clock.now() - startedNs);
                const trip = overLimits(limits, s, wall);
                if (trip)
                  resolve({ kind: "resource-limit", sample: s, tripped: trip, wallMs: wall });
              }, waitSamples.everyMs);
            });
        let waited, peerDecision;
        try {
          const timeoutMs = diagnostic
            ? diagnostic.clamp("attempt observation", timeouts.attemptMs, attemptStart)
            : (deps.stageBudget?.(timeouts.attemptMs, attemptStart) ?? timeouts.attemptMs);
          waited = await wait(
            Promise.race([
              page.waitNotify(attempt.token, timeoutMs, (payload) => {
                late.push(payload);
                acc.lateTokens += 1;
                append({
                  type: "late-token",
                  label: "not-success",
                  armedToken: attempt.token,
                  payload,
                });
              }),
              server.waitExit().then((exit) => ({ kind: "server-exit", exit })),
              ...(resourceTrip ? [resourceTrip] : []),
            ]),
          );
          if (deps.fixedPeer) {
            signal?.throwIfAborted();
            const attemptEnd = attemptStart + BigInt(timeouts.attemptMs) * 1_000_000n;
            const end = deps.workDeadlineNs < attemptEnd ? deps.workDeadlineNs : attemptEnd;
            peerDecision = {
              deadlineNs: String(end),
              dispatchNs: String(clock.now()),
              kind: waited.kind,
            };
            if (waited.kind === "notify" && waited.t1 > end) {
              peerDecision.lateNotify = { receivedNs: String(waited.t1), payload: waited.payload };
              waited = { kind: "timeout" };
            }
          }
        } finally {
          clearInterval(sampler);
        }
        const evidence = page.collectAttempt(t0);
        let classification = evidence;
        if (peerDecision) {
          const cutoff = waited.kind === "notify" ? waited.t1 : BigInt(peerDecision.deadlineNs);
          const during = (rows) =>
            rows.filter((row) => BigInt(row.ns) >= t0 && BigInt(row.ns) <= cutoff);
          classification = {
            mainFrameNavigations: during(evidence.navigations).length,
            newDocuments: during(evidence.docInits).length,
            livereloadEvents: during(evidence.wsFrames).map((f) => f.event),
          };
          peerDecision.window = {
            fromNs: String(t0),
            throughNs: String(cutoff),
            ...classification,
          };
        }
        const serverPidAfter = server.pid();
        const serverLog = logMark === null ? null : server.logSince(logMark, t0);

        let outcome;
        const rec = {
          ...recBase,
          preDocUUID,
          ...(peerDecision && { peerDecision }),
          t0Ns: String(t0),
          serverPidBefore,
          serverPidAfter,
          evidence,
          writeEvidence: attempt.write.evidence ?? null,
          serverLog,
          lateTokensDuringAttempt: late.length,
          waitSamples: {
            ...waitSamples,
            label: "sampled during the armed wait; not a lifetime peak",
          },
          injectKill,
        };
        if (deps.fixedPeer) {
          const write = attempt.write;
          const expected = sha256(write.bytes);
          const actual = fs.hashOf(write.path);
          peerWrite = {
            path: write.path,
            originalSha256: rec.targetSha256Before,
            inode: write.evidence?.inodeBefore,
          };
          rec.peerDisk = {
            bytes: Buffer.byteLength(write.bytes),
            expectedSha256: expected,
            observedSha256: actual,
            ok:
              /^[a-f0-9]{64}$/.test(peerWrite.originalSha256) &&
              expected === actual &&
              expected !== peerWrite.originalSha256 &&
              Number.isSafeInteger(peerWrite.inode) &&
              peerWrite.inode === write.evidence?.inodeAfter,
          };
          append({ type: "fixed-peer-edit-disk", ...peerWrite, ...rec.peerDisk });
          if (!rec.peerDisk.ok)
            throw new Error("fixed peer edited bytes or in-place inode mismatch");
        }
        if (writeSteps) {
          rec.writeSteps = writeSteps;
          rec.tLastWriteNs = String(tLast);
        }
        if (waited.kind === "notify" && !(waited.t1 > (tLast ?? t0))) {
          outcome = "precondition-unstable";
          rec.unstable = {
            reason: "notify-not-after-t0",
            t1Ns: String(waited.t1),
            notify: waited.payload,
          };
          if (!signal?.aborted) await wait(page.disarm());
          rec.observerDisarmed = true;
          acc.lastArmedUnknownOperation = { ...where, token: attempt.token, writeCompleted: true };
        } else if (waited.kind === "notify") {
          outcome = "success";
          rec.t1Ns = String(waited.t1);
          rec.latencyMs = nsToMs(waited.t1 - t0);
          if (tLast !== null) {
            rec.latencyFromFirstWriteMs = rec.latencyMs;
            rec.latencyFromLastWriteMs = nsToMs(waited.t1 - tLast);
          }
          rec.endpoint = "predicate confirmed + 2 rAF + binding delivery";
          rec.notify = waited.payload;
          if (attempt.watch) rec.watchSeen = waited.payload?.watchSeen ?? {};
          rec.mechanism = classifyMechanism({
            preDocUUID,
            notifyDocUUID: waited.payload.docUUID,
            mainFrameNavigations: classification.mainFrameNavigations,
            serverPidBefore,
            serverPidAfter,
            livereloadEvents: classification.livereloadEvents ?? [],
          });
          acc.lastArmedUnknownOperation = null;
        } else {
          if (waited.kind === "server-exit") {
            outcome = "server-exit";
            rec.exit = waited.exit;
          } else if (waited.kind === "resource-limit") {
            outcome = "resource-limit";
            rec.resource = {
              sample: waited.sample,
              tripped: waited.tripped,
              wallMs: waited.wallMs,
              limits,
            };
          } else {
            outcome = classification.newDocuments > 0 ? "stale" : "timeout";
            rec.timeoutMs = timeouts.attemptMs;
          }
          const disarmed = await wait(page.disarm());
          if (attempt.watch) rec.watchSeen = disarmed?.watchSeen ?? null;
          rec.observerDisarmed = true;
          rec.nativeWorkCancelled = false;
          acc.lastArmedUnknownOperation = { ...where, token: attempt.token, writeCompleted: true };
        }
        if (rec.expect === "timeout") {
          rec.controlPass = outcome === "timeout" || outcome === "stale";
          if (rec.controlPass) {
            acc.expectedControlPasses += 1;
            acc.expectedTimeouts += 1;
          }
        }
        if (rec.expect === "classify" && (outcome === "timeout" || outcome === "stale"))
          acc.expectedTimeouts += 1;
        rec.outcome = outcome;
        if (op.inject && op.inject !== "restore-blocked") {
          rec.controlPass = injectPass(op.inject, outcome, rec);
          if (rec.controlPass) {
            acc.expectedControlPasses += 1;
            acc.expectedErrors += ERROR_OUTCOMES.has(outcome) ? 1 : 0;
          }
        }
        rec.sampleAfter = server.sampleResources();
        withProvenance(rec);
        append(rec);
        // Post-apply validation can fail after a persisted attempt. Before
        // this point, a browser notification alone cannot resolve the write.
        current.resolved = true;
        count(acc, outcome);
        if (op.inject && op.inject !== "restore-blocked" && rec.controlPass !== true) {
          notFired(
            where,
            op.inject,
            `outcome ${outcome}${rec.injectKill && !rec.injectKill.ok ? `; kill refused: ${rec.injectKill.reason}` : ""}`,
          );
          break outer;
        }
        if (outcome !== "success") {
          acc.stoppedByExpectedControl = rec.controlPass === true;
          acc.stoppedByClassifyTimeout =
            rec.expect === "classify" && (outcome === "timeout" || outcome === "stale");
          stop(outcome, where);
          if (diagnostic) await diagnostic.afterAttempt(rec, attemptStart);
          break outer;
        }
        if (diagnostic) await diagnostic.afterAttempt(rec, attemptStart);
        if (rec.expect === "timeout") {
          stop("negative-control-unexpected-success", where);
          break outer;
        }

        const supplementalStart = diagnostic || deps.stageBudget ? clock.now() : null;
        for (const check of attempt.supplemental) {
          let res;
          try {
            res = await stage("supplemental", timeouts.supplementalMs, supplementalStart, (ms) =>
              page.supplemental(check, ms, { quietMs: timeouts.quietMs ?? 1500 }),
            );
          } catch (error) {
            res = { ok: false, error: String(error?.message ?? error) };
          }
          append({
            type: "supplemental",
            label: "supplemental",
            measured: false,
            ...where,
            token: attempt.token,
            check,
            ...res,
          });
          if (!res.ok) {
            acc.supplementalFailures += 1;
            acc.error += 1;
            stop("supplemental-failed", where);
            break outer;
          }
        }
        if (op.inject === "restore-blocked") {
          // Apply the block now, right after the injected op, so no later op runs on a blocked file.
          lastCleanupControlPass = null;
          await cleanup(activeSnapshot, "inject:restore-blocked", [attempt.write.path]);
          activeSnapshot = null;
          if (lastCleanupControlPass) stop("restoration-failed", where);
          else
            notFired(where, "restore-blocked", "the blocked restoration did not fail as declared");
          break outer;
        }
      }
      if (diagnostic?.afterEdits) {
        // Every edit has resolved. A boundary failure belongs to the scenario,
        // with no last armed edit to classify as unknown.
        current = {
          where: { scenario: scenario.id, op: null },
          armedToken: null,
          writeDone: false,
          resolved: false,
        };
        await diagnostic.afterEdits();
      }
      const ok = await cleanup(activeSnapshot, "scenario-end");
      activeSnapshot = null;
      if (!ok) {
        stop("restoration-failed", { scenario: scenario.id, op: null });
        break;
      }
      if (deps.afterScenario) await deps.afterScenario({ scenario });
    }
  } catch (error) {
    try {
      if (!signal?.aborted) await wait(page.disarm());
    } catch {
      // the page itself may be what failed; the record below carries the error
    }
    if (
      current.armedToken &&
      current.writeDone &&
      (!diagnostic?.requiresPostApply || !current.resolved)
    )
      acc.lastArmedUnknownOperation = {
        ...current.where,
        token: current.armedToken,
        writeCompleted: true,
      };
    append({
      type: "controller-error",
      outcome: "controller-error",
      where: current.where,
      armedToken: current.armedToken,
      writePerformed: current.writeDone,
      error: String(error?.stack ?? error),
    });
    acc.error += 1;
    if (!stopped) stop(diagnostic?.failure?.reason ?? "controller-error", current.where);
  }

  if (activeSnapshot) {
    let ok = false;
    try {
      ok = await cleanup(activeSnapshot, `after-stop:${acc.stopReason}`);
    } catch (error) {
      append({
        type: "cleanup",
        label: "cleanup",
        measured: false,
        reason: `after-stop:${acc.stopReason}`,
        ok: false,
        error: String(error?.stack ?? error),
      });
      acc.restorationFailures += 1;
      acc.error += 1;
    }
    if (!ok && acc.stopReason !== "restoration-failed")
      acc.stopReason = `${acc.stopReason}+restoration-failed`;
  }
  append({ type: "batch-end", stopped, accounting: acc });
  return { accounting: acc, stopped };
}
