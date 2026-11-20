import { distribution } from "./stats.mjs";
import { nsToMs } from "./util.mjs";

export const CALIBRATION_CELLS = [
  "binding-round-trip",
  "two-raf",
  "synthetic-dom",
  "synthetic-css",
  "observer-init-after-navigation",
  "stale-then-new-token",
  "late-token",
  "reload-survival-controller-triggered",
  "native-reload-survival",
];

const tok = () => `cal-${Math.random().toString(16).slice(2, 12)}`;

async function syntheticTrial(observer, kind, timeoutMs) {
  const token = tok();
  const predicate =
    kind === "dom"
      ? { kind: "text", selector: `[data-dbo-synthetic="${token}"]`, equals: token }
      : { kind: "computed-style", selector: ":root", property: "--dbo-synthetic", equals: token };
  const armed = await observer.arm(token, predicate);
  if (!armed.armed) return { ok: false, armed };
  observer.markAttempt();
  const t0 = observer.clock();
  if (kind === "dom") {
    await observer.page.evaluate((t) => {
      const el = document.createElement("div");
      el.setAttribute("data-dbo-synthetic", t);
      el.textContent = t;
      document.body.appendChild(el);
    }, token);
  } else {
    await observer.page.evaluate((t) => {
      const style = document.createElement("style");
      style.textContent = `:root { --dbo-synthetic: ${t}; }`;
      document.head.appendChild(style);
    }, token);
  }
  const waited = await observer.waitNotify(token, timeoutMs);
  if (waited.kind !== "notify") await observer.disarm();
  return {
    ok: waited.kind === "notify",
    ms: waited.kind === "notify" ? nsToMs(waited.t1 - t0) : null,
    token,
    evidence: observer.collectAttempt(),
  };
}

/**
 * A real, unmeasured (calibration-labelled) in-place body edit of article b
 * while armed on its page. Passes only on a new document with navigationType
 * "reload" and a livereload `page` frame on the WebSocket, then restores the
 * file and confirms the original state live.
 */
export async function nativeReloadSurvival({
  observer,
  fixtureOps,
  timeoutMs,
  restoreMs,
  quietMs,
}) {
  const snap = fixtureOps.snapshot();
  const target = fixtureOps.route("article", "b");
  const state = await observer.prepare(target);
  const attempt = fixtureOps.prepareAttempt({
    op: "edit-body",
    article: "b",
    writeMethod: "in-place",
  });
  const armed = await observer.arm(attempt.token, attempt.predicate);
  observer.markAttempt();
  let t0 = null;
  let writeError = null;
  if (armed.armed) {
    try {
      t0 = fixtureOps.performWrite(attempt.write, () => observer.clock());
    } catch (error) {
      writeError = String(error?.message ?? error);
    }
  }
  const waited =
    t0 === null ? { kind: "not-written" } : await observer.waitNotify(attempt.token, timeoutMs);
  const evidence = observer.collectAttempt(t0);
  if (waited.kind !== "notify") await observer.disarm();
  const restore = fixtureOps.restore(snap);
  const live = [];
  if (restore.mismatches.length === 0) {
    for (const check of fixtureOps.liveChecks(snap)) {
      try {
        live.push({ ...check, ...(await observer.liveCheck(check, restoreMs)) });
      } catch (error) {
        live.push({ ...check, ok: false, error: String(error?.message ?? error) });
      }
    }
  }
  const restorationOk = restore.mismatches.length === 0 && live.every((l) => l.ok);
  const quiet = restorationOk ? await observer.quietWindow(quietMs) : null;
  const notify = waited.kind === "notify" ? waited.payload : null;
  return {
    ok:
      armed.armed &&
      notify !== null &&
      notify.docUUID !== state.docUUID &&
      notify.navigationType === "reload" &&
      evidence.livereloadEvents.includes("page") &&
      restorationOk,
    ms: notify ? nsToMs(waited.t1 - t0) : null,
    calibrationEdit: {
      label: "calibration edit (unmeasured)",
      method: attempt.write.method,
      path: attempt.write.path,
      token: attempt.token,
      writeEvidence: attempt.write.evidence ?? null,
      writeError,
    },
    preDocUUID: state.docUUID,
    notifyDocUUID: notify?.docUUID ?? null,
    navigationType: notify?.navigationType ?? null,
    outcome: waited.kind,
    evidence,
    restorationOk,
    restoration: {
      writeMethod: restore.writeMethod,
      actions: restore.actions,
      mismatches: restore.mismatches,
      live,
      quiet,
    },
    measures:
      "real in-place edit -> livereload page frame -> native reload -> armed predicate in the new document",
  };
}

/**
 * Calibration cells measured through the same observer. Raw distributions are
 * recorded and never subtracted from workload latencies. Controller-triggered
 * navigation/reload happens only here, never inside a measured attempt.
 */
export async function runCalibration({
  observer,
  append,
  cells,
  repeats,
  timeoutMs,
  staleTimeoutMs,
  pagePath,
  fixtureOps = null,
  restoreMs = 30000,
  quietMs = 1500,
}) {
  const results = {};
  const record = (cell, i, res) =>
    append({ type: "calibration", label: "calibration", cell, i, ...res });
  for (const cell of cells) {
    const values = [];
    let failures = 0;
    for (let i = 0; i < repeats; i += 1) {
      let res;
      if (cell === "binding-round-trip" || cell === "two-raf") {
        await observer.prepare(pagePath);
        const p = await observer.ping(cell === "two-raf" ? "two-raf" : "direct", timeoutMs);
        res = {
          ok: p.ok,
          ms: p.ms,
          timedOut: p.timedOut ?? false,
          docUUID: p.payload?.docUUID ?? null,
          measures: cell === "two-raf" ? "command -> 2 rAF -> binding" : "command -> binding",
        };
      } else if (cell === "synthetic-dom" || cell === "synthetic-css") {
        await observer.prepare(pagePath);
        res = {
          ...(await syntheticTrial(observer, cell === "synthetic-dom" ? "dom" : "css", timeoutMs)),
          measures: "controller evaluate -> change -> predicate + 2 rAF -> binding",
        };
      } else if (cell === "observer-init-after-navigation") {
        const next = observer.nextDocInit();
        const t0 = observer.clock();
        await observer.page.goto(observer.url(pagePath), { waitUntil: "commit" });
        const { rec, t } = await next;
        res = {
          ok: true,
          ms: nsToMs(t - t0),
          docUUID: rec.docUUID,
          measures: "goto start -> observer docInit binding",
        };
        await observer.page.waitForLoadState("load");
      } else if (cell === "stale-then-new-token") {
        await observer.prepare(pagePath);
        const stale = tok();
        const armed = await observer.arm(stale, {
          kind: "text",
          selector: "[data-dbo-never]",
          equals: stale,
        });
        const waited = await observer.waitNotify(stale, staleTimeoutMs);
        await observer.disarm();
        const fresh = await syntheticTrial(observer, "dom", timeoutMs);
        res = {
          ok: armed.armed && waited.kind === "timeout" && fresh.ok,
          staleOutcome: waited.kind,
          staleExpected: "timeout",
          staleTimeoutMs,
          freshTokenOk: fresh.ok,
          ms: fresh.ms,
          measures: "stale token times out (expected), then a new token succeeds",
        };
      } else if (cell === "late-token") {
        await observer.prepare(pagePath);
        const first = tok();
        const predicate = {
          kind: "text",
          selector: `[data-dbo-synthetic="${first}"]`,
          equals: first,
        };
        await observer.arm(first, predicate);
        const late = [];
        const second = tok();
        const secondWait = observer.waitNotify(second, timeoutMs, (p) => late.push(p));
        await observer.page.evaluate((t) => {
          const el = document.createElement("div");
          el.setAttribute("data-dbo-synthetic", t);
          el.textContent = t;
          document.body.appendChild(el);
        }, first);
        const waited = await secondWait;
        await observer.disarm();
        res = {
          ok: late.some((p) => p.token === first) && waited.kind === "timeout",
          lateTokens: late.map((p) => p.token),
          secondOutcome: waited.kind,
          measures:
            "a real delivery for a token the controller no longer waits for is recorded late, never success",
        };
      } else if (cell === "native-reload-survival") {
        res = await nativeReloadSurvival({ observer, fixtureOps, timeoutMs, restoreMs, quietMs });
        if (res.restorationOk === false) {
          record(cell, i, res);
          results[cell] = {
            repeats,
            failures: failures + 1,
            distributionMs: distribution(values),
            aborted: "restoration-failed",
          };
          results.aborted = { cell, i, reason: "restoration-failed" };
          return results;
        }
      } else if (cell === "reload-survival-controller-triggered") {
        const state = await observer.prepare(pagePath);
        const token = tok();
        const armed = await observer.arm(token, {
          kind: "document-uuid-not",
          value: state.docUUID,
        });
        observer.markAttempt();
        const t0 = observer.clock();
        await observer.page.evaluate(() => {
          setTimeout(() => location.reload(), 0);
        });
        const waited = await observer.waitNotify(token, timeoutMs);
        const evidence = observer.collectAttempt();
        if (waited.kind !== "notify") await observer.disarm();
        res = {
          ok:
            armed.armed &&
            waited.kind === "notify" &&
            waited.payload.docUUID !== state.docUUID &&
            evidence.mainFrameNavigations > 0,
          ms: waited.kind === "notify" ? nsToMs(waited.t1 - t0) : null,
          preDocUUID: state.docUUID,
          notifyDocUUID: waited.payload?.docUUID ?? null,
          navigationType: waited.payload?.navigationType ?? null,
          evidence,
          measures:
            "expectation armed before a controller-triggered reload is satisfied only in the new document",
        };
      } else {
        throw new Error(`unknown calibration cell ${cell}`);
      }
      if (!res.ok) failures += 1;
      if (typeof res.ms === "number") values.push(res.ms);
      record(cell, i, res);
    }
    results[cell] = { repeats, failures, distributionMs: distribution(values) };
  }
  return results;
}
