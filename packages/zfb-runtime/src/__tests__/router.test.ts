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

  it("respects a page module's content_type override", async () => {
    const xmlPage: PageModule = {
      default: () => ({
        type: "rss",
        props: { children: { type: "channel", props: { children: "..." }, key: null } },
        key: null,
      }),
      content_type: "application/xml; charset=utf-8",
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

  it("can read content via getCollection() inside paths()", async () => {
    // Simulate a page whose paths() calls getCollection — the canonical
    // basic-blog pattern. The snapshot must be registered before paths()
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
