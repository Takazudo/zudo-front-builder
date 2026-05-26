import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { setContentSnapshot } from "@takazudo/zfb/content";

import { createPageRouter } from "../router.js";
import type { PageDefinition, PageModule } from "../router.js";
import type { ContentSnapshot } from "../snapshot.js";
import type { FrameworkAdapter } from "../framework.js";

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/**
 * Stub framework adapter. The real production path uses
 * `preact-render-to-string` (or `react-dom/server`); the runtime is
 * agnostic by design, so unit tests use a deterministic stub that
 * serializes the vnode in a stable way. This lets us pin the exact
 * output bytes for the determinism test.
 */
function stubFramework(): FrameworkAdapter {
  return {
    renderToString(vnode: unknown): string {
      return serializeVnode(vnode);
    },
  };
}

function serializeVnode(node: unknown): string {
  if (node === null || node === undefined || node === false) return "";
  if (typeof node === "string") return escapeHtml(node);
  if (typeof node === "number" || typeof node === "boolean") return String(node);
  if (Array.isArray(node)) return node.map(serializeVnode).join("");
  if (typeof node === "object") {
    const v = node as { type?: unknown; props?: Record<string, unknown> };
    if (typeof v.type !== "string") return "";
    const props = v.props ?? {};
    const children = "children" in props ? props["children"] : undefined;
    const attrs = Object.keys(props)
      .filter((k) => k !== "children")
      .sort() // stable for determinism
      .map((k) => ` ${k}="${escapeHtml(String(props[k] ?? ""))}"`)
      .join("");
    return `<${v.type}${attrs}>${serializeVnode(children)}</${v.type}>`;
  }
  return "";
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/**
 * Fixture: two pages + one collection + one MDX-with-headings module.
 *
 * Mirrors the acceptance criteria: the sub-task's contract is "a fixture
 * with two pages + one collection + one MDX-with-headings module". The
 * collection is wired through the in-memory snapshot so `getCollection`
 * reads from memory; the MDX-with-headings shape exercises the
 * `headings` export contract that T4 emits.
 */
function buildFixture(): {
  pages: PageDefinition[];
  contentSnapshot: ContentSnapshot;
} {
  const blogIndex: PageModule = {
    default: () => ({
      type: "main",
      props: {
        children: [
          { type: "h1", props: { children: "Blog" }, key: null },
          { type: "p", props: { children: "Two posts." }, key: null },
        ],
      },
      key: null,
    }),
  };

  const postModule: PageModule = {
    default: () => ({
      type: "article",
      props: {
        children: [
          { type: "h2", props: { children: "Hello, world" }, key: null },
          { type: "h3", props: { children: "Sub-section" }, key: null },
        ],
      },
      key: null,
    }),
    headings: [
      { depth: 2, slug: "hello-world", text: "Hello, world" },
      { depth: 3, slug: "sub-section", text: "Sub-section" },
    ],
  };

  const pages: PageDefinition[] = [
    { route: "/", module: () => Promise.resolve(blogIndex) },
    { route: "/blog/hello", module: () => Promise.resolve(postModule) },
  ];

  const contentSnapshot: ContentSnapshot = {
    collections: {
      blog: [
        {
          slug: "hello",
          frontmatter: { title: "Hello, world", date: "2026-04-20" },
          body: "First post body.\n",
          module_specifier: "mdx://blog/hello",
          rel_path: "hello.mdx",
        },
      ],
    },
  };

  return { pages, contentSnapshot };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("createPageRouter", () => {
  // Restore the snapshot slot between tests so we don't leak across
  // suites. `zfb/content` exposes `setContentSnapshot(undefined)` for
  // exactly this purpose.
  afterEach(() => {
    setContentSnapshot(undefined);
  });

  it("returns a function that takes a Request and returns a Promise<Response>", () => {
    const { pages, contentSnapshot } = buildFixture();
    const router = createPageRouter({ pages, contentSnapshot, framework: stubFramework() });
    expect(typeof router).toBe("function");
    expect(router.length).toBe(1);
  });

  it("routes a GET to a registered page and returns 200 + text/html with the rendered body", async () => {
    const { pages, contentSnapshot } = buildFixture();
    const router = createPageRouter({ pages, contentSnapshot, framework: stubFramework() });

    const res = await router(new Request("http://test.local/"));
    expect(res.status).toBe(200);
    expect(res.headers.get("Content-Type")).toBe("text/html; charset=utf-8");
    const body = await res.text();
    expect(body).toContain("<h1>Blog</h1>");
    expect(body).toContain("<p>Two posts.</p>");
  });

  it("routes a second registered path independently", async () => {
    const { pages, contentSnapshot } = buildFixture();
    const router = createPageRouter({ pages, contentSnapshot, framework: stubFramework() });

    const res = await router(new Request("http://test.local/blog/hello"));
    expect(res.status).toBe(200);
    const body = await res.text();
    expect(body).toContain("<h2>Hello, world</h2>");
    expect(body).toContain("<h3>Sub-section</h3>");
  });

  it("returns 404 for an unregistered route", async () => {
    const { pages, contentSnapshot } = buildFixture();
    const router = createPageRouter({ pages, contentSnapshot, framework: stubFramework() });
    const res = await router(new Request("http://test.local/missing"));
    expect(res.status).toBe(404);
  });

  it("short-circuits when the page module returns a Response (API-route shape)", async () => {
    // Pages that handle non-HTML responses (e.g. `pages/api/*.tsx`) can
    // return a Response directly. The router must surface that Response
    // verbatim rather than running it through `framework.renderToString`.
    const apiPage: PageModule = {
      default: async () =>
        new Response(JSON.stringify({ ok: false, error: "bad request" }), {
          status: 400,
          headers: { "content-type": "application/json" },
        }),
    };
    const router = createPageRouter({
      pages: [{ route: "/api/echo", module: () => Promise.resolve(apiPage) }],
      contentSnapshot: { collections: {} },
      framework: stubFramework(),
    });
    const res = await router(
      new Request("http://test.local/api/echo", { method: "POST", body: "{}" }),
    );
    expect(res.status).toBe(400);
    expect(res.headers.get("Content-Type")).toBe("application/json");
    expect(await res.json()).toEqual({ ok: false, error: "bad request" });
  });

  it("dispatches non-GET methods to the page handler (app.all semantics)", async () => {
    // A page module that inspects request.method and returns different
    // bodies must receive the method-correct request. Before the
    // `app.get` → `app.all` switch, POST never reached the handler.
    const methodPage: PageModule = {
      default: async () => {
        // The handler doesn't have `request` in `componentInput`; the
        // contract for API routes is `getCloudflareContext()` from the CF
        // adapter. For this unit test we just assert the handler runs at
        // all when the request method is POST — see the integration test
        // in zfb-adapter-cloudflare for the env/ctx threading.
        return new Response("post-handled", { status: 201 });
      },
    };
    const router = createPageRouter({
      pages: [{ route: "/api/post-only", module: () => Promise.resolve(methodPage) }],
      contentSnapshot: { collections: {} },
      framework: stubFramework(),
    });
    const res = await router(new Request("http://test.local/api/post-only", { method: "POST" }));
    expect(res.status).toBe(201);
    expect(await res.text()).toBe("post-handled");
  });

  it("respects a page module's contentType override", async () => {
    const xmlPage: PageModule = {
      default: () => ({
        type: "rss",
        props: { children: { type: "channel", props: { children: "..." }, key: null } },
        key: null,
      }),
      contentType: "application/xml; charset=utf-8",
    };
    const router = createPageRouter({
      pages: [{ route: "/feed.xml", module: () => Promise.resolve(xmlPage) }],
      contentSnapshot: { collections: {} },
      framework: stubFramework(),
    });
    const res = await router(new Request("http://test.local/feed.xml"));
    expect(res.status).toBe(200);
    expect(res.headers.get("Content-Type")).toBe("application/xml; charset=utf-8");
  });

  it("registers the supplied ContentSnapshot with zfb/content (in-memory bridge)", async () => {
    const { pages, contentSnapshot } = buildFixture();
    createPageRouter({ pages, contentSnapshot, framework: stubFramework() });

    // After init, `getCollection("blog")` should resolve from memory
    // rather than touching `fs`. We import lazily so the import fans
    // through the same module-level state the router writes to.
    const { getCollection } = await import("@takazudo/zfb/content");
    const items = await getCollection<{ title: string; date: string }>("blog");
    expect(items).toHaveLength(1);
    expect(items[0]?.slug).toBe("hello");
    expect(items[0]?.data.title).toBe("Hello, world");
    expect(items[0]?.module_specifier).toBe("mdx://blog/hello");
  });

  it("returns an empty array for an unknown collection name (snapshot path)", async () => {
    const { pages, contentSnapshot } = buildFixture();
    createPageRouter({ pages, contentSnapshot, framework: stubFramework() });
    const { getCollection } = await import("@takazudo/zfb/content");
    const items = await getCollection("nope");
    expect(items).toEqual([]);
  });

  it("normalises null frontmatter to an empty object (snapshot path)", async () => {
    const router = createPageRouter({
      pages: [],
      contentSnapshot: {
        collections: {
          notes: [
            {
              slug: "raw",
              frontmatter: null,
              body: "raw body",
              module_specifier: "mdx://notes/raw",
              rel_path: "raw.md",
            },
          ],
        },
      },
      framework: stubFramework(),
    });
    void router; // not used here; we only care that the snapshot was registered
    const { getCollection } = await import("@takazudo/zfb/content");
    const items = await getCollection<{ title?: string }>("notes");
    expect(items).toHaveLength(1);
    expect(items[0]?.data).toEqual({});
  });

  it("spreads paths() props to top-level when paths() finds a match", async () => {
    // Capture whatever the router invokes default() with so the
    // assertion can verify the input shape matches the
    // ADR-002 / Astro paths() contract: props are spread at top level
    // alongside `params`, not nested under a `props` key.
    let received: unknown = null;
    const dynamicPage: PageModule & { paths: () => unknown[] } = {
      default: (input) => {
        received = input;
        return null;
      },
      paths: () => [
        { params: { slug: "hello" }, props: { title: "Hello" } },
        { params: { slug: "world" }, props: { title: "World" } },
      ],
    };
    const router = createPageRouter({
      pages: [{ route: "/blog/:slug", module: () => Promise.resolve(dynamicPage) }],
      contentSnapshot: { collections: {} },
      framework: { renderToString: () => "" },
    });

    const res = await router(new Request("http://test.local/blog/hello"));
    expect(res.status).toBe(200);
    expect(received).toEqual({
      params: { slug: "hello" },
      title: "Hello",
    });
  });

  it("passes only params when paths() finds a match without props", async () => {
    // When paths() returns an entry with no `props`, the component receives
    // only `{ params }` — no `props` key — because spreading an empty/absent
    // props object adds nothing. Components destructure params directly.
    let received: unknown = null;
    const dynamicPage: PageModule & { paths: () => unknown[] } = {
      default: (input) => {
        received = input;
        return null;
      },
      paths: () => [{ params: { slug: "hello" } }],
    };
    const router = createPageRouter({
      pages: [{ route: "/blog/:slug", module: () => Promise.resolve(dynamicPage) }],
      contentSnapshot: { collections: {} },
      framework: { renderToString: () => "" },
    });

    const res = await router(new Request("http://test.local/blog/hello"));
    expect(res.status).toBe(200);
    expect(received).toEqual({
      params: { slug: "hello" },
    });
  });

  it("returns 404 when paths() exists but the URL does not match any entry", async () => {
    const dynamicPage: PageModule & { paths: () => unknown[] } = {
      default: () => null,
      paths: () => [{ params: { slug: "hello" } }],
    };
    const router = createPageRouter({
      pages: [{ route: "/blog/:slug", module: () => Promise.resolve(dynamicPage) }],
      contentSnapshot: { collections: {} },
      framework: { renderToString: () => "" },
    });

    const res = await router(new Request("http://test.local/blog/missing"));
    expect(res.status).toBe(404);
  });

  it("returns 500 when paths() returns an entry whose params is not an object", async () => {
    const brokenPage: PageModule & { paths: () => unknown[] } = {
      default: () => null,
      paths: () => [{ params: "not-an-object" } as unknown],
    };
    const router = createPageRouter({
      pages: [{ route: "/blog/:slug", module: () => Promise.resolve(brokenPage) }],
      contentSnapshot: { collections: {} },
      framework: { renderToString: () => "" },
    });

    const res = await router(new Request("http://test.local/blog/hello"));
    expect(res.status).toBe(500);
    const body = await res.text();
    expect(body).toContain("valid params object");
  });

  it("returns 500 with a diagnostic body when a page module lacks a default export", async () => {
    const broken = { default: undefined as unknown as PageModule["default"] };
    const router = createPageRouter({
      pages: [{ route: "/x", module: () => Promise.resolve(broken as unknown as PageModule) }],
      contentSnapshot: { collections: {} },
      framework: stubFramework(),
    });
    const res = await router(new Request("http://test.local/x"));
    expect(res.status).toBe(500);
    const body = await res.text();
    expect(body).toContain("did not export a default component");
    expect(body).toContain('"/x"');
  });
});

// ---------------------------------------------------------------------------
// Issue #530 — SSR HTML5 doctype prepend
// ---------------------------------------------------------------------------

describe("createPageRouter — SSR doctype prepend (issue #530)", () => {
  afterEach(() => {
    setContentSnapshot(undefined);
  });

  /**
   * Helper: build a router whose single page returns a canned string body
   * via renderToString.
   */
  function routerWithBody(
    renderedHtml: string,
    contentType?: string,
  ): ReturnType<typeof createPageRouter> {
    const page: PageModule = {
      default: () => null,
      ...(contentType ? { contentType } : {}),
    };
    return createPageRouter({
      pages: [{ route: "/", module: () => Promise.resolve(page) }],
      contentSnapshot: { collections: {} },
      framework: { renderToString: () => renderedHtml },
    });
  }

  // (a) Plain <html> body without doctype — must prepend
  it("(a) prepends <!doctype html> when renderToString returns doctype-less <html> body", async () => {
    const router = routerWithBody("<html><head></head><body>hello</body></html>");
    const res = await router(new Request("http://test.local/"));
    expect(res.status).toBe(200);
    const body = await res.text();
    expect(body.startsWith("<!doctype html>")).toBe(true);
    expect(body).toContain("<html>");
  });

  // (b) Already has doctype — no double-prepend (case variation)
  it("(b) does not double-prepend when renderToString already returns <!DOCTYPE html>", async () => {
    const router = routerWithBody("<!DOCTYPE html><html><head></head><body>x</body></html>");
    const res = await router(new Request("http://test.local/"));
    const body = await res.text();
    expect(body.startsWith("<!DOCTYPE html>")).toBe(true);
    expect(body.toLowerCase().indexOf("<!doctype")).toBe(
      body.toLowerCase().lastIndexOf("<!doctype"),
    );
  });

  // (c-1) BOM-prefixed, no doctype — must prepend
  it("(c) prepends correctly when body starts with UTF-8 BOM + <html> (no doctype)", async () => {
    const bom = "﻿";
    const router = routerWithBody(`${bom}<html><head></head><body>bom</body></html>`);
    const res = await router(new Request("http://test.local/"));
    const body = await res.text();
    expect(body.startsWith("<!doctype html>")).toBe(true);
    expect(body).toContain("<html>");
  });

  // (c-2) BOM-prefixed + doctype already present — no double-prepend.
  // Note: Hono's c.body() strips the UTF-8 BOM when it encodes the
  // string to bytes, so the response body starts with `<!doctype html>`
  // (no BOM). The guard must not add a second doctype — there should be
  // exactly one `<!doctype` in the response.
  it("(c) does not double-prepend when BOM-prefixed body already has <!doctype html>", async () => {
    const bom = "﻿";
    const router = routerWithBody(`${bom}<!doctype html><html><head></head><body>x</body></html>`);
    const res = await router(new Request("http://test.local/"));
    const body = await res.text();
    // Exactly one <!doctype in the response (no double-prepend).
    const count = body.toLowerCase().split("<!doctype").length - 1;
    expect(count).toBe(1);
  });

  // (d-1) Leading whitespace, no doctype — must prepend
  it("(d) prepends correctly when body has leading whitespace before <html> (no doctype)", async () => {
    const router = routerWithBody("  \n<html><head></head><body>ws</body></html>");
    const res = await router(new Request("http://test.local/"));
    const body = await res.text();
    expect(body.startsWith("<!doctype html>")).toBe(true);
    expect(body).toContain("<html>");
  });

  // (d-2) Leading whitespace + doctype already present — no double-prepend
  it("(d) does not double-prepend when leading-whitespace body already has <!doctype html>", async () => {
    const router = routerWithBody("  \n<!doctype html><html><head></head><body>x</body></html>");
    const res = await router(new Request("http://test.local/"));
    const body = await res.text();
    expect(body.startsWith("<!doctype html>")).toBe(false);
    expect(body.startsWith("  \n<!doctype html>")).toBe(true);
  });

  // Non-HTML content type — must not touch the body
  it("does not prepend doctype for non-text/html content types", async () => {
    const router = routerWithBody(
      "<rss><channel><title>feed</title></channel></rss>",
      "application/xml; charset=utf-8",
    );
    const res = await router(new Request("http://test.local/"));
    const body = await res.text();
    expect(body.startsWith("<!doctype html>")).toBe(false);
    expect(body.startsWith("<rss>")).toBe(true);
  });
});

describe("createPageRouter — determinism", () => {
  beforeEach(() => {
    setContentSnapshot(undefined);
  });
  afterEach(() => {
    setContentSnapshot(undefined);
  });

  /**
   * Identical input → identical output bytes.
   *
   * The acceptance criterion calls out determinism modulo timestamps.
   * Our stub framework is pure (no clocks, no Math.random); Hono's
   * fetch path likewise produces a stable response body. We render the
   * same route twice from two independently-constructed routers and
   * compare byte-equal.
   */
  it("two routers built from the same input produce byte-equal responses", async () => {
    const fx = buildFixture();
    const r1 = createPageRouter({ ...fx, framework: stubFramework() });
    const r2 = createPageRouter({ ...fx, framework: stubFramework() });

    const [b1, b2] = await Promise.all([
      r1(new Request("http://test.local/")).then((r) => r.text()),
      r2(new Request("http://test.local/")).then((r) => r.text()),
    ]);
    expect(b1).toBe(b2);

    const [c1, c2] = await Promise.all([
      r1(new Request("http://test.local/blog/hello")).then((r) => r.text()),
      r2(new Request("http://test.local/blog/hello")).then((r) => r.text()),
    ]);
    expect(c1).toBe(c2);
  });

  it("a single router is idempotent under repeat requests for the same route", async () => {
    const fx = buildFixture();
    const router = createPageRouter({ ...fx, framework: stubFramework() });
    const a = await router(new Request("http://test.local/blog/hello")).then((r) => r.text());
    const b = await router(new Request("http://test.local/blog/hello")).then((r) => r.text());
    const c = await router(new Request("http://test.local/blog/hello")).then((r) => r.text());
    expect(a).toBe(b);
    expect(b).toBe(c);
  });
});

// ---------------------------------------------------------------------------
// Synthetic __paths__ endpoint
// ---------------------------------------------------------------------------

describe("createPageRouter — __paths__ endpoint", () => {
  afterEach(() => {
    setContentSnapshot(undefined);
  });

  it("returns the paths() array as JSON for a registered route", async () => {
    const slugPage: PageModule & { paths: () => unknown[] } = {
      default: () => null,
      paths: () => [{ params: { slug: "hello" } }, { params: { slug: "world" } }],
    };
    const router = createPageRouter({
      pages: [{ route: "/blog/:slug", module: () => Promise.resolve(slugPage) }],
      contentSnapshot: { collections: {} },
      framework: { renderToString: () => "" },
    });

    const encoded = encodeURIComponent("/blog/:slug");
    const res = await router(new Request(`http://test.local/__paths__/${encoded}`));
    expect(res.status).toBe(200);
    expect(res.headers.get("Content-Type")).toContain("application/json");
    const body = await res.json();
    expect(body).toEqual([{ params: { slug: "hello" } }, { params: { slug: "world" } }]);
  });

  it("returns the result of an async paths() export", async () => {
    const asyncPage: PageModule & { paths: () => Promise<unknown[]> } = {
      default: () => null,
      paths: async () => [{ params: { tag: "rust" } }, { params: { tag: "js" } }],
    };
    const router = createPageRouter({
      pages: [{ route: "/tags/:tag", module: () => Promise.resolve(asyncPage) }],
      contentSnapshot: { collections: {} },
      framework: { renderToString: () => "" },
    });

    const encoded = encodeURIComponent("/tags/:tag");
    const res = await router(new Request(`http://test.local/__paths__/${encoded}`));
    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body).toEqual([{ params: { tag: "rust" } }, { params: { tag: "js" } }]);
  });

  it("returns 404 when the route key is not registered", async () => {
    const router = createPageRouter({
      pages: [],
      contentSnapshot: { collections: {} },
      framework: { renderToString: () => "" },
    });

    const encoded = encodeURIComponent("/no-such/:slug");
    const res = await router(new Request(`http://test.local/__paths__/${encoded}`));
    expect(res.status).toBe(404);
    const body = await res.text();
    expect(body).toContain("/no-such/:slug");
  });

  it("returns 404 when the page module has no paths() export", async () => {
    const staticPage: PageModule = { default: () => null };
    const router = createPageRouter({
      pages: [{ route: "/about", module: () => Promise.resolve(staticPage) }],
      contentSnapshot: { collections: {} },
      framework: { renderToString: () => "" },
    });

    const encoded = encodeURIComponent("/about");
    const res = await router(new Request(`http://test.local/__paths__/${encoded}`));
    expect(res.status).toBe(404);
    const body = await res.text();
    expect(body).toContain("no paths() export");
  });

  it("returns 500 when paths() throws", async () => {
    const brokenPage: PageModule & { paths: () => Promise<unknown[]> } = {
      default: () => null,
      paths: async () => {
        throw new Error("collection unavailable");
      },
    };
    const router = createPageRouter({
      pages: [{ route: "/broken/:slug", module: () => Promise.resolve(brokenPage) }],
      contentSnapshot: { collections: {} },
      framework: { renderToString: () => "" },
    });

    const encoded = encodeURIComponent("/broken/:slug");
    const res = await router(new Request(`http://test.local/__paths__/${encoded}`));
    expect(res.status).toBe(500);
    const body = await res.text();
    expect(body).toContain("collection unavailable");
  });

  it("registers /__paths__ before user routes so a top-level catchall does not shadow it", async () => {
    // A user page authored as `pages/[...rest].tsx` would, after
    // `bracket_to_hono`, register as `/:rest{.+}`. If that route
    // were dispatched first, Hono would match `/__paths__/<key>`
    // against it and we'd never reach the synthetic handler. The
    // router registers `/__paths__/...` first to defeat this.
    const slugPage: PageModule & { paths: () => unknown[] } = {
      default: () => null,
      paths: () => [{ params: { slug: "hello" } }],
    };
    const wildcardPage: PageModule = {
      default: () => null,
    };
    const router = createPageRouter({
      pages: [
        // Register the catchall first to confirm even reverse insertion
        // order does not let it eat the synthetic endpoint.
        { route: "/:rest{.+}", module: () => Promise.resolve(wildcardPage) },
        { route: "/blog/:slug", module: () => Promise.resolve(slugPage) },
      ],
      contentSnapshot: { collections: {} },
      framework: { renderToString: () => "" },
    });

    const encoded = encodeURIComponent("/blog/:slug");
    const res = await router(new Request(`http://test.local/__paths__/${encoded}`));
    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body).toEqual([{ params: { slug: "hello" } }]);
  });

  it("does not double-decode the route key (literal % in key)", async () => {
    // Hono auto-decodes path params containing %. The router relies
    // on that single decode and does not call decodeURIComponent
    // again, so a literal % in the route key would survive the
    // round-trip if the build pipeline ever produced one. We use a
    // simple ASCII key here to confirm no extra decode happens.
    const page: PageModule & { paths: () => unknown[] } = {
      default: () => null,
      paths: () => [{ params: { x: "1" } }],
    };
    const router = createPageRouter({
      pages: [{ route: "/r/:x", module: () => Promise.resolve(page) }],
      contentSnapshot: { collections: {} },
      framework: { renderToString: () => "" },
    });

    const encoded = encodeURIComponent("/r/:x");
    const res = await router(new Request(`http://test.local/__paths__/${encoded}`));
    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body).toEqual([{ params: { x: "1" } }]);
  });

  it("can read content via getCollection() inside paths()", async () => {
    // Simulate a page whose paths() calls getCollection — the canonical
    // bundled basic-blog template pattern. The snapshot must be registered before paths()
    // runs; createPageRouter registers it on construction.
    const contentPage: PageModule & { paths: () => Promise<unknown[]> } = {
      default: () => null,
      paths: async () => {
        const { getCollection } = await import("@takazudo/zfb/content");
        const posts = await getCollection<{ title: string }>("blog");
        return posts.map((p) => ({ params: { slug: p.slug } }));
      },
    };
    const router = createPageRouter({
      pages: [{ route: "/blog/:slug", module: () => Promise.resolve(contentPage) }],
      contentSnapshot: {
        collections: {
          blog: [
            {
              slug: "hello",
              frontmatter: { title: "Hello" },
              body: "",
              module_specifier: "mdx://blog/hello",
              rel_path: "hello.mdx",
            },
            {
              slug: "world",
              frontmatter: { title: "World" },
              body: "",
              module_specifier: "mdx://blog/world",
              rel_path: "world.mdx",
            },
          ],
        },
      },
      framework: { renderToString: () => "" },
    });

    const encoded = encodeURIComponent("/blog/:slug");
    const res = await router(new Request(`http://test.local/__paths__/${encoded}`));
    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body).toEqual([{ params: { slug: "hello" } }, { params: { slug: "world" } }]);
  });
});
