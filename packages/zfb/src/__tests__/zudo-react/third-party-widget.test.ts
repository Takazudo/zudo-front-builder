import { beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { flush, getScope, h, signal, type Diagnostic, type Ref } from "../../zudo-react/index.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";
import { hydrate } from "../../zudo-react/client.js";

type WidgetInstance = { destroy(): void };
type WidgetModule = { mount(host: Element): WidgetInstance };
type WidgetLoader = () => Promise<WidgetModule>;

// Same component as the EN/JA documentation; the loader is injected to control import timing.
let loadWidget: WidgetLoader; // Injected only by the test harness.

function ThirdPartyWidget() {
  const host: Ref<Element> = { current: null };
  const status = signal("Loading widget…");
  const scope = getScope();
  const abortSignal = scope.abortSignal;

  scope.onActivate(() => {
    let instance: WidgetInstance | undefined;
    void loadWidget()
      .then((widget) => {
        if (abortSignal.aborted) return;
        const element = host.current;
        if (!element) return;
        try {
          instance = widget.mount(element);
          status.value = "";
        } catch {
          element.replaceChildren();
          status.value = "Widget unavailable.";
        }
      })
      .catch(() => {
        if (!abortSignal.aborted) status.value = "Widget unavailable.";
      });

    return () => {
      const mounted = instance;
      instance = undefined;
      try {
        mounted?.destroy();
      } finally {
        host.current?.replaceChildren();
      }
    };
  });

  return h("section", null, h("div", { ref: host }), h("p", { role: "status", children: status }));
}

const identity = { component: "ThirdPartyWidget", build: "b1" };
let diagnostics: Diagnostic[];
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((ok, fail) => {
    resolve = ok;
    reject = fail;
  });
  return { promise, resolve, reject };
}
function server() {
  const outer = document.createElement("div");
  document.body.append(outer);
  outer.innerHTML = renderToString(islandRoot(h(ThirdPartyWidget, null), { identity }));
  return outer.firstElementChild!;
}
function hydrateWidget(loader: WidgetLoader) {
  loadWidget = loader;
  const root = server();
  const host = root.querySelector("section > div")!;
  expect(host.childNodes).toHaveLength(0);
  const handle = hydrate(h(ThirdPartyWidget, null), root, {
    identity,
    report: (value) => diagnostics.push(value),
  })!;
  expect(handle).not.toBeNull();
  return { root, host, handle };
}
function fakeWidget(options: { throwBeforeReturn?: boolean; allocateBeforeThrow?: boolean } = {}) {
  let live = 0;
  let mounts = 0;
  let destroys = 0;
  let rollbacks = 0;
  const listener = vi.fn();
  const mount = vi.fn((host: Element): WidgetInstance => {
    mounts++;
    if (options.throwBeforeReturn && !options.allocateBeforeThrow) throw new Error("mount failed");
    const node = document.createElement("b");
    host.append(node);
    window.addEventListener("widget-event", listener);
    const timer = window.setInterval(() => {}, 1000);
    live++;
    let cleaned = false;
    const release = () => {
      if (cleaned) return;
      cleaned = true;
      live--;
      window.removeEventListener("widget-event", listener);
      window.clearInterval(timer);
      node.remove();
    };
    if (options.throwBeforeReturn) {
      release(); // Adapter rolls back allocations before rethrowing.
      rollbacks++;
      throw new Error("mount failed");
    }
    return {
      destroy: () => {
        destroys++;
        release();
      },
    };
  });
  return {
    mount,
    listener,
    get live() {
      return live;
    },
    get mounts() {
      return mounts;
    },
    get destroys() {
      return destroys;
    },
    get rollbacks() {
      return rollbacks;
    },
  };
}
async function settle() {
  await Promise.resolve();
  await Promise.resolve();
  await flush();
}
beforeEach(() => {
  document.body.replaceChildren();
  diagnostics = [];
});

describe("third-party widget ownership", () => {
  it("mounts after import and destroys once on disposal", async () => {
    const importResult = deferred<WidgetModule>();
    const widget = fakeWidget();
    const clearTimer = vi.spyOn(window, "clearInterval");
    const { root, host, handle } = hydrateWidget(() => importResult.promise);
    expect(widget.mount).not.toHaveBeenCalled();
    importResult.resolve(widget);
    await settle();
    expect(widget.mount).toHaveBeenCalledTimes(1);
    expect(widget.mount).toHaveBeenCalledWith(host);
    expect(widget.live).toBe(1);
    expect(root.querySelector('[role="status"]')?.textContent).toBe("");
    handle.dispose();
    handle.dispose();
    expect(widget.destroys).toBe(1);
    expect(clearTimer).toHaveBeenCalledTimes(1);
    clearTimer.mockRestore();
    expect(widget.live).toBe(0);
    expect(host.childNodes).toHaveLength(0);
    window.dispatchEvent(new Event("widget-event"));
    expect(widget.listener).not.toHaveBeenCalled();
    expect(diagnostics).toEqual([]);
  });

  it("never mounts if disposed while the import is pending", async () => {
    const importResult = deferred<WidgetModule>();
    const widget = fakeWidget();
    const { host, handle } = hydrateWidget(() => importResult.promise);
    handle.dispose();
    importResult.resolve(widget);
    await settle();
    expect(widget.mount).not.toHaveBeenCalled();
    expect(widget.live).toBe(0);
    expect(host.childNodes).toHaveLength(0);
    expect(diagnostics).toEqual([]);
  });

  it("shows fallback for a rejected import without diagnostics", async () => {
    const importResult = deferred<WidgetModule>();
    const { root, handle } = hydrateWidget(() => importResult.promise);
    importResult.reject(new Error("network"));
    await settle();
    expect(root.querySelector('[role="status"]')?.textContent).toBe("Widget unavailable.");
    expect(diagnostics).toEqual([]);
    handle.dispose();
  });

  it.each([false, true])(
    "shows fallback after a throwing mount (partial allocation: %s)",
    async (partial) => {
      const widget = fakeWidget({ throwBeforeReturn: true, allocateBeforeThrow: partial });
      const clearTimer = vi.spyOn(window, "clearInterval");
      const { root, host, handle } = hydrateWidget(() => Promise.resolve(widget));
      await settle();
      expect(widget.mounts).toBe(1);
      expect(widget.live).toBe(0);
      expect(host.childNodes).toHaveLength(0);
      expect(root.querySelector('[role="status"]')?.textContent).toBe("Widget unavailable.");
      handle.dispose();
      expect(widget.destroys).toBe(0); // The island never received an instance.
      expect(widget.rollbacks).toBe(partial ? 1 : 0);
      expect(clearTimer).toHaveBeenCalledTimes(partial ? 1 : 0);
      clearTimer.mockRestore();
      window.dispatchEvent(new Event("widget-event"));
      expect(widget.listener).not.toHaveBeenCalled();
      expect(diagnostics).toEqual([]);
    },
  );

  it("balances three revisits with at most one live instance", async () => {
    const widget = fakeWidget();
    for (let visit = 1; visit <= 3; visit++) {
      const { host, handle } = hydrateWidget(() => Promise.resolve(widget));
      await settle();
      expect(widget.live).toBe(1);
      expect(widget.mounts).toBe(visit);
      handle.dispose();
      expect(widget.live).toBe(0);
      expect(widget.destroys).toBe(visit);
      expect(host.childNodes).toHaveLength(0);
    }
    expect(diagnostics).toEqual([]);
  });
});
