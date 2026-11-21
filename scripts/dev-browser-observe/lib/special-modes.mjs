import { nsToMs } from "./util.mjs";

const wait = (ms) => new Promise((r) => setTimeout(r, ms));

async function doWrites(fs, writes, clock) {
  if (writes.length > 1 && fs.performWrites) return fs.performWrites(writes, () => clock.now());
  const t0 = fs.performWrite(writes[0], () => clock.now());
  return {
    t0,
    tLast: t0,
    steps: [
      {
        i: 0,
        method: writes[0].method,
        path: writes[0].path,
        to: writes[0].to ?? null,
        tNs: String(t0),
        evidence: writes[0].evidence ?? null,
      },
    ],
  };
}

/**
 * Classify one observed page of an expected-negative op:
 * "error" (5xx, or a declared error marker in the page text), "last-good"
 * (200 and the original content still renders), "navigation-error", or "other".
 */
export function classifyNegativeState(view, lastGood, errorMarkers = []) {
  if (!view.ok) return "navigation-error";
  const excerpt = view.bodyExcerpt ?? "";
  if ((view.httpStatus ?? 0) >= 500) return "error";
  if (errorMarkers.some((m) => excerpt.includes(m) || (view.title ?? "").includes(m)))
    return "error";
  if (view.httpStatus === 200 && lastGood?.matched === true) return "last-good";
  return "other";
}

/**
 * hidden-edit: edit an article the browser never visited while the active
 * page is elsewhere; record active-page effects for `observeMs`, wait
 * `accessDelayMs` (not update latency), then navigate and time the first
 * correct display. Request evidence shows whether anything fetched it early.
 */
async function runHidden(ctx) {
  const { attempt, page, fs, clock, timeouts, recBase } = ctx;
  const mark = page.mark();
  const pre = ctx.preparedState ?? null;
  const { t0, steps } = await doWrites(fs, attempt.writes, clock);
  attempt.applied?.();
  await wait(attempt.observeMs);
  const activeEffects = page.since(mark, t0);
  await wait(attempt.accessDelayMs);
  const beforeNav = page.since(mark, t0);
  const prefetchRequests = beforeNav.requests.filter((r) => {
    try {
      return new URL(r.url).pathname === attempt.hiddenPath;
    } catch {
      return false;
    }
  });
  const nav = await page.navigateAndAwait(
    attempt.hiddenPath,
    attempt.hiddenPredicate,
    timeouts.attemptMs,
    "hidden",
  );
  const outcome = nav.ok ? "success" : nav.error ? "prepare-error" : "stale";
  return {
    rec: {
      ...recBase,
      mode: "hidden",
      outcome,
      t0Ns: String(t0),
      writeSteps: steps,
      activePath: attempt.targetPath,
      activeDocUUIDBefore: pre?.docUUID ?? null,
      activeEffects: {
        ...activeEffects,
        reloaded: activeEffects.newDocuments > 0,
        observeMs: attempt.observeMs,
      },
      accessDelayMs: attempt.accessDelayMs,
      accessDelayNote: "declared wait before navigating; not update latency",
      prefetchRequests,
      firstDisplayAfterNavigationMs: nav.ok ? nav.sinceNavigationMs : null,
      endpoint: "navigation start -> correct body (first read or armed confirmation)",
      navigation: nav,
      writePerformed: true,
    },
    outcome,
    writeDone: true,
  };
}

/** final-state: writes whose net effect is the declared final state; settle, then check it. No latency claim. */
async function runFinalState(ctx) {
  const { attempt, page, fs, clock, timeouts, recBase } = ctx;
  const mark = page.mark();
  const { t0, tLast, steps } = await doWrites(fs, attempt.writes, clock);
  attempt.applied?.();
  const settle = await page.settle({
    mark,
    startNs: tLast,
    quietMs: timeouts.quietMs ?? 1500,
    requireNativeUpdate: true,
    rule: "require-native-update",
    boundMs: timeouts.attemptMs,
  });
  const checks = [];
  for (const check of attempt.finalChecks) {
    try {
      checks.push({ check, ...(await page.supplemental(check, timeouts.supplementalMs)) });
    } catch (error) {
      checks.push({ check, ok: false, error: String(error?.message ?? error) });
    }
  }
  const outcome = checks.every((c) => c.ok) ? "success" : "final-state-mismatch";
  return {
    rec: {
      ...recBase,
      mode: "final-state",
      outcome,
      t0Ns: String(t0),
      writeSteps: steps,
      declared: attempt.declared ?? null,
      settle,
      finalChecks: checks,
      endpoint: "final state after settle (no latency claim)",
      writePerformed: true,
    },
    outcome,
    writeDone: true,
  };
}

/**
 * expected-negative: an invalid edit whose declared expected state is an
 * error or the last good page. Observe each declared page, classify, then run
 * the recovery writes and require live behaviour back. An unexpected state or
 * a failed recovery is never relabelled as expected.
 */
async function runExpectedNegative(ctx) {
  const { attempt, page, fs, clock, timeouts, recBase, op } = ctx;
  const mark = page.mark();
  const { t0, tLast, steps } = await doWrites(fs, attempt.writes, clock);
  const settle = await page.settle({
    mark,
    startNs: tLast,
    quietMs: timeouts.quietMs ?? 1500,
    requireNativeUpdate: true,
    rule: "require-native-update",
    boundMs: op.settleMs ?? timeouts.restoreMs,
  });
  const observed = [];
  for (const o of attempt.observe) {
    const view = await page.observePage(o.path);
    let lastGood = null;
    try {
      lastGood = view.ok ? await page.evaluateNow(o.lastGood) : null;
    } catch (error) {
      lastGood = { matched: false, error: String(error?.message ?? error) };
    }
    observed.push({
      path: o.path,
      view,
      lastGood,
      state: classifyNegativeState(view, lastGood, attempt.errorMarkers),
    });
  }
  const statePass = observed.every((o) => attempt.expectState.includes(o.state));
  const r0 = clock.now();
  let recoverySteps = null;
  let recoveryError = null;
  try {
    recoverySteps = (await doWrites(fs, attempt.recovery, clock)).steps;
  } catch (error) {
    recoveryError = String(error?.message ?? error);
  }
  const recoveryChecks = [];
  if (!recoveryError) {
    for (const check of attempt.recoveryChecks) {
      try {
        recoveryChecks.push({
          check,
          ...(await page.supplemental(check, op.recoveryTimeoutMs ?? timeouts.restoreMs)),
        });
      } catch (error) {
        recoveryChecks.push({ check, ok: false, error: String(error?.message ?? error) });
      }
    }
  }
  const recoveryOk = !recoveryError && recoveryChecks.every((c) => c.ok);
  const outcome = !statePass
    ? "expected-negative-mismatch"
    : !recoveryOk
      ? "recovery-failed"
      : "expected-negative-observed";
  return {
    rec: {
      ...recBase,
      mode: "expected-negative",
      outcome,
      t0Ns: String(t0),
      writeSteps: steps,
      expectState: attempt.expectState,
      settle,
      observed,
      statePass,
      recovery: {
        steps: recoverySteps,
        error: recoveryError,
        checks: recoveryChecks,
        ok: recoveryOk,
        sinceRecoveryWriteMs: nsToMs(clock.now() - r0),
        note: "includes the check navigations",
      },
      writePerformed: true,
    },
    outcome,
    writeDone: true,
  };
}

export const SPECIAL_MODES = {
  hidden: runHidden,
  "final-state": runFinalState,
  "expected-negative": runExpectedNegative,
};

export function runSpecialMode(mode, ctx) {
  const fn = SPECIAL_MODES[mode];
  if (!fn) throw new Error(`unknown attempt mode ${mode}`);
  return fn(ctx);
}
