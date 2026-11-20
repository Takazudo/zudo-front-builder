import { nsToMs } from "./util.mjs";

export const OUTCOMES = [
  "success",
  "timeout",
  "stale",
  "precondition-failed",
  "arm-error",
  "prepare-error",
  "write-error",
  "server-exit",
  "resource-limit",
  "supplemental-failed",
  "restoration-failed",
  "identity-rejected",
];

const ERROR_OUTCOMES = new Set(
  OUTCOMES.filter((o) => !["success", "timeout", "stale"].includes(o)),
);

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
    restorationFailures: 0,
    supplementalFailures: 0,
    lastArmedUnknownOperation: null,
    stoppedByExpectedControl: false,
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
}) {
  if (serverPidBefore !== serverPidAfter) return "server-restart";
  if (notifyDocUUID !== preDocUUID && mainFrameNavigations > 0) return "native-full-reload";
  if (notifyDocUUID === preDocUUID && mainFrameNavigations === 0) return "in-document-update";
  return "unclassified";
}

function count(acc, outcome) {
  if (outcome === "success") acc.success += 1;
  else if (outcome === "timeout") acc.timeout += 1;
  else if (outcome === "stale") acc.stale += 1;
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
  const startedNs = deps.startedNs ?? clock.now();
  let activeSnapshot = null;
  let stopped = false;

  const append = (rec) => recorder.append(rec);

  const stop = (reason, where) => {
    acc.stopReason = reason;
    acc.stoppedAfter = where;
    stopped = true;
  };

  const cleanup = async (snapshot, reason) => {
    const restore = fs.restore(snapshot);
    const alive = server.isAlive();
    const live = [];
    let liveSkipped = null;
    if (restore.mismatches.length > 0) liveSkipped = "hash-mismatch";
    else if (!alive.alive) liveSkipped = "server-not-alive";
    else {
      for (const check of fs.liveChecks(snapshot)) {
        try {
          live.push({ ...check, ...(await page.liveCheck(check, timeouts.restoreMs)) });
        } catch (error) {
          live.push({ ...check, ok: false, error: String(error?.message ?? error) });
        }
      }
    }
    const ok = restore.mismatches.length === 0 && live.every((l) => l.ok);
    append({
      type: "cleanup",
      label: "cleanup",
      measured: false,
      reason,
      ok,
      restored: restore.restored,
      mismatches: restore.mismatches,
      live,
      liveSkipped,
    });
    if (!ok) {
      acc.restorationFailures += 1;
      acc.error += 1;
    }
    return ok;
  };

  outer: for (const scenario of scenarios) {
    activeSnapshot = fs.snapshot();
    append({
      type: "scenario-start",
      scenario: scenario.id,
      ops: scenario.ops.map((o) => o.id ?? o.op),
    });
    for (const op of scenario.ops) {
      const where = { scenario: scenario.id, op: op.id ?? op.op };
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
      const overRss =
        limits.rssCeilingKiB &&
        typeof sample?.summedRssKiB === "number" &&
        sample.summedRssKiB > limits.rssCeilingKiB;
      const overWall = limits.wallCeilingMs && wallMs > limits.wallCeilingMs;
      if (overRss || overWall) {
        append({
          ...base,
          type: "precheck",
          outcome: "resource-limit",
          sample,
          wallMs,
          limits,
          tripped: overRss ? "rss" : "wall",
        });
        acc.error += 1;
        stop("resource-limit", where);
        break outer;
      }

      let attempt;
      try {
        attempt = fs.prepareAttempt(op);
        await page.prepare(attempt.targetPath);
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
      };

      const armed = await page.arm(attempt.token, attempt.predicate);
      if (!armed.armed) {
        const outcome = armed.precondition ? "precondition-failed" : "arm-error";
        append({ ...recBase, outcome, arm: armed, writePerformed: false });
        count(acc, outcome);
        stop(outcome, where);
        break outer;
      }
      const preDocUUID = armed.docUUID;
      const logMark = server.logMark?.() ?? null;
      const serverPidBefore = server.pid();
      page.markAttempt();

      let t0;
      try {
        t0 = fs.performWrite(attempt.write, () => clock.now());
      } catch (error) {
        await page.disarm();
        acc.lastArmedUnknownOperation = {
          ...where,
          token: attempt.token,
          writeCompleted: "unknown",
        };
        append({
          ...recBase,
          outcome: "write-error",
          error: String(error?.message ?? error),
          writePerformed: "unknown",
          observerDisarmed: true,
          nativeWorkCancelled: false,
        });
        count(acc, "write-error");
        stop("write-error", where);
        break outer;
      }

      const late = [];
      const waited = await Promise.race([
        page.waitNotify(attempt.token, timeouts.attemptMs, (payload) => {
          late.push(payload);
          acc.lateTokens += 1;
          append({ type: "late-token", label: "not-success", armedToken: attempt.token, payload });
        }),
        server.waitExit().then((exit) => ({ kind: "server-exit", exit })),
      ]);
      const evidence = page.collectAttempt();
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
      };
      if (waited.kind === "notify") {
        outcome = "success";
        rec.t1Ns = String(waited.t1);
        rec.latencyMs = nsToMs(waited.t1 - t0);
        rec.endpoint = "predicate confirmed + 2 rAF + binding delivery";
        rec.notify = waited.payload;
        rec.mechanism = classifyMechanism({
          preDocUUID,
          notifyDocUUID: waited.payload.docUUID,
          mainFrameNavigations: evidence.mainFrameNavigations,
          serverPidBefore,
          serverPidAfter,
        });
        acc.lastArmedUnknownOperation = null;
      } else {
        if (waited.kind === "server-exit") {
          outcome = "server-exit";
          rec.exit = waited.exit;
        } else {
          outcome = evidence.newDocuments > 0 ? "stale" : "timeout";
          rec.timeoutMs = timeouts.attemptMs;
        }
        await page.disarm();
        rec.observerDisarmed = true;
        rec.nativeWorkCancelled = false;
        acc.lastArmedUnknownOperation = { ...where, token: attempt.token, writeCompleted: true };
      }
      if (rec.expect !== "success") {
        rec.controlPass =
          outcome === rec.expect || (rec.expect === "timeout" && outcome === "stale");
        if (rec.controlPass) acc.expectedControlPasses += 1;
      }
      rec.outcome = outcome;
      rec.sampleAfter = server.sampleResources();
      append(rec);
      count(acc, outcome);
      if (outcome !== "success") {
        acc.stoppedByExpectedControl = rec.controlPass === true;
        stop(outcome, where);
        break outer;
      }
      if (rec.expect !== "success") {
        stop("negative-control-unexpected-success", where);
        break outer;
      }

      for (const check of attempt.supplemental) {
        let res;
        try {
          res = await page.supplemental(check, timeouts.supplementalMs);
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
    }
    const ok = await cleanup(activeSnapshot, "scenario-end");
    activeSnapshot = null;
    if (!ok) {
      stop("restoration-failed", { scenario: scenario.id, op: null });
      break;
    }
  }

  if (activeSnapshot) {
    const ok = await cleanup(activeSnapshot, `after-stop:${acc.stopReason}`);
    if (!ok && acc.stopReason !== "restoration-failed")
      acc.stopReason = `${acc.stopReason}+restoration-failed`;
  }
  append({ type: "batch-end", stopped, accounting: acc });
  return { accounting: acc, stopped };
}
