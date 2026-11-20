import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { ObserverPage, isLivereloadSocket, livereloadFrame } from "../lib/browser.mjs";
import { nativeReloadSurvival } from "../lib/calibrate.mjs";
import {
  loadFixture,
  materialize,
  prepareAttempt,
  routePath,
  warmPaths,
  zfbConfigFor,
} from "../lib/fixture.mjs";
import { HARNESS_DIR, sha256File } from "../lib/util.mjs";

const FIXTURE = join(HARNESS_DIR, "fixtures", "tiny-two-article");
const ORIGINAL_A1_SHA256 = "10cb58dbc77234bc378397601c9b55b534c69b2a43c7e8b488bb32cbeebe38a6";
const tmps = [];
const tmp = () => {
  const d = mkdtempSync(join(tmpdir(), "dbo-r2-"));
  tmps.push(d);
  return d;
};
afterEach(() => {
  while (tmps.length) rmSync(tmps.pop(), { recursive: true, force: true });
});
const adapter = (id) => readFileSync(join(FIXTURE, "adapters", id, "content.ts.adapter"), "utf8");

describe("adapters", () => {
  it("A1 is the exact original input and the manifest records its provenance", () => {
    const m = loadFixture(FIXTURE);
    expect(sha256File(join(FIXTURE, m.adapters.variants.A1.file))).toBe(ORIGINAL_A1_SHA256);
    expect(m.adapters.variants.A1).toMatchObject({
      provenance: "original-input",
      sha256: ORIGINAL_A1_SHA256,
    });
    expect(m.adapters.variants.A0.provenance).toBe("reconstructed");
    expect(m.adapters.variants.A3.provenance).toBe("reconstructed-diagnostic");
  });

  it("A2 is A1 plus the import and exactly one snapshot-touch line", () => {
    const a1 = adapter("A1").split("\n");
    const a2 = adapter("A2").split("\n");
    expect(a2).toHaveLength(a1.length + 1);
    expect(a2[0]).toBe(
      'import { getCollection, getContentSnapshot, type CollectionEntry } from "@takazudo/zfb/content";',
    );
    const i = a2.indexOf("  void getContentSnapshot()?.collections.articles;");
    expect(a2[i + 1]).toBe("  if (cached) return cached;");
    expect([...a2.slice(1, i), ...a2.slice(i + 1)]).toEqual(a1.slice(1));
  });

  it("A3 rebuilds on a changed snapshot slot and is labelled reconstructed", () => {
    const a3 = adapter("A3");
    expect(a3).toContain("RECONSTRUCTED diagnostic");
    expect(a3).toContain("if (cached && slot === cachedSlot) return cached;");
    expect(a3).toContain("cachedSlot = slot;");
  });

  it("materializes the chosen adapter byte-for-byte to app/lib/content.ts", () => {
    const m = materialize({
      fixtureDir: FIXTURE,
      workDir: join(tmp(), "w"),
      placement: "external",
      adapter: "A1",
      trailingSlash: true,
    });
    expect(sha256File(join(m.appDir, "lib", "content.ts"))).toBe(ORIGINAL_A1_SHA256);
  });

  it("the detail route's paths() is the original reproducer line", () => {
    const route = readFileSync(join(FIXTURE, "app/pages/articles/[slug].tsx"), "utf8");
    expect(route).toContain(
      "  return getIndex().articles.map((entry) => ({ params: { slug: entry.slug } }));",
    );
    expect(route).toContain(
      "export default function ArticlePage({ params }: { params: { slug: string } })",
    );
  });
});

describe("trailingSlash and placement", () => {
  it("external placement matches the original ../content/articles shape", () => {
    const m = loadFixture(FIXTURE);
    expect(zfbConfigFor(m, "external", { trailingSlash: true })).toEqual({
      wind: false,
      trailingSlash: true,
      collections: [{ name: "articles", path: "../content/articles", allowOutsideRoot: true }],
    });
    expect(zfbConfigFor(m, "internal").trailingSlash).toBeUndefined();
  });

  it("routes, predicates and supplemental paths follow trailingSlash", () => {
    const m = materialize({
      fixtureDir: FIXTURE,
      workDir: join(tmp(), "w"),
      placement: "external",
      adapter: "A0",
      trailingSlash: true,
    });
    expect(routePath(m.manifest, "article", { slug: "a", trailingSlash: true })).toBe(
      "/articles/a/",
    );
    const a = prepareAttempt(
      m,
      { op: "edit-title", article: "a", writeMethod: "in-place" },
      { token: "t" },
    );
    expect(a.targetPath).toBe("/articles/a/");
    expect(a.supplemental[0].path).toBe("/");
    expect(prepareAttempt(m, { op: "edit-css", writeMethod: "in-place" }).targetPath).toBe(
      "/independent/",
    );
    expect(warmPaths(m, { warmOrder: "index-first" })).toEqual(["/", "/articles/a/"]);
    expect(warmPaths(m, { warmOrder: "detail-first", warmDetail: "b" })).toEqual([
      "/articles/b/",
      "/",
    ]);
    expect(warmPaths(m, { warmOrder: "none" })).toEqual([]);
  });

  it("a title edit targeted at / arms on the home listing and checks the detail page after", () => {
    const m = materialize({
      fixtureDir: FIXTURE,
      workDir: join(tmp(), "w"),
      placement: "internal",
      adapter: "A0",
    });
    const a = prepareAttempt(
      m,
      { op: "edit-title", article: "a", writeMethod: "in-place", targetPath: "/" },
      { token: "tok" },
    );
    expect(a.targetPath).toBe("/");
    expect(a.predicate).toEqual({
      kind: "text",
      selector: '[data-home-title="a"]',
      includes: "tok",
    });
    expect(a.supplemental).toEqual([
      {
        path: "/articles/a",
        predicate: { kind: "text", selector: '[data-article-title="a"]', includes: "tok" },
      },
    ]);
  });
});

describe("observer page evidence (no browser)", () => {
  it("recognizes the livereload socket and parses frames", () => {
    expect(isLivereloadSocket("ws://127.0.0.1:5000/__zfb/reload/ws")).toBe(true);
    expect(isLivereloadSocket("ws://127.0.0.1:5000/base/__zfb/reload/ws?x=1")).toBe(true);
    expect(isLivereloadSocket("ws://127.0.0.1:5000/other")).toBe(false);
    expect(livereloadFrame('{"event":"page","data":""}', 5n, "u")).toMatchObject({
      event: "page",
      ns: "5",
    });
    expect(livereloadFrame("garbage", 5n, "u")).toMatchObject({
      event: "unparsed",
      raw: "garbage",
    });
  });

  it("collects frames relative to t0 and flags pre-write instability", () => {
    const p = new ObserverPage(null, "", () => 0n);
    p.markPre();
    expect(p.preWriteInstability()).toBeNull();
    p.wsFrames.push({ ns: "10", event: "islands" });
    expect(p.preWriteInstability()).toBeNull();
    p.wsFrames.push({ ns: "20", event: "page" });
    expect(p.preWriteInstability().wsFrames.map((f) => f.event)).toEqual(["page"]);
    p.markAttempt();
    p.wsFrames.push({ ns: "3000000", event: "css" });
    p.navigations.push({ ns: "4000000", url: "x" });
    const ev = p.collectAttempt(1_000_000n);
    expect(ev).toMatchObject({ mainFrameNavigations: 1, livereloadEvents: ["css"] });
    expect(ev.wsFrames[0].msFromT0).toBe(2);
  });
});

function fakeCalObserver({ notify, frames }) {
  const o = new ObserverPage(null, "", () => 1_000_000n);
  o.prepare = async () => ({ docUUID: "doc-1" });
  o.arm = async () => ({ armed: true, docUUID: "doc-1" });
  o.disarm = async () => null;
  o.liveCheck = async () => ({ ok: true });
  o.quietWindow = async (ms) => ({ quietMs: ms });
  o.waitNotify = async () => {
    o.wsFrames.push(...frames);
    o.navigations.push({ ns: "2000000", url: "b" });
    return notify ? { kind: "notify", payload: notify, t1: 3_000_000n } : { kind: "timeout" };
  };
  return o;
}

describe("native-reload-survival calibration cell", () => {
  const ops = () => {
    const m = materialize({
      fixtureDir: FIXTURE,
      workDir: join(tmp(), "w"),
      placement: "internal",
      adapter: "A0",
    });
    return {
      m,
      fixtureOps: {
        snapshot: () => ({ files: {}, dirs: [] }),
        prepareAttempt: (op) => prepareAttempt(m, op),
        performWrite: (w, onT0) => onT0(),
        restore: () => ({ writeMethod: "in-place", actions: [], mismatches: [] }),
        liveChecks: () => [{ path: "/articles/b", predicate: {} }],
        route: m.route,
      },
    };
  };

  it("passes only with a new document, navigationType reload and a page frame", async () => {
    const { fixtureOps } = ops();
    const ok = await nativeReloadSurvival({
      observer: fakeCalObserver({
        notify: { docUUID: "doc-2", navigationType: "reload" },
        frames: [{ ns: "1500000", event: "page" }],
      }),
      fixtureOps,
      timeoutMs: 10,
      restoreMs: 10,
      quietMs: 1,
    });
    expect(ok).toMatchObject({
      ok: true,
      restorationOk: true,
      calibrationEdit: { method: "in-place", label: "calibration edit (unmeasured)" },
    });
  });

  it("fails without a livereload page frame even if the document reloaded", async () => {
    const { fixtureOps } = ops();
    const res = await nativeReloadSurvival({
      observer: fakeCalObserver({
        notify: { docUUID: "doc-2", navigationType: "reload" },
        frames: [],
      }),
      fixtureOps,
      timeoutMs: 10,
      restoreMs: 10,
      quietMs: 1,
    });
    expect(res.ok).toBe(false);
  });

  it("fails when the document did not change", async () => {
    const { fixtureOps } = ops();
    const res = await nativeReloadSurvival({
      observer: fakeCalObserver({
        notify: { docUUID: "doc-1", navigationType: "navigate" },
        frames: [{ ns: "1", event: "page" }],
      }),
      fixtureOps,
      timeoutMs: 10,
      restoreMs: 10,
      quietMs: 1,
    });
    expect(res.ok).toBe(false);
  });
});

describe("configFormat", () => {
  const ORIGINAL_CONFIG_SHA256 = "7839c840ae456b8b868463d2bc4cfb654ce490a5d80ace58cd71ab86c5c6f602";
  const mat = (opts) =>
    materialize({ fixtureDir: FIXTURE, workDir: join(tmp(), "w"), adapter: "A1", ...opts });

  it("ts in the original shape writes the byte-exact original zfb.config.ts", () => {
    const m = mat({ placement: "external", trailingSlash: true, configFormat: "ts" });
    expect(sha256File(join(m.appDir, "zfb.config.ts"))).toBe(ORIGINAL_CONFIG_SHA256);
    expect(m).toMatchObject({
      configFile: "zfb.config.ts",
      configProvenance: "original-input",
      configOriginalSha256: ORIGINAL_CONFIG_SHA256,
      configSha256: ORIGINAL_CONFIG_SHA256,
    });
    expect(existsSync(join(m.appDir, "zfb.config.json"))).toBe(false);
  });

  it("ts in any other shape writes a generated defineConfig labelled reconstructed", () => {
    for (const opts of [
      { placement: "internal", trailingSlash: true },
      { placement: "external", trailingSlash: false },
    ]) {
      const m = mat({ ...opts, configFormat: "ts" });
      const text = readFileSync(join(m.appDir, "zfb.config.ts"), "utf8");
      expect(m.configProvenance).toBe("reconstructed");
      expect(text).toContain("RECONSTRUCTED by dev-browser-observe");
      expect(text).toContain('import { defineConfig } from "@takazudo/zfb/config";');
      expect(text).toContain(
        opts.placement === "internal"
          ? '"path": "content/articles"'
          : '"path": "../content/articles"',
      );
      expect(text.includes('"trailingSlash": true')).toBe(opts.trailingSlash);
    }
  });

  it("json stays the default", () => {
    const m = mat({ placement: "external", trailingSlash: true });
    expect(m).toMatchObject({
      configFormat: "json",
      configFile: "zfb.config.json",
      configProvenance: "generated-json",
    });
    expect(existsSync(join(m.appDir, "zfb.config.ts"))).toBe(false);
  });
});
