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
    this.attemptMark = { docInits: 0, navigations: 0 };
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
  }

  onNotify(payload, t1) {
    if (this.waiter && payload.token === this.waiter.token) {
      const w = this.waiter;
      this.waiter = null;
      clearTimeout(w.timer);
      w.resolve({ kind: "notify", payload, t1 });
      return;
    }
    if (!this.waiter && payload.token === this.armedToken) {
      this.early.set(payload.token, { payload, t1 });
      return;
    }
    if (this.waiter) this.waiter.onLate({ ...payload, receivedNs: String(t1) });
    else this.onStray({ ...payload, receivedNs: String(t1) });
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
    await this.page.goto(this.url(path), { waitUntil: "load", timeout: timeoutMs });
    const state = await this.page.evaluate(() => window.__dbo.state());
    if (state.armed) await this.page.evaluate(() => window.__dbo.disarm());
    return state;
  }

  async arm(token, predicate) {
    this.armedToken = token;
    const res = await this.page.evaluate(([t, p]) => window.__dbo.arm(t, p), [token, predicate]);
    if (!res.armed) this.armedToken = null;
    return res;
  }

  markAttempt() {
    this.attemptMark = { docInits: this.docInits.length, navigations: this.navigations.length };
  }

  collectAttempt() {
    const docInits = this.docInits.slice(this.attemptMark.docInits);
    const navigations = this.navigations.slice(this.attemptMark.navigations);
    return {
      mainFrameNavigations: navigations.length,
      newDocuments: docInits.length,
      docInits,
      navigations,
    };
  }

  waitNotify(token, timeoutMs, onLate = () => {}) {
    const early = this.early.get(token);
    if (early) {
      this.early.delete(token);
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
  async navigateAndAwait(path, predicate, timeoutMs, tokenPrefix) {
    const navStart = this.clock();
    const state = await this.prepare(path);
    const token = `${tokenPrefix}-${Math.random().toString(16).slice(2, 10)}`;
    const armed = await this.arm(token, predicate);
    if (!armed.armed && armed.precondition) {
      this.armedToken = null;
      return {
        ok: true,
        firstReadMatched: true,
        docUUID: state.docUUID,
        sinceNavigationMs: nsToMs(this.clock() - navStart),
        actual: armed.actual,
      };
    }
    if (!armed.armed) return { ok: false, error: armed.error ?? "arm failed" };
    const waited = await this.waitNotify(token, timeoutMs, (p) => this.onStray(p));
    if (waited.kind !== "notify") await this.disarm();
    this.armedToken = null;
    return {
      ok: waited.kind === "notify",
      firstReadMatched: false,
      docUUID: state.docUUID,
      sinceNavigationMs: nsToMs(this.clock() - navStart),
      notify: waited.payload ?? null,
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
