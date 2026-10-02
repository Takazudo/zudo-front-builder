import { afterEach, describe, expect, it, vi } from "vitest";
import { Island } from "../../island.js";
import { h, signal } from "../../zudo-react/index.js";
import { createIslandTest, withIslandTestContext } from "../../zudo-react/testing.js";
import { renderToString } from "../../zudo-react/server.js";
import { subscriberCount } from "../../zudo-react/reactive.js";

const tests: Array<ReturnType<typeof createIslandTest>> = [];
afterEach(() => {
  for (const test of tests) test.dispose();
  tests.length = 0;
  document.body.replaceChildren();
});

describe("createIslandTest", () => {
  it("renders transport HTML, hydrates without replacing nodes, flushes, and disposes", async () => {
    const value = signal("first");
    function Counter(props: { label: string }) {
      return h("button", null, props.label, value);
    }
    const test = createIslandTest(Counter, { label: "go" }, { document });
    tests.push(test);
    expect(test.html).toContain('data-zfb-island="Counter"');
    const button = test.host.querySelector("button")!;
    const root = test.hydrate();
    expect(root).not.toBeNull();
    expect(test.host.querySelector("button")).toBe(button);
    expect(test.diagnostics).toEqual([]);
    value.value = "second";
    await test.flush();
    expect(button.textContent).toBe("gosecond");
    expect(subscriberCount(value)).toBe(1);
    test.dispose();
    test.dispose();
    expect(root?.disposed).toBe(true);
    expect(test.host.isConnected).toBe(false);
    expect(subscriberCount(value)).toBe(0);
  });

  it("fails hydration after host mutation without mounting and allows explicit identity override", () => {
    function Card() {
      return h("p", null, "server");
    }
    const test = createIslandTest(Card, {}, { document });
    tests.push(test);
    test.host.querySelector("p")!.textContent = "changed";
    const before = test.host.innerHTML;
    expect(test.hydrate()).toBeNull();
    expect(test.host.innerHTML).toBe(before);
    expect(test.diagnostics.at(-1)?.code).toBe("ZR_HYDRATION_MISMATCH");
    test.identity = { component: "Wrong", build: "test" };
    expect(test.mount()).toBeNull();
    expect(test.diagnostics.at(-1)?.code).toBe("ZR_IDENTITY");
    test.dispose();
    expect(test.host.isConnected).toBe(false);
  });

  it("uses an explicit client identity override for negative tests", () => {
    function Card() {
      return h("p", null, "server");
    }
    const test = createIslandTest(
      Card,
      {},
      {
        document,
        identity: { component: "Card", build: "wrong-build" },
      },
    );
    tests.push(test);
    expect(test.host.getAttribute("data-zfb-build")).toBe("test");
    expect(test.hydrate()).toBeNull();
    expect(test.diagnostics.at(-1)?.code).toBe("ZR_IDENTITY");
  });

  it("returns the mounted root handle", () => {
    function Card() {
      return h("p", null, "client");
    }
    const test = createIslandTest(Card, {}, { document });
    tests.push(test);
    const original = test.host.querySelector("p");
    const root = test.mount();
    expect(root).not.toBeNull();
    expect(test.host.querySelector("p")).not.toBe(original);
    expect(test.diagnostics).toEqual([]);
  });

  it("reports malformed serialized props without activating", () => {
    function Card() {
      return h("p", null, "client");
    }
    const test = createIslandTest(Card, {}, { document });
    tests.push(test);
    test.host.setAttribute("data-props", "{");
    expect(test.hydrate()).toBeNull();
    expect(test.diagnostics.at(-1)).toMatchObject({ code: "ZR_PROPS", phase: "preflight" });
    expect(test.host.querySelector("p")?.textContent).toBe("client");
  });

  it("removes its host even if a test moves it before disposal", () => {
    function Card() {
      return h("p", null, "client");
    }
    const test = createIslandTest(Card, {}, { document });
    tests.push(test);
    document.body.append(test.host);
    test.dispose();
    expect(test.host.isConnected).toBe(false);
  });
});

describe("withIslandTestContext", () => {
  it("registers actual component names for a full page and restores other site fields", () => {
    function Card() {
      return h("p", null, "card");
    }
    const scope = globalThis as typeof globalThis & { __zfb?: Record<string, unknown> };
    const previous = scope.__zfb;
    scope.__zfb = { site: "https://example.test", base: "/docs" };
    const installed = scope.__zfb;
    try {
      const html = withIslandTestContext(
        { components: { unrelatedKey: Card }, build: "b2" },
        () => {
          expect(scope.__zfb).toMatchObject({
            site: "https://example.test",
            base: "/docs",
            zudoReactBuild: "b2",
            zudoReactIslands: ["Card"],
          });
          return renderToString(h("main", null, h(Island, { children: h(Card, {}) })));
        },
      );
      expect(html).toContain('data-zfb-island="Card"');
      expect(html).toContain('data-zfb-build="b2"');
      expect(scope.__zfb).toBe(installed);
    } finally {
      if (previous === undefined) delete scope.__zfb;
      else scope.__zfb = previous;
    }
  });

  it("restores metadata after throws and rejects async callbacks", () => {
    function Card() {
      return h("p", null, "card");
    }
    const scope = globalThis as typeof globalThis & { __zfb?: Record<string, unknown> };
    const previous = scope.__zfb;
    delete scope.__zfb;
    try {
      expect(() =>
        withIslandTestContext({ components: [Card] }, () => {
          throw new Error("failed");
        }),
      ).toThrow("failed");
      expect(Object.hasOwn(scope, "__zfb")).toBe(false);
      const callback = vi.fn(() => Promise.resolve("later"));
      expect(() => withIslandTestContext({ components: [Card] }, callback)).toThrow("synchronous");
      expect(Object.hasOwn(scope, "__zfb")).toBe(false);
    } finally {
      if (previous === undefined) delete scope.__zfb;
      else scope.__zfb = previous;
    }
  });
});
