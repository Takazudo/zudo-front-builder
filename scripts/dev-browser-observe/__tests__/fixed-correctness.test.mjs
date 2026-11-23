import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { ObserverPage, declaredSocketFor, goneRead } from "../lib/browser.mjs";
import { distributionOf } from "../lib/distribution.mjs";
import {
  EMPTY_HOME_PROBE,
  FN_SEL,
  fieldNotesOps,
  liveRestoreChecks,
  materializeFieldNotes,
  planFieldNotesOp,
} from "../lib/fieldnotes.mjs";
import {
  DEFAULT_FRONTMATTER_BREAKAGE,
  breakFrontmatter,
  createModel,
} from "../lib/fieldnotes-model.mjs";
import { semanticErrors } from "../lib/profile.mjs";
import { acceptConditionalStates, runSpecialMode } from "../lib/special-modes.mjs";
import { classifyMechanism } from "../lib/state-machine.mjs";
import { validateAs } from "../lib/schema.mjs";
import { HARNESS_DIR } from "../lib/util.mjs";
import { SPEC, articleRow, articleText, makeFakeSource } from "./fieldnotes-fake.mjs";

const FIXTURE = join(HARNESS_DIR, "fixtures", "field-notes");
const example = (n) => JSON.parse(readFileSync(join(HARNESS_DIR, "profiles", n), "utf8"));
const tmps = [];
const tmp = () => {
  const d = mkdtempSync(join(tmpdir(), "dbo-fc-"));
  tmps.push(d);
  return d;
};
afterEach(() => {
  while (tmps.length) rmSync(tmps.pop(), { recursive: true, force: true });
});

function fakeM(count = 20) {
  const root = tmp();
  const fake = makeFakeSource(join(root, "src"), count);
  return materializeFieldNotes({
    fixtureDir: FIXTURE,
    sourceDir: join(root, "src"),
    workDir: join(root, "work"),
    adapter: "direct-read",
    managedCss: true,
    expectedDatasetSha256: fake.datasetSha256,
  });
}

describe("(2) gone checks mark the removed page by its title and body element, not its slug", () => {
  // Views as the native runs recorded them.
  const zfb404 = {
    ok: true,
    httpStatus: 404,
    title: "zfb dev — 404",
    bodyExcerpt: "404 — page not in cache\n\nThe dev server has no rendered HTML for this URL.",
    notInCache: true,
  };
  const astro404 = {
    ok: true,
    httpStatus: 404,
    title: "404: Not Found",
    bodyExcerpt: "404: Not found\nPath: /articles/article-00101/",
  };
  const stale200 = { ok: true, httpStatus: 200, bodyExcerpt: "Title 101 · 00101 body" };
  const view = (v, text, selector, marker) => ({
    ...v,
    containsMarker: marker === null ? null : (v.bodyExcerpt ?? "").includes(marker),
    selectorPresent: selector === null ? null : selector,
  });
  const oldRule = (v) =>
    v.ok && (v.httpStatus === 404 || v.httpStatus === 410) && v.containsMarker !== true;

  it("zfb's results are unchanged; only a 404 page that echoes the path changes verdict", () => {
    const slug = "article-00101";
    const title = "Title 101 · 00101";
    for (const [name, v, expectedOld, expectedNew] of [
      ["zfb 404", zfb404, true, true],
      ["stale 200", stale200, false, false],
      ["astro 404 echoing the path", astro404, false, true],
    ]) {
      const old = oldRule(view(v, null, null, slug));
      const now = goneRead(view(v, null, false, title));
      expect([name, old, now]).toEqual([name, expectedOld, expectedNew]);
    }
    expect(
      goneRead({ ok: true, httpStatus: 404, containsMarker: false, selectorPresent: true }),
    ).toBe(false);
    expect(
      goneRead({ ok: true, httpStatus: 410, containsMarker: true, selectorPresent: false }),
    ).toBe(false);
  });

  it("restore live checks of a removed article carry its title and body selector", () => {
    const model = createModel(
      { articles: Array.from({ length: 20 }, (_, i) => articleRow(i + 1)) },
      SPEC,
    );
    const gone = liveRestoreChecks(
      model,
      ["article-00021"],
      new Map([["article-00021", "T 21"]]),
    ).filter((c) => c.kind === "gone");
    expect(gone).toEqual([
      {
        kind: "gone",
        path: "/articles/article-00021/",
        absentText: "T 21",
        absentSelector: FN_SEL.body("article-00021"),
      },
    ]);
    expect(
      liveRestoreChecks(model, ["article-00021"]).find((c) => c.kind === "gone").absentText,
    ).toBeNull();
  });

  it("an applied add-article records the added title for the restore's gone check", () => {
    const m = fakeM();
    const ops = fieldNotesOps(m);
    ops.snapshot();
    const a = ops.prepareAttempt({ op: "add-article", writeMethod: "create" });
    a.applied();
    const gone = ops.liveChecks().find((c) => c.kind === "gone");
    const added = a.nextModel.articles.find(
      (x) => !m.model.articles.some((y) => y.slug === x.slug),
    );
    expect(gone).toMatchObject({
      path: `/articles/${added.slug}/`,
      absentText: added.title,
      absentSelector: FN_SEL.body(added.slug),
    });
    expect(gone.absentText).not.toContain(added.slug);
  });

  it("goneCheck passes the selector to the page read and needs two clean reads without a confirmed update", async () => {
    const o = new ObserverPage(null, "", () => 1n);
    const calls = [];
    o.observePage = async (path, opts) => {
      calls.push(opts);
      return { ok: true, path, httpStatus: 404, containsMarker: false, selectorPresent: false };
    };
    const r = await o.goneCheck(
      { kind: "gone", path: "/a/", absentText: "T", absentSelector: '[data-article-body="a"]' },
      { quietMs: 1 },
    );
    expect(r).toMatchObject({ ok: true, basis: "stable-reads", stableReads: 2 });
    expect(calls[0]).toEqual({ markerText: "T", absentSelector: '[data-article-body="a"]' });
  });
});

describe("(3) versioned invalid-frontmatter breakage", () => {
  const text = articleText(articleRow(9));
  const original = (t) =>
    t.replace(/^title: .*$/m, (line) => line.replace(/"$/, "") + "\n  : [unbalanced");

  it("the default is byte-identical to the original op; the new variant is an unclosed flow sequence", () => {
    expect(DEFAULT_FRONTMATTER_BREAKAGE).toBe("title-unbalanced-quote-v1");
    expect(breakFrontmatter(text)).toBe(original(text));
    expect(breakFrontmatter(text, "title-unbalanced-quote-v1")).toBe(original(text));
    const flow = breakFrontmatter(text, "unclosed-flow-sequence-v1");
    expect(flow.split("\n").find((l) => l.startsWith("title:"))).toBe("title: [unbalanced");
    expect(flow.replace("title: [unbalanced", "")).toBe(text.replace(/^title: .*$/m, ""));
    expect(() => breakFrontmatter(text, "nope")).toThrow(/unknown frontmatter breakage/);
  });

  it("the op keeps the original breakage unless a profile selects another, and records which", () => {
    const m = fakeM();
    const plain = planFieldNotesOp(m, m.model, {
      op: "edit-invalid-frontmatter",
      article: "article-00009",
    });
    expect(plain.declared).toEqual({
      breakage: "title-unbalanced-quote-v1",
      breakageExplicit: false,
    });
    const before = readFileSync(join(m.contentDir, "article-00009.md"), "utf8");
    expect(plain.writes[0].bytes.toString()).toBe(original(before));
    const flow = planFieldNotesOp(m, m.model, {
      op: "edit-invalid-frontmatter",
      article: "article-00009",
      breakage: "unclosed-flow-sequence-v1",
    });
    expect(flow.declared).toEqual({
      breakage: "unclosed-flow-sequence-v1",
      breakageExplicit: true,
    });
    expect(flow.writes[0].bytes.toString()).toContain("\ntitle: [unbalanced\n");
  });

  it("breakage is only for edit-invalid-frontmatter; the evidence file records every parser", () => {
    const p = example("field-notes-fixed-next-negatives-100.example.json");
    expect(semanticErrors(p)).toEqual([]);
    const bad = structuredClone(p);
    bad.scenarios[0].ops[0].breakage = "unclosed-flow-sequence-v1";
    expect(semanticErrors(bad).join("\n")).toMatch(
      /breakage applies only to edit-invalid-frontmatter/,
    );
    const ev = JSON.parse(
      readFileSync(join(FIXTURE, "evidence", "frontmatter-breakage-verification.json"), "utf8"),
    );
    expect(ev.executed.versions).toMatchObject({ "js-yaml": "3.15.2", yaml: "2.9.1" });
    const flow = ev.executed.results.find((r) => r.variant.startsWith("unclosed-flow-sequence-v1"));
    for (const k of ["jsyaml", "grayMatter", "yaml", "astroParseFrontmatter"])
      expect(flow[k]).toMatch(/^ERROR/);
    const old = ev.executed.results.find((r) => r.variant.startsWith("title-unbalanced-quote-v1"));
    expect(old.jsyaml).toMatch(/^parsed/);
    expect(ev.zfb.executed).toBe(false);
  });
});

describe("(4) fixed zfb: route-not-expanded only with the declared server rejection line", () => {
  const realLine =
    '⚠ dynamic SSG route /articles/:slug not expanded in dev: worker returned 500 for /__paths__//articles/:slug: [zfb-runtime] /__paths__: paths() threw for "/articles/:slug": Article slug mismatch: article-00008-renamed';

  function fixedZfbM() {
    const m = fakeM();
    m.entry = distributionOf(FIXTURE, "zfb");
    return m;
  }

  it("the plan carries the condition only when asked, and only where the framework declares it", () => {
    const m = fixedZfbM();
    const op = { op: "rename-filename-only", article: "article-00008", writeMethod: "rename" };
    expect(planFieldNotesOp(m, m.model, op).conditionalStates).toEqual([]);
    const a = planFieldNotesOp(m, m.model, { ...op, acceptRouteNotExpandedWithEvidence: true });
    expect(a.conditionalStates).toHaveLength(1);
    expect(a.conditionalStates[0]).toMatchObject({
      state: "route-not-expanded",
      path: "/articles/article-00008/",
    });
    expect(new RegExp(a.conditionalStates[0].serverLine).test(realLine)).toBe(true);
    expect(
      new RegExp(a.conditionalStates[0].serverLine).test(
        realLine.replace("00008-renamed", "00008-renamedX"),
      ),
    ).toBe(false);
    const sourceBuilt = fakeM();
    expect(() =>
      planFieldNotesOp(sourceBuilt, sourceBuilt.model, {
        ...op,
        acceptRouteNotExpandedWithEvidence: true,
      }),
    ).toThrow(/negativeEvidence.routeNotExpanded/);
  });

  it("accepted with the line, still a mismatch without it", () => {
    const m = fixedZfbM();
    const a = planFieldNotesOp(m, m.model, {
      op: "rename-filename-only",
      article: "article-00008",
      writeMethod: "rename",
      acceptRouteNotExpandedWithEvidence: true,
    });
    const observed = () => [
      { path: "/", state: "last-good" },
      { path: "/articles/article-00008/", state: "route-not-expanded" },
    ];
    const withLine = acceptConditionalStates(observed(), a, () => ({
      lines: [{ text: "info ok" }, { text: realLine }],
      truncated: false,
    }));
    expect(withLine.map((o) => o.expected)).toEqual([true, true]);
    expect(withLine[1].conditional.matched).toBe(realLine);
    const without = acceptConditionalStates(observed(), a, () => ({
      lines: [{ text: "info ok" }],
      truncated: false,
    }));
    expect(without.map((o) => o.expected)).toEqual([true, false]);
    expect(without[1].conditional.matched).toBeNull();
    const noLog = acceptConditionalStates(observed(), a, () => null);
    expect(noLog[1]).toMatchObject({ expected: false, conditional: { logAvailable: false } });
    const plain = acceptConditionalStates(observed(), { ...a, conditionalStates: [] }, () => {
      throw new Error("not read");
    });
    expect(plain[1].expected).toBe(false);
  });

  it("end to end through the expected-negative mode: both branches", async () => {
    const m = fixedZfbM();
    const attempt = planFieldNotesOp(m, m.model, {
      op: "rename-filename-only",
      article: "article-00008",
      writeMethod: "rename",
      acceptRouteNotExpandedWithEvidence: true,
    });
    const run = async (lines) => {
      const page = {
        mark: () => ({}),
        settle: async () => ({ settled: true }),
        observePage: async (path) =>
          path === "/"
            ? { ok: true, path, httpStatus: 200 }
            : { ok: true, path, httpStatus: 404, notInCache: true, markersFound: [] },
        evaluateNow: async (pred) => ({ matched: pred !== EMPTY_HOME_PROBE }),
        supplemental: async () => ({ ok: true }),
      };
      let n = 0n;
      const res = await runSpecialMode("expected-negative", {
        attempt,
        page,
        fs: { performWrite: (_w, now) => now() },
        clock: { now: () => (n += 1n) },
        timeouts: { quietMs: 1, restoreMs: 10 },
        recBase: {},
        op: {},
        server: { logMark: () => 0, logSince: () => ({ lines: lines.map((text) => ({ text })) }) },
      });
      return res;
    };
    const ok = await run([realLine]);
    expect(ok.outcome).toBe("expected-negative-observed");
    expect(ok.rec.observedSummary[1]).toMatchObject({
      state: "route-not-expanded",
      expected: true,
      conditional: { matched: realLine },
    });
    const bad = await run(["unrelated line"]);
    expect(bad.outcome).toBe("expected-negative-mismatch");
    expect(bad.rec.observedSummary[1].expected).toBe(false);
  });

  it("the option is refused outside fixed-distribution zfb and outside rename-filename-only", () => {
    const zfb = example("field-notes-fixed-zfb-negatives-100.example.json");
    expect(semanticErrors(zfb)).toEqual([]);
    const astro = example("field-notes-fixed-astro-negatives-100.example.json");
    astro.scenarios[0].ops[0].acceptRouteNotExpandedWithEvidence = true;
    expect(semanticErrors(astro).join("\n")).toMatch(/fixed-distribution zfb profiles only/);
    const sb = example("field-notes-direct-read.example.json");
    const ren = sb.scenarios.find((s) => s.id === "fn-neg-rename-filename-only");
    ren.ops[0].acceptRouteNotExpandedWithEvidence = true;
    expect(semanticErrors(sb).join("\n")).toMatch(/fixed-distribution zfb profiles only/);
    expect(validateAs("profile", sb)).toEqual([]);
  });
});

describe("(HMR) declared framework update sockets for settle and mechanism", () => {
  const next = distributionOf(FIXTURE, "next").updateSignal.sockets;
  const astro = distributionOf(FIXTURE, "astro").updateSignal.sockets;

  it("matches the declared socket URLs only", () => {
    expect(declaredSocketFor("ws://127.0.0.1:4313/_next/hmr?id=x", next)?.name).toBe("next-hmr");
    expect(declaredSocketFor("ws://127.0.0.1:4313/_next/hmrx", next)).toBeNull();
    expect(declaredSocketFor("ws://127.0.0.1:4312/?token=abc", astro)?.name).toBe("vite-hmr");
    expect(
      declaredSocketFor("ws://127.0.0.1:4312/__zfb/reload/ws", [...next, ...astro]),
    ).toBeNull();
  });

  it("maps Next and Vite messages; unknown and unparsable ones are recorded but are no update evidence", () => {
    const o = new ObserverPage(null, "", () => 5n);
    o.updateSockets = [...next, ...astro];
    const nu = "ws://h/_next/hmr";
    const vu = "ws://h/?token=t";
    o.recordSocketFrame(nu, JSON.stringify({ type: "serverComponentChanges", hash: "x" }));
    o.recordSocketFrame(nu, JSON.stringify({ type: "built", hash: "y" }));
    o.recordSocketFrame(nu, Buffer.from([0, 1, 2]));
    o.recordSocketFrame(vu, JSON.stringify({ type: "update", updates: [{ type: "css-update" }] }));
    o.recordSocketFrame(vu, JSON.stringify({ type: "update", updates: [{ type: "js-update" }] }));
    o.recordSocketFrame(vu, JSON.stringify({ type: "full-reload", path: "*" }));
    o.recordSocketFrame(vu, JSON.stringify({ type: "custom", event: "astro:content-changed" }));
    o.recordSocketFrame("ws://h/other", JSON.stringify({ type: "full-reload" }));
    expect(o.wsFrames.map((f) => [f.socket, f.rawType, f.event])).toEqual([
      ["next-hmr", "serverComponentChanges", "rsc"],
      ["vite-hmr", "update:css-update", "css"],
      ["vite-hmr", "update:js-update", "module"],
      ["vite-hmr", "full-reload", "page"],
    ]);
    expect(o.otherFrames.map((f) => [f.rawType, f.event])).toEqual([
      ["built", "unmapped"],
      [null, "unparsed"],
      ["custom:astro:content-changed", "unmapped"],
    ]);
    const ev = o.since({ docInits: 0, navigations: 0, wsFrames: 0, otherFrames: 0, requests: 0 });
    expect(ev.otherFrames).toHaveLength(3);
    expect(ev.livereloadEvents).toEqual(["rsc", "css", "module", "page"]);
  });

  it("zfb livereload frames are recorded exactly as before, with no otherFrames", () => {
    const o = new ObserverPage(null, "", () => 7n);
    o.recordSocketFrame("ws://h/__zfb/reload/ws", JSON.stringify({ event: "page", data: {} }));
    expect(o.wsFrames).toMatchObject([{ event: "page", url: "ws://h/__zfb/reload/ws" }]);
    expect(o.wsFrames[0].socket).toBeUndefined();
    expect(o.otherFrames).toEqual([]);
    expect("otherFrames" in o.since(o.mark())).toBe(false);
  });

  it("settle treats an RSC or module update as the native update; an unmapped frame does not end it", async () => {
    let now = 0n;
    const o = new ObserverPage(null, "", () => now);
    o.updateSockets = next;
    const tick = setInterval(() => (now += 5_000_000n), 1);
    const mark = o.mark();
    o.recordSocketFrame("ws://h/_next/hmr", JSON.stringify({ type: "built" }));
    const none = await o.settle({
      mark,
      startNs: 0n,
      quietMs: 10,
      boundMs: 60,
      requireNativeUpdate: true,
      stepMs: 1,
    });
    expect(none).toMatchObject({ settled: false, reason: "bound-exceeded-without-native-update" });
    const mark2 = o.mark();
    o.recordSocketFrame("ws://h/_next/hmr", JSON.stringify({ type: "serverComponentChanges" }));
    const rsc = await o.settle({
      mark: mark2,
      startNs: now,
      quietMs: 10,
      boundMs: 500,
      requireNativeUpdate: true,
      stepMs: 1,
    });
    clearInterval(tick);
    expect(rsc).toMatchObject({ settled: true, nativeUpdateObserved: true });
  });

  it("mechanism: rsc and module updates get their own labels; zfb's labels are unchanged", () => {
    const base = {
      preDocUUID: "a",
      notifyDocUUID: "a",
      mainFrameNavigations: 0,
      serverPidBefore: 1,
      serverPidAfter: 1,
    };
    expect(classifyMechanism({ ...base, livereloadEvents: ["rsc"] })).toBe("rsc-update");
    expect(classifyMechanism({ ...base, livereloadEvents: ["module"] })).toBe("hmr-module-update");
    expect(classifyMechanism({ ...base, livereloadEvents: ["css", "module"] })).toBe(
      "css-replacement",
    );
    expect(classifyMechanism({ ...base, livereloadEvents: [] })).toBe("in-document-update");
    expect(
      classifyMechanism({
        ...base,
        notifyDocUUID: "b",
        mainFrameNavigations: 1,
        livereloadEvents: ["page"],
      }),
    ).toBe("native-full-reload");
  });
});
