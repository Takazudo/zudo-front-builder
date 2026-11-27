// Level 1: the SSR asset finalizer (#4887). Its rewrite step is a port of
// `boundary_replace` in crates/zfb-build/src/pipeline/prod.rs and its head
// step mirrors crates/zfb-build/src/head_inject.rs, so these cases pin the
// same byte forms the SSG pipeline writes.

import { describe, expect, it } from "vite-plus/test";

import { finalizeHtmlAssets, snapshotPageAssets, type PageAssetManifest } from "../page-assets.js";

const CSS_LINK = (href: string) => `<link rel="stylesheet" href="${href}">`;
const MODULE_SCRIPT = (src: string) => `<script type="module" src="${src}"></script>`;

const manifest = (base = ""): PageAssetManifest => ({
  assets: [
    {
      kind: "css",
      stableUrl: `${base}/assets/styles.css`,
      url: `${base}/assets/styles-aaa111.css`,
    },
    {
      kind: "islands",
      stableUrl: `${base}/assets/islands.js`,
      url: `${base}/assets/islands-bbb222.js`,
    },
    {
      kind: "client-script",
      stableUrl: `${base}/assets/menu.js`,
      url: `${base}/assets/menu-ccc333.js`,
    },
  ],
});

const doc = (head: string, body = "") => `<html><head>${head}</head><body>${body}</body></html>`;

describe("finalizeHtmlAssets: identity", () => {
  it("returns the same string with no manifest or an empty record", () => {
    const html = doc(CSS_LINK("/assets/styles.css"));
    expect(finalizeHtmlAssets(html, undefined)).toBe(html);
    expect(finalizeHtmlAssets(html, { assets: [] })).toBe(html);
  });

  it("leaves unknown references alone and invents no hashed URL", () => {
    const html = `<p><a href="/assets/other.css">x</a><img src="/assets/styles.png"></p>`;
    expect(finalizeHtmlAssets(html, manifest())).toBe(html);
  });
});

describe("finalizeHtmlAssets: stable URL rewrite", () => {
  it.each([
    ["root", ""],
    ["subpath base", "/docs"],
    ["absolute-URL base", "https://cdn.example.test/site"],
  ])("rewrites existing references (%s) and does not add a tag twice", (_name, base) => {
    const html = doc(
      CSS_LINK(`${base}/assets/styles.css`) + MODULE_SCRIPT(`${base}/assets/islands.js`),
      `<script src="${base}/assets/menu.js"></script>`,
    );
    expect(finalizeHtmlAssets(html, manifest(base))).toBe(
      doc(
        CSS_LINK(`${base}/assets/styles-aaa111.css`) +
          MODULE_SCRIPT(`${base}/assets/islands-bbb222.js`),
        `<script src="${base}/assets/menu-ccc333.js"></script>`,
      ),
    );
  });

  it("does not add the base twice when the reference already carries it", () => {
    const out = finalizeHtmlAssets(doc(CSS_LINK("/docs/assets/styles.css")), manifest("/docs"));
    expect(out).toContain(CSS_LINK("/docs/assets/styles-aaa111.css"));
    expect(out).not.toContain("/docs/docs/");
  });

  it("keeps query and fragment suffixes", () => {
    const out = finalizeHtmlAssets(
      `<a href="/assets/styles.css?v=1">a</a><a href="/assets/menu.js#top">b</a>`,
      manifest(),
    );
    expect(out).toBe(
      `<a href="/assets/styles-aaa111.css?v=1">a</a><a href="/assets/menu-ccc333.js#top">b</a>`,
    );
  });

  it("rewrites at every delimiter of the Rust boundary set", () => {
    for (const [open, close] of [
      ['"', '"'],
      ["'", "'"],
      ["(", ")"],
      [" ", " "],
      ["=", ","],
      ["\\", ";"],
      ["\n", "\r"],
      ["\t", "<"],
      [">", "?"],
    ] as const) {
      expect(finalizeHtmlAssets(`${open}/assets/menu.js${close}`, manifest())).toBe(
        `${open}/assets/menu-ccc333.js${close}`,
      );
    }
    expect(finalizeHtmlAssets("/assets/menu.js", manifest())).toBe("/assets/menu-ccc333.js");
  });

  it("does not touch .map files, longer names or look-alikes", () => {
    const html = [
      `<a href="/assets/styles.css.map">`,
      `<a href="/assets/styles.cssx">`,
      `<a href="/my/assets/styles.css">`,
      `<a href="x/assets/styles.css">`,
      `<a href="/assets/styles.css-old">`,
      `<a href="/assets/styles_css">`,
    ].join("");
    expect(finalizeHtmlAssets(html, manifest())).toBe(html);
  });

  it("applies the longest key first so nested keys do not clobber each other", () => {
    const nested: PageAssetManifest = {
      assets: [
        { kind: "client-script", stableUrl: "/a.js", url: "/a-111.js" },
        { kind: "client-script", stableUrl: "/a.js/b.js", url: "/a.js/b-222.js" },
      ],
    };
    expect(finalizeHtmlAssets(`"/a.js" "/a.js/b.js"`, nested)).toBe(`"/a-111.js" "/a.js/b-222.js"`);
  });

  it("preserves Unicode around matches and treats non-ASCII neighbours as non-boundaries", () => {
    const html = `<p>日本語 "/assets/menu.js" 🎉</p><p>é/assets/menu.js</p><p>/assets/menu.js😀</p>`;
    expect(finalizeHtmlAssets(html, manifest())).toBe(
      `<p>日本語 "/assets/menu-ccc333.js" 🎉</p><p>é/assets/menu.js</p><p>/assets/menu.js😀</p>`,
    );
  });

  it("rewrites the attribute-escaped spelling of a key to the escaped final URL", () => {
    const escaped: PageAssetManifest = {
      assets: [
        { kind: "css", stableUrl: "/assets/a.css?x=1&y=2", url: "/assets/a-999.css?x=1&y=2" },
      ],
    };
    const out = finalizeHtmlAssets(
      `<html><head><link rel="stylesheet" href="/assets/a.css?x=1&amp;y=2"></head></html>`,
      escaped,
    );
    expect(out).toBe(
      `<html><head><link rel="stylesheet" href="/assets/a-999.css?x=1&amp;y=2"></head></html>`,
    );
  });
});

describe("finalizeHtmlAssets: head injection", () => {
  it("inserts CSS then islands before the first </head>, never client scripts", () => {
    const out = finalizeHtmlAssets(doc("<title>t</title>"), manifest());
    expect(out).toBe(
      doc(
        "<title>t</title>" +
          CSS_LINK("/assets/styles-aaa111.css") +
          MODULE_SCRIPT("/assets/islands-bbb222.js"),
      ),
    );
    expect(out).not.toContain("menu");
  });

  it("matches </head> case-insensitively and uses the first one", () => {
    const out = finalizeHtmlAssets("<HTML><HEAD></HEAD><BODY></HEAD></BODY></HTML>", manifest());
    expect(out.indexOf(CSS_LINK("/assets/styles-aaa111.css"))).toBe("<HTML><HEAD>".length);
    expect(out.match(/rel="stylesheet"/g)).toHaveLength(1);
  });

  it("escapes attribute values when writing a tag", () => {
    const odd: PageAssetManifest = {
      assets: [{ kind: "css", stableUrl: "/s.css", url: `/s-1.css?a=1&b="2"<'` }],
    };
    expect(finalizeHtmlAssets(doc(""), odd)).toBe(
      doc(CSS_LINK("/s-1.css?a=1&amp;b=&quot;2&quot;&lt;&#39;")),
    );
  });

  it("adds nothing and synthesizes no document without </head>", () => {
    const fragment = `<div><link rel="stylesheet" href="/assets/styles.css"></div>`;
    expect(finalizeHtmlAssets(fragment, manifest())).toBe(
      `<div><link rel="stylesheet" href="/assets/styles-aaa111.css"></div>`,
    );
    expect(finalizeHtmlAssets("<p>plain</p>", manifest())).toBe("<p>plain</p>");
  });

  it("is idempotent", () => {
    for (const html of [
      doc(""),
      doc(CSS_LINK("/assets/styles.css"), `<script src="/assets/menu.js"></script>`),
      "<p>no head /assets/menu.js</p>",
    ]) {
      const once = finalizeHtmlAssets(html, manifest());
      expect(finalizeHtmlAssets(once, manifest())).toBe(once);
    }
  });
});

describe("snapshotPageAssets", () => {
  it("copies and freezes the record", () => {
    const source = { assets: [{ kind: "css" as const, stableUrl: "/a.css", url: "/a-1.css" }] };
    const snap = snapshotPageAssets(source);
    source.assets[0]!.url = "/mutated.css";
    expect(snap?.assets[0]?.url).toBe("/a-1.css");
    expect(Object.isFrozen(snap)).toBe(true);
    expect(Object.isFrozen(snap?.assets)).toBe(true);
    expect(Object.isFrozen(snap?.assets[0])).toBe(true);
  });

  it("rejects a malformed record", () => {
    expect(() => snapshotPageAssets({} as never)).toThrow(TypeError);
    expect(() =>
      snapshotPageAssets({ assets: [{ kind: "font", stableUrl: "/a", url: "/b" }] } as never),
    ).toThrow(/assets\.assets\[0\]/);
  });
});
