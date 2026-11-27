// Router-level contract for SSR shared assets and `htmlPage` (#4887).
//
// Uses the real owned renderer (unlike router.test.ts, which mocks it), so the
// JSX cases prove what a built SSR bundle sends.

import { describe, expect, it } from "vite-plus/test";
import { jsx } from "@takazudo/zfb/zudo-react/jsx-runtime";

import { createPageRouter, type PageModule } from "../router.js";
import { htmlPage } from "../server.js";
import { isHtmlPage } from "../html-page.js";
import type { PageAssetManifest } from "../page-assets.js";

const ASSETS: PageAssetManifest = {
  assets: [
    { kind: "css", stableUrl: "/assets/styles.css", url: "/assets/styles-aaa111.css" },
    { kind: "islands", stableUrl: "/assets/islands.js", url: "/assets/islands-bbb222.js" },
    { kind: "client-script", stableUrl: "/assets/menu.js", url: "/assets/menu-ccc333.js" },
  ],
};

const FINAL_HEAD =
  '<link rel="stylesheet" href="/assets/styles-aaa111.css">' +
  '<script type="module" src="/assets/islands-bbb222.js"></script>';

function Layout({ children }: { children?: unknown }) {
  return jsx("html", {
    children: [
      jsx("head", { children: jsx("title", { children: "t" }) }),
      jsx("body", {
        children: [children, jsx("script", { src: "/assets/menu.js" })],
      }),
    ],
  });
}

const layoutPage = (text: string) => jsx(Layout, { children: jsx("main", { children: text }) });

// `null` builds a router without the `assets` option.
function routerFor(page: PageModule, assets: PageAssetManifest | null = ASSETS) {
  return createPageRouter({
    pages: [{ route: "/", module: async () => page }],
    contentSnapshot: { collections: {} },
    ...(assets !== null && { assets }),
  });
}

const get = (router: ReturnType<typeof createPageRouter>, method = "GET") =>
  router(new Request("https://example.test/", { method }));

const EXPECTED_LAYOUT_HTML =
  "<!doctype html>\n<html><head><title>t</title>" +
  FINAL_HEAD +
  '</head><body><main>hi</main><script src="/assets/menu-ccc333.js"></script></body></html>';

describe("managed JSX HTML", () => {
  it("finishes asset URLs and head tags for an ordinary JSX page", async () => {
    const res = await get(routerFor({ default: () => layoutPage("hi") }));
    expect(res.status).toBe(200);
    expect(res.headers.get("content-type")).toBe("text/html; charset=utf-8");
    expect(await res.text()).toBe(EXPECTED_LAYOUT_HTML);
  });

  it("treats an explicit text/html contentType as managed", async () => {
    const res = await get(
      routerFor({ default: () => layoutPage("hi"), contentType: "TEXT/HTML;charset=utf-8" }),
    );
    expect(await res.text()).toBe(EXPECTED_LAYOUT_HTML);
  });

  it("is byte-identical to today without an assets option or with an empty record", async () => {
    const page: PageModule = { default: () => layoutPage("hi") };
    const expected =
      '<!doctype html>\n<html><head><title>t</title></head><body><main>hi</main><script src="/assets/menu.js"></script></body></html>';
    expect(await (await get(routerFor(page, null))).text()).toBe(expected);
    expect(await (await get(routerFor(page, { assets: [] }))).text()).toBe(expected);
  });

  it("does not touch non-HTML contentType JSX", async () => {
    const res = await get(
      routerFor({
        contentType: "text/plain; charset=utf-8",
        default: () =>
          jsx("html", {
            children: jsx("head", { children: jsx("link", { href: "/assets/styles.css" }) }),
          }),
      }),
    );
    expect(await res.text()).toBe('<html><head><link href="/assets/styles.css"></head></html>');
  });

  it("does not touch plain string results, even as text/html", async () => {
    const html = '<html><head></head><body><script src="/assets/menu.js"></script></body></html>';
    const res = await get(routerFor({ default: () => html }));
    expect(await res.text()).toBe(`<!doctype html>\n${html}`);
  });

  it("snapshots the record at router creation", async () => {
    const mutable = {
      assets: [
        { kind: "css" as const, stableUrl: "/assets/styles.css", url: "/assets/styles-aaa111.css" },
      ],
    };
    const router = routerFor({ default: () => layoutPage("hi") }, mutable);
    mutable.assets[0]!.url = "/assets/changed.css";
    expect(await (await get(router)).text()).toContain("/assets/styles-aaa111.css");
  });
});

describe("raw Response results stay passthrough", () => {
  const cases: [string, () => Response][] = [
    [
      "HTML",
      () =>
        new Response('<html><head></head><a href="/assets/styles.css"></a></html>', {
          headers: { "content-type": "text/html" },
        }),
    ],
    ["JSON", () => Response.json({ href: "/assets/styles.css" })],
    [
      "binary",
      () =>
        new Response(new Uint8Array([0, 1, 2, 255]), {
          headers: { "content-type": "application/octet-stream" },
        }),
    ],
    [
      "redirect",
      () => new Response(null, { status: 302, headers: { location: "/assets/styles.css" } }),
    ],
  ];

  it.each(cases)(
    "returns a raw %s Response as-is without reading its body",
    async (_name, make) => {
      const original = make();
      const res = await get(routerFor({ default: () => original }));
      expect(res).toBe(original);
      expect(original.bodyUsed).toBe(false);
    },
  );

  it("does not pre-read a raw stream: the first chunk arrives before later ones exist", async () => {
    let releaseSecond!: () => void;
    const second = new Promise<void>((resolve) => (releaseSecond = resolve));
    const encoder = new TextEncoder();
    const stream = new ReadableStream<Uint8Array>({
      async start(controller) {
        controller.enqueue(encoder.encode("first"));
        await second;
        controller.enqueue(encoder.encode("second"));
        controller.close();
      },
    });
    const res = await get(
      routerFor({
        default: () => new Response(stream, { headers: { "content-type": "text/html" } }),
      }),
    );
    const reader = res.body!.getReader();
    const chunk = await reader.read();
    expect(new TextDecoder().decode(chunk.value)).toBe("first");
    releaseSecond();
    const rest = await reader.read();
    expect(new TextDecoder().decode(rest.value)).toBe("second");
  });
});

describe("HEAD", () => {
  it("sends no body for a managed JSX page", async () => {
    const res = await get(routerFor({ default: () => layoutPage("hi") }), "HEAD");
    expect(res.status).toBe(200);
    expect(await res.text()).toBe("");
  });

  it("sends no body for an htmlPage result", async () => {
    const res = await get(
      routerFor({ default: () => htmlPage(layoutPage("hi"), { status: 404 }) }),
      "HEAD",
    );
    expect(res.status).toBe(404);
    expect(await res.text()).toBe("");
  });

  it("sends no body for a raw Response", async () => {
    const res = await get(routerFor({ default: () => new Response("raw") }), "HEAD");
    expect(await res.text()).toBe("");
  });
});

describe("htmlPage in the router", () => {
  it("renders a 404 JSX page with its headers, cookies and finished assets", async () => {
    const res = await get(
      routerFor({
        default: () =>
          htmlPage(layoutPage("hi"), {
            status: 404,
            statusText: "Not Here",
            headers: [
              ["Cache-Control", "no-store"],
              ["Vary", "Accept-Language"],
              ["X-Custom", "1"],
              ["Set-Cookie", "a=1; Path=/"],
              ["Set-Cookie", "b=2; Path=/"],
              ["Content-Length", "3"],
              ["ETag", '"abc"'],
              ["Digest", "sha-256=x"],
              ["Content-Digest", "sha-256=:x:"],
              ["Repr-Digest", "sha-256=:x:"],
              ["Content-MD5", "x"],
              ["Content-Encoding", "identity"],
            ],
          }),
      }),
    );
    expect(res.status).toBe(404);
    expect(res.statusText).toBe("Not Here");
    expect(res.headers.get("cache-control")).toBe("no-store");
    expect(res.headers.get("vary")).toBe("Accept-Language");
    expect(res.headers.get("x-custom")).toBe("1");
    expect(res.headers.getSetCookie()).toEqual(["a=1; Path=/", "b=2; Path=/"]);
    expect(res.headers.get("content-type")).toBe("text/html; charset=utf-8");
    for (const name of [
      "etag",
      "digest",
      "content-digest",
      "repr-digest",
      "content-md5",
      "content-encoding",
    ]) {
      expect(res.headers.has(name)).toBe(false);
    }
    const body = await res.text();
    expect(body).toBe(EXPECTED_LAYOUT_HTML);
    expect(res.headers.get("content-length")).not.toBe("3");
  });

  it("uses a string body as-is apart from the doctype and asset finalizer", async () => {
    const res = await get(
      routerFor({
        default: () =>
          htmlPage('<html><head></head><body><a href="/assets/menu.js">m</a></body></html>'),
      }),
    );
    expect(res.status).toBe(200);
    expect(await res.text()).toBe(
      `<!doctype html>\n<html><head>${FINAL_HEAD}</head><body><a href="/assets/menu-ccc333.js">m</a></body></html>`,
    );
  });

  it("without an assets option only renders, keeping the body bytes", async () => {
    const res = await get(
      routerFor({ default: () => htmlPage("<p>/assets/menu.js</p>", { status: 410 }) }, null),
    );
    expect(res.status).toBe(410);
    expect(await res.text()).toBe("<p>/assets/menu.js</p>");
  });

  it("reports the original render error through the debug replay", async () => {
    const router = createPageRouter({
      pages: [
        {
          route: "/",
          module: async () => ({
            default: () => htmlPage(jsx("input", { autoComplete: "off" }), { status: 404 }),
          }),
        },
      ],
      contentSnapshot: { collections: {} },
      includeErrorStack: true,
      assets: ASSETS,
    });
    const res = await get(router);
    expect(res.status).toBe(500);
    const body = await res.text();
    expect(body).toContain("ZR_PROP_DIALECT");
    const line = body.split("\n").find((l) => l.startsWith("[zfb-render-diagnostic] "));
    expect(JSON.parse(line!.slice("[zfb-render-diagnostic] ".length))).toHaveProperty(
      "site.kind",
      "generated",
    );
  });
});

describe("htmlPage descriptor", () => {
  it("is frozen, branded across module copies and not a Response", () => {
    const page = htmlPage("<p>x</p>");
    expect(Object.isFrozen(page)).toBe(true);
    expect(Object.isFrozen(page.headers)).toBe(true);
    expect(page).not.toBeInstanceOf(Response);
    expect(isHtmlPage(page)).toBe(true);
    expect(isHtmlPage({ [Symbol.for("@takazudo/zfb-runtime/html-page-v1")]: true })).toBe(true);
    expect(isHtmlPage({ body: "x", status: 200 })).toBe(false);
    expect(page.status).toBe(200);
    expect(page.headers).toEqual([["content-type", "text/html; charset=utf-8"]]);
  });

  it.each([
    [{ status: 199 }, /200 to 599/],
    [{ status: 600 }, /200 to 599/],
    [{ status: 404.5 }, /integer/],
    [{ status: 204 }, /raw Response/],
    [{ status: 205 }, /raw Response/],
    [{ status: 304 }, /raw Response/],
    [{ statusText: "bad\nline" }, /statusText/],
    [{ headers: { "Content-Type": "application/json" } }, /text\/html/],
    [{ headers: { "Content-Encoding": "gzip" } }, /Content-Encoding/],
  ])("rejects %o with a TypeError", (init, message) => {
    expect(() => htmlPage("<p>x</p>", init as never)).toThrow(TypeError);
    expect(() => htmlPage("<p>x</p>", init as never)).toThrow(message);
  });

  it("rejects a Response or nested descriptor body", () => {
    expect(() => htmlPage(new Response("x"))).toThrow(TypeError);
    expect(() => htmlPage(htmlPage("x"))).toThrow(TypeError);
  });

  it("accepts an explicit text/html Content-Type with parameters", () => {
    expect(
      htmlPage("x", { headers: { "content-type": "text/html; charset=UTF-8" } }).headers,
    ).toEqual([["content-type", "text/html; charset=UTF-8"]]);
  });
});
