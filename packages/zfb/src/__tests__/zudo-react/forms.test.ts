import { beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { h, signal, computed, flush } from "../../zudo-react/index.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";
import { hydrate, mount } from "../../zudo-react/client.js";
import type { Diagnostic } from "../../zudo-react/index.js";

const identity = { component: "Demo", build: "b1" };
let diagnostics: Diagnostic[];
function host(node: ReturnType<typeof h>) {
  const outer = document.createElement("div");
  document.body.append(outer);
  outer.innerHTML = renderToString(islandRoot(node, { identity }));
  return outer.firstElementChild!;
}
const options = () => ({ identity, report: (item: Diagnostic) => diagnostics.push(item) });
beforeEach(() => {
  document.body.replaceChildren();
  diagnostics = [];
});

describe("form hydration", () => {
  it.each(["input", "textarea"] as const)(
    "binds display-only %s on the mapped node and restores its initial value on reset",
    async (tag) => {
      const source = signal("server");
      const value = computed(() => source.value);
      function Demo() {
        const control =
          tag === "input"
            ? h("input", { readonly: true, value })
            : h("textarea", { disabled: true, value });
        return h("form", { children: control });
      }
      const root = host(h(Demo, {}));
      const node = root.querySelector(tag) as HTMLInputElement | HTMLTextAreaElement;
      source.value = "initial";
      const listenerSpy = vi.spyOn(node, "addEventListener");
      const handle = hydrate(h(Demo, {}), root, options());
      expect(handle).not.toBeNull();
      expect(root.querySelector(tag)).toBe(node);
      expect(node.value).toBe("initial");
      expect(node.getAttribute("value")).toBe(tag === "input" ? "server" : null);
      source.value = "updated";
      await flush();
      expect(node.value).toBe("updated");
      expect(node.getAttribute("value")).toBe(tag === "input" ? "server" : null);
      for (const event of ["input", "change", "compositionstart", "compositionend"])
        expect(listenerSpy.mock.calls.map(([name]) => name)).not.toContain(event);
      (root.querySelector("form") as HTMLFormElement).reset();
      expect(node.value).toBe("server");
      handle!.dispose();
      source.value = "after disposal";
      await flush();
      expect(node.value).toBe("server");
      expect(diagnostics).toEqual([]);
    },
  );
  it("sets the initial value on a client-only mount and reports later invalid values", async () => {
    const value = signal<string | number>("first");
    function Demo() {
      return h("form", { children: h("input", { readonly: true, value }) });
    }
    const root = host(h(Demo, {}));
    root.setAttribute("data-zfb-island-skip-ssr", "Demo");
    root.replaceChildren();
    const handle = mount(h(Demo, {}), root, options());
    expect(handle).not.toBeNull();
    const node = root.querySelector("input")!;
    expect(node.value).toBe("first");
    expect(node.getAttribute("value")).toBe("first");
    value.value = 1;
    await flush();
    expect(diagnostics.at(-1)?.code).toBe("ZR_SUBSCRIBER_ERROR");
    expect(diagnostics.at(-1)?.actual).toContain("ZR_MODEL_VALUE");
    expect(node.value).toBe("first");
    handle!.dispose();
  });
  it("restores the initial value on a client-only mount after an update", async () => {
    const value = signal("first");
    function Demo() {
      return h("form", { children: h("textarea", { readonly: true, value }) });
    }
    const root = host(h(Demo, {}));
    root.setAttribute("data-zfb-island-skip-ssr", "Demo");
    root.replaceChildren();
    const handle = mount(h(Demo, {}), root, options());
    expect(handle).not.toBeNull();
    const node = root.querySelector("textarea")!;
    expect(node.getAttribute("value")).toBe("first");
    value.value = "updated";
    await flush();
    expect(node.value).toBe("updated");
    expect(node.getAttribute("value")).toBe("first");
    (root.querySelector("form") as HTMLFormElement).reset();
    expect(node.value).toBe("first");
    handle!.dispose();
  });
  it("rejects non-string display-only values during client preflight", () => {
    for (const node of [
      h("input", { readonly: true, value: signal(1) }),
      h("textarea", { disabled: true, value: signal(1) }),
      h("input", { type: "number", readonly: true, value: signal("1") }),
    ]) {
      let child = h("div");
      function Demo() {
        return child;
      }
      const root = host(h(Demo, {}));
      root.setAttribute("data-zfb-island-skip-ssr", "Demo");
      child = node;
      expect(mount(h(Demo, {}), root, options())).toBeNull();
    }
    expect(diagnostics.map((item) => item.code)).toEqual([
      "ZR_MODEL_VALUE",
      "ZR_MODEL_VALUE",
      "ZR_MODEL_UNSUPPORTED",
    ]);
  });
  it.each([
    ["text", () => h("input", { defaultValue: "a" }), "value", "a"],
    ["checkbox", () => h("input", { type: "checkbox", defaultChecked: true }), "checked", true],
    ["radio", () => h("input", { type: "radio", defaultChecked: true }), "checked", true],
    ["textarea", () => h("textarea", { defaultValue: "a" }), "value", "a"],
    [
      "select",
      () =>
        h("select", {
          defaultValue: "a",
          children: h("option", { value: "a", children: "A" }),
        }),
      "value",
      "a",
    ],
  ] as const)(
    "keeps %s defaults through mount and hydration",
    (_kind, control, property, expected) => {
      function Demo() {
        return control();
      }
      const mounted = host(h(Demo, {}));
      mounted.setAttribute("data-zfb-island-skip-ssr", "Demo");
      mounted.replaceChildren();
      expect(mount(h(Demo, {}), mounted, options())).not.toBeNull();
      expect((mounted.firstElementChild as unknown as Record<string, unknown>)[property]).toBe(
        expected,
      );

      const hydrated = host(h(Demo, {}));
      expect(hydrate(h(Demo, {}), hydrated, options())).not.toBeNull();
      expect((hydrated.firstElementChild as unknown as Record<string, unknown>)[property]).toBe(
        expected,
      );
      expect(diagnostics).toEqual([]);
    },
  );
  it.each(["number", "range", "color", "date", "time"])(
    "rejects %s defaultValue during both mount and hydration",
    (type) => {
      let invalid = false;
      function Demo() {
        return h("input", invalid ? { type, defaultValue: "2" } : { type, value: "2" });
      }
      const mounted = host(h(Demo, {}));
      mounted.setAttribute("data-zfb-island-skip-ssr", "Demo");
      mounted.replaceChildren();
      invalid = true;
      expect(mount(h(Demo, {}), mounted, options())).toBeNull();
      expect(diagnostics.at(-1)?.code).toBe("ZR_MODEL_UNSUPPORTED");

      invalid = false;
      const hydrated = host(h(Demo, {}));
      invalid = true;
      diagnostics = [];
      expect(hydrate(h(Demo, {}), hydrated, options())).toBeNull();
      expect(diagnostics.at(-1)?.code).toBe("ZR_MODEL_UNSUPPORTED");
    },
  );
  it.each(["text", "textarea", "checkbox", "select", "radio"] as const)(
    "adopts live %s state before derived binding",
    async (kind) => {
      const model =
        kind === "checkbox"
          ? signal(false)
          : kind === "radio"
            ? signal<string | null>("a")
            : signal("a");
      const derived = computed(() => String(model.value));
      function Demo() {
        const control =
          kind === "text"
            ? h("input", { modelValue: model })
            : kind === "textarea"
              ? h("textarea", { modelValue: model })
              : kind === "checkbox"
                ? h("input", { type: "checkbox", modelChecked: model })
                : kind === "select"
                  ? h("select", {
                      modelValue: model,
                      children: [
                        h("option", { value: "a", children: "A" }),
                        h("option", { value: "b", children: "B" }),
                      ],
                    })
                  : [
                      h("input", { type: "radio", name: "r", value: "a", modelValue: model }),
                      h("input", { type: "radio", name: "r", value: "b", modelValue: model }),
                    ];
        return h("form", { children: [control, h("output", { children: derived })] });
      }
      const root = host(h(Demo, {}));
      const input = root.querySelector(
        kind === "textarea" ? "textarea" : kind === "select" ? "select" : "input",
      ) as HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement;
      if (kind === "checkbox") (input as HTMLInputElement).checked = true;
      else if (kind === "radio")
        (root.querySelectorAll("input")[1] as HTMLInputElement).checked = true;
      else input.value = "b";
      const handle = hydrate(h(Demo, {}), root, options());
      expect(handle).not.toBeNull();
      expect(model.value).toBe(kind === "checkbox" ? true : "b");
      await flush();
      expect(root.querySelector("output")!.textContent).toBe(String(model.value));
      expect(diagnostics).toEqual([]);
      if (kind === "checkbox") model.value = false as never;
      else model.value = "a" as never;
      await flush();
      expect(
        kind === "checkbox"
          ? (input as HTMLInputElement).checked
          : kind === "radio"
            ? (root.querySelector("input") as HTMLInputElement).checked
            : input.value,
      ).toBe(kind === "checkbox" ? false : kind === "radio" ? true : "a");
      handle!.dispose();
    },
  );
  it("skips equal value assignment, disposes both directions", async () => {
    const model = signal("a");
    function Demo() {
      return h("input", { modelValue: model });
    }
    const root = host(h(Demo, {}));
    const input = root.querySelector("input")!;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    const spy = vi.spyOn(HTMLInputElement.prototype, "value", "set");
    const handle = hydrate(h(Demo, {}), root, options())!;
    model.value = "b";
    input.value = "b";
    spy.mockClear();
    await flush();
    expect(spy).not.toHaveBeenCalled();
    handle.dispose();
    input.value = "c";
    input.dispatchEvent(new Event("input"));
    expect(model.value).toBe("b");
    model.value = "d";
    await flush();
    expect(input.value).toBe("c");
    spy.mockRestore();
    void setter;
  });
  it("resets after default action and respects cancellation", async () => {
    const model = signal("a");
    function Demo() {
      return h("form", { children: h("input", { modelValue: model }) });
    }
    const root = host(h(Demo, {}));
    const form = root.querySelector("form")!;
    const input = form.querySelector("input")!;
    hydrate(h(Demo, {}), root, options());
    model.value = "b";
    await flush();
    form.reset();
    await Promise.resolve();
    await flush();
    expect(model.value).toBe("a");
    expect(input.value).toBe("a");
    model.value = "c";
    await flush();
    form.addEventListener("reset", (event) => event.preventDefault(), { once: true });
    form.dispatchEvent(new Event("reset", { cancelable: true }));
    await Promise.resolve();
    expect(model.value).toBe("c");
    expect(input.value).toBe("c");
  });
  it("synthetic composition buffers writes and reads final edit", async () => {
    const model = signal("a");
    function Demo() {
      return h("input", { modelValue: model });
    }
    const root = host(h(Demo, {}));
    const input = root.querySelector("input")!;
    hydrate(h(Demo, {}), root, options());
    input.dispatchEvent(new Event("compositionstart"));
    input.value = "途中";
    model.value = "external";
    await flush();
    expect(input.value).toBe("途中");
    input.value = "確定";
    input.dispatchEvent(new Event("compositionend"));
    expect(model.value).toBe("確定");
  });
  it("synthetic isComposing input and boot-path state suppress model writes", async () => {
    const model = signal("a");
    function Demo() {
      return h("input", { modelValue: model });
    }
    const root = host(h(Demo, {}));
    const input = root.querySelector("input")!;
    (input as unknown as Record<symbol, string>)[
      Symbol.for("@takazudo/zfb/zudo-react/composition-v1")
    ] = "active";
    hydrate(h(Demo, {}), root, options());
    input.value = "中";
    input.dispatchEvent(new InputEvent("input", { isComposing: true }));
    expect(model.value).toBe("中");
    model.value = "external";
    await flush();
    expect(input.value).toBe("中");
    input.value = "終";
    input.dispatchEvent(new Event("compositionend"));
    expect(model.value).toBe("終");
  });
  it("unknown focused composition postpones writes until blur", async () => {
    const model = signal("a");
    function Demo() {
      return h("input", { modelValue: model });
    }
    const root = host(h(Demo, {}));
    const input = root.querySelector("input")!;
    input.focus();
    hydrate(h(Demo, {}), root, options());
    model.value = "external";
    await flush();
    expect(input.value).toBe("a");
    input.value = "final";
    input.blur();
    expect(model.value).toBe("final");
  });
  it("rejects conflicting shared controls before attaching listeners", () => {
    const model = signal("a");
    function Demo() {
      return h("div", {
        children: [h("input", { modelValue: model }), h("input", { modelValue: model })],
      });
    }
    const root = host(h(Demo, {}));
    root.querySelectorAll("input")[1]!.value = "b";
    expect(hydrate(h(Demo, {}), root, options())).toBeNull();
    expect(diagnostics[0]!).toMatchObject({ code: "ZR_MODEL_CONFLICT" });
  });
  it("rejects a brandless model during preflight without a write", () => {
    const real = signal("a");
    let model: unknown = real;
    function Demo() {
      return h("input", { modelValue: model });
    }
    const root = host(h(Demo, {}));
    let current = "a";
    const fake = {
      $$zudoReactive: "zudo-react.reactive.v1",
      get value() {
        return current;
      },
      set value(next: string) {
        current = next;
      },
    };
    model = fake;
    expect(hydrate(h(Demo, {}), root, options())).toBeNull();
    expect(diagnostics.at(-1)!.code).toBe("ZR_MODEL_READONLY");
    expect(current).toBe("a");
  });
  it("mount initializes from a model", () => {
    const model = signal("b");
    function Demo() {
      return h("select", {
        modelValue: model,
        children: [
          h("option", { value: "a", children: "A" }),
          h("option", { value: "b", children: "B" }),
        ],
      });
    }
    const root = host(h(Demo, {}));
    root.setAttribute("data-zfb-island-skip-ssr", "Demo");
    root.replaceChildren();
    expect(mount(h(Demo, {}), root, options())).not.toBeNull();
    expect(root.querySelector("select")!.value).toBe("b");
  });
});

describe("radio ownership and model values", () => {
  it("rejects same-name radios across roots until the owner is disposed", () => {
    function Demo() {
      return h("input", {
        type: "radio",
        name: "shared",
        value: "a",
        modelValue: signal<string | null>("a"),
      });
    }
    const first = host(h(Demo, {}));
    const second = host(h(Demo, {}));
    const owner = hydrate(h(Demo, {}), first, options())!;
    expect(owner).not.toBeNull();
    expect(hydrate(h(Demo, {}), second, options())).toBeNull();
    expect(diagnostics.at(-1)!.code).toBe("ZR_MODEL_CONFLICT");
    owner.dispose();
    expect(hydrate(h(Demo, {}), second, options())).not.toBeNull();
  });
  it("ignores unchecked radio change and blur", () => {
    const model = signal<string | null>("b");
    function Demo() {
      return h("div", {
        children: [
          h("input", { type: "radio", name: "g", value: "a", modelValue: model }),
          h("input", { type: "radio", name: "g", value: "b", modelValue: model }),
        ],
      });
    }
    const root = host(h(Demo, {}));
    hydrate(h(Demo, {}), root, options());
    const unchecked = root.querySelector("input")!;
    unchecked.dispatchEvent(new Event("change"));
    unchecked.dispatchEvent(new Event("blur"));
    expect(model.value).toBe("b");
  });
  it("reports a select model value with no matching option without changing the DOM", async () => {
    const model = signal("a");
    function Demo() {
      return h("select", {
        modelValue: model,
        children: h("option", { value: "a", children: "A" }),
      });
    }
    const root = host(h(Demo, {}));
    const select = root.querySelector("select")!;
    hydrate(h(Demo, {}), root, options());
    model.value = "missing";
    await flush();
    expect(select.value).toBe("a");
    expect(diagnostics.at(-1)!.code).toBe("ZR_MODEL_VALUE");
  });
});
