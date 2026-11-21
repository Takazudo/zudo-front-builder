import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import { HARNESS_DIR, hrtimeNs, isoNow, nsToMs, sha256File } from "./util.mjs";

export const OBSERVER_PATH = join(HARNESS_DIR, "observer-init.js");

export const DEFAULT_BROWSER = {
  headless: true,
  args: [
    "--disable-background-timer-throttling",
    "--disable-renderer-backgrounding",
    "--disable-backgrounding-occluded-windows",
  ],
  viewport: { width: 1280, height: 800 },
  deviceScaleFactor: 1,
};

function playwrightVersion() {
  try {
    return createRequire(import.meta.url)("@playwright/test/package.json").version;
  } catch {
    return "unavailable";
  }
}

/**
 * One owned Chromium (Playwright launch: fresh temporary profile), one
 * context, one page. Never touches the user's browser profile.
 */
export async function launchOwnedBrowser(
  opts = {},
  { chromiumImpl = null, observerFactory = null } = {},
) {
  const cfg = { ...DEFAULT_BROWSER, ...opts };
  const chromium = chromiumImpl ?? (await import("@playwright/test")).chromium;
  const executablePath = chromium.executablePath();
  const browser = await chromium.launch({ headless: cfg.headless, args: cfg.args });
  try {
    const context = await browser.newContext({
      viewport: cfg.viewport,
      deviceScaleFactor: cfg.deviceScaleFactor,
      serviceWorkers: "block",
    });
    const identity = {
      engine: "chromium",
      executablePath,
      executableSha256: sha256File(executablePath),
      version: browser.version(),
      headless: cfg.headless,
      args: cfg.args,
      viewport: cfg.viewport,
      deviceScaleFactor: cfg.deviceScaleFactor,
      profile: "playwright-temporary",
      playwrightVersion: playwrightVersion(),
      observerSha256: sha256File(OBSERVER_PATH),
    };
    const observer = observerFactory
      ? observerFactory(context)
      : new ObserverPage(context, readFileSync(OBSERVER_PATH, "utf8"));
    await observer.install();
    return { browser, context, observer, identity };
  } catch (error) {
    try {
      await browser.close();
    } catch {
      // the launch error below is the one to report
    }
    throw error;
  }
}

/** Close context and browser independently so a context failure never leaks Chromium. */
export async function closeOwnedBrowser(handle, { timeoutMs = 10000 } = {}) {
  const errors = [];
  const bounded = async (step, fn) => {
    let timer = null;
    const timeout = new Promise((resolve) => {
      timer = setTimeout(() => resolve("timeout"), timeoutMs);
    });
    try {
      const r = await Promise.race([fn().then(() => "done"), timeout]);
      if (r === "timeout")
        errors.push({ step, error: `timed out after ${timeoutMs} ms`, timedOut: true });
    } catch (error) {
      errors.push({ step, error: String(error?.message ?? error) });
    } finally {
      clearTimeout(timer);
    }
  };
  await bounded("context.close", () => handle.context.close());
  await bounded("browser.close", () => handle.browser.close());
  return errors;
}

export function isLivereloadSocket(url) {
  return /\/__zfb\/reload\/ws(?:[?#]|$)/.test(url);
}

/** livereload.js frames are JSON `{ event, data }`; anything else is kept raw. */
export function livereloadFrame(payload, ns, url) {
  const text = typeof payload === "string" ? payload : Buffer.from(payload).toString("utf8");
  let event = "unparsed";
  let data = null;
  try {
    const msg = JSON.parse(text);
    if (msg && typeof msg.event === "string") {
      event = msg.event;
      data = msg.data ?? null;
    }
  } catch {
    event = "unparsed";
  }
  return {
    at: isoNow(),
    ns: String(ns),
    url,
    event,
    data,
    raw: event === "unparsed" ? text.slice(0, 200) : undefined,
  };
}

export class ObserverPage {
  constructor(context, observerSource, clock = hrtimeNs) {
    this.context = context;
    this.observerSource = observerSource;
    this.clock = clock;
    this.page = null;
    this.baseUrl = null;
    this.docInits = [];
    this.navigations = [];
    this.waiter = null;
    this.armedToken = null;
    this.early = new Map();
    this.pingWaiters = new Map();
    this.docInitWaiters = [];
    this.wsFrames = [];
    this.requests = [];
    this.resolved = new Set();
    this.duplicates = [];
    this.attemptMark = { docInits: 0, navigations: 0, wsFrames: 0 };
    this.preMark = null;
    this.onStray = () => {};
  }

  async install() {
    await this.context.exposeBinding("__dboNotify", (_source, payload) =>
      this.onNotify(payload, this.clock()),
    );
    await this.context.exposeBinding("__dboPing", (_source, payload) =>
      this.onPing(payload, this.clock()),
    );
    await this.context.exposeBinding("__dboDocInit", (_source, payload) =>
      this.onDocInit(payload, this.clock()),
    );
    await this.context.addInitScript({ content: this.observerSource });
    this.page = await this.context.newPage();
    this.page.on("framenavigated", (frame) => {
      if (frame === this.page.mainFrame())
        this.navigations.push({ at: isoNow(), ns: String(this.clock()), url: frame.url() });
    });
    this.page.on("request", (req) => {
      this.requests.push({
        at: isoNow(),
        ns: String(this.clock()),
        url: req.url(),
        resourceType: req.resourceType(),
        navigation: req.isNavigationRequest(),
      });
    });
    this.page.on("websocket", (ws) => {
      if (!isLivereloadSocket(ws.url())) return;
      ws.on("framereceived", ({ payload }) =>
        this.wsFrames.push(livereloadFrame(payload, this.clock(), ws.url())),
      );
    });
  }

  /** Returns true so the observer persists `delivered` only after this ack. */
  onNotify(payload, t1) {
    const received = { ...payload, receivedNs: String(t1) };
    if (this.resolved.has(payload.token) || (!this.waiter && this.early.has(payload.token))) {
      this.duplicates.push(received);
      return true;
    }
    if (this.waiter && payload.token === this.waiter.token) {
      const w = this.waiter;
      this.waiter = null;
      clearTimeout(w.timer);
      this.resolved.add(payload.token);
      w.resolve({ kind: "notify", payload, t1 });
      return true;
    }
    if (!this.waiter && payload.token === this.armedToken) {
      this.early.set(payload.token, { payload, t1 });
      return true;
    }
    if (this.waiter) this.waiter.onLate(received);
    else this.onStray(received);
    return true;
  }

  onPing(payload, t1) {
    const w = this.pingWaiters.get(payload.id);
    if (w) {
      this.pingWaiters.delete(payload.id);
      w({ payload, t1 });
    }
  }

  onDocInit(payload, t) {
    const rec = { ...payload, ns: String(t), at: isoNow() };
    this.docInits.push(rec);
    const waiters = this.docInitWaiters;
    this.docInitWaiters = [];
    for (const w of waiters) w(rec, t);
  }

  /** Next document init, or null after `timeoutMs` (the waiter is then removed). */
  nextDocInit(timeoutMs = 30000) {
    return new Promise((resolve) => {
      const waiter = (rec, t) => {
        clearTimeout(timer);
        resolve({ rec, t });
      };
      const timer = setTimeout(() => {
        this.docInitWaiters = this.docInitWaiters.filter((w) => w !== waiter);
        resolve(null);
      }, timeoutMs);
      this.docInitWaiters.push(waiter);
    });
  }

  url(path) {
    return new URL(path, `${this.baseUrl}/`).href;
  }

  /** Drop any pending measured wait so a later delivery is never routed to it. */
  cancelWait() {
    if (!this.waiter) return null;
    const w = this.waiter;
    this.waiter = null;
    clearTimeout(w.timer);
    w.resolve({ kind: "cancelled" });
    return w.token;
  }

  hasEarly(token) {
    return this.early.has(token);
  }

  async disarm() {
    this.armedToken = null;
    this.cancelWait();
    this.early.clear();
    try {
      return await this.page.evaluate(() => (window.__dbo ? window.__dbo.disarm() : null));
    } catch (error) {
      return { error: String(error?.message ?? error) };
    }
  }

  /** Pre-attempt navigation (never inside a measured attempt). */
  async prepare(path, timeoutMs = 30000) {
    await this.disarm();
    const response = await this.page.goto(this.url(path), {
      waitUntil: "load",
      timeout: timeoutMs,
    });
    const state = await this.page.evaluate(() => window.__dbo.state());
    if (state.armed) await this.page.evaluate(() => window.__dbo.disarm());
    return { ...state, httpStatus: response ? response.status() : null };
  }

  /** After an aborted navigation, wait (bounded) for the document that replaced it; never reloads. */
  async awaitResultingDocument(timeoutMs) {
    const started = this.clock();
    try {
      await this.page.waitForLoadState("load", { timeout: timeoutMs });
      const state = await this.page.evaluate(() => (window.__dbo ? window.__dbo.state() : null));
      if (state?.armed) await this.page.evaluate(() => window.__dbo.disarm());
      return {
        ok: state !== null,
        url: this.page.url(),
        state,
        waitedMs: nsToMs(this.clock() - started),
      };
    } catch (error) {
      return {
        ok: false,
        error: String(error?.message ?? error),
        waitedMs: nsToMs(this.clock() - started),
      };
    }
  }

  /** Read-only snapshot of the current document for a failed check's record. */
  async failureSnapshot(predicate) {
    try {
      return await this.page.evaluate((p) => {
        const st = window.__dbo ? window.__dbo.state() : null;
        return {
          url: location.href,
          title: document.title,
          readyState: document.readyState,
          docUUID: st ? st.docUUID : null,
          armed: st ? st.armed : null,
          predicateNow: window.__dbo ? window.__dbo.evaluate(p) : null,
          bodyExcerpt: (document.body ? document.body.innerText : "").slice(0, 600),
          notInCache:
            (document.body ? document.body.innerText : "").indexOf("page not in cache") !== -1,
        };
      }, predicate);
    } catch (error) {
      return { error: String(error?.message ?? error) };
    }
  }

  /**
   * Cleanup settle: wait until nothing (livereload frame, main-frame
   * navigation, new document) has happened for `quietMs`, measured from the
   * restore write or the last such event, bounded by `boundMs`. Records
   * whether the restore's own native update (page/css frame) was observed.
   */
  /**
   * Cleanup/after-write settle. With `requireNativeUpdate`, the settle ends only
   * after the write's own native update is observed (a livereload page/css
   * frame or a new document) followed by `quietMs` of quiet; otherwise
   * `quietMs` of quiet alone suffices. Always bounded by `boundMs`.
   */
  async settle({
    mark,
    startNs,
    quietMs,
    boundMs,
    requireNativeUpdate = false,
    rule = null,
    stepMs = 50,
  }) {
    const begun = this.clock();
    for (;;) {
      const ev = this.since(mark, startNs);
      const lastNs = [
        startNs,
        ...[...ev.wsFrames, ...ev.navigations, ...ev.docInits].map((x) => BigInt(x.ns)),
      ].reduce((a, b) => (b > a ? b : a));
      const now = this.clock();
      const quietFor = nsToMs(now - lastNs);
      const waitedMs = nsToMs(now - begun);
      const nativeUpdateObserved =
        ev.wsFrames.some((f) => f.event === "page" || f.event === "css") || ev.newDocuments > 0;
      const done = quietFor >= quietMs && (!requireNativeUpdate || nativeUpdateObserved);
      if (done || waitedMs >= boundMs) {
        return {
          settled: done,
          rule: rule ?? (requireNativeUpdate ? "require-native-update" : "quiet-only"),
          reason: done
            ? nativeUpdateObserved
              ? "native-update-observed-then-quiet"
              : "no-frame-for-quietMs"
            : requireNativeUpdate && !nativeUpdateObserved
              ? "bound-exceeded-without-native-update"
              : "bound-exceeded",
          nativeUpdateObserved,
          quietMs,
          boundMs,
          waitedMs,
          ...ev,
        };
      }
      await new Promise((r) => setTimeout(r, stepMs));
    }
  }

  async arm(token, predicate, watch = null) {
    this.armedToken = token;
    const res = await this.page.evaluate(
      ([t, p, w]) => window.__dbo.arm(t, p, w),
      [token, predicate, watch],
    );
    if (!res.armed) this.armedToken = null;
    return res;
  }

  mark() {
    return {
      docInits: this.docInits.length,
      navigations: this.navigations.length,
      wsFrames: this.wsFrames.length,
      requests: this.requests.length,
    };
  }

  since(mark, t0 = null) {
    const rel = (x) => (t0 === null ? x : { ...x, msFromT0: nsToMs(BigInt(x.ns) - t0) });
    const docInits = this.docInits.slice(mark.docInits).map(rel);
    const navigations = this.navigations.slice(mark.navigations).map(rel);
    const wsFrames = this.wsFrames.slice(mark.wsFrames).map(rel);
    const requests = this.requests.slice(mark.requests ?? this.requests.length).map(rel);
    return {
      mainFrameNavigations: navigations.length,
      newDocuments: docInits.length,
      livereloadEvents: wsFrames.map((f) => f.event),
      docInits,
      navigations,
      wsFrames,
      requests,
    };
  }

  markAttempt() {
    this.attemptMark = this.mark();
  }

  collectAttempt(t0 = null) {
    return this.since(this.attemptMark, t0);
  }

  /** Start of the pre-write window (after prepare, before arming). */
  markPre() {
    this.preMark = this.mark();
  }

  /** Navigations, new documents or reload/css frames since markPre; nonempty means unstable. */
  preWriteInstability() {
    if (!this.preMark) return null;
    const ev = this.since(this.preMark);
    const frames = ev.wsFrames.filter((f) => f.event === "page" || f.event === "css");
    if (ev.mainFrameNavigations === 0 && ev.newDocuments === 0 && frames.length === 0) return null;
    return { ...ev, wsFrames: frames };
  }

  /** Hold for `ms` and record what the page saw; never navigates or reloads. */
  async quietWindow(ms) {
    const m = this.mark();
    const start = this.clock();
    await new Promise((r) => setTimeout(r, ms));
    return { quietMs: ms, observedMs: nsToMs(this.clock() - start), ...this.since(m, start) };
  }

  waitNotify(token, timeoutMs, onLate = () => {}) {
    const early = this.early.get(token);
    if (early) {
      this.early.delete(token);
      this.resolved.add(token);
      return Promise.resolve({ kind: "notify", ...early });
    }
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        if (this.waiter?.token === token) this.waiter = null;
        resolve({ kind: "timeout" });
      }, timeoutMs);
      this.waiter = { token, resolve, onLate, timer };
    });
  }

  /**
   * Standardized warming rule for supplemental and restoration checks: one
   * navigation, then an observer-armed wait up to `timeoutMs` with no reload
   * or HTTP polling. If a native reload aborts the navigation, the resulting
   * document is awaited (bounded) and, only if it is not the target, the
   * target is navigated to once more. Every step is recorded.
   */
  async navigateAndAwait(path, predicate, timeoutMs, tokenPrefix) {
    const navStart = this.clock();
    const m = this.mark();
    const recovery = [];
    let state = null;
    for (let i = 0; state === null; i += 1) {
      try {
        state = await this.prepare(path);
      } catch (error) {
        const msg = String(error?.message ?? error);
        const aborted = /ERR_ABORTED|interrupted by another navigation|frame was detached/i.test(
          msg,
        );
        if (!aborted || i >= 1) {
          return {
            ok: false,
            error: msg,
            recovery,
            evidence: this.since(m, navStart),
            failure: await this.failureSnapshot(predicate),
          };
        }
        const resulting = await this.awaitResultingDocument(timeoutMs);
        const onTarget =
          resulting.ok && new URL(resulting.url).pathname === new URL(this.url(path)).pathname;
        recovery.push({
          at: isoNow(),
          abortedBy: msg.split("\n")[0],
          resulting,
          usedResultingDocument: onTarget,
        });
        if (onTarget) state = resulting.state;
      }
    }
    const token = `${tokenPrefix}-${Math.random().toString(16).slice(2, 10)}`;
    const armed = await this.arm(token, predicate);
    const base = {
      docUUID: state.docUUID,
      httpStatus: state.httpStatus ?? null,
      firstReadActual: armed.actual ?? null,
      recovery,
    };
    if (!armed.armed && armed.precondition) {
      this.armedToken = null;
      const sinceNavigationMs = nsToMs(this.clock() - navStart);
      const page = await this.failureSnapshot(predicate);
      const transient = page?.notInCache === true;
      return {
        ok: !transient,
        firstReadMatched: true,
        ...base,
        sinceNavigationMs,
        actual: armed.actual,
        evidence: this.since(m, navStart),
        page,
        ...(transient ? { transient: "transient-not-in-cache" } : {}),
      };
    }
    if (!armed.armed)
      return {
        ok: false,
        error: armed.error ?? "arm failed",
        ...base,
        evidence: this.since(m, navStart),
      };
    const waited = await this.waitNotify(token, timeoutMs, (p) => this.onStray(p));
    const sinceNavigationMs = nsToMs(this.clock() - navStart);
    const failure = waited.kind === "notify" ? null : await this.failureSnapshot(predicate);
    const page = failure ?? (await this.failureSnapshot(predicate));
    if (waited.kind !== "notify") await this.disarm();
    this.armedToken = null;
    // zfb dev's "404 — page not in cache … rebuild may still be in flight" is a transient state, never a pass.
    const transient = page?.notInCache === true || failure?.notInCache === true;
    return {
      ok: waited.kind === "notify" && !transient,
      ...(transient ? { transient: "transient-not-in-cache" } : {}),
      firstReadMatched: false,
      ...base,
      sinceNavigationMs,
      notify: waited.payload ?? null,
      evidence: this.since(m, navStart),
      failure,
      page,
    };
  }

  /** Read-only predicate evaluation in the current document (no arming). */
  async evaluateNow(predicate) {
    return this.page.evaluate(
      (p) =>
        window.__dbo ? window.__dbo.evaluate(p) : { matched: false, error: "observer missing" },
      predicate,
    );
  }

  supplemental(check, timeoutMs) {
    if (check.kind === "gone") return this.goneCheck(check);
    return this.navigateAndAwait(check.path, check.predicate, timeoutMs, "supp");
  }

  /** One fresh navigation and a read-only look at the result (status, title, excerpt, marker text). */
  async observePage(path, { markerText = null } = {}) {
    const navStart = this.clock();
    try {
      await this.disarm();
      const response = await this.page.goto(this.url(path), { waitUntil: "load", timeout: 30000 });
      const view = await this.page.evaluate((marker) => {
        const text = document.body ? document.body.innerText : "";
        return {
          url: location.href,
          title: document.title,
          containsMarker: marker ? text.indexOf(marker) !== -1 : null,
          notInCache: text.indexOf("page not in cache") !== -1,
          bodyExcerpt: text.slice(0, 600),
        };
      }, markerText);
      return {
        ok: true,
        path,
        httpStatus: response ? response.status() : null,
        sinceNavigationMs: nsToMs(this.clock() - navStart),
        ...view,
      };
    } catch (error) {
      return {
        ok: false,
        path,
        error: String(error?.message ?? error),
        sinceNavigationMs: nsToMs(this.clock() - navStart),
      };
    }
  }

  /**
   * A removed URL must answer 404/410 AND must not still render the removed
   * content: a 200 error page or a 200 last-good page is a failure.
   */
  async goneCheck(check) {
    const view = await this.observePage(check.path, { markerText: check.absentText ?? null });
    const statusOk = view.httpStatus === 404 || view.httpStatus === 410;
    return {
      ...view,
      kind: "gone",
      ok: view.ok && statusOk && view.containsMarker !== true && view.notInCache !== true,
      statusOk,
      ...(view.notInCache ? { evidence: "transient-not-in-cache" } : {}),
    };
  }

  liveCheck(check, timeoutMs) {
    if (check.kind === "gone") return this.goneCheck(check);
    return this.navigateAndAwait(check.path, check.predicate, timeoutMs, "restore");
  }

  /** Bounded ping: a missing binding delivery is a recorded timeout, never a hang. */
  async ping(mode, timeoutMs = 10000) {
    const id = `${mode}-${Math.random().toString(16).slice(2, 10)}`;
    let timer = null;
    const got = new Promise((resolve) => this.pingWaiters.set(id, resolve));
    const timedOut = new Promise((resolve) => {
      timer = setTimeout(() => resolve(null), timeoutMs);
    });
    const t0 = this.clock();
    try {
      if (mode === "two-raf") await this.page.evaluate((x) => window.__dbo.pingAfterTwoRaf(x), id);
      else await this.page.evaluate((x) => window.__dbo.ping(x), id);
      const r = await Promise.race([got, timedOut]);
      if (r === null) return { ok: false, timedOut: true, timeoutMs, t0, ms: null, payload: null };
      return { ok: true, t0, t1: r.t1, ms: nsToMs(r.t1 - t0), payload: r.payload };
    } finally {
      clearTimeout(timer);
      this.pingWaiters.delete(id);
    }
  }
}
