import { afterEach, beforeEach, expect, it, vi } from "vite-plus/test";
import { For, Show, flush, h, signal, type Diagnostic } from "../../zudo-react/index.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";
import { hydrate, mount } from "../../zudo-react/client.js";
import { report } from "../../zudo-react/root.js";
const identity = { component: "Demo", build: "b1" };
function containerFor(node: ReturnType<typeof h>): Element {
  const host = document.createElement("div");
  document.body.append(host);
  host.innerHTML = renderToString(islandRoot(node, { identity }));
  return host.firstElementChild!;
}
beforeEach(() => document.body.replaceChildren());
afterEach(() => vi.restoreAllMocks());
for (const attach of [mount, hydrate]) {
  it(`${attach.name} removes a Show branch inserted after an initially false root`, async () => {
    const visible = signal(false);
    function Demo() {
      return Show({ when: visible, children: () => h("p", null, "later") });
    }
    const container = containerFor(h(Demo, {}));
    const handle = attach(h(Demo, {}), container, { identity })!;
    expect(container.querySelector("p")).toBeNull();
    visible.value = true;
    await flush();
    expect(container.querySelector("p")?.textContent).toBe("later");
    handle.unmount();
    expect(container.childNodes).toHaveLength(0);
  });

  it(`${attach.name} removes an initially shown branch after it turns false`, async () => {
    const visible = signal(true);
    function Demo() {
      return Show({ when: visible, children: () => h("p", null, "shown") });
    }
    const container = containerFor(h(Demo, {}));
    const handle = attach(h(Demo, {}), container, { identity })!;
    expect(container.querySelector("p")?.textContent).toBe("shown");
    visible.value = false;
    await flush();
    expect(container.querySelector("p")).toBeNull();
    handle.unmount();
    expect(container.childNodes).toHaveLength(0);
  });

  it(`${attach.name} removes the current Show fallback and preserves foreign siblings`, async () => {
    const visible = signal(true);
    function Demo() {
      return Show({
        when: visible,
        children: () => h("p", null, "shown"),
        fallback: () => h("aside", null, "fallback"),
      });
    }
    const container = containerFor(h(Demo, {}));
    const handle = attach(h(Demo, {}), container, { identity })!;
    const comments = [...container.childNodes].filter(
      (node) => node.nodeType === Node.COMMENT_NODE,
    );
    const start = comments.find((node) => node.textContent?.includes(":s:"))!;
    const end = comments.find(
      (node) =>
        node.textContent ===
        `/${start.textContent?.split(":")[0]}:${start.textContent?.split(":")[1]}:${start.textContent?.split(":")[2]}`,
    );
    // Place foreign nodes on both sides of the dynamic range, inside the root component markers.
    const before = document.createElement("i");
    const after = document.createElement("i");
    before.textContent = "before";
    after.textContent = "after";
    container.insertBefore(before, start);
    container.insertBefore(after, end!.nextSibling);
    visible.value = false;
    await flush();
    expect(container.querySelector("p")).toBeNull();
    expect(container.querySelector("aside")?.textContent).toBe("fallback");
    handle.dispose();
    expect(container.querySelector("aside")).not.toBeNull();
    handle.unmount();
    expect([...container.childNodes]).toEqual([before, after]);
    handle.unmount();
    expect([...container.childNodes]).toEqual([before, after]);
  });

  it(`${attach.name} removes newly inserted and moved root For items`, async () => {
    const items = signal(["a", "b"]);
    function Demo() {
      return For({ each: items, by: (item) => item, children: (item) => h("p", null, item) });
    }
    const container = containerFor(h(Demo, {}));
    const handle = attach(h(Demo, {}), container, { identity })!;
    const original = [...container.querySelectorAll("p")];
    items.value = ["b", "c", "a"];
    await flush();
    expect([...container.querySelectorAll("p")].map((node) => node.textContent)).toEqual([
      "b",
      "c",
      "a",
    ]);
    expect(container.querySelectorAll("p")[0]).toBe(original[1]);
    items.value = ["c", "a"];
    await flush();
    expect(original[1].isConnected).toBe(false);
    handle.unmount();
    expect(container.childNodes).toHaveLength(0);
  });

  it(`${attach.name} removes nested dynamic ranges and an empty reactive text slot`, async () => {
    const visible = signal(false);
    const items = signal(["a"]);
    const text = signal("");
    function Demo() {
      return [
        Show({
          when: visible,
          children: () =>
            For({ each: items, by: (item) => item, children: (item) => h("b", null, item) }),
        }),
        text,
      ];
    }
    const container = containerFor(h(Demo, {}));
    const handle = attach(h(Demo, {}), container, { identity })!;
    visible.value = true;
    await flush();
    items.value = ["a", "b"];
    text.value = "tail";
    await flush();
    expect([...container.querySelectorAll("b")].map((node) => node.textContent)).toEqual([
      "a",
      "b",
    ]);
    expect(container.textContent).toBe("abtail");
    handle.unmount();
    expect(container.childNodes).toHaveLength(0);
  });

  it(`${attach.name} removes an intrinsic wrapper after a nested Show update`, async () => {
    const visible = signal(false);
    function Demo() {
      return h("section", null, Show({ when: visible, children: () => h("p", null, "yes") }));
    }
    const container = containerFor(h(Demo, {}));
    const handle = attach(h(Demo, {}), container, { identity })!;
    visible.value = true;
    await flush();
    expect(container.querySelector("section p")?.textContent).toBe("yes");
    handle.unmount();
    expect(container.childNodes).toHaveLength(0);
  });
}
it("stores a shared-symbol handle and stale disposal cannot remove a replacement", () => {
  function Demo() {
    return h("p", { children: "value" });
  }
  const host = document.createElement("div");
  document.body.append(host);
  host.innerHTML = renderToString(islandRoot(h(Demo, {}), { identity }));
  const container = host.firstElementChild!;
  const first = hydrate(h(Demo, {}), container, { identity })!;
  const key = Symbol.for("@takazudo/zfb/zudo-react/root-v1");
  expect((container as unknown as Record<symbol, unknown>)[key]).toBe(first);
  first.dispose();
  const second = mount(h(Demo, {}), container, { identity })!;
  const replacement = container.querySelector("p")!;
  first.unmount();
  expect(container.querySelector("p")).toBe(replacement);
  second.unmount();
  expect(container.childNodes).toHaveLength(0);
});

it("logs a searchable hydration mismatch line followed by one structured diagnostic", () => {
  function Demo() {
    return h("p", null, "expected secret text");
  }
  const host = document.createElement("div");
  document.body.append(host);
  host.innerHTML = renderToString(islandRoot(h(Demo, {}), { identity }));
  const container = host.firstElementChild!;
  container.querySelector("p")!.textContent = "actual secret text";
  const logged = vi.spyOn(console, "error").mockImplementation(() => {});

  expect(hydrate(h(Demo, {}), container, { identity })).toBeNull();
  expect(logged).toHaveBeenCalledTimes(1);
  const [line, detail] = logged.mock.calls[0]!;
  expect(line).toMatch(
    /^\[zudo-react\] ZR_HYDRATION_MISMATCH preflight Demo \/html:nth-child\(1\)\/body:nth-child\(2\)\/.+: hydration mismatch$/,
  );
  expect(line).not.toContain("secret");
  expect(detail).toMatchObject({
    code: "ZR_HYDRATION_MISMATCH",
    phase: "preflight",
    component: "Demo",
    protocol: "zudo-react/1",
    build: "b1",
  });
});

it("logs reactive update errors once without exposing rawHtml on the line", async () => {
  const html = signal<unknown>("<em>safe</em>");
  function Demo() {
    return h("div", { rawHtml: html });
  }
  const host = document.createElement("div");
  document.body.append(host);
  host.innerHTML = renderToString(islandRoot(h(Demo, {}), { identity }));
  const container = host.firstElementChild!;
  const logged = vi.spyOn(console, "error").mockImplementation(() => {});
  const handle = hydrate(h(Demo, {}), container, { identity });
  expect(handle).not.toBeNull();

  html.value = { secret: "private rawHtml" };
  await flush();
  expect(logged).toHaveBeenCalledTimes(1);
  const [line, detail] = logged.mock.calls[0]!;
  expect(line).toMatch(
    /^\[zudo-react\] ZR_SUBSCRIBER_ERROR update Demo \/html:nth-child\(1\)\/.+: reactive update failed$/,
  );
  expect(line).not.toContain("private rawHtml");
  expect(detail).toMatchObject({ code: "ZR_SUBSCRIBER_ERROR", phase: "update" });
  handle?.dispose();
});

it("keeps a custom reporter exclusive and isolates a throwing reporter", () => {
  const value: Diagnostic = {
    code: "ZR_HYDRATION_MISMATCH",
    phase: "preflight",
    component: "Demo",
    componentStack: ["Demo"],
    path: "/Users/example/private.ts",
    expected: "<secret>",
    actual: "<private rawHtml>",
    protocol: "zudo-react/1",
    build: "b1",
  };
  const logged = vi.spyOn(console, "error").mockImplementation(() => {});
  const custom = vi.fn();
  report({ identity, report: custom }, value);
  expect(custom).toHaveBeenCalledTimes(1);
  expect(custom).toHaveBeenCalledWith(value);
  expect(logged).not.toHaveBeenCalled();

  report({ identity }, value);
  expect(logged).toHaveBeenCalledTimes(1);
  expect(logged.mock.calls[0]![0]).toBe(
    "[zudo-react] ZR_HYDRATION_MISMATCH preflight Demo [path]: hydration mismatch",
  );
  expect(logged.mock.calls[0]![1]).toBe(value);

  logged.mockClear();
  const failure = new Error("reporter failed");
  expect(() =>
    report(
      {
        identity,
        report: () => {
          throw failure;
        },
      },
      value,
    ),
  ).not.toThrow();
  expect(logged).toHaveBeenCalledTimes(1);
  expect(logged).toHaveBeenCalledWith(failure);
});
