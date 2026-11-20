// #4750: `devTrailingSlashDispatch` (native `zfb dev` with `trailingSlash: true`)
// selects the slashless route for an eligible slash URL while the handler still
// sees the original Request. The default router stays strict.
import { describe, expect, it, vi } from "vite-plus/test";

import { createPageRouter, devTrailingSlashLookupPath } from "../router.js";
import type { PageDefinition, PageModule } from "../router.js";
import type { ContentSnapshot } from "../snapshot.js";

vi.mock("@takazudo/zfb/zudo-react/server", () => ({
  renderToString(node: unknown): string {
    return typeof node === "string" ? node : JSON.stringify(node);
  },
}));

const snapshot: ContentSnapshot = { collections: {} };

interface Seen {
  route: string;
  input: unknown;
}

function echoPage(route: string, seen: Seen[]): PageDefinition {
  const mod: PageModule = {
    default: (input: unknown) => {
      seen.push({ route, input });
      return `${route}|${JSON.stringify(input)}`;
    },
  };
  return { route, module: () => Promise.resolve(mod) };
}

const ROUTES = ["/request", "/live/:slug", "/req/:rest{.+}", "/opt/:rest{.+}?", "/a/:x/b"];

function routers() {
  const seen: Seen[] = [];
  const pages = ROUTES.map((r) => echoPage(r, seen));
  return {
    seen,
    dev: createPageRouter({ pages, contentSnapshot: snapshot, devTrailingSlashDispatch: true }),
    strict: createPageRouter({ pages, contentSnapshot: snapshot }),
    off: createPageRouter({ pages, contentSnapshot: snapshot, devTrailingSlashDispatch: false }),
  };
}

const get = (path: string, init?: RequestInit) => new Request(`http://localhost${path}`, init);

describe("devTrailingSlashLookupPath", () => {
  it.each([
    ["/live/first/", "/live/first"],
    ["/request/", "/request"],
    ["/opt/", "/opt"],
    ["/opt/a/b/", "/opt/a/b"],
    ["/v/a%2Fb/", "/v/a%2Fb"],
  ])("strips one literal trailing slash: %s", (input, expected) => {
    expect(devTrailingSlashLookupPath(input)).toBe(expected);
  });

  it.each([
    "/",
    "/live/first",
    "/live/first//",
    "//",
    "/feed.xml/",
    "/live/v1.2/",
    "/__paths__/x/",
    "/live/a%2F",
  ])("leaves %s unchanged", (input) => {
    expect(devTrailingSlashLookupPath(input)).toBe(input);
  });
});

describe("createPageRouter devTrailingSlashDispatch", () => {
  it("routes the reported slash URLs like their slashless controls", async () => {
    const { dev } = routers();
    for (const [slash, bare] of [
      ["/live/first/", "/live/first"],
      ["/request/", "/request"],
    ]) {
      const a = await dev(get(slash));
      const b = await dev(get(bare));
      expect(a.status).toBe(200);
      expect(b.status).toBe(200);
      expect(await a.text()).toBe(await b.text());
    }
  });

  it("keeps dynamic and catch-all params identical to the slashless form", async () => {
    const { dev } = routers();
    expect(await (await dev(get("/live/first/"))).text()).toBe(
      '/live/:slug|{"params":{"slug":"first"}}',
    );
    expect(await (await dev(get("/req/a/b/"))).text()).toBe(
      '/req/:rest{.+}|{"params":{"rest":"a/b"}}',
    );
    expect(await (await dev(get("/opt/a/b/"))).text()).toBe(
      '/opt/:rest{.+}?|{"params":{"rest":"a/b"}}',
    );
    // Optional catch-all zero-segment slash form reaches the bare match.
    const zero = await dev(get("/opt/"));
    expect(zero.status).toBe(200);
    expect(await zero.text()).toBe(await (await dev(get("/opt"))).text());
    expect(await (await dev(get("/a/1/b/"))).text()).toBe('/a/:x/b|{"params":{"x":"1"}}');
  });

  it("still rejects a zero-segment required catch-all", async () => {
    const { dev } = routers();
    expect((await dev(get("/req/"))).status).toBe(404);
    expect((await dev(get("/req"))).status).toBe(404);
  });

  it("does not normalize root, repeated slashes, extensions or encoded slashes", async () => {
    const { dev } = routers();
    expect((await dev(get("/live/first//"))).status).toBe(404);
    expect((await dev(get("/live//first/"))).status).toBe(404);
    expect((await dev(get("/live/v1.2/"))).status).toBe(404);
    // `%2E` decodes to `.`, so it is an extension too (matches the Rust rule).
    expect((await dev(get("/live/v1%2E2/"))).status).toBe(404);
    expect((await dev(get("/"))).status).toBe(404);
    // `%2F` stays encoded through Hono's decodeURI, so it is a param byte,
    // never an eligible trailing slash.
    const enc = await dev(get("/live/a%2F"));
    expect(await enc.text()).toBe(await (await routers().strict(get("/live/a%2F"))).text());
  });

  it("routes with the original Request untouched (url, query, method, headers, body)", async () => {
    const mod: PageModule = { default: () => "ok" };
    const router = createPageRouter({
      pages: [{ route: "/request", module: () => Promise.resolve(mod) }],
      contentSnapshot: snapshot,
      devTrailingSlashDispatch: true,
    });
    const original = get("/request/?b=2&a=1&a=%20", {
      method: "POST",
      headers: { "x-probe": "kept", "content-type": "text/plain" },
      body: "payload",
    });
    const res = await router(original);
    expect(res.status).toBe(200);
    expect(original.url).toBe("http://localhost/request/?b=2&a=1&a=%20");
    expect(original.method).toBe("POST");
    expect(original.headers.get("x-probe")).toBe("kept");
    expect(original.bodyUsed).toBe(false);
    expect(await original.text()).toBe("payload");
  });

  it("answers HEAD on a slash URL like the slashless form", async () => {
    const { dev } = routers();
    const a = await dev(get("/request/", { method: "HEAD" }));
    const b = await dev(get("/request", { method: "HEAD" }));
    expect(a.status).toBe(b.status);
    expect(a.headers.get("content-type")).toBe(b.headers.get("content-type"));
  });

  it("leaves the default and explicit-false routers strict", async () => {
    const { strict, off } = routers();
    for (const r of [strict, off]) {
      expect((await r(get("/live/first/"))).status).toBe(404);
      expect((await r(get("/request/"))).status).toBe(404);
      expect((await r(get("/opt/"))).status).toBe(404);
      expect((await r(get("/live/first"))).status).toBe(200);
    }
  });
});
