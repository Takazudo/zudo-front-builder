// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { mountIslands as mountOwnedIslands, unmountIslands } from "../runtime.js";
import {
  mountTestIslands as mountIslands,
  mountNewTestIslands as mountNewIslands,
} from "./owned-manifest-fixture.js";

const key = Symbol.for("@takazudo/zfb/zudo-react/root-v1");
const composition = Symbol.for("@takazudo/zfb/zudo-react/composition-v1");
const tracker = Symbol.for("@takazudo/zfb/zudo-react/composition-tracker-v1");

beforeEach(() => {
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("island root lifecycle", () => {
  it("rejects a void mount result without installing a root handle", () => {
    document.body.innerHTML =
      '<div data-zfb-island="Void" data-zfb-transport="json/1" data-zfb-protocol="zudo-react/1" data-zfb-build="b1" data-props="{}"></div>';
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const element = document.body.firstElementChild!;
    mountOwnedIslands({
      Void: {
        identity: { component: "Void", build: "b1" },
        mount: (() => undefined) as never,
      },
    });
    expect(element.hasAttribute("data-zfb-island-mounted")).toBe(false);
    expect((element as unknown as Record<symbol, unknown>)[key]).toBeUndefined();
    expect(error).toHaveBeenCalledWith(
      expect.stringContaining('island "Void" mount failed'),
      expect.objectContaining({ message: expect.stringContaining("ZR_ROOT_HANDLE") }),
    );
  });

  it("fails one owned island closed on metadata or props mismatch and mounts its neighbour", () => {
    document.body.innerHTML =
      '<div data-zfb-island="Bad" data-zfb-transport="json/1" data-zfb-protocol="zudo-react/1" data-zfb-build="old" data-props="{}"></div><div data-zfb-island="Good" data-zfb-transport="json/1" data-zfb-protocol="zudo-react/1" data-zfb-build="new" data-props="{&quot;value&quot;:1}"></div>';
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const bad = vi.fn();
    const good = vi.fn(() => ({ dispose: vi.fn() }));
    mountIslands({
      Bad: { identity: { component: "Bad", build: "new" }, mount: bad },
      Good: { identity: { component: "Good", build: "new" }, mount: good },
    });
    expect(bad).not.toHaveBeenCalled();
    expect(good).toHaveBeenCalledWith({ value: 1 }, expect.any(Element), "hydrate");
    expect(error).toHaveBeenCalledTimes(1);
  });
  it("isolates mount failures and retries only the failed island", () => {
    document.body.innerHTML = '<div data-zfb-island="Bad"></div><div data-zfb-island="Good"></div>';
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const bad = vi.fn().mockImplementationOnce(() => {
      throw new Error("bad mount");
    });
    const good = vi.fn();
    const manifest = { Bad: { mount: bad }, Good: { mount: good } };
    mountIslands(manifest);
    expect(good).toHaveBeenCalledTimes(1);
    expect(error).toHaveBeenCalledTimes(1);
    expect(error.mock.calls[0]?.[0]).toContain('"Bad"');
    mountIslands(manifest);
    expect(bad).toHaveBeenCalledTimes(2);
    expect(good).toHaveBeenCalledTimes(1);
  });

  it("disposes returned handles once and isolates disposal failures", () => {
    document.body.innerHTML = '<div data-zfb-island="Bad"></div><div data-zfb-island="Good"></div>';
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const badDispose = vi.fn(() => {
      throw new Error("bad dispose");
    });
    const goodDispose = vi.fn();
    mountIslands({
      Bad: { mount: () => ({ dispose: badDispose }) },
      Good: { mount: () => ({ dispose: goodDispose }) },
    });
    const el = document.querySelector('[data-zfb-island="Good"]')!;
    expect((el as unknown as Record<symbol, unknown>)[key]).toBeDefined();
    unmountIslands();
    unmountIslands();
    expect(badDispose).toHaveBeenCalledTimes(1);
    expect(goodDispose).toHaveBeenCalledTimes(1);
    expect(error.mock.calls[0]?.[0]).toContain('"Bad"');
  });

  it("replaces a persisted handle in render mode and retains an unchanged handle", () => {
    document.body.innerHTML = '<div data-zfb-island="Panel" data-props="{}"></div>';
    const el = document.body.firstElementChild!;
    const dispose = vi.fn();
    const mount = vi.fn(() => ({ dispose }));
    mountIslands({ Panel: { mount } });
    mountNewIslands();
    expect(mount).toHaveBeenCalledTimes(1);
    expect(dispose).not.toHaveBeenCalled();
    el.setAttribute("data-zfb-island-remount", "");
    mountNewIslands();
    expect(dispose).toHaveBeenCalledTimes(1);
    expect(mount).toHaveBeenCalledTimes(2);
    expect(mount.mock.calls[1]?.[2]).toBe("render");
  });

  it("re-import disposes the prior handle before mounting once in render mode", async () => {
    document.body.innerHTML = '<div data-zfb-island="Fresh"></div>';
    const order: string[] = [];
    mountIslands({ Fresh: { mount: () => ({ dispose: () => order.push("dispose") }) } });
    vi.resetModules();
    const fresh = await import("../runtime.js");
    mountIslands(
      {
        Fresh: {
          mount: (_props, _el, mode) => {
            order.push(mode);
            return { dispose: vi.fn() };
          },
        },
      },
      fresh.mountIslands,
    );
    expect(order).toEqual(["dispose", "render"]);
  });

  it("continues remounting after one root's replacement throws", () => {
    document.body.innerHTML = '<div data-zfb-island="Bad"></div><div data-zfb-island="Good"></div>';
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const bad = vi
      .fn()
      .mockImplementationOnce(() => undefined)
      .mockImplementationOnce(() => {
        throw new Error("replacement failed");
      });
    const good = vi.fn();
    mountIslands({ Bad: { mount: bad }, Good: { mount: good } });
    for (const el of document.querySelectorAll("[data-zfb-island]")) {
      el.setAttribute("data-zfb-island-remount", "");
    }
    mountNewIslands();
    expect(bad).toHaveBeenCalledTimes(2);
    expect(good).toHaveBeenCalledTimes(2);
    expect(error.mock.calls[0]?.[0]).toContain('"Bad"');
  });

  it("schedules contract marked skip-SSR roots by data-when", () => {
    vi.useFakeTimers();
    try {
      document.body.innerHTML =
        '<div data-zfb-island-skip-ssr="Lazy" data-zfb-transport="json/1" data-when="idle"></div>';
      const mount = vi.fn();
      mountIslands({ Lazy: { mount } });
      expect(mount).not.toHaveBeenCalled();
      vi.advanceTimersByTime(0);
      expect(mount).toHaveBeenCalledTimes(1);
      expect(mount.mock.calls[0]?.[2]).toBe("render");
    } finally {
      vi.useRealTimers();
    }
  });

  it("captures composition before a deferred root activates and installs once", () => {
    document.body.innerHTML =
      '<input id="editor"><div data-zfb-island="Deferred" data-when="idle"></div>';
    const first = (document as unknown as Record<symbol, unknown>)[tracker];
    const mount = vi.fn();
    mountIslands({ Deferred: { mount } });
    const installed = (document as unknown as Record<symbol, unknown>)[tracker];
    expect(installed).toBeTypeOf("function");
    const input = document.querySelector("input")!;
    input.dispatchEvent(new Event("compositionstart", { bubbles: true }));
    expect((input as unknown as Record<symbol, unknown>)[composition]).toBe("active");
    input.dispatchEvent(new Event("compositionend", { bubbles: true }));
    expect((input as unknown as Record<symbol, unknown>)[composition]).toBe("idle");
    mountIslands({ Deferred: { mount } });
    expect((document as unknown as Record<symbol, unknown>)[tracker]).toBe(installed);
    expect(first === undefined || first === installed).toBe(true);
  });
});
