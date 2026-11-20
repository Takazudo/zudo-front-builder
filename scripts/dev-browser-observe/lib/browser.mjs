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
export async function launchOwnedBrowser(opts = {}) {
  const cfg = { ...DEFAULT_BROWSER, ...opts };
  const { chromium } = await import("@playwright/test");
  const executablePath = chromium.executablePath();
  const browser = await chromium.launch({ headless: cfg.headless, args: cfg.args });
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
  const observer = new ObserverPage(context, readFileSync(OBSERVER_PATH, "utf8"));
  await observer.install();
  return { browser, context, observer, identity };
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

  nextDocInit() {
    return new Promise((resolve) => this.docInitWaiters.push((rec, t) => resolve({ rec, t })));
  }

  url(path) {
    return new URL(path, `${this.baseUrl}/`).href;
  }

  async disarm() {
    this.armedToken = null;
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
  async settle({ mark, startNs, quietMs, boundMs, stepMs = 50 }) {
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
      const nativeUpdateObserved = ev.wsFrames.some((f) => f.event === "page" || f.event === "css");
      if (quietFor >= quietMs || waitedMs >= boundMs) {
        return {
          settled: quietFor >= quietMs,
          reason:
            quietFor >= quietMs
              ? nativeUpdateObserved
                ? "native-update-observed-then-quiet"
                : "no-frame-for-quietMs"
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

  async arm(token, predicate) {
    this.armedToken = token;
    const res = await this.page.evaluate(([t, p]) => window.__dbo.arm(t, p), [token, predicate]);
    if (!res.armed) this.armedToken = null;
    return res;
  }

  mark() {
    return {
      docInits: this.docInits.length,
      navigations: this.navigations.length,
      wsFrames: this.wsFrames.length,
    };
  }

  since(mark, t0 = null) {
    const rel = (x) => (t0 === null ? x : { ...x, msFromT0: nsToMs(BigInt(x.ns) - t0) });
    const docInits = this.docInits.slice(mark.docInits).map(rel);
    const navigations = this.navigations.slice(mark.navigations).map(rel);
    const wsFrames = this.wsFrames.slice(mark.wsFrames).map(rel);
    return {
      mainFrameNavigations: navigations.length,
      newDocuments: docInits.length,
      livereloadEvents: wsFrames.map((f) => f.event),
      docInits,
      navigations,
      wsFrames,
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
   * navigation, then an observer-armed wait up to `timeoutMs` with no reload,
   * re-navigation or HTTP polling. Already-true at first read counts as ok.
   */
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
      return {
        ok: true,
        firstReadMatched: true,
        ...base,
        sinceNavigationMs: nsToMs(this.clock() - navStart),
        actual: armed.actual,
        evidence: this.since(m, navStart),
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
    const failure = waited.kind === "notify" ? null : await this.failureSnapshot(predicate);
    if (waited.kind !== "notify") await this.disarm();
    this.armedToken = null;
    return {
      ok: waited.kind === "notify",
      firstReadMatched: false,
      ...base,
      sinceNavigationMs: nsToMs(this.clock() - navStart),
      notify: waited.payload ?? null,
      evidence: this.since(m, navStart),
      failure,
    };
  }

  supplemental(check, timeoutMs) {
    return this.navigateAndAwait(check.path, check.predicate, timeoutMs, "supp");
  }

  liveCheck(check, timeoutMs) {
    return this.navigateAndAwait(check.path, check.predicate, timeoutMs, "restore");
  }

  async ping(mode) {
    const id = `${mode}-${Math.random().toString(16).slice(2, 10)}`;
    const got = new Promise((resolve) => this.pingWaiters.set(id, resolve));
    const t0 = this.clock();
    if (mode === "two-raf") await this.page.evaluate((x) => window.__dbo.pingAfterTwoRaf(x), id);
    else await this.page.evaluate((x) => window.__dbo.ping(x), id);
    const { t1, payload } = await got;
    return { t0, t1, ms: nsToMs(t1 - t0), payload };
  }
}
