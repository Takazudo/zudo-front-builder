import { beforeEach, describe, expect, it } from "vitest";
import { For, Show, computed, flush, getScope, h, signal } from "../../zudo-react/index.js";
import { subscriberCount } from "../../zudo-react/reactive.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";
import { hydrate } from "../../zudo-react/client.js";
import type { Diagnostic } from "../../zudo-react/index.js";

const identity = { component: "Demo", build: "b1" };
let diagnostics: Diagnostic[];
const options = () => ({ identity, report: (value: Diagnostic) => diagnostics.push(value) });
function server(node: ReturnType<typeof h>): Element {
  const host = document.createElement("div");
  document.body.append(host);
  host.innerHTML = renderToString(islandRoot(node, { identity }));
  return host.firstElementChild!;
}
beforeEach(() => {
  document.body.replaceChildren();
  diagnostics = [];
});

describe("structural regions", () => {
  it("adopts both Show states and disposes a toggled branch", async () => {
    const when = signal(true);
    const inner = signal("A");
    let started = 0;
    let cleaned = 0;
    function Demo() {
      return Show({
        when,
        children: () => {
          getScope().onActivate(() => {
            started++;
            return () => {
              cleaned++;
            };
          });
          return h("b", null, inner);
        },
      });
    }
    const root = server(h(Demo, null));
    const old = root.querySelector("b");
    const handle = hydrate(h(Demo, null), root, options())!;
    expect(handle).not.toBeNull();
    expect(root.querySelector("b")).toBe(old);
    expect(started).toBe(1);
    expect(subscriberCount(inner)).toBe(1);
    when.value = false;
    await flush();
    expect(root.querySelector("b")).toBeNull();
    expect(cleaned).toBe(1);
    expect(subscriberCount(inner)).toBe(0);
    when.value = true;
    await flush();
    expect(root.querySelector("b")).not.toBe(old);
    expect(started).toBe(2);
    handle.dispose();
    expect(cleaned).toBe(2);
    when.value = false;
    await flush();
    expect(root.querySelector("b")).not.toBeNull();
    expect(diagnostics).toEqual([]);
  });
  it("adopts a false branch and empty list without inserting nodes", () => {
    const when = signal(false);
    const each = signal<string[]>([]);
    function Demo() {
      return h(
        "div",
        null,
        Show({ when, children: () => h("b", null, "yes") }),
        For({ each, by: (x) => x, children: (x) => h("i", null, x) }),
      );
    }
    const root = server(h(Demo, null));
    const before = [...root.querySelector("div")!.childNodes];
    expect(hydrate(h(Demo, null), root, options())).not.toBeNull();
    expect([...root.querySelector("div")!.childNodes]).toEqual(before);
  });
  it("moves retained keys with nodes, scopes, focus and local state", async () => {
    const each = signal([{ id: "a" }, { id: "b" }, { id: "c" }]);
    const setups: string[] = [];
    const cleanups: string[] = [];
    function Demo() {
      return h(
        "div",
        null,
        For({
          each,
          by: (x) => x.id,
          children: (item, index) => {
            const id = item.value.id;
            setups.push(id);
            getScope().onCleanup(() => cleanups.push(id));
            const local = signal(`${id}!`);
            return h("button", { type: "button", children: [local, index] });
          },
        }),
      );
    }
    const root = server(h(Demo, null));
    setups.length = 0;
    cleanups.length = 0;
    const handle = hydrate(h(Demo, null), root, options())!;
    const nodes = [...root.querySelectorAll("button")];
    nodes[1]!.focus();
    each.value = [{ id: "c" }, { id: "b" }, { id: "a" }];
    await flush();
    expect(diagnostics).toEqual([]);
    expect([...root.querySelectorAll("button")]).toEqual([nodes[2], nodes[1], nodes[0]]);
    expect(document.activeElement).toBe(nodes[1]);
    expect(setups).toEqual(["a", "b", "c"]);
    expect(cleanups).toEqual([]);
    each.value = [{ id: "c" }, { id: "a" }];
    await flush();
    expect(cleanups).toEqual(["b"]);
    each.value = [{ id: "b" }, { id: "c" }, { id: "a" }];
    await flush();
    expect(root.querySelector("button")).not.toBe(nodes[1]);
    expect(setups).toEqual(["a", "b", "c", "b"]);
    handle.dispose();
    expect(cleanups.sort()).toEqual(["a", "b", "b", "c"]);
    expect(diagnostics).toEqual([]);
  });
  it("rejects duplicate keys without a partial update", async () => {
    const each = signal([{ id: "a" }, { id: "b" }]);
    function Demo() {
      return h(
        "div",
        null,
        For({ each, by: (x) => x.id, children: (x) => h("b", null, x.value.id) }),
      );
    }
    const root = server(h(Demo, null));
    const handle = hydrate(h(Demo, null), root, options());
    expect(handle).not.toBeNull();
    const before = root.innerHTML;
    each.value = [{ id: "a" }, { id: "a" }];
    await flush();
    expect(root.innerHTML).toBe(before);
    expect(diagnostics[0]?.code).toBe("ZR_DUPLICATE_KEY");
  });
  it("keeps a moved input's model binding and removes it with the item", async () => {
    const each = signal(["a", "b"]);
    const models = new Map([
      ["a", signal("A")],
      ["b", signal("B")],
    ]);
    function Demo() {
      return h(
        "div",
        null,
        For({
          each,
          by: (x) => x,
          children: (item) => h("input", { type: "text", modelValue: models.get(item.value)! }),
        }),
      );
    }
    const root = server(h(Demo, null));
    const handle = hydrate(h(Demo, null), root, options())!;
    const input = root.querySelectorAll("input")[1]!;
    input.value = "edited";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    expect(models.get("b")!.value).toBe("edited");
    each.value = ["b", "a"];
    await flush();
    expect(root.querySelector("input")).toBe(input);
    expect(input.value).toBe("edited");
    expect(subscriberCount(models.get("b")!)).toBe(1);
    each.value = ["a"];
    await flush();
    expect(subscriberCount(models.get("b")!)).toBe(0);
    handle.dispose();
  });
  it("updates retained item and index views without rerunning setup", async () => {
    const each = signal([
      { id: "a", label: "A" },
      { id: "b", label: "B" },
    ]);
    let setups = 0;
    function Demo() {
      return For({
        each,
        by: (item) => item.id,
        children: (item, index) => {
          setups++;
          return h(
            "b",
            null,
            index,
            ":",
            computed(() => item.value.label),
          );
        },
      });
    }
    const root = server(h(Demo, null));
    setups = 0;
    const handle = hydrate(h(Demo, null), root, options())!;
    const nodes = [...root.querySelectorAll("b")];
    each.value = [
      { id: "b", label: "Bee" },
      { id: "a", label: "Aye" },
    ];
    await flush();
    expect([...root.querySelectorAll("b")]).toEqual([nodes[1], nodes[0]]);
    expect([...root.querySelectorAll("b")].map((node) => node.textContent)).toEqual([
      "0:Bee",
      "1:Aye",
    ]);
    expect(setups).toBe(2);
    handle.dispose();
  });
  it("leaves the list live when a new item factory throws", async () => {
    const each = signal(["a"]);
    function Demo() {
      return For({
        each,
        by: (item) => item,
        children: (item) => {
          if (item.value === "bad") throw new Error("factory failed");
          return h("b", null, item.value);
        },
      });
    }
    const root = server(h(Demo, null));
    const handle = hydrate(h(Demo, null), root, options())!;
    const before = root.innerHTML;
    each.value = ["a", "bad"];
    await flush();
    expect(root.innerHTML).toBe(before);
    expect(root.querySelector("b")?.textContent).toBe("a");
    expect(diagnostics.some((item) => item.actual.includes("factory failed"))).toBe(true);
    handle.dispose();
  });
  it("fails closed when server and client list lengths differ", () => {
    const each = signal(["a", "b"]);
    function Demo() {
      return For({ each, by: (x) => x, children: (x) => h("b", null, x.value) });
    }
    const root = server(h(Demo, null));
    const before = root.innerHTML;
    each.value = ["a"];
    expect(hydrate(h(Demo, null), root, options())).toBeNull();
    expect(root.innerHTML).toBe(before);
    expect(diagnostics[0]?.code).toBe("ZR_HYDRATION_MISMATCH");
  });
  it("fails hydration with a named duplicate-key diagnostic", () => {
    const each = signal(["a", "b"]);
    function Demo() {
      return For({ each, by: (item) => item, children: (item) => h("b", null, item.value) });
    }
    const root = server(h(Demo, null));
    const before = root.innerHTML;
    each.value = ["a", "a"];
    expect(hydrate(h(Demo, null), root, options())).toBeNull();
    expect(root.innerHTML).toBe(before);
    expect(diagnostics[0]?.code).toBe("ZR_DUPLICATE_KEY");
  });
});
