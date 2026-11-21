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
 * Classify one observed page of an expected-negative op, with its basis:
 * - "error": a 5xx, or a declared error marker in the page text/title;
 * - "route-not-expanded": zfb dev's 404 "page not in cache" for this URL
 *   (the route was not expanded, e.g. paths() threw or no longer lists it);
 * - "empty-collection": a 200 page whose declared empty probe matches
 *   (the content snapshot failed and getCollection() returned nothing);
 * - "last-good": a 200 page still rendering the original content;
 * - "navigation-error" or "other" otherwise.
 */
export function classifyNegative(view, { lastGood = null, empty = null, errorMarkers = [] } = {}) {
  if (!view.ok)
    return { state: "navigation-error", basis: `navigation failed: ${view.error ?? "unknown"}` };
  const excerpt = view.bodyExcerpt ?? "";
  const status = view.httpStatus ?? null;
  if ((status ?? 0) >= 500) return { state: "error", basis: `http-${status}` };
  // markersFound is computed over the page's full innerText and title; the excerpt is only a fallback.
  const marker = Array.isArray(view.markersFound)
    ? errorMarkers.find((m) => view.markersFound.includes(m))
    : errorMarkers.find((m) => excerpt.includes(m) || (view.title ?? "").includes(m));
  if (marker) return { state: "error", basis: `error-marker: ${marker} (http-${status})` };
  if (status === 404 && view.notInCache === true)
    return { state: "route-not-expanded", basis: "http-404 page not in cache" };
  if (status === 200 && empty?.matched === true)
    return {
      state: "empty-collection",
      basis: `http-200 empty probe matched (${JSON.stringify(empty.actual ?? null)})`,
    };
  if (status === 200 && lastGood?.matched === true)
    return { state: "last-good", basis: "http-200 original content predicate matched" };
  return {
    state: "other",
    basis: `http-${status} notInCache=${view.notInCache ?? null} lastGood=${lastGood?.matched ?? null} empty=${empty?.matched ?? null}`,
  };
}

export function classifyNegativeState(view, lastGood, errorMarkers = [], empty = null) {
  return classifyNegative(view, { lastGood, empty, errorMarkers }).state;
}

/**
 * hidden-edit: edit an article the browser never visited while the active
 * page is elsewhere; record active-page effects for `observeMs`, wait
 * `accessDelayMs` (not update latency), then navigate and time the first
 * correct display. Request evidence shows whether anything fetched it early.
 */
export function pathnameOf(url) {
  try {
    return new URL(url).pathname;
  } catch {
    return null;
  }
}

/** hidden-edit outcome from the navigation check's stage; stale only when a rendered document showed the old body. */
export function hiddenOutcome(nav) {
  if (nav.ok) return "success";
  if (nav.stage === "transient") return "transient-not-rendered";
  if (nav.stage === "arm") return "arm-error";
  if (nav.stage === "navigation" || (nav.error && nav.stage === undefined)) return "prepare-error";
  if (nav.stage === "wait") return "stale";
  return "hidden-check-failed";
}

async function runHidden(ctx) {
  const { attempt, page, fs, clock, timeouts, recBase } = ctx;
  // Run-time guard: our page must never have requested the hidden article before the edit.
  const visited = (page.requests ?? []).filter((r) => pathnameOf(r.url) === attempt.hiddenPath);
  if (visited.length > 0) {
    return {
      rec: {
        ...recBase,
        mode: "hidden",
        outcome: "hidden-precondition-visited",
        visitedRequests: visited,
        writePerformed: false,
      },
      outcome: "hidden-precondition-visited",
      writeDone: false,
    };
  }
  const mark = page.mark();
  const pre = ctx.preparedState ?? null;
  const { t0, steps } = await doWrites(fs, attempt.writes, clock);
  ctx.writeDone = true;
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
  const outcome = hiddenOutcome(nav);
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
  ctx.writeDone = true;
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
      checks.push({
        check,
        ...(await page.supplemental(check, timeouts.supplementalMs, {
          quietMs: timeouts.quietMs ?? 1500,
          nativeUpdateConfirmed: settle.settled === true && settle.nativeUpdateObserved === true,
        })),
      });
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
  ctx.writeDone = true;
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
    const view = await page.observePage(o.path, { errorMarkers: attempt.errorMarkers });
    const probe = async (pred) => {
      if (!pred || !view.ok) return null;
      try {
        return await page.evaluateNow(pred);
      } catch (error) {
        return { matched: false, error: String(error?.message ?? error) };
      }
    };
    const lastGood = await probe(o.lastGood);
    const empty = await probe(o.empty);
    const c = classifyNegative(view, { lastGood, empty, errorMarkers: attempt.errorMarkers });
    observed.push({ path: o.path, state: c.state, basis: c.basis, view, lastGood, empty });
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
          ...(await page.supplemental(check, op.recoveryTimeoutMs ?? timeouts.restoreMs, {
            quietMs: timeouts.quietMs ?? 1500,
          })),
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
      observedSummary: observed.map((o) => ({
        path: o.path,
        state: o.state,
        basis: o.basis,
        httpStatus: o.view.httpStatus ?? null,
        expected: attempt.expectState.includes(o.state),
      })),
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
