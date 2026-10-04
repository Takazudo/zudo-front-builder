import { afterEach, beforeEach, expect, it, vi } from "vite-plus/test";
import { flush, h, signal, type Diagnostic } from "../../zudo-react/index.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";
import { hydrate, mount } from "../../zudo-react/client.js";
import { report } from "../../zudo-react/root.js";
const identity = { component: "Demo", build: "b1" };
beforeEach(() => document.body.replaceChildren());
afterEach(() => vi.restoreAllMocks());
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
