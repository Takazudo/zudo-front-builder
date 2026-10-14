// @verification: issue #4166 navigation-stall diagnostics; measurement only, never a gate or CI step.
// Profiles soft navigations into and out of the EN Wind compatibility page
// (#4156) against controls, keeping per-round evidence. See NAV-STALL.md.
import { chromium } from "@playwright/test";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { arch, cpus, platform, release, totalmem } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

const REPO_ROOT = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const DOCS_ROOT = join(REPO_ROOT, "docs");
const env = process.env;
const int = (value, fallback) => {
  if (value === undefined || value === "") return fallback;
  const parsed = Number.parseInt(value, 10);
  if (!Number.isFinite(parsed) || parsed < 0)
    throw new Error(`expected a non-negative integer, got ${value}`);
  return parsed;
};
const flag = (value, fallback) => (value === undefined || value === "" ? fallback : value === "1");

// NAV_STALL_ORIGINS="static=http://127.0.0.1:4334,wrangler=http://127.0.0.1:4333"
// interleaves several hosts serving one built tree in the same run; otherwise
// DOCS_VERIFY_ORIGIN is the single host, labelled by NAV_STALL_HOST.
const hosts = (
  env.NAV_STALL_ORIGINS
    ? env.NAV_STALL_ORIGINS.split(",").map((entry) => {
        const at = entry.indexOf("=");
        if (at < 1) throw new Error(`NAV_STALL_ORIGINS entry must be label=origin, got ${entry}`);
        return { label: entry.slice(0, at).trim(), origin: entry.slice(at + 1).trim() };
      })
    : [
        {
          label: env.NAV_STALL_HOST ?? "unlabeled",
          origin: env.DOCS_VERIFY_ORIGIN ?? "http://127.0.0.1:4333",
        },
      ]
).map((host) => ({ ...host, origin: host.origin.replace(/\/$/, "") }));
const outputPath = resolve(env.NAV_STALL_OUTPUT ?? "/tmp/zfb-nav-stall/summary.json");
const traceDir = resolve(env.NAV_STALL_TRACE_DIR ?? join(dirname(outputPath), "traces"));
const ROUNDS = int(env.NAV_STALL_ROUNDS, 4);
const WARMUP = int(env.NAV_STALL_WARMUP, 1);
const TRACE = flag(env.NAV_STALL_TRACE, true);
const CPU_PROFILE = flag(env.NAV_STALL_TRACE_CPU_PROFILE, false);
const CENSUS = flag(env.NAV_STALL_CENSUS, true);
const LAYER_TREE = flag(env.NAV_STALL_LAYER_TREE, false);
const HEADED = flag(env.NAV_STALL_HEADED, false);
// verify.mjs, where the Wrangler-only gap was observed, runs with "reduce".
const REDUCED_MOTION = env.NAV_STALL_REDUCED_MOTION ?? "reduce";
// "disabled" removes document.startViewTransition before the router loads, so
// the router takes its simulated-transition path (a diagnostic toggle only).
const VIEW_TRANSITIONS = env.NAV_STALL_VIEW_TRANSITIONS ?? "native";
const DPR = Number(env.NAV_STALL_DPR ?? 1);
// Controlled CSS experiment without touching product CSS. Adopted as a
// constructable sheet so it survives the router's head/body swaps.
const EXTRA_CSS = env.NAV_STALL_EXTRA_CSS ?? "";
const QUIET_MS = int(env.NAV_STALL_QUIET_MS, 1000);
// Liveness guardrails, not assertions: verify.mjs's 20 s settle budget covers
// two <= 7.5 s stalls (#4156); 30 s leaves room for a worse host. Exceeding one
// records the round as unsettled/unswapped instead of failing the run.
const SETTLE_BUDGET_MS = int(env.NAV_STALL_SETTLE_BUDGET_MS, 30_000);
const SWAP_BUDGET_MS = int(env.NAV_STALL_SWAP_BUDGET_MS, 30_000);
const FREEZE_MS = 4000;
if (!["reduce", "no-preference"].includes(REDUCED_MOTION))
  throw new Error(
    `NAV_STALL_REDUCED_MOTION must be reduce or no-preference, got ${REDUCED_MOTION}`,
  );
if (!["native", "disabled"].includes(VIEW_TRANSITIONS))
  throw new Error(`NAV_STALL_VIEW_TRANSITIONS must be native or disabled, got ${VIEW_TRANSITIONS}`);

const DESKTOP = { width: 1440, height: 1000 };
const MOBILE = { width: 390, height: 844 };
// "enter" soft-navigates neighbour -> target, "leave" target -> neighbour.
const ALL_CASES = {
  "en-desktop-compat": {
    role: "target",
    viewport: DESKTOP,
    neighbour: "/docs/zudo-wind/coming-from-tailwind/",
    target: "/docs/zudo-wind/compatibility/",
  },
  "ja-desktop-compat": {
    role: "control",
    viewport: DESKTOP,
    neighbour: "/ja/docs/zudo-wind/coming-from-tailwind/",
    target: "/ja/docs/zudo-wind/compatibility/",
  },
  "en-mobile-compat": {
    role: "control",
    viewport: MOBILE,
    neighbour: "/docs/zudo-wind/coming-from-tailwind/",
    target: "/docs/zudo-wind/compatibility/",
  },
  "en-desktop-ordinary": {
    role: "control",
    viewport: DESKTOP,
    neighbour: "/docs/zudo-wind/overview/",
    target: "/docs/zudo-wind/configuration/",
  },
};
const caseIds = env.NAV_STALL_CASES
  ? env.NAV_STALL_CASES.split(",").map((id) => id.trim())
  : Object.keys(ALL_CASES);
for (const id of caseIds) if (!ALL_CASES[id]) throw new Error(`unknown NAV_STALL_CASES id ${id}`);

const DEFAULT_CATEGORIES = [
  "devtools.timeline",
  "disabled-by-default-devtools.timeline",
  "disabled-by-default-devtools.timeline.frame",
  "toplevel",
  "blink.user_timing",
  "loading",
  "v8.execute",
];
const traceCategories = env.NAV_STALL_TRACE_CATEGORIES
  ? env.NAV_STALL_TRACE_CATEGORIES.split(",")
  : [
      ...DEFAULT_CATEGORIES,
      ...(CPU_PROFILE
        ? ["disabled-by-default-v8.cpu_profiler", "disabled-by-default-devtools.timeline.stack"]
        : []),
    ];
// Main-thread trace events summed per navigation phase. Names follow the
// Chromium trace; topEvents in each round also lists whatever else dominated.
const KEY_EVENTS = [
  "RunTask",
  "Layerize",
  "Layout",
  "UpdateLayoutTree",
  "PrePaint",
  "Paint",
  "UpdateLayerTree",
  "Commit",
  "ParseHTML",
  "FunctionCall",
  "EvaluateScript",
  "v8.compile",
  "MajorGC",
  "MinorGC",
];
const ZFB_EVENTS = [
  "zfb:before-preparation",
  "zfb:after-preparation",
  "zfb:before-swap",
  "zfb:after-swap",
  "zfb:page-load",
  "zfb:navigation-aborted",
];

const sha256 = (data) => createHash("sha256").update(data).digest("hex");
const round1 = (n) => (n == null || !Number.isFinite(n) ? null : Math.round(n * 10) / 10);
const median = (values) => {
  const sorted = values.filter((v) => v != null && Number.isFinite(v)).sort((a, b) => a - b);
  if (sorted.length === 0) return null;
  const mid = sorted.length >> 1;
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
};
const tryRun = (cmd, args, cwd = REPO_ROOT) => {
  try {
    return execFileSync(cmd, args, {
      cwd,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    return null;
  }
};
const packageVersion = (name) => {
  const file = join(DOCS_ROOT, "node_modules", name, "package.json");
  return existsSync(file) ? JSON.parse(readFileSync(file, "utf8")).version : null;
};

// ── In-page instrumentation (runs before any page script) ──────────────────
function instrument({ disableViewTransitions, zfbEvents, extraCss }) {
  if (extraCss) {
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(extraCss);
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
  }
  const s = {
    events: [],
    longTasks: [],
    loaf: [],
    gaps: [],
    prev: performance.now(),
    lastGap: performance.now(),
    maxGap: 0,
    swapped: false,
    pageLoaded: false,
  };
  window.__navStall = s;
  // The default 250-entry buffer can fill on a docs page and drop the
  // navigation's own document fetch.
  performance.setResourceTimingBufferSize(2000);
  const mark = (name, detail) => {
    const t = performance.now();
    s.events.push({ name, t, ...detail });
    try {
      performance.mark(`nav-stall:${name}`);
    } catch {}
    return t;
  };
  s.reset = () => {
    s.events = [];
    s.longTasks = [];
    s.loaf = [];
    s.gaps = [];
    s.maxGap = 0;
    s.swapped = false;
    s.pageLoaded = false;
    performance.clearResourceTimings();
  };
  for (const method of ["pushState", "replaceState"]) {
    const original = History.prototype[method];
    History.prototype[method] = function (state, title, url) {
      mark(method, { url: url == null ? null : String(url) });
      return original.apply(this, arguments);
    };
  }
  if (disableViewTransitions) {
    delete Document.prototype.startViewTransition;
  } else if (typeof Document.prototype.startViewTransition === "function") {
    const original = Document.prototype.startViewTransition;
    Document.prototype.startViewTransition = function (arg) {
      mark("vt-start");
      const wrap =
        (update) =>
        async (...args) => {
          mark("vt-update-start");
          try {
            return await update?.(...args);
          } finally {
            mark("vt-update-end");
          }
        };
      const wrapped =
        typeof arg === "function"
          ? wrap(arg)
          : arg && typeof arg === "object"
            ? { ...arg, update: wrap(arg.update) }
            : arg;
      const vt = original.call(this, wrapped);
      for (const key of ["updateCallbackDone", "ready", "finished"])
        vt[key]?.then(
          () => mark(`vt-${key}`),
          () => mark(`vt-${key}-rejected`),
        );
      return vt;
    };
  }
  const originalFetch = window.fetch;
  window.fetch = function (input, init) {
    const url = typeof input === "string" ? input : (input?.url ?? String(input));
    mark("fetch-start", { url });
    const promise = originalFetch.apply(this, arguments);
    promise.then(
      (res) => mark("fetch-headers", { url, status: res.status }),
      () => mark("fetch-error", { url }),
    );
    return promise;
  };
  window.addEventListener(
    "click",
    (event) => {
      const link = event.target?.closest?.("a[href]");
      if (link) mark("click", { href: link.getAttribute("href") });
    },
    true,
  );
  for (const name of zfbEvents)
    document.addEventListener(
      name,
      () => {
        mark(name);
        if (name === "zfb:after-swap") s.swapped = true;
        if (name === "zfb:page-load") s.pageLoaded = true;
      },
      true,
    );
  const observe = (type, onEntry) => {
    try {
      new PerformanceObserver((list) => list.getEntries().forEach(onEntry)).observe({
        type,
        buffered: true,
      });
    } catch {}
  };
  observe("longtask", (e) => s.longTasks.push({ start: e.startTime, duration: e.duration }));
  observe("long-animation-frame", (e) =>
    s.loaf.push({
      start: e.startTime,
      duration: e.duration,
      renderStart: e.renderStart,
      styleAndLayoutStart: e.styleAndLayoutStart,
      blockingDuration: e.blockingDuration,
      scripts: (e.scripts ?? [])
        .slice()
        .sort((a, b) => b.duration - a.duration)
        .slice(0, 3)
        .map((sc) => ({ invoker: sc.invoker, sourceURL: sc.sourceURL, duration: sc.duration })),
    }),
  );
  // Same rAF quiet detector as verify.mjs: every frame gap above 50 ms counts.
  const tick = (now) => {
    const gap = now - s.prev;
    if (gap > 50) {
      s.lastGap = now;
      s.gaps.push([s.prev, gap]);
      if (s.gaps.length > 500) s.gaps.shift();
    }
    s.maxGap = Math.max(s.maxGap, gap);
    s.prev = now;
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

function pageCensus() {
  const all = document.querySelectorAll("*");
  const c = {
    lang: document.documentElement.lang,
    nodes: all.length,
    tables: document.querySelectorAll("table").length,
    tableRows: document.querySelectorAll("tr").length,
    code: document.querySelectorAll("code").length,
    links: document.querySelectorAll("a[href]").length,
    sticky: 0,
    fixed: 0,
    willChange: 0,
    transform: 0,
    backdropFilter: 0,
    filter: 0,
    opacityBelowOne: 0,
    zIndexPositioned: 0,
    contain: 0,
    contentVisibility: 0,
    scrollContainers: 0,
    overflowingScrollContainers: 0,
    viewTransitionNames: [],
  };
  for (const el of all) {
    const cs = getComputedStyle(el);
    if (cs.position === "sticky") c.sticky++;
    if (cs.position === "fixed") c.fixed++;
    if (cs.willChange !== "auto") c.willChange++;
    if (cs.transform !== "none") c.transform++;
    if ((cs.backdropFilter ?? "none") !== "none") c.backdropFilter++;
    if (cs.filter !== "none") c.filter++;
    if (Number(cs.opacity) < 1) c.opacityBelowOne++;
    if (cs.position !== "static" && cs.zIndex !== "auto") c.zIndexPositioned++;
    if (cs.contain !== "none") c.contain++;
    if ((cs.contentVisibility ?? "visible") !== "visible") c.contentVisibility++;
    const scrolls = /(auto|scroll)/.test(cs.overflowX + cs.overflowY);
    if (scrolls) {
      c.scrollContainers++;
      if (el.scrollWidth > el.clientWidth || el.scrollHeight > el.clientHeight)
        c.overflowingScrollContainers++;
    }
    const vtName = cs.viewTransitionName;
    if (vtName && vtName !== "none") c.viewTransitionNames.push(vtName);
  }
  const main = document.querySelector("main");
  c.document = {
    scrollWidth: document.documentElement.scrollWidth,
    scrollHeight: document.documentElement.scrollHeight,
    clientWidth: document.documentElement.clientWidth,
  };
  c.main = main ? { scrollWidth: main.scrollWidth, scrollHeight: main.scrollHeight } : null;
  c.tableWrappers = [...document.querySelectorAll("table")].map((table) => {
    const wrapper = table.parentElement;
    return {
      wrapperClientWidth: wrapper?.clientWidth ?? null,
      tableScrollWidth: table.scrollWidth,
      tableHeight: Math.round(table.getBoundingClientRect().height),
    };
  });
  c.overflowingTables = c.tableWrappers.filter(
    (t) => t.wrapperClientWidth != null && t.tableScrollWidth > t.wrapperClientWidth,
  ).length;
  const sidebar = document.querySelector("#desktop-sidebar");
  c.desktopSidebar = sidebar
    ? {
        display: getComputedStyle(sidebar).display,
        links: sidebar.querySelectorAll("a[href]").length,
        nodes: sidebar.querySelectorAll("*").length,
      }
    : null;
  return c;
}

// ── CDP tracing ─────────────────────────────────────────────────────────────
async function startTrace(cdp) {
  await cdp.send("Tracing.start", {
    transferMode: "ReturnAsStream",
    traceConfig: { recordMode: "recordAsMuchAsPossible", includedCategories: traceCategories },
  });
}
async function stopTrace(cdp) {
  const complete = new Promise((resolveComplete) =>
    cdp.once("Tracing.tracingComplete", resolveComplete),
  );
  await cdp.send("Tracing.end");
  const { stream } = await complete;
  const chunks = [];
  for (;;) {
    const { data, eof, base64Encoded } = await cdp.send("IO.read", {
      handle: stream,
      size: 1 << 20,
    });
    chunks.push(base64Encoded ? Buffer.from(data, "base64").toString("utf8") : data);
    if (eof) break;
  }
  await cdp.send("IO.close", { handle: stream });
  return chunks.join("");
}

// Attribute main-thread trace work to the navigation phases delimited by the
// nav-stall:* user-timing marks the page emitted.
function analyzeTrace(text) {
  const parsed = JSON.parse(text);
  const events = Array.isArray(parsed) ? parsed : (parsed.traceEvents ?? []);
  const threadNames = new Map();
  for (const e of events)
    if (e.ph === "M" && e.name === "thread_name")
      threadNames.set(`${e.pid}:${e.tid}`, e.args?.name);
  const marks = events.filter((e) => typeof e.name === "string" && e.name.startsWith("nav-stall:"));
  const clickMark = marks.find((e) => e.name === "nav-stall:click");
  if (!clickMark) return { error: "no nav-stall:click mark in trace" };
  const pid = clickMark.pid;
  const mainTid = [...threadNames.entries()].find(
    ([key, name]) => key.startsWith(`${pid}:`) && name === "CrRendererMain",
  )?.[0];
  const tid = mainTid ? Number(mainTid.split(":")[1]) : clickMark.tid;
  const t0 = clickMark.ts;
  const firstMark = (name) =>
    marks.find((e) => e.name === `nav-stall:${name}` && e.ts >= t0)?.ts ?? null;
  const boundaries = [
    ["click", t0],
    ["pushState", firstMark("pushState")],
    ["vt-start", firstMark("vt-start")],
    ["zfb:before-swap", firstMark("zfb:before-swap")],
    ["zfb:after-swap", firstMark("zfb:after-swap")],
    ["zfb:page-load", firstMark("zfb:page-load")],
    ["vt-finished", firstMark("vt-finished")],
  ]
    .filter(([, ts]) => ts != null)
    .sort((a, b) => a[1] - b[1]);
  const phaseOf = (ts) => {
    if (ts < t0) return "before-click";
    let phase = "click";
    for (const [name, at] of boundaries) if (ts >= at) phase = name;
    return `after:${phase}`;
  };
  // Complete events plus B/E pairs, main renderer thread only.
  const spans = [];
  const open = [];
  for (const e of events) {
    if (e.pid !== pid || e.tid !== tid) continue;
    if (e.ph === "X" && typeof e.dur === "number")
      spans.push({ name: e.name, ts: e.ts, dur: e.dur, tdur: e.tdur });
    else if (e.ph === "B") open.push(e);
    else if (e.ph === "E") {
      const begin = open.pop();
      if (begin) spans.push({ name: begin.name, ts: begin.ts, dur: e.ts - begin.ts });
    }
  }
  const rel = (ts) => round1((ts - t0) / 1000);
  const totals = {};
  const byPhase = {};
  const nameTotals = new Map();
  for (const span of spans) {
    if (span.ts < t0) continue;
    nameTotals.set(span.name, (nameTotals.get(span.name) ?? 0) + span.dur);
    if (!KEY_EVENTS.includes(span.name)) continue;
    const ms = span.dur / 1000;
    totals[span.name] ??= { totalMs: 0, maxMs: 0, count: 0 };
    totals[span.name].totalMs += ms;
    totals[span.name].maxMs = Math.max(totals[span.name].maxMs, ms);
    totals[span.name].count++;
    const phase = phaseOf(span.ts);
    byPhase[phase] ??= {};
    byPhase[phase][span.name] = (byPhase[phase][span.name] ?? 0) + ms;
  }
  for (const t of Object.values(totals)) {
    t.totalMs = round1(t.totalMs);
    t.maxMs = round1(t.maxMs);
  }
  for (const phase of Object.values(byPhase))
    for (const name of Object.keys(phase)) phase[name] = round1(phase[name]);
  const longTasks = spans
    .filter((s) => s.name === "RunTask" && s.ts >= t0 && s.dur >= 50_000)
    .sort((a, b) => b.dur - a.dur)
    .slice(0, 10)
    .map((task) => {
      const inside = {};
      for (const s of spans)
        if (
          s !== task &&
          KEY_EVENTS.includes(s.name) &&
          s.name !== "RunTask" &&
          s.ts >= task.ts &&
          s.ts + s.dur <= task.ts + task.dur
        )
          inside[s.name] = (inside[s.name] ?? 0) + s.dur / 1000;
      const top = Object.entries(inside)
        .sort((a, b) => b[1] - a[1])
        .slice(0, 4)
        .map(([name, ms]) => ({ name, ms: round1(ms) }));
      return {
        startMs: rel(task.ts),
        durationMs: round1(task.dur / 1000),
        // Thread (CPU) time; far below durationMs means the thread was descheduled.
        cpuMs: typeof task.tdur === "number" ? round1(task.tdur / 1000) : null,
        phase: phaseOf(task.ts),
        top,
      };
    });
  const topEvents = [...nameTotals.entries()]
    .sort((a, b) => b[1] - a[1])
    .slice(0, 15)
    .map(([name, us]) => ({ name, totalMs: round1(us / 1000) }));
  return {
    phases: Object.fromEntries(boundaries.map(([name, ts]) => [name, rel(ts)])),
    totals,
    byPhase,
    longTasks,
    topEvents,
  };
}

// ── Navigation steps ────────────────────────────────────────────────────────
async function settle(page) {
  const started = Date.now();
  try {
    await page.waitForFunction(
      (quiet) => {
        const s = window.__navStall;
        const now = performance.now();
        return !!s && now - s.lastGap >= quiet && now - s.prev < 100;
      },
      QUIET_MS,
      { polling: 100, timeout: SETTLE_BUDGET_MS },
    );
    return { settled: true, waitedMs: Date.now() - started };
  } catch (error) {
    if (error?.name !== "TimeoutError") throw error;
    return { settled: false, waitedMs: Date.now() - started };
  }
}

async function softNavigate(page, cdp, origin, path, traceFile) {
  const pre = await settle(page);
  await page.evaluate((path) => {
    window.__navStall.reset();
    const a = document.createElement("a");
    a.href = path;
    a.textContent = "nav-stall navigation";
    a.id = "nav-stall-link";
    document.querySelector("main").prepend(a);
  }, path);
  if (traceFile) await startTrace(cdp);
  let swapError = null;
  try {
    await page.locator("#nav-stall-link").click();
    await page.waitForFunction(() => window.__navStall.pageLoaded === true, null, {
      polling: 100,
      timeout: SWAP_BUDGET_MS,
    });
  } catch (error) {
    if (error?.name !== "TimeoutError") throw error;
    swapError = `no zfb:page-load within ${SWAP_BUDGET_MS} ms`;
  }
  const post = await settle(page);
  let trace = null;
  if (traceFile) {
    const text = await stopTrace(cdp);
    writeFileSync(traceFile, gzipSync(text));
    try {
      trace = analyzeTrace(text);
    } catch (error) {
      trace = { error: `trace analysis failed: ${error.message}` };
    }
  }
  const data = await page.evaluate((target) => {
    const s = window.__navStall;
    const resources = performance
      .getEntriesByType("resource")
      .filter((r) => new URL(r.name).pathname === target)
      .map((r) => ({
        initiatorType: r.initiatorType,
        startTime: r.startTime,
        responseStart: r.responseStart,
        responseEnd: r.responseEnd,
        transferSize: r.transferSize,
        decodedBodySize: r.decodedBodySize,
      }));
    return {
      events: s.events,
      longTasks: s.longTasks,
      loaf: s.loaf,
      gaps: s.gaps,
      lastGap: s.lastGap,
      maxGap: s.maxGap,
      resources,
      now: performance.now(),
    };
  }, path);
  await page.locator("#nav-stall-link").evaluateAll((nodes) => nodes.forEach((n) => n.remove()));
  return { pre, post, swapError, trace, data, url: page.url(), expectedUrl: origin + path };
}

function summarizeNavigation(result) {
  const { data } = result;
  const first = (name, after = -Infinity) =>
    data.events.find((e) => e.name === name && e.t >= after)?.t ?? null;
  const click = first("click");
  const at = (name) => (click == null ? null : first(name, click));
  const pushState = at("pushState");
  const beforeSwap = at("zfb:before-swap");
  const afterSwap = at("zfb:after-swap");
  const pageLoad = at("zfb:page-load");
  const diff = (a, b) => (a == null || b == null ? null : round1(b - a));
  const tasks = data.longTasks.filter((t) => click == null || t.start + t.duration >= click);
  const longest = tasks.reduce((m, t) => Math.max(m, t.duration), 0);
  const doc = data.resources.filter((r) => click == null || r.startTime >= click).at(-1) ?? null;
  const quietSince =
    click == null || !result.post.settled ? null : Math.max(data.lastGap, pageLoad ?? click);
  const rel = (t) => (click == null || t == null ? null : round1(t - click));
  return {
    url: result.url,
    urlMatches: result.url === result.expectedUrl,
    swapError: result.swapError,
    preSettle: result.pre,
    postSettle: result.post,
    timeline: {
      clickToFetchStartMs: diff(click, at("fetch-start")),
      fetchResponseStartMs: doc ? rel(doc.responseStart) : null,
      fetchResponseEndMs: doc ? rel(doc.responseEnd) : null,
      documentBytes: doc?.decodedBodySize ?? null,
      clickToPushStateMs: diff(click, pushState),
      pushStateToBeforeSwapMs: diff(pushState, beforeSwap),
      vtStartToUpdateStartMs: diff(at("vt-start"), at("vt-update-start")),
      beforeSwapToAfterSwapMs: diff(beforeSwap, afterSwap),
      afterSwapToPageLoadMs: diff(afterSwap, pageLoad),
      clickToPageLoadMs: diff(click, pageLoad),
      clickToSettledMs: rel(quietSince),
    },
    events: data.events
      .filter((e) => click == null || e.t >= click)
      .filter((e) => e.name !== "replaceState" || e.url != null)
      .map((e) => ({ ...e, t: rel(e.t) })),
    longestTaskMs: round1(longest),
    freezes: tasks.filter((t) => t.duration >= FREEZE_MS).length,
    longTasks: tasks
      .slice()
      .sort((a, b) => b.duration - a.duration)
      .slice(0, 10)
      .map((t) => ({ startMs: rel(t.start), durationMs: round1(t.duration) })),
    loaf: data.loaf
      .filter((f) => click == null || f.start + f.duration >= click)
      .sort((a, b) => b.duration - a.duration)
      .slice(0, 5)
      .map((f) => ({
        startMs: rel(f.start),
        durationMs: round1(f.duration),
        renderMs: f.renderStart ? round1(f.start + f.duration - f.renderStart) : null,
        styleAndLayoutMs: f.styleAndLayoutStart
          ? round1(f.start + f.duration - f.styleAndLayoutStart)
          : null,
        blockingMs: round1(f.blockingDuration),
        scripts: f.scripts.map((sc) => ({ ...sc, duration: round1(sc.duration) })),
      })),
    maxFrameGapMs: round1(data.maxGap),
    frameGaps: data.gaps
      .filter(([start, gap]) => gap >= 100 && (click == null || start + gap >= click))
      .slice(-20)
      .map(([start, gap]) => [rel(start), round1(gap)]),
    trace: result.trace,
  };
}

// ── Run ─────────────────────────────────────────────────────────────────────
const assets = new Map();
const summary = {
  issue: "https://github.com/Takazudo/zudo-front-builder/issues/4166",
  generatedAt: new Date().toISOString(),
  source: {
    sha: tryRun("git", ["rev-parse", "HEAD"]),
    branch: tryRun("git", ["rev-parse", "--abbrev-ref", "HEAD"]),
    dirty: (tryRun("git", ["status", "--porcelain"]) ?? "") !== "",
  },
  packages: Object.fromEntries(
    [
      "@takazudo/zfb",
      "@takazudo/zfb-runtime",
      "@takazudo/zudo-doc",
      "@takazudo/zfb-adapter-cloudflare",
      "wrangler",
    ].map((name) => [name, packageVersion(name)]),
  ),
  zfbBinary: tryRun(join(DOCS_ROOT, "node_modules/.bin/zfb"), ["--version"], DOCS_ROOT),
  machine: {
    platform: platform(),
    arch: arch(),
    osRelease: release(),
    cpu: cpus()[0]?.model ?? null,
    cpuCount: cpus().length,
    memoryGb: round1(totalmem() / 2 ** 30),
    node: process.version,
  },
  settings: {
    hosts,
    cases: Object.fromEntries(caseIds.map((id) => [id, ALL_CASES[id]])),
    rounds: ROUNDS,
    warmup: WARMUP,
    order: "each round runs every host x case once; the order rotates by round",
    headless: !HEADED,
    deviceScaleFactor: DPR,
    reducedMotion: REDUCED_MOTION,
    viewTransitions: VIEW_TRANSITIONS,
    extraCss: EXTRA_CSS || null,
    trace: TRACE ? { categories: traceCategories, dir: traceDir } : false,
    quietMs: QUIET_MS,
    settleBudgetMs: SETTLE_BUDGET_MS,
    swapBudgetMs: SWAP_BUDGET_MS,
    freezeThresholdMs: FREEZE_MS,
  },
  preflight: [],
  transitions: {},
  census: {},
  rounds: [],
  errors: [],
};

async function preflight() {
  const dist = resolve(env.NAV_STALL_DIST ?? join(DOCS_ROOT, "dist"));
  for (const host of hosts)
    for (const id of caseIds)
      for (const path of [ALL_CASES[id].neighbour, ALL_CASES[id].target]) {
        const response = await fetch(host.origin + path, { redirect: "manual" });
        const body = Buffer.from(await response.arrayBuffer());
        if (response.status !== 200)
          throw new Error(
            `${host.label} ${path} answered HTTP ${response.status}; build/serve the docs first`,
          );
        const file = join(dist, path, "index.html");
        const distSha = existsSync(file) ? sha256(readFileSync(file)) : null;
        summary.preflight.push({
          host: host.label,
          path,
          server: response.headers.get("server"),
          htmlSha256: sha256(body),
          distSha256: distSha,
          servedMatchesDist: distSha == null ? null : distSha === sha256(body),
        });
      }
}

function recordAsset(response) {
  const type = response.request().resourceType();
  if (!["script", "stylesheet", "document", "fetch"].includes(type)) return;
  const url = response.url();
  if (assets.has(url)) return;
  const headers = response.headers();
  assets.set(url, {
    type,
    status: response.status(),
    contentEncoding: headers["content-encoding"] ?? null,
    cacheControl: headers["cache-control"] ?? null,
    sha256: null,
  });
  response
    .body()
    .then((body) => {
      assets.get(url).sha256 = sha256(body);
      assets.get(url).bytes = body.length;
    })
    .catch(() => {});
}

async function runSequence(browser, host, id, roundLabel, measured) {
  const spec = ALL_CASES[id];
  const context = await browser.newContext({
    viewport: spec.viewport,
    deviceScaleFactor: DPR,
    reducedMotion: REDUCED_MOTION,
  });
  const record = {
    host: host.label,
    case: id,
    role: spec.role,
    round: roundLabel,
    measured,
    navigations: {},
    errors: [],
  };
  try {
    await context.addInitScript(instrument, {
      disableViewTransitions: VIEW_TRANSITIONS === "disabled",
      zfbEvents: ZFB_EVENTS,
      extraCss: EXTRA_CSS,
    });
    const page = await context.newPage();
    page.on("pageerror", (e) => record.errors.push(`pageerror: ${e.message}`));
    page.on("console", (m) => m.type() === "error" && record.errors.push(`console: ${m.text()}`));
    page.on("response", recordAsset);
    const cdp = await context.newCDPSession(page);
    await page.goto(host.origin + spec.neighbour);
    await page.locator("main").waitFor();
    const key = `${host.label}/${id}`;
    if (!summary.transitions[key])
      summary.transitions[key] = await page.evaluate(() => ({
        enabled: !!document.querySelector('[name="zfb-view-transitions-enabled"]'),
        fallback:
          document
            .querySelector('[name="zfb-view-transitions-fallback"]')
            ?.getAttribute("content") ?? null,
        nativeAvailable: typeof document.startViewTransition === "function",
        prefersReducedMotion: matchMedia("(prefers-reduced-motion: reduce)").matches,
      }));
    for (const [direction, path] of [
      ["enter", spec.target],
      ["leave", spec.neighbour],
    ]) {
      const traceFile =
        TRACE && measured
          ? join(traceDir, `${host.label}-${id}-${roundLabel}-${direction}.json.gz`)
          : null;
      const result = await softNavigate(page, cdp, host.origin, path, traceFile);
      record.navigations[direction] = { path, traceFile, ...summarizeNavigation(result) };
      const censusKey = `${host.label}/${id}/${path}`;
      if (CENSUS && measured && !summary.census[censusKey]) {
        const census = await page.evaluate(pageCensus);
        if (LAYER_TREE) census.compositedLayers = await layerCount(cdp);
        summary.census[censusKey] = census;
      }
    }
  } catch (error) {
    record.errors.push(`run: ${error.message}`);
  } finally {
    await context.close();
  }
  return record;
}

async function layerCount(cdp) {
  const layers = new Promise((resolveLayers) => {
    const onChange = ({ layers }) => {
      if (!layers) return;
      cdp.off("LayerTree.layerTreeDidChange", onChange);
      resolveLayers(layers.length);
    };
    cdp.on("LayerTree.layerTreeDidChange", onChange);
  });
  await cdp.send("LayerTree.enable");
  // A guardrail: the event normally arrives with the next frame.
  const count = await Promise.race([layers, new Promise((r) => setTimeout(() => r(null), 10_000))]);
  await cdp.send("LayerTree.disable");
  return count;
}

const stat = (values) => {
  const clean = values.filter((v) => v != null && Number.isFinite(v));
  return {
    median: round1(median(clean)),
    max: clean.length ? round1(Math.max(...clean)) : null,
    min: clean.length ? round1(Math.min(...clean)) : null,
    values: values.map(round1),
  };
};

function buildStats() {
  const groups = new Map();
  for (const record of summary.rounds) {
    if (!record.measured) continue;
    for (const [direction, nav] of Object.entries(record.navigations)) {
      const key = `${record.host}/${record.case}/${direction}`;
      if (!groups.has(key))
        groups.set(key, {
          host: record.host,
          case: record.case,
          role: record.role,
          direction,
          navs: [],
        });
      groups.get(key).navs.push(nav);
    }
  }
  return Object.fromEntries(
    [...groups].map(([key, g]) => [
      key,
      {
        host: g.host,
        case: g.case,
        role: g.role,
        direction: g.direction,
        rounds: g.navs.length,
        roundsWithFreeze: g.navs.filter((n) => n.freezes > 0).length,
        swapErrors: g.navs.filter((n) => n.swapError).length,
        urlMismatches: g.navs.filter((n) => !n.urlMatches).length,
        unsettled: g.navs.filter((n) => !n.postSettle.settled).length,
        longestTaskMs: stat(g.navs.map((n) => n.longestTaskMs)),
        layerizeTotalMs: stat(
          g.navs.map((n) => (n.trace?.totals ? (n.trace.totals.Layerize?.totalMs ?? 0) : null)),
        ),
        layerizeMaxMs: stat(
          g.navs.map((n) => (n.trace?.totals ? (n.trace.totals.Layerize?.maxMs ?? 0) : null)),
        ),
        pushStateToBeforeSwapMs: stat(g.navs.map((n) => n.timeline.pushStateToBeforeSwapMs)),
        clickToPageLoadMs: stat(g.navs.map((n) => n.timeline.clickToPageLoadMs)),
        clickToSettledMs: stat(g.navs.map((n) => n.timeline.clickToSettledMs)),
        maxFrameGapMs: stat(g.navs.map((n) => n.maxFrameGapMs)),
      },
    ]),
  );
}

// Predeclared #4166 target: no repeated >= 4 s freeze and at least a halved
// median longest task on the target case; controls are reported as ratios
// for the manager to judge "no material regression".
function compareWithBaseline(stats, baselinePath) {
  const baseline = JSON.parse(readFileSync(baselinePath, "utf8"));
  const rows = {};
  for (const [key, current] of Object.entries(stats)) {
    const before = baseline.stats?.[key];
    if (!before) continue;
    const ratio =
      before.longestTaskMs.median && current.longestTaskMs.median != null
        ? round1(current.longestTaskMs.median / before.longestTaskMs.median)
        : null;
    rows[key] = {
      role: current.role,
      baselineSha: baseline.source?.sha ?? null,
      baselineMedianLongestTaskMs: before.longestTaskMs.median,
      currentMedianLongestTaskMs: current.longestTaskMs.median,
      medianLongestTaskRatio: ratio,
      baselineRoundsWithFreeze: before.roundsWithFreeze,
      currentRoundsWithFreeze: current.roundsWithFreeze,
      baselineMedianPushStateToBeforeSwapMs: before.pushStateToBeforeSwapMs.median,
      currentMedianPushStateToBeforeSwapMs: current.pushStateToBeforeSwapMs.median,
      ...(current.role === "target"
        ? {
            baselineRepeatedFreeze: before.roundsWithFreeze >= 2,
            freezeEliminated: current.roundsWithFreeze === 0,
            medianLongestTaskHalved: ratio != null && ratio <= 0.5,
          }
        : {}),
    };
  }
  return { baseline: baselinePath, rows };
}

function printTable(stats) {
  const fmt = (s) => (s.median == null ? "-" : `${s.median}/${s.max}`);
  console.log(
    "host/case/direction  longestTask med/max  layerize med/max  pushState->swap med/max  freezeRounds",
  );
  for (const [key, s] of Object.entries(stats))
    console.log(
      `${key}  ${fmt(s.longestTaskMs)}  ${fmt(s.layerizeTotalMs)}  ${fmt(s.pushStateToBeforeSwapMs)}  ${s.roundsWithFreeze}/${s.rounds}`,
    );
}

mkdirSync(dirname(outputPath), { recursive: true });
if (TRACE) mkdirSync(traceDir, { recursive: true });
let exitCode = 0;
let browser;
try {
  await preflight();
  browser = await chromium.launch({ headless: !HEADED });
  summary.browser = { name: "chromium", version: browser.version() };
  const units = hosts.flatMap((host) => caseIds.map((id) => ({ host, id })));
  const total = WARMUP + ROUNDS;
  for (let r = 0; r < total; r++) {
    const measured = r >= WARMUP;
    const label = measured ? `r${r - WARMUP + 1}` : `w${r + 1}`;
    const order = units.map((_, i) => units[(i + r) % units.length]);
    for (const { host, id } of order) {
      const record = await runSequence(browser, host, id, label, measured);
      summary.rounds.push(record);
      console.log(
        `[nav-stall] ${label} ${host.label}/${id}: enter ${record.navigations.enter?.longestTaskMs ?? "-"} ms, leave ${record.navigations.leave?.longestTaskMs ?? "-"} ms longest task${record.errors.length ? ` (${record.errors.length} errors)` : ""}`,
      );
    }
  }
  summary.stats = buildStats();
  if (env.NAV_STALL_BASELINE)
    summary.comparison = compareWithBaseline(summary.stats, env.NAV_STALL_BASELINE);
  printTable(summary.stats);
} catch (error) {
  summary.runError = error.stack ?? String(error);
  console.error(`[nav-stall] run failed: ${error.message}`);
  exitCode = 1;
} finally {
  await browser?.close();
  summary.assets = Object.fromEntries([...assets].sort(([a], [b]) => a.localeCompare(b)));
  writeFileSync(outputPath, `${JSON.stringify(summary, null, 2)}\n`);
  console.log(
    `[nav-stall] summary: ${outputPath}${TRACE ? `\n[nav-stall] traces: ${traceDir}` : ""}`,
  );
}
process.exit(exitCode);
