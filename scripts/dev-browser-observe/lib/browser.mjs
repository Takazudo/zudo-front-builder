import { readFileSync, realpathSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { join } from "node:path";
import { timeBrowserAsync, timeBrowserSync } from "./browser-timing.mjs";
import { HARNESS_DIR, hrtimeNs, isoNow, nsToMs, sha256, sha256File } from "./util.mjs";

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

const TIMING_PATH_LIMIT = 4096;

function timingPath(path, name = "path") {
  if (typeof path !== "string") return { [name]: null };
  return {
    [name]: path.slice(0, TIMING_PATH_LIMIT),
    ...(path.length > TIMING_PATH_LIMIT && { [`${name}Truncated`]: true }),
  };
}

function observerFileDigest(timing, parentId) {
  const path = OBSERVER_PATH;
  if (!timing) return sha256File(path);
  const details = { role: "observer-source", ...timingPath(path) };
  const bytes = timing.syncSpan("browser.file.read", parentId, details, (row) => {
    const value = readFileSync(path);
    if (row) row.bytes = value.length;
    return value;
  });
  return digestBytes(bytes, timing, parentId, details);
}

async function executableDigest(
  path,
  role,
  { timing, parentId, readExecutable, signal, onReadState },
) {
  signal?.throwIfAborted();
  const details = { role, ...timingPath(path) };
  onReadState?.(details, "pending");
  let invoked = false;
  let readRow;
  const cancelled = () => {
    if (readRow) readRow.cancelRequestedNs = String(timing.clock());
  };
  let bytes;
  try {
    signal?.throwIfAborted();
    // Await the original read, including when cancellation is requested. The
    // session still owns startup and any launched browser until it settles.
    bytes = await timeBrowserAsync(
      timing,
      "browser.file.read",
      parentId,
      (_id, row) => {
        readRow = row;
        if (readRow && signal) signal.addEventListener("abort", cancelled, { once: true });
        invoked = true;
        return readExecutable(path, { signal: signal ?? undefined });
      },
      { ...details, cancelRequestedNs: null },
    );
    if (readRow) readRow.bytes = bytes.length;
  } catch (error) {
    onReadState?.(details, invoked ? "rejected" : "cancelled-before-read");
    throw error;
  } finally {
    if (readRow && signal) signal.removeEventListener("abort", cancelled);
  }
  onReadState?.(details, "fulfilled");
  signal?.throwIfAborted();
  return digestBytes(bytes, timing, parentId, details);
}

function digestBytes(bytes, timing, parentId, details) {
  return timeBrowserSync(
    timing,
    "browser.file.digest",
    parentId,
    { ...details, bytes: bytes.length },
    (row) => {
      const digest = sha256(bytes);
      if (row) row.digest = digest;
      return digest;
    },
  );
}

// A failed read must not hold the next check behind a pending Playwright navigation.
async function boundedObservation(read, timeoutMs) {
  let timer;
  try {
    return await Promise.race([
      read,
      new Promise((resolve) => {
        timer = setTimeout(() => resolve({ error: "observation timed out", timeoutMs }), timeoutMs);
      }),
    ]);
  } catch (error) {
    return { error: String(error?.message ?? error) };
  } finally {
    clearTimeout(timer);
  }
}

export const PREPARE_INTERRUPTED = "PREPARE_INTERRUPTED";

/**
 * A prepare whose page was replaced under it (Playwright: "Execution context was destroyed",
 * a detached frame) gets the code PREPARE_INTERRUPTED; any other error passes through.
 */
export function prepareInterruption(error, path) {
  const message = String(error?.message ?? error);
  if (!/Execution context was destroyed|frame was detached|navigation/i.test(message)) return error;
  return Object.assign(new Error(`prepare of ${path} interrupted by a navigation: ${message}`), {
    code: PREPARE_INTERRUPTED,
    path,
    cause: error,
  });
}

/**
 * One owned Chromium (Playwright launch: fresh temporary profile), one
 * context, one page. Never touches the user's browser profile.
 */
export async function launchOwnedBrowser(
  opts = {},
  {
    chromiumImpl = null,
    observerFactory = null,
    readExecutable = readFile,
    onReadState,
    signal,
    launchTimeoutMs,
    timing = null,
    parentId = null,
  } = {},
) {
  signal?.throwIfAborted();
  const cfg = { ...DEFAULT_BROWSER, ...opts };
  // Keep Playwright import, identity reads and actual launch in their original order.
  const chromium =
    chromiumImpl ??
    (
      await timeBrowserAsync(
        timing,
        "browser.playwright.import",
        parentId,
        () => import("@playwright/test"),
      )
    ).chromium;
  signal?.throwIfAborted();
  const executablePath = timeBrowserSync(
    timing,
    "browser.executable.default-path",
    parentId,
    { role: "library-default" },
    (row) => {
      const path = chromium.executablePath();
      if (row) Object.assign(row, timingPath(path));
      return path;
    },
  );
  const diagnosticExecutable = cfg.diagnosticExecutable;
  const actualPath = diagnosticExecutable
    ? timeBrowserSync(
        timing,
        "browser.executable.resolve",
        parentId,
        { role: "selected-launched" },
        (row) => {
          if (row) Object.assign(row, timingPath(diagnosticExecutable.path));
          const path = realpathSync(diagnosticExecutable.path);
          if (row) Object.assign(row, timingPath(path, "resolvedPath"));
          return path;
        },
      )
    : null;
  const readOptions = { timing, parentId, readExecutable, signal, onReadState };
  const actualHash = actualPath
    ? await executableDigest(actualPath, "selected-launched", readOptions)
    : null;
  signal?.throwIfAborted();
  if (diagnosticExecutable && actualHash !== diagnosticExecutable.sha256)
    throw new Error(`diagnostic browser executable digest differs: ${actualPath}`);
  const timeout = actualPath ? (launchTimeoutMs?.() ?? diagnosticExecutable.timeoutMs) : undefined;
  signal?.throwIfAborted();
  const browser = await timeBrowserAsync(timing, "browser.chromium.launch", parentId, () =>
    chromium.launch({
      headless: cfg.headless,
      args: cfg.args,
      ...(actualPath ? { executablePath: actualPath, timeout } : {}),
    }),
  );
  try {
    signal?.throwIfAborted();
    const context = await timeBrowserAsync(timing, "browser.context.create", parentId, () =>
      browser.newContext({
        viewport: cfg.viewport,
        deviceScaleFactor: cfg.deviceScaleFactor,
        serviceWorkers: "block",
      }),
    );
    const executableSha256 = await executableDigest(executablePath, "library-default", readOptions);
    signal?.throwIfAborted();
    const identity = {
      engine: "chromium",
      executablePath,
      executableSha256,
      ...(actualPath
        ? { launchedExecutablePath: actualPath, launchedExecutableSha256: actualHash }
        : {}),
      version: timeBrowserSync(timing, "browser.version", parentId, {}, () => browser.version()),
      headless: cfg.headless,
      args: cfg.args,
      viewport: cfg.viewport,
      deviceScaleFactor: cfg.deviceScaleFactor,
      profile: "playwright-temporary",
      playwrightVersion: timeBrowserSync(
        timing,
        "browser.playwright.version",
        parentId,
        {},
        playwrightVersion,
      ),
      observerSha256: observerFileDigest(timing, parentId),
    };
    const observer = timeBrowserSync(timing, "browser.observer.create", parentId, {}, (create) => {
      if (observerFactory) return observerFactory(context);
      const source = timeBrowserSync(
        timing,
        "browser.observer.source-read",
        create?.id ?? null,
        { role: "observer-source", ...timingPath(OBSERVER_PATH) },
        (row) => {
          const value = readFileSync(OBSERVER_PATH, "utf8");
          if (row) row.decodedUtf8Bytes = Buffer.byteLength(value, "utf8");
          return value;
        },
      );
      return new ObserverPage(context, source);
    });
    signal?.throwIfAborted();
    await timeBrowserAsync(timing, "browser.observer.install", parentId, (id) =>
      observerFactory ? observer.install() : observer.install(timing, id, signal),
    );
    signal?.throwIfAborted();
    return { browser, context, observer, identity };
  } catch (error) {
    // Preserve the original setup error. Return an unresolved browser to the
    // session finalizer when this first close fails or stalls.
    let timer;
    try {
      await Promise.race([
        timeBrowserAsync(timing, "browser.cleanup.close", parentId, () => browser.close()),
        new Promise((_, reject) => {
          timer = setTimeout(() => reject(new Error("browser close timed out")), 2000);
        }),
      ]);
    } catch (cleanupError) {
      const ownership = {
        partialBrowser: { browser, context: { close: async () => {} } },
        browserCleanupError: String(cleanupError),
      };
      // Abort reasons may be primitives or immutable errors.
      if (error instanceof Error && Object.isExtensible(error)) Object.assign(error, ownership);
      else
        throw Object.assign(
          new Error(String(error?.message ?? error), { cause: error }),
          ownership,
        );
    } finally {
      clearTimeout(timer);
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

export const TRANSIENT_REREAD_MS = 500;

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

/**
 * One read of a gone check: a 404 or 410 whose body does not contain the removed page's marker
 * text and, when declared, has no element matching its selector.
 */
export function goneRead(v) {
  return (
    v.ok &&
    (v.httpStatus === 404 || v.httpStatus === 410) &&
    v.containsMarker !== true &&
    v.selectorPresent !== true
  );
}

/** Update events that count as a framework's own update (settle evidence; mechanism input). */
export const UPDATE_EVENTS = new Set(["page", "css", "rsc", "module"]);

/** Declared framework frames that only show the server is busy: they extend the quiet window but are never update evidence. */
export const ACTIVITY_EVENTS = new Set(["activity"]);

/** The declared update socket (distributions.json updateSignal.sockets) a WebSocket URL belongs to. */
export function declaredSocketFor(url, sockets = []) {
  return sockets.find((s) => new RegExp(s.url).test(url)) ?? null;
}

/**
 * A frame from a declared framework HMR socket. Its raw type is payload[typeKey], refined by the
 * update kinds of a Vite `update` (`update:css-update`) or a Vite custom event
 * (`custom:astro:content-changed`). The declared events map the most specific raw type first,
 * then the base type; anything else is `unmapped` and recorded, never used as update evidence.
 */
export function updateSignalFrame(payload, ns, url, socket, astroFooter = false) {
  const text = typeof payload === "string" ? payload : Buffer.from(payload).toString("utf8");
  let msg = null;
  try {
    msg = JSON.parse(text);
  } catch {
    msg = null;
  }
  const base = msg && typeof msg[socket.typeKey] === "string" ? msg[socket.typeKey] : null;
  let rawType = base;
  if (base && Array.isArray(msg.updates))
    rawType = `${base}:${[...new Set(msg.updates.map((u) => u?.type).filter(Boolean))].sort().join("+")}`;
  else if (base && typeof msg.event === "string") rawType = `${base}:${msg.event}`;
  const event =
    (rawType && socket.events[rawType]) ??
    (base && socket.events[base]) ??
    (base ? "unmapped" : "unparsed");
  return {
    at: isoNow(),
    ns: String(ns),
    url,
    socket: socket.name,
    ...(astroFooter && {
      hasUpdatePath: Object.hasOwn(msg ?? {}, "path"),
      updatePath: typeof msg?.path === "string" ? msg.path.slice(0, 256) : null,
      updates: Array.isArray(msg?.updates)
        ? msg.updates
            .slice(0, 8)
            .map((u) =>
              Object.fromEntries(
                ["type", "path", "acceptedPath"]
                  .filter((k) => typeof u?.[k] === "string")
                  .map((k) => [k, u[k].slice(0, 256)]),
              ),
            )
        : [],
      layoutModule:
        event === "module" &&
        Array.isArray(msg?.updates) &&
        msg.updates.length === 1 &&
        msg.updates[0]?.type === "js-update" &&
        msg.updates[0].path === "/src/layouts/Site.astro" &&
        msg.updates[0].acceptedPath === "/src/layouts/Site.astro",
      pageSignal:
        event === "page" &&
        (msg.path === undefined || msg.path === "*" || msg.path === "/src/layouts/Site.astro"),
    }),
    rawType,
    event,
    raw: base ? undefined : text.slice(0, 200),
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
    this.otherFrames = [];
    this.updateSockets = [];
    this.peerSocketTracking = null;
    this.requests = [];
    this.resolved = new Set();
    this.duplicates = [];
    this.attemptMark = { docInits: 0, navigations: 0, wsFrames: 0 };
    this.preMark = null;
    this.onStray = () => {};
  }

  async install(timing, parentId, signal) {
    await timeBrowserAsync(timing, "browser.observer.binding.notify", parentId, () =>
      this.context.exposeBinding("__dboNotify", (source, payload) =>
        this.onNotify(payload, this.clock(), source),
      ),
    );
    signal?.throwIfAborted();
    await timeBrowserAsync(timing, "browser.observer.binding.ping", parentId, () =>
      this.context.exposeBinding("__dboPing", (_source, payload) =>
        this.onPing(payload, this.clock()),
      ),
    );
    signal?.throwIfAborted();
    await timeBrowserAsync(timing, "browser.observer.binding.doc-init", parentId, () =>
      this.context.exposeBinding("__dboDocInit", (source, payload) =>
        this.onDocInit(payload, this.clock(), source),
      ),
    );
    signal?.throwIfAborted();
    await timeBrowserAsync(timing, "browser.observer.init-script", parentId, () =>
      this.context.addInitScript({ content: this.observerSource }),
    );
    signal?.throwIfAborted();
    this.page = await timeBrowserAsync(timing, "browser.observer.page.create", parentId, () =>
      this.context.newPage(),
    );
    signal?.throwIfAborted();
    timeBrowserSync(timing, "browser.observer.listeners", parentId, {}, () => {
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
        const url = ws.url();
        if (!isLivereloadSocket(url) && !declaredSocketFor(url, this.updateSockets)) return;
        if (this.peerSocketTracking) {
          if (
            !declaredSocketFor(url, this.updateSockets) ||
            new URL(url).host !== new URL(this.baseUrl).host
          )
            return;
          const row = { url, openedNs: String(this.clock()), firstFrameNs: null };
          this.peerSocketTracking.set(ws, row);
          ws.on("framereceived", () => {
            row.firstFrameNs ??= String(this.clock());
          });
          ws.on("close", () => this.peerSocketTracking.delete(ws));
        }
        ws.on("framereceived", ({ payload }) => this.recordSocketFrame(url, payload));
      });
    });
  }

  /**
   * zfb livereload frames go to wsFrames unchanged. A declared framework socket's frame goes to
   * wsFrames when it maps to an update or activity event, otherwise to otherFrames (recorded,
   * not evidence).
   */
  recordSocketFrame(url, payload) {
    if (isLivereloadSocket(url)) {
      this.wsFrames.push(livereloadFrame(payload, this.clock(), url));
      return;
    }
    const socket = declaredSocketFor(url, this.updateSockets);
    if (!socket) return;
    const frame = updateSignalFrame(
      payload,
      this.clock(),
      url,
      socket,
      Boolean(this.peerSocketTracking),
    );
    const kept = UPDATE_EVENTS.has(frame.event) || ACTIVITY_EVENTS.has(frame.event);
    (kept ? this.wsFrames : this.otherFrames).push(frame);
  }

  /** Returns true so the observer persists `delivered` only after this ack. */
  onNotify(payload, t1, source) {
    if (this.peerSocketTracking) {
      const last = this.docInits.at(-1);
      const designatedMain = Boolean(
        this.page && source?.page === this.page && source.frame === this.page.mainFrame(),
      );
      payload = {
        ...payload,
        producer: {
          designatedMain,
          frameUrl: designatedMain ? source.frame.url() : null,
          latestDocUUID: last?.designatedMain === true ? last.docUUID : null,
          latestDocNs: last?.designatedMain === true ? last.ns : null,
          receivedNs: String(t1),
        },
      };
    }
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

  onDocInit(payload, t, source) {
    if (this.peerSocketTracking) {
      if (!this.page || source?.page !== this.page || source.frame !== this.page.mainFrame())
        return;
      payload = { ...payload, designatedMain: true };
    }
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

  async clearDocumentToken(token) {
    // A timed-out Playwright evaluate can still execute after navigation. Only clear the
    // specific registration we observed, never a later check's token.
    if (!token) return null;
    return boundedObservation(
      this.page.evaluate((expected) => {
        const observer = window.__dbo;
        return observer?.state().armed?.token === expected ? observer.disarm() : null;
      }, token),
      500,
    );
  }

  async disarm() {
    const token = this.armedToken ?? this.waiter?.token;
    this.armedToken = null;
    this.cancelWait();
    this.early.clear();
    return this.clearDocumentToken(token);
  }

  /** Pre-attempt navigation (never inside a measured attempt). */
  async prepare(path, timeoutMs = 30000) {
    await this.disarm();
    const response = await this.page.goto(this.url(path), {
      waitUntil: "load",
      timeout: timeoutMs,
    });
    let state;
    try {
      state = await this.page.evaluate(() => ({
        ...window.__dbo.state(),
        notInCache:
          (document.body ? document.body.innerText : "").indexOf("page not in cache") !== -1,
      }));
      if (state.armed) {
        const cleared = await this.clearDocumentToken(state.armed.token);
        if (cleared?.error) throw new Error(cleared.error);
      }
    } catch (error) {
      throw prepareInterruption(error, path);
    }
    return { ...state, httpStatus: response ? response.status() : null };
  }

  /** After an aborted navigation, wait (bounded) for the document that replaced it; never reloads. */
  async awaitResultingDocument(timeoutMs) {
    const started = this.clock();
    try {
      await this.page.waitForLoadState("load", { timeout: timeoutMs });
      // Same facts prepare() computes, so the transient re-read loop also applies after an abort.
      const state = await this.page.evaluate(() => {
        if (!window.__dbo) return null;
        const nav = performance.getEntriesByType("navigation")[0];
        return {
          ...window.__dbo.state(),
          notInCache:
            (document.body ? document.body.innerText : "").indexOf("page not in cache") !== -1,
          httpStatus:
            nav && typeof nav.responseStatus === "number" && nav.responseStatus > 0
              ? nav.responseStatus
              : null,
        };
      });
      if (state?.armed) {
        const cleared = await this.clearDocumentToken(state.armed.token);
        if (cleared?.error) throw new Error(cleared.error);
      }
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
   * Cleanup/after-write settle. With `requireNativeUpdate`, the settle ends only
   * after a native update is observed (a livereload page/css
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
    bodyRevision = null,
  }) {
    const begun = this.clock();
    let revision = null;
    let evaluatedEvents;
    for (;;) {
      const ev = this.since(mark, startNs);
      const events = [...ev.wsFrames, ...ev.navigations, ...ev.docInits];
      // A fixed peer may change its DOM without creating a new document. Even
      // an unmapped declared socket frame interrupts quiet, but only a mapped
      // update or document initialization can prove native restoration.
      if (bodyRevision?.allowSameDocument) events.push(...(ev.otherFrames ?? []));
      const lastNs = events.reduce(
        (last, row) => (BigInt(row.ns) > last ? BigInt(row.ns) : last),
        startNs,
      );
      const now = this.clock();
      const quietFor = nsToMs(now - lastNs);
      const waitedMs = nsToMs(now - begun);
      const nativeUpdateObserved =
        ev.wsFrames.some((f) => UPDATE_EVENTS.has(f.event)) || ev.newDocuments > 0;
      const doc =
        ev.docInits.at(-1) ?? (bodyRevision?.allowSameDocument ? this.docInits.at(-1) : null);
      if (
        bodyRevision &&
        doc &&
        quietFor >= quietMs &&
        waitedMs < boundMs &&
        (revision?.docUUID !== doc.docUUID ||
          revision?.loading ||
          (bodyRevision.allowSameDocument && evaluatedEvents !== events.length))
      ) {
        evaluatedEvents = events.length;
        const view = await boundedObservation(
          this.page.evaluate(
            (predicate) => ({
              url: location.href,
              docUUID: window.__dbo?.state().docUUID,
              loading: document.readyState === "loading",
              ...window.__dbo?.evaluate(predicate),
            }),
            bodyRevision.predicate,
          ),
          Math.max(1, Math.ceil(boundMs - waitedMs)),
        );
        revision = {
          docUUID: doc.docUUID,
          observedDocUUID: view.docUUID ?? null,
          ...(bodyRevision.allowSameDocument && { actual: view.actual ?? null }),
          url: view.url ?? null,
          loading: view.loading === true,
          matched:
            !view.loading &&
            view.matched === true &&
            (!bodyRevision.affectedDocUUID ||
              ev.docInits.length > 0 ||
              view.docUUID === bodyRevision.affectedDocUUID) &&
            view.docUUID === doc.docUUID &&
            view.url === this.url(bodyRevision.path),
          ...(view.error ? { error: view.error } : {}),
        };
        // Events may have arrived during evaluate. Recheck the latest UUID, quiet and deadline.
        if (!revision.loading) continue;
      }
      const done =
        quietFor >= quietMs &&
        (!requireNativeUpdate || nativeUpdateObserved) &&
        (!bodyRevision ||
          (waitedMs < boundMs && revision?.matched && revision.docUUID === doc?.docUUID));
      if (done || waitedMs >= boundMs) {
        return {
          settled: Boolean(done),
          ...(bodyRevision ? { bodyRevision: revision ?? { matched: false } } : {}),
          rule: rule ?? (requireNativeUpdate ? "require-native-update" : "quiet-only"),
          reason: done
            ? nativeUpdateObserved
              ? "native-update-observed-then-quiet"
              : "no-frame-for-quietMs"
            : bodyRevision
              ? "bound-exceeded-without-restored-body"
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
      ([t, p, w, captureUrl]) => {
        const result = window.__dbo.arm(t, p, w);
        return captureUrl ? { ...result, observedUrl: location.href } : result;
      },
      [token, predicate, watch, Boolean(this.peerSocketTracking)],
    );
    if (!res.armed) this.armedToken = null;
    return res;
  }

  mark() {
    return {
      docInits: this.docInits.length,
      navigations: this.navigations.length,
      wsFrames: this.wsFrames.length,
      otherFrames: this.otherFrames.length,
      requests: this.requests.length,
    };
  }

  since(mark, t0 = null) {
    const rel = (x) => (t0 === null ? x : { ...x, msFromT0: nsToMs(BigInt(x.ns) - t0) });
    const docInits = this.docInits.slice(mark.docInits).map(rel);
    const navigations = this.navigations.slice(mark.navigations).map(rel);
    const wsFrames = this.wsFrames.slice(mark.wsFrames).map(rel);
    const requests = this.requests.slice(mark.requests ?? this.requests.length).map(rel);
    const otherFrames = this.otherFrames
      .slice(mark.otherFrames ?? this.otherFrames.length)
      .map(rel);
    return {
      mainFrameNavigations: navigations.length,
      newDocuments: docInits.length,
      livereloadEvents: wsFrames.map((f) => f.event),
      docInits,
      navigations,
      wsFrames,
      requests,
      ...(otherFrames.length ? { otherFrames } : {}),
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
    const frames = ev.wsFrames.filter((f) =>
      this.peerSocketTracking
        ? UPDATE_EVENTS.has(f.event)
        : f.event === "page" || f.event === "css",
    );
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
   * target is navigated to once more. A reload that destroys the context
   * while arming (Next HMR after a rename is restored, #4752) is awaited the
   * same way. Both share one re-navigation per
   * check; every step is recorded, and a second interruption is a failure.
   */
  async navigateAndAwait(path, predicate, timeoutMs, tokenPrefix) {
    const navStart = this.clock();
    const m = this.mark();
    const recovery = [];
    const transientReads = [];
    const deadline = navStart + BigInt(Math.round(timeoutMs * 1e6));
    const newToken = () => `${tokenPrefix}-${Math.random().toString(16).slice(2, 10)}`;
    let token = null;
    const interrupted = async (stage, msg) => ({
      ok: false,
      stage,
      error: msg,
      docUUID: state?.docUUID ?? null,
      httpStatus: state?.httpStatus ?? null,
      transientReads,
      recovery,
      evidence: this.since(m, navStart),
      sinceNavigationMs: nsToMs(this.clock() - navStart),
      failure: await boundedObservation(this.failureSnapshot(predicate), 500),
    });
    let state = null;
    let armed;
    let reuse = null;
    for (;;) {
      state = reuse;
      reuse = null;
      while (state === null) {
        try {
          state = await this.prepare(path);
        } catch (error) {
          const msg = String(error?.message ?? error);
          const aborted =
            error?.code === PREPARE_INTERRUPTED ||
            /ERR_ABORTED|interrupted by another navigation|frame was detached/i.test(msg);
          if (!aborted || recovery.length >= 1) return interrupted("navigation", msg);
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
      // zfb dev answers "404 — page not in cache" while a rebuild is in flight (and for removed URLs).
      // A presence check never accepts it: re-read by navigation only, bounded by the check timeout.
      while (state.notInCache && this.clock() < deadline) {
        transientReads.push({
          at: isoNow(),
          httpStatus: state.httpStatus ?? null,
          evidence: "transient-not-in-cache",
        });
        await new Promise((r) => setTimeout(r, TRANSIENT_REREAD_MS));
        try {
          state = await this.prepare(path);
        } catch (error) {
          transientReads.push({ at: isoNow(), error: String(error?.message ?? error) });
        }
      }
      if (state.notInCache) {
        return {
          ok: false,
          stage: "transient",
          transient: "transient-not-in-cache",
          transientReads,
          httpStatus: state.httpStatus ?? null,
          recovery,
          sinceNavigationMs: nsToMs(this.clock() - navStart),
          evidence: this.since(m, navStart),
          failure: await boundedObservation(this.failureSnapshot(predicate), 500),
        };
      }
      // A fresh token per arm, so a notify from an interrupted arm's persisted record stays distinct.
      token = newToken();
      try {
        armed = await this.arm(token, predicate);
        break;
      } catch (error) {
        this.armedToken = null;
        const msg = String(error?.message ?? error);
        if (!/Execution context was destroyed|frame was detached|Cannot find context/i.test(msg))
          throw error;
        if (recovery.length >= 1) return interrupted("arm", msg);
        const remainingMs = Math.max(1, timeoutMs - nsToMs(this.clock() - navStart));
        const resulting = await this.awaitResultingDocument(remainingMs);
        const onTarget =
          resulting.ok && new URL(resulting.url).pathname === new URL(this.url(path)).pathname;
        recovery.push({
          at: isoNow(),
          armInterruptedBy: msg.split("\n")[0],
          resulting,
          usedResultingDocument: onTarget,
        });
        if (onTarget) reuse = resulting.state;
      }
    }
    const base = {
      transientReads,
      docUUID: state.docUUID,
      httpStatus: state.httpStatus ?? null,
      firstReadActual: armed.actual ?? null,
      recovery,
    };
    if (this.peerSocketTracking) {
      base.armedDocUUID = armed.docUUID;
      base.observedUrl = armed.observedUrl;
      if (
        !state.docUUID ||
        state.docUUID !== armed.docUUID ||
        state.url !== this.url(path) ||
        armed.observedUrl !== state.url ||
        this.clock() > deadline
      ) {
        if (armed.armed) await this.disarm();
        return { ...base, ok: false, stage: "fresh-document-mismatch" };
      }
    }
    if (!armed.armed && armed.precondition) {
      this.armedToken = null;
      const sinceNavigationMs = nsToMs(this.clock() - navStart);
      const page = await this.failureSnapshot(predicate);
      const transient = page?.notInCache === true;
      return {
        ok: !transient,
        stage: transient ? "transient" : "confirmed",
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
        stage: "arm",
        error: armed.error ?? "arm failed",
        ...base,
        evidence: this.since(m, navStart),
      };
    // The wait gets what is left of the check budget after navigation and transient re-reads.
    const remainingMs = Math.max(1, timeoutMs - nsToMs(this.clock() - navStart));
    const waited = await this.waitNotify(token, remainingMs, (p) => this.onStray(p));
    const sinceNavigationMs = nsToMs(this.clock() - navStart);
    const failure =
      waited.kind === "notify"
        ? null
        : await boundedObservation(this.failureSnapshot(predicate), 500);
    const page = failure ?? (await this.failureSnapshot(predicate));
    // Successful notification leaves its acknowledged registration in sessionStorage.
    // Retain that token until the next prepare clears this exact registration.
    if (waited.kind !== "notify") await this.disarm();
    // zfb dev's "404 — page not in cache … rebuild may still be in flight" is a transient state, never a pass.
    const transient = page?.notInCache === true || failure?.notInCache === true;
    return {
      ok:
        waited.kind === "notify" &&
        !transient &&
        (!this.peerSocketTracking ||
          (waited.payload.producer?.designatedMain === true &&
            waited.payload.producer.frameUrl === state.url &&
            waited.payload.producer.latestDocUUID === armed.docUUID &&
            waited.payload.producer.receivedNs === String(waited.t1) &&
            waited.payload.token === token &&
            waited.payload.docUUID === armed.docUUID &&
            waited.payload.armedDocUUID === armed.docUUID &&
            waited.payload.url === state.url &&
            waited.t1 <= deadline)),
      stage:
        waited.kind === "notify"
          ? transient
            ? "transient"
            : "confirmed"
          : transient
            ? "transient"
            : "wait",
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

  /**
   * Read-only look at the OPEN document, with no navigation, reload or HTTP
   * request: URL, title, docUUID, declared error markers, and the status of
   * the document's original navigation (labelled as such; null when unknown).
   */
  async inspectOpen({ errorMarkers = [] } = {}) {
    try {
      const view = await this.page.evaluate((markers) => {
        const text = document.body ? document.body.innerText : "";
        const nav = performance.getEntriesByType("navigation")[0];
        return {
          url: location.href,
          title: document.title,
          markersFound: markers.filter(
            (m) => text.indexOf(m) !== -1 || document.title.indexOf(m) !== -1,
          ),
          notInCache: text.indexOf("page not in cache") !== -1,
          bodyExcerpt: text.slice(0, 600),
          // The status of this document's original navigation, before any edit: not evidence of a later state.
          originalNavigationStatus:
            nav && typeof nav.responseStatus === "number" && nav.responseStatus > 0
              ? nav.responseStatus
              : null,
          docUUID: window.__dbo ? window.__dbo.state().docUUID : null,
        };
      }, errorMarkers);
      return { ok: true, open: true, ...view };
    } catch (error) {
      return { ok: false, open: true, error: String(error?.message ?? error) };
    }
  }

  /** Read-only predicate evaluation in the current document (no arming). */
  async evaluateNow(predicate) {
    return this.page.evaluate(
      (p) =>
        window.__dbo ? window.__dbo.evaluate(p) : { matched: false, error: "observer missing" },
      predicate,
    );
  }

  supplemental(check, timeoutMs, goneOpts = {}) {
    if (check.kind === "gone") return this.goneCheck(check, goneOpts);
    return this.navigateAndAwait(check.path, check.predicate, timeoutMs, "supp");
  }

  /** One fresh navigation and a read-only look at the result (status, title, excerpt, marker text). */
  async observePage(path, { markerText = null, errorMarkers = [], absentSelector = null } = {}) {
    const navStart = this.clock();
    try {
      await this.disarm();
      const response = await this.page.goto(this.url(path), { waitUntil: "load", timeout: 30000 });
      const view = await this.page.evaluate(
        ([marker, markers, selector]) => {
          const text = document.body ? document.body.innerText : "";
          return {
            url: location.href,
            title: document.title,
            containsMarker: marker ? text.indexOf(marker) !== -1 : null,
            selectorPresent: selector ? document.querySelector(selector) !== null : null,
            markersFound: markers.filter(
              (m) => text.indexOf(m) !== -1 || document.title.indexOf(m) !== -1,
            ),
            notInCache: text.indexOf("page not in cache") !== -1,
            bodyExcerpt: text.slice(0, 600),
          };
        },
        [markerText, errorMarkers, absentSelector],
      );
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
   * A removed URL passes when it answers 404/410 without the removed content
   * (zfb dev's steady body for a removed URL is the generic "page not in
   * cache" 404). Basis: the preceding settle observed the write's own native
   * update, or a second read after `quietMs` returns the same 404 with no
   * content. A 200 error page or still-rendered content fails.
   */
  async goneCheck(check, { quietMs = 1500, nativeUpdateConfirmed = false } = {}) {
    const read = () =>
      this.observePage(check.path, {
        markerText: check.absentText ?? null,
        absentSelector: check.absentSelector ?? null,
      });
    const looksGone = goneRead;
    const reads = [await read()];
    let basis = null;
    if (looksGone(reads[0]) && nativeUpdateConfirmed) basis = "settle-native-update";
    else if (looksGone(reads[0])) {
      await new Promise((r) => setTimeout(r, quietMs));
      reads.push(await read());
      if (looksGone(reads[1])) basis = "stable-reads";
    }
    const last = reads[reads.length - 1];
    return {
      ...last,
      kind: "gone",
      ok: basis !== null,
      basis,
      statusOk: last.httpStatus === 404 || last.httpStatus === 410,
      stableReads: reads.filter(looksGone).length,
      reads: reads.map((v) => ({
        httpStatus: v.httpStatus ?? null,
        title: v.title ?? null,
        notInCache: v.notInCache ?? null,
        containsMarker: v.containsMarker ?? null,
        selectorPresent: v.selectorPresent ?? null,
        error: v.error ?? null,
      })),
    };
  }

  liveCheck(check, timeoutMs, goneOpts = {}) {
    if (check.kind === "gone") return this.goneCheck(check, goneOpts);
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
