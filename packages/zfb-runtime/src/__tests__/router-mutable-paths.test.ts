import { describe, expect, it, vi } from "vite-plus/test";
import { createPageRouter } from "../router.js";
import type { PageModule } from "../router.js";

function router(module: PageModule, route = "/posts/:slug") {
  const fetch = createPageRouter({
    pages: [{ route, module: async () => module }],
    contentSnapshot: { collections: {} },
  });
  return (path: string) => fetch(new Request(`https://example.test${path}`));
}

describe("legacy mutable paths — baseline compatibility matrix", () => {
  it("rejects an initially malformed late entry even when the first entry matches", async () => {
    const entries: unknown[] = [{ params: { slug: "a" } }, {}];
    const render = vi.fn(() => "ok");
    const fetch = router({ paths: () => entries, default: render });
    expect((await fetch("/posts/a")).status).toBe(500);
    expect(render).not.toHaveBeenCalled();
    entries.pop();
    expect((await fetch("/posts/a")).status).toBe(200);
    expect(render).toHaveBeenCalledTimes(1);
  });

  it("detects a malformed tail before rendering, including after first success, and recovers", async () => {
    const entries: unknown[] = [{ params: { slug: "a" } }];
    const render = vi.fn(() => "ok");
    const fetch = router({ paths: () => entries, default: render });
    expect((await fetch("/posts/a")).status).toBe(200);
    entries.push({ params: null });
    expect((await fetch("/posts/a")).status).toBe(500);
    expect(render).toHaveBeenCalledTimes(1);
    entries.pop();
    expect((await fetch("/posts/a")).status).toBe(200);
    entries[0] = { props: {} };
    expect((await fetch("/posts/a")).status).toBe(500);
    entries[0] = { params: { slug: "a" } };
    expect((await fetch("/posts/a")).status).toBe(200);
  });

  it("observes slug/shape edits, additions, deletions and array slot replacement", async () => {
    const entries = [{ params: { slug: "a" }, props: { label: "A" } }];
    const fetch = router({ paths: () => entries, default: (p) => String(p.label) });
    expect(await (await fetch("/posts/a")).text()).toBe("A");
    entries[0]!.params.slug = "b";
    expect((await fetch("/posts/a")).status).toBe(404);
    expect(await (await fetch("/posts/b")).text()).toBe("A");
    entries.push({ params: { slug: "c" }, props: { label: "C" } });
    expect(await (await fetch("/posts/c")).text()).toBe("C");
    entries.splice(0, 1);
    expect((await fetch("/posts/b")).status).toBe(404);
    entries[0] = { params: { slug: "d" }, props: { label: "D" } };
    expect((await fetch("/posts/c")).status).toBe(404);
    expect(await (await fetch("/posts/d")).text()).toBe("D");
    Reflect.deleteProperty(entries[0]!.params, "slug");
    expect((await fetch("/posts/d")).status).toBe(404);
    // Legacy coercion of a missing parameter is intentionally unchanged.
    expect(await (await fetch("/posts/undefined")).text()).toBe("D");
  });

  it("keeps first-match duplicate order and live props references/priority", async () => {
    const first = { params: { slug: "a" }, props: { label: "first", params: "override" } };
    const second = { params: { slug: "a" }, props: { label: "second", params: "second override" } };
    const entries = [first, second];
    const fetch = router({ paths: () => entries, default: (p) => `${p.label}:${p.params}` });
    expect(await (await fetch("/posts/a")).text()).toBe("first:override");
    first.props.label = "edited";
    expect(await (await fetch("/posts/a")).text()).toBe("edited:override");
    entries.reverse();
    expect(await (await fetch("/posts/a")).text()).toBe("second:second override");
    entries[0]!.props = { label: "replacement", params: "replacement override" };
    expect(await (await fetch("/posts/a")).text()).toBe("replacement:replacement override");
  });

  it("preserves component mutations of source params and catchall arrays", async () => {
    const entry = { params: { slug: ["a"] } };
    const fetch = router(
      {
        paths: () => [entry],
        default: (p) => {
          expect(p.params).toBe(entry.params);
          const before = entry.params.slug.join("/");
          entry.params.slug.push("b");
          return before;
        },
      },
      "/posts/:slug{.+}",
    );
    expect(await (await fetch("/posts/a")).text()).toBe("a");
    expect((await fetch("/posts/a")).status).toBe(404);
    expect(await (await fetch("/posts/a/b")).text()).toBe("a/b");
    expect(Object.isFrozen(entry.params)).toBe(false);
  });

  it("continues to evaluate legacy getters/custom coercion per request", async () => {
    let slug = "a";
    const coercion = vi.fn(() => slug);
    const readParams = vi.fn(() => ({ slug: { toString: coercion } }));
    const entry = Object.defineProperty({}, "params", { get: readParams });
    const fetch = router({ paths: () => [entry], default: () => "ok" });
    expect((await fetch("/posts/a")).status).toBe(200);
    slug = "b";
    expect((await fetch("/posts/b")).status).toBe(200);
    expect(readParams).toHaveBeenCalledTimes(6); // validate, match, component input
    expect(coercion).toHaveBeenCalledTimes(2);
  });

  it("never implicitly freezes production values", async () => {
    const entry = { params: { slug: "a" } };
    const entries = [entry];
    const fetch = router({ paths: () => entries, default: () => "ok" });
    expect((await fetch("/posts/a")).status).toBe(200);
    expect([entries, entry, entry.params].map(Object.isFrozen)).toEqual([false, false, false]);
    entry.params.slug = "b";
    expect((await fetch("/posts/b")).status).toBe(200);
  });
});
