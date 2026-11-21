import { runSpecialMode } from "./special-modes.mjs";
import { nsToMs } from "./util.mjs";

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
];

const ERROR_OUTCOMES = new Set(
  OUTCOMES.filter(
    (o) => !["success", "timeout", "stale", "expected-negative-observed"].includes(o),
  ),
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
  if (!newDoc && mainFrameNavigations === 0) return css ? "css-replacement" : "in-document-update";
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

function count(acc, outcome) {
  if (outcome === "success") acc.success += 1;
  else if (outcome === "timeout") acc.timeout += 1;
  else if (outcome === "stale") acc.stale += 1;
  else if (outcome === "expected-negative-observed") acc.expectedNegativePasses += 1;
  else if (ERROR_OUTCOMES.has(outcome)) acc.error += 1;
}

/**
 * Fail-stop batch runner. Every non-success outcome is appended (and fsynced
 * by the recorder) BEFORE the batch stops; no further workload edit follows.
 * Cleanup restoration always runs afterwards and is never a measured trial.
 *
 * deps: { recorder, clock, server, page, fs, limits, timeouts, startedNs }
 */
export async function runBatch({ scenarios, deps }) {
  const { recorder, clock, server, page, fs, limits = {}, timeouts } = deps;
  const planned = scenarios.reduce((n, s) => n + s.ops.length, 0);
  const acc = newAccounting(planned);
  if (deps.accountingRef) deps.accountingRef.current = acc;
  const startedNs = deps.startedNs ?? clock.now();
  let activeSnapshot = null;
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
    const alive = server.isAlive();
    const live = [];
    let liveSkipped = null;
    let settle = null;
    if (restore.mismatches.length > 0) liveSkipped = "hash-mismatch";
    else if (!alive.alive) liveSkipped = "server-not-alive";
    else {
      if (page.settle && mark)
        settle = await page.settle({
          mark,
          startNs: restoreNs,
          quietMs: timeouts.quietMs ?? 1500,
          boundMs: timeouts.restoreMs,
          ...settleRule(restore.actions ?? [], fs.noAutoUpdatePaths ?? []),
        });
      for (const check of fs.liveChecks(snapshot)) {
        try {
          live.push({
            ...check,
            ...(await page.liveCheck(check, timeouts.restoreMs, {
              quietMs: timeouts.quietMs ?? 1500,
              nativeUpdateConfirmed:
                settle?.settled === true && settle?.nativeUpdateObserved === true,
            })),
          });
        } catch (error) {
          live.push({ ...check, ok: false, error: String(error?.message ?? error) });
        }
      }
    }
    const ok = restore.mismatches.length === 0 && live.every((l) => l.ok);
    let quiet = null;
    if (ok && liveSkipped === null && page.quietWindow)
      quiet = await page.quietWindow(timeouts.quietMs ?? 1500);
    const cleanupRec = {
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
      activeSnapshot = fs.snapshot();
      append({
        type: "scenario-start",
        scenario: scenario.id,
        ops: scenario.ops.map((o) => o.id ?? o.op),
        warmOrder: scenario.warmOrder ?? "none",
      });
      if (scenario.warmOrder && scenario.warmOrder !== "none") {
        const paths = fs.warmPaths(scenario);
        const visits = [];
        try {
          for (const path of paths) {
            const t = clock.now();
            const state = await page.prepare(path);
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
        current = { where, armedToken: null, writeDone: false, resolved: false };
        const base = {
          type: "attempt",
          label: "measured",
          scenario: scenario.id,
          opId: op.id ?? op.op,
          op: op.op,
          article: op.article ?? null,
          expect: op.expect ?? "success",
        };

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

        let attempt;
        let preparedState = null;
        try {
          attempt = fs.prepareAttempt(op);
          if (op.inject === "precondition-true") attempt.predicate = ALWAYS_TRUE_PREDICATE;
          if (op.inject === "write-error") {
            attempt.injectedFrom = attempt.write.path;
            attempt.write = { ...attempt.write, path: fs.missingSibling(attempt.write.path) };
            if (attempt.writes) attempt.writes = [attempt.write, ...attempt.writes.slice(1)];
          }
          preparedState = await page.prepare(attempt.targetPath);
          page.markPre?.();
        } catch (error) {
          append({
            ...base,
            type: "precheck",
            outcome: "prepare-error",
            error: String(error?.message ?? error),
          });
          acc.error += 1;
          stop("prepare-error", where);
          break outer;
        }

        acc.attempted += 1;
        acc.notAttempted -= 1;
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
          targetSha256Before: fs.hashOf
            ? fs.hashOf(attempt.injectedFrom ?? attempt.write.path)
            : null,
          writeMethods: attempt.writes
            ? attempt.writes.map((w) => w.method)
            : [attempt.write.method],
          declared: attempt.declared ?? null,
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
          };
          let special;
          try {
            special = await runSpecialMode(attempt.mode, specialCtx);
          } catch (error) {
            // A throw after (some) writes must not lose that the workload changed the files.
            const steps = error?.steps ?? specialCtx.writeSteps ?? null;
            const wrote =
              specialCtx.writeDone === true ||
              (Array.isArray(error?.steps) && error.steps.length > 0);
            special = {
              rec: {
                ...recBase,
                mode: attempt.mode,
                outcome: `${attempt.mode}-error`,
                error: String(error?.stack ?? error),
                writePerformed: wrote,
                writeSteps: steps,
              },
              outcome: `${attempt.mode}-error`,
              writeDone: wrote,
            };
          }
          current.writeDone = special.writeDone;
          const rec = special.rec;
          const outcome = special.outcome;
          if (rec.expect === "classify" && (outcome === "timeout" || outcome === "stale"))
            acc.expectedTimeouts += 1;
          rec.sampleAfter = server.sampleResources();
          append(rec);
          count(acc, outcome);
          if (outcome === "success" || outcome === "expected-negative-observed") {
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
          armed = await page.arm(attempt.token, attempt.predicate, attempt.watch ?? null);
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
          await page.disarm();
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

        let t0;
        let tLast = null;
        let writeSteps = null;
        try {
          if (attempt.writes && attempt.writes.length > 1 && fs.performWrites) {
            const r = await fs.performWrites(attempt.writes, () => clock.now());
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
          await page.disarm();
          acc.lastArmedUnknownOperation = {
            ...where,
            token: attempt.token,
            writeCompleted: "unknown",
          };
          const wr = { ...recBase, outcome: "write-error" };
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
          count(acc, "write-error");
          if (op.inject && wr.controlPass !== true)
            notFired(where, op.inject, "outcome write-error");
          else stop("write-error", where);
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
        const resourceTrip = new Promise((resolve) => {
          sampler = setInterval(async () => {
            const s = server.sampleResourcesAsync
              ? await server.sampleResourcesAsync()
              : server.sampleResources();
            waitSamples.count += 1;
            if (typeof s?.summedRssKiB === "number")
              waitSamples.peakSummedRssKiB = Math.max(waitSamples.peakSummedRssKiB, s.summedRssKiB);
            const wall = nsToMs(clock.now() - startedNs);
            const trip = overLimits(limits, s, wall);
            if (trip) resolve({ kind: "resource-limit", sample: s, tripped: trip, wallMs: wall });
          }, waitSamples.everyMs);
        });
        let waited;
        try {
          waited = await Promise.race([
            page.waitNotify(attempt.token, timeouts.attemptMs, (payload) => {
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
            resourceTrip,
          ]);
        } finally {
          clearInterval(sampler);
        }
        current.resolved = true;
        const evidence = page.collectAttempt(t0);
        const serverPidAfter = server.pid();
        const serverLog = logMark === null ? null : server.logSince(logMark, t0);

        let outcome;
        const rec = {
          ...recBase,
          preDocUUID,
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
          await page.disarm();
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
            mainFrameNavigations: evidence.mainFrameNavigations,
            serverPidBefore,
            serverPidAfter,
            livereloadEvents: evidence.livereloadEvents ?? [],
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
            outcome = evidence.newDocuments > 0 ? "stale" : "timeout";
            rec.timeoutMs = timeouts.attemptMs;
          }
          const disarmed = await page.disarm();
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
        append(rec);
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
          break outer;
        }
        if (rec.expect === "timeout") {
          stop("negative-control-unexpected-success", where);
          break outer;
        }

        for (const check of attempt.supplemental) {
          let res;
          try {
            res = await page.supplemental(check, timeouts.supplementalMs, {
              quietMs: timeouts.quietMs ?? 1500,
            });
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
      const ok = await cleanup(activeSnapshot, "scenario-end");
      activeSnapshot = null;
      if (!ok) {
        stop("restoration-failed", { scenario: scenario.id, op: null });
        break;
      }
    }
  } catch (error) {
    try {
      await page.disarm();
    } catch {
      // the page itself may be what failed; the record below carries the error
    }
    if (current.armedToken && current.writeDone)
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
    stop("controller-error", current.where);
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
