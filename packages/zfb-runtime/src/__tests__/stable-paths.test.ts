import { afterEach, describe, expect, it, vi } from "vite-plus/test";
import { createPageRouter, stablePaths } from "../server.js";
import type { PageModule, StablePathsEntry } from "../server.js";
import { isStablePaths } from "../stable-paths.js";
import { isPathsEntry } from "../paths-entry.js";

// Test-only observation at the actual validator boundary. No production counter,
// environment branch, or replacement validation implementation is involved.
vi.mock("../paths-entry.js", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../paths-entry.js")>();
  return { ...actual, isPathsEntry: vi.fn(actual.isPathsEntry) };
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.mocked(isPathsEntry).mockClear();
});

function router(module: PageModule, route = "/posts/:slug") {
  const fetch = createPageRouter({
    pages: [{ route, module: async () => module }],
    contentSnapshot: { collections: {} },
  });
  return (path: string) => fetch(new Request(`https://example.test${path}`));
}

function unchecked(entries: unknown) {
  return stablePaths(entries as StablePathsEntry[]);
}

describe("stablePaths ownership", () => {
  it("supports an empty owned snapshot without admitting later source additions", async () => {
    const source: StablePathsEntry[] = [];
    const snapshot = stablePaths(source);
    source.push({ params: { slug: "a" } });
    const render = vi.fn(() => "unexpected");
    const fetch = router({ paths: () => snapshot, default: render });
    expect(Object.isFrozen(snapshot)).toBe(true);
    expect(isStablePaths(snapshot)).toBe(true);
    expect((await fetch("/posts/a")).status).toBe(404);
    expect((await fetch("/posts/a")).status).toBe(404);
    expect(await (await fetch(`/__paths__/${encodeURIComponent("/posts/:slug")}`)).json()).toEqual(
      [],
    );
    expect(render).not.toHaveBeenCalled();
  });

  it("owns membership/order/entry shells/params/arrays while retaining props", async () => {
    const props = { label: "before", nested: { value: 1 } };
    const source = [{ params: { slug: ["a", "b"] }, props }];
    const snapshot = stablePaths(source);
    expect(snapshot).not.toBe(source);
    expect(snapshot[0]).not.toBe(source[0]);
    expect(snapshot[0]!.params).not.toBe(source[0]!.params);
    expect(snapshot[0]!.params.slug).not.toBe(source[0]!.params.slug);
    expect(snapshot[0]!.props).toBe(props);
    expect(
      [snapshot, snapshot[0], snapshot[0]!.params, snapshot[0]!.params.slug].map(Object.isFrozen),
    ).toEqual([true, true, true, true]);
    expect(
      [source, source[0], source[0]!.params, source[0]!.params.slug, props, props.nested].map(
        Object.isFrozen,
      ),
    ).toEqual([false, false, false, false, false, false]);
    source[0]!.params.slug.push("changed");
    source[0]!.params = { slug: ["replacement"] };
    source[0]!.props = { label: "replacement", nested: { value: 2 } };
    source.splice(0, 1, { params: { slug: ["new"] }, props });
    const render = vi.fn((p: Record<string, unknown>) => {
      expect(p.params).toBe(snapshot[0]!.params);
      expect(p.nested).toBe(props.nested);
      expect(() => (p.params as { slug: string[] }).slug.push("mutation")).toThrow(TypeError);
      return String(p.label);
    });
    const fetch = router({ paths: () => snapshot, default: render }, "/posts/:slug{.+}");
    expect(await (await fetch("/posts/a/b")).text()).toBe("before");
    props.label = "after";
    expect(await (await fetch("/posts/a/b")).text()).toBe("after");
    expect((await fetch("/posts/new")).status).toBe(404);
    expect(render).toHaveBeenCalledTimes(2);
  });

  it("retains per-request props access and params override precedence", async () => {
    let counter = 0;
    const override = { mutable: true };
    const props = {
      params: override,
      get label() {
        return ++counter;
      },
    };
    const snapshot = stablePaths([{ params: { slug: "a" }, props }]);
    expect(counter).toBe(0);
    const fetch = router({
      paths: () => snapshot,
      default: (p) => {
        expect(p.params).toBe(override);
        return String(p.label);
      },
    });
    expect(await (await fetch("/posts/a")).text()).toBe("1");
    expect(await (await fetch("/posts/a")).text()).toBe("2");
  });

  it("captures duplicate order before later source reordering", async () => {
    const source = [
      { params: { slug: "a" }, props: { label: "first" } },
      { params: { slug: "a" }, props: { label: "second" } },
    ];
    const snapshot = stablePaths(source);
    const fetch = router({ paths: () => snapshot, default: (p) => String(p.label) });
    expect(await (await fetch("/posts/a")).text()).toBe("first");
    source.reverse();
    expect(await (await fetch("/posts/a")).text()).toBe("first");
  });

  it("safely copies a literal __proto__ own parameter and null-prototype params", async () => {
    const params = JSON.parse('{"__proto__":"safe","slug":"a"}');
    const snapshot = stablePaths([{ params }]);
    expect(Object.getPrototypeOf(snapshot[0]!.params)).toBeNull();
    expect(Object.hasOwn(snapshot[0]!.params, "__proto__")).toBe(true);
    expect(snapshot[0]!.params.__proto__).toBe("safe");
    const fetch = router({ paths: () => snapshot, default: (p) => JSON.stringify(p.params) });
    expect(await (await fetch("/posts/a")).json()).toEqual(params);
    expect(
      unchecked([{ params: Object.assign(Object.create(null), { slug: "a" }) }])[0]!.params.slug,
    ).toBe("a");
  });

  it("keeps brand private and rejects structural/frozen lookalikes", () => {
    const snapshot = stablePaths([{ params: { slug: "a" } }]);
    expect(isStablePaths(snapshot)).toBe(true);
    expect(isStablePaths(Object.freeze([...snapshot]))).toBe(false);
    expect(isStablePaths(new Proxy(snapshot, {}))).toBe(false);
    expect(Reflect.ownKeys(snapshot)).toEqual(["0", "length"]);
    expect(Reflect.ownKeys(snapshot[0]!)).toEqual(["params"]);
  });
});

describe("stablePaths preparation failures", () => {
  it.each(["entry slot", "params", "props", "param", "parameter slot"])(
    "rejects %s accessors without invoking them",
    (where) => {
      const getter = vi.fn(() => {
        throw new Error("getter ran");
      });
      const entry = { params: { slug: ["a"] }, props: {} };
      const entries = [entry];
      const [target, key] =
        where === "entry slot"
          ? [entries, "0"]
          : where === "params"
            ? [entry, "params"]
            : where === "props"
              ? [entry, "props"]
              : where === "param"
                ? [entry.params, "slug"]
                : [entry.params.slug, "0"];
      Object.defineProperty(target, key, { get: getter });
      expect(() => unchecked(entries)).toThrow("must not be an accessor");
      expect(getter).not.toHaveBeenCalled();
      expect(isStablePaths(entries)).toBe(false);
      expect(Object.isFrozen(entries)).toBe(false);
    },
  );

  it("rejects coercion objects and custom prototypes without calling coercion", () => {
    const coerce = vi.fn(() => "a");
    for (const slug of [
      { toString: coerce },
      { [Symbol.toPrimitive]: coerce },
      new String("a"),
      [["a"]],
    ]) {
      expect(() => unchecked([{ params: { slug } }])).toThrow("must be a string or finite number");
    }
    for (const input of [
      Object.setPrototypeOf([], {}),
      [Object.create({ params: { slug: "a" } })],
      [{ params: Object.create({ slug: "a" }) }],
      [{ params: { slug: Object.setPrototypeOf(["a"], {}) } }],
    ]) {
      expect(() => unchecked(input)).toThrow(/ordinary array|plain object/);
    }
    expect(coerce).not.toHaveBeenCalled();
  });

  it.each([
    null,
    {},
    [null],
    [{}],
    [{ params: null }],
    [{ params: [] }],
    new Array(1),
    [{ params: { slug: new Array(1) } }],
    [{ params: { slug: Infinity } }],
    [{ params: { slug: undefined } }],
    [{ params: { slug: true } }],
    [{ params: { slug: null } }],
    [{ params: { slug: Symbol("a") } }],
    [{ params: { slug: 1n } }],
  ])("rejects malformed ordinary data %#", (input) => {
    expect(() => unchecked(input)).toThrow(TypeError);
  });

  it("rejects symbol/hidden params and array hooks without executing them", () => {
    const getter = vi.fn();
    const hidden = Object.defineProperty({}, "slug", { value: "a" });
    expect(() => unchecked([{ params: hidden }])).toThrow("must be enumerable");
    expect(() => unchecked([{ params: { [Symbol("slug")]: "a" } }])).toThrow("only string keys");
    const slug = Object.defineProperty(["a"], "join", { get: getter });
    expect(() => unchecked([{ params: { slug } }])).toThrow("only array elements");
    expect(getter).not.toHaveBeenCalled();
  });

  it("does not brand partial preparation; corrected source can retry", () => {
    const source = [{ params: { slug: "a" } }, { params: { slug: {} as unknown } }];
    expect(() => unchecked(source)).toThrow();
    expect(isStablePaths(source)).toBe(false);
    source[1]!.params.slug = "b";
    const snapshot = unchecked(source);
    expect(isStablePaths(snapshot)).toBe(true);
    expect(snapshot.map((e) => e.params.slug)).toEqual(["a", "b"]);
  });

  it("allows proxy introspection effects and propagates traps without branding source", () => {
    const trap = vi.fn(() => {
      throw new Error("proxy trap");
    });
    const source = new Proxy([{ params: { slug: "a" } }], { getOwnPropertyDescriptor: trap });
    expect(() => unchecked(source)).toThrow("proxy trap");
    expect(trap).toHaveBeenCalled();
    expect(isStablePaths(source)).toBe(false);
  });
});

for (const stable of [false, true]) {
  describe(`${stable ? "stable" : "legacy"} matching compatibility`, () => {
    const capture = (entries: StablePathsEntry[]) => (stable ? stablePaths(entries) : entries);
    it.each([
      ["/posts/:slug", "/posts/42", { slug: 42 }],
      ["/posts/:slug", "/posts/a%20b%25%E6%97%A5", { slug: "a b%日" }],
      ["/posts/:slug{.+}", "/posts/a/b", { slug: ["a", "b"] }],
      ["/posts/:slug{.+}", "/posts/42/b", { slug: [42, "b"] }],
      ["/posts/:slug{.+}?", "/posts", { slug: [] }],
      ["/:lang/:slug", "/ja/a%3Ab%7Cc", { lang: "ja", slug: "a:b|c" }],
    ] as const)(
      "matches %s at %s without changing parameter values",
      async (route, path, params) => {
        const entries = capture([{ params }]);
        const fetch = router(
          { paths: () => entries, default: (p) => JSON.stringify(p.params) },
          route,
        );
        expect(await (await fetch(path)).json()).toEqual(params);
        expect((await fetch(`${path}/missing`)).status).toBe(404);
      },
    );

    it("preserves [] versus empty string and absent optional params", async () => {
      for (const params of [{ slug: "" }, {}]) {
        const fetch = router(
          { paths: () => capture([{ params }]), default: () => "wrong" },
          "/posts/:slug{.+}?",
        );
        expect((await fetch("/posts")).status).toBe(404);
      }
      const entries = capture([
        { params: { slug: "" }, props: { label: "empty" } },
        { params: { slug: [] }, props: { label: "array" } },
      ]);
      const fetch = router(
        { paths: () => entries, default: (p) => String(p.label) },
        "/posts/:slug{.+}?",
      );
      expect(await (await fetch("/posts")).text()).toBe("array");
    });

    it("preserves first-match behavior for duplicate and slash-equivalent values", async () => {
      const entries = capture([
        { params: { slug: ["a", "b"] }, props: { label: "first" } },
        { params: { slug: "a/b" }, props: { label: "second" } },
      ]);
      const fetch = router(
        { paths: () => entries, default: (p) => String(p.label) },
        "/posts/:slug{.+}",
      );
      expect(await (await fetch("/posts/a/b")).text()).toBe("first");
      expect(await (await fetch("/posts/a/b")).text()).toBe("first");
    });
  });
}

describe("stable validation lifetime and work", () => {
  it.each([1, 16, 128])(
    "does linear total preparation and full validation for N=%i requests/entries",
    async (n) => {
      const source = Array.from({ length: n }, (_, i) => ({
        params: { slug: [String(i)], section: "guide" },
      }));
      const watched = new Set<object>([
        source,
        ...source.flatMap((entry) => [entry, entry.params, entry.params.slug]),
      ]);
      const descriptor = vi.spyOn(Object, "getOwnPropertyDescriptor");
      const keys = vi.spyOn(Reflect, "ownKeys");
      const prototype = vi.spyOn(Object, "getPrototypeOf");
      const count = (spy: typeof descriptor | typeof keys | typeof prototype) =>
        spy.mock.calls.filter(([target]) => watched.has(target)).length;
      const snapshot = stablePaths(source);
      const preparation = [count(descriptor), count(keys), count(prototype)];
      expect(preparation).toEqual([7 * n + 1, 2 * n + 1, 3 * n + 1]);
      for (const entry of snapshot) {
        watched.add(entry);
        watched.add(entry.params);
        watched.add(entry.params.slug as object);
      }
      watched.add(snapshot);
      const paths = vi.fn(() => snapshot);
      const fetch = router({ paths, default: () => "ok" }, "/:section/:slug");
      // __paths__ must serialize ordinary wire data and share evaluation with renders.
      expect(
        await (await fetch(`/__paths__/${encodeURIComponent("/:section/:slug")}`)).json(),
      ).toEqual(source);
      for (let i = 0; i < n; i++) expect((await fetch(`/guide/${i}`)).status).toBe(200);
      expect((await fetch("/guide/missing")).status).toBe(404);
      expect(paths).toHaveBeenCalledTimes(1);
      expect(isPathsEntry).toHaveBeenCalledTimes(n);
      expect([count(descriptor), count(keys), count(prototype)]).toEqual(preparation);
    },
  );

  it("validates ordinary frozen arrays on every request without implicit opt-in", async () => {
    const entries = Object.freeze([{ params: { slug: "a" } }, { params: { slug: "b" } }]);
    const fetch = router({ paths: () => entries, default: () => "ok" });
    for (let i = 0; i < 3; i++) expect((await fetch("/posts/a")).status).toBe(200);
    expect(isPathsEntry).toHaveBeenCalledTimes(6);
    entries[1]!.params = null as never;
    expect((await fetch("/posts/a")).status).toBe(500);
  });

  it("isolates validation by router generation even when a snapshot is shared", async () => {
    const snapshot = stablePaths([{ params: { slug: "a" } }]);
    const paths = vi.fn(() => snapshot);
    const module = { paths, default: () => "ok" };
    const first = router(module);
    const second = router(module);
    await first("/posts/a");
    await first("/posts/a");
    expect(isPathsEntry).toHaveBeenCalledTimes(1);
    await second("/posts/a");
    await second("/posts/a");
    expect(isPathsEntry).toHaveBeenCalledTimes(2);
    expect(paths).toHaveBeenCalledTimes(2);
    const fresh = router({
      paths: () => stablePaths([{ params: { slug: "b" } }]),
      default: () => "new",
    });
    expect((await fresh("/posts/a")).status).toBe(404);
    expect(await (await fresh("/posts/b")).text()).toBe("new");
    expect(await (await first("/posts/a")).text()).toBe("ok");
  });

  it.each([false, true])(
    "retries %s async preparation failure and validates only complete snapshots",
    async (asyncFailure) => {
      const source = [{ params: { slug: "a" } }, { params: { slug: {} as unknown } }];
      const prepare = () => unchecked(source);
      const paths = vi.fn(asyncFailure ? async () => prepare() : prepare);
      const render = vi.fn(() => "ok");
      const fetch = router({ paths, default: render });
      expect((await fetch("/posts/a")).status).toBe(500);
      expect(isPathsEntry).not.toHaveBeenCalled();
      expect(render).not.toHaveBeenCalled();
      source[1]!.params.slug = "b";
      expect((await fetch("/posts/a")).status).toBe(200);
      expect((await fetch("/posts/b")).status).toBe(200);
      expect(paths).toHaveBeenCalledTimes(2);
      expect(isPathsEntry).toHaveBeenCalledTimes(2);
    },
  );

  it("shares an in-flight evaluation between endpoint and render, including reject/retry", async () => {
    let resolve!: (value: readonly StablePathsEntry[]) => void;
    let reject!: (reason: Error) => void;
    const paths = vi.fn(
      () =>
        new Promise<readonly StablePathsEntry[]>((res, rej) => {
          resolve = res;
          reject = rej;
        }),
    );
    const fetch = router({ paths, default: () => "ok" });
    const endpoint = `/__paths__/${encodeURIComponent("/posts/:slug")}`;
    const rejected = [fetch(endpoint), fetch("/posts/a")];
    await vi.waitFor(() => expect(paths).toHaveBeenCalledTimes(1));
    reject(new Error("retry me"));
    expect((await Promise.all(rejected)).map((r) => r.status)).toEqual([500, 500]);
    const retried = [fetch(endpoint), fetch("/posts/a"), fetch("/posts/a")];
    await vi.waitFor(() => expect(paths).toHaveBeenCalledTimes(2));
    resolve(stablePaths([{ params: { slug: "a" } }]));
    expect((await Promise.all(retried)).map((r) => r.status)).toEqual([200, 200, 200]);
    expect(isPathsEntry).toHaveBeenCalledTimes(1);
  });

  it("does not cache no-paths SSR, getStaticProps, or render failures", async () => {
    let serial = 0;
    const ssr = router({ default: (p) => `${JSON.stringify(p.params)}:${++serial}` });
    expect(await (await ssr("/posts/a")).text()).toBe('{"slug":"a"}:1');
    expect(await (await ssr("/posts/b")).text()).toBe('{"slug":"b"}:2');
    const getStaticProps = vi.fn(async () => ({ props: { serial: ++serial } }));
    const staticFetch = router({ getStaticProps, default: (p) => String(p.serial) }, "/");
    expect(await (await staticFetch("/")).text()).toBe("3");
    expect(await (await staticFetch("/")).text()).toBe("4");
    expect(getStaticProps).toHaveBeenCalledTimes(2);
    let fail = true;
    const fetch = router({
      paths: () => stablePaths([{ params: { slug: "a" } }]),
      default: () => {
        if (fail) throw new Error("render failed");
        return "ok";
      },
    });
    expect((await fetch("/posts/a")).status).toBe(500);
    fail = false;
    expect(await (await fetch("/posts/a")).text()).toBe("ok");
    expect(isPathsEntry).toHaveBeenCalledTimes(1);
  });
});
