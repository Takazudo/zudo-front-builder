import { beforeEach, describe, expect, it } from "vite-plus/test";
import {
  ErrorBoundary,
  For,
  Show,
  flush,
  getScope,
  h,
  signal,
  type Diagnostic,
} from "../../zudo-react/index.js";
import { subscriberCount } from "../../zudo-react/reactive.js";
import { hydrate, mount } from "../../zudo-react/client.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";

// Permanent L1 SSR and L2 DOM coverage of the public subtree recovery contract.
const identity = { component: "Demo", build: "boundary-test" };
let diagnostics: Diagnostic[];
const options = () => ({ identity, report: (value: Diagnostic) => diagnostics.push(value) });
function host(node: ReturnType<typeof h>, skipSsr = false): Element {
  const parent = document.createElement("div");
  document.body.append(parent);
  parent.innerHTML = renderToString(islandRoot(node, { identity, skipSsr }));
  return parent.firstElementChild!;
}
beforeEach(() => {
  document.body.replaceChildren();
  diagnostics = [];
});

describe("ErrorBoundary server rendering", () => {
  it("discards partial output and disposes the failed attempt exactly once", () => {
    let cleaned = 0;
    const original = new Error("broken subtree");
    function Broken() {
      getScope().onCleanup(() => cleaned++);
      throw original;
    }
    const output = renderToString(
      h(
        "main",
        null,
        ErrorBoundary({
          children: () => [h("i", null, "partial"), h(Broken, null)],
          fallback: (error) => {
            expect(error).toBe(original);
            return h("b", null, "fallback");
          },
        }),
        h("span", null, "sibling"),
      ),
    );
    expect(output).toBe("<main><b>fallback</b><span>sibling</span></main>");
    expect(cleaned).toBe(1);
  });
  it("propagates a failing fallback to the next boundary and preserves unhandled errors", () => {
    const original = new Error("fallback failed");
    const inner = () =>
      ErrorBoundary({
        children: () => {
          throw Error("child");
        },
        fallback: () => {
          throw original;
        },
      });
    expect(
      renderToString(
        ErrorBoundary({ children: inner, fallback: (error) => h("b", null, String(error)) }),
      ),
    ).toBe("<b>Error: fallback failed</b>");
    expect(() => renderToString(inner())).toThrow(original);
  });
  it("rolls back radio ownership and anonymous form counters after failure", () => {
    const failed = signal<string | null>("a");
    const good = signal<string | null>("a");
    const output = renderToString(
      ErrorBoundary({
        children: () => [
          h(
            "form",
            null,
            h("input", { type: "radio", name: "choice", value: "a", modelValue: failed }),
          ),
          h(() => {
            throw Error("late");
          }, null),
        ],
        fallback: () =>
          h(
            "form",
            null,
            h("input", { type: "radio", name: "choice", value: "a", modelValue: good }),
          ),
      }),
    );
    expect(output).toContain('<form><input type="radio" name="choice" value="a" checked></form>');
  });
  it("does not conceal nested island or reserved rawHtml integrity failures", () => {
    for (const children of [
      () => h("div", { rawHtml: "<!--zr:1:9:c-->" }),
      () => h("div", { "data-zfb-island": "Injected" }),
    ]) {
      function Demo() {
        return ErrorBoundary({ children, fallback: () => "hidden" });
      }
      expect(() => renderToString(islandRoot(h(Demo, null), { identity }))).toThrow(
        /ZR_RAW_HTML|ZR_NESTED_ISLAND/,
      );
    }
  });
});

describe("ErrorBoundary client recovery", () => {
  it.each([
    [false, true],
    [true, true],
    [true, false],
  ])(
    "remounts only its region when server failure=%s and client failure=%s",
    (serverFailure, clientFailure) => {
      let broken = serverFailure;
      function Child() {
        if (broken) throw Error("broken");
        return h("b", null, "healthy");
      }
      function Demo() {
        return [
          ErrorBoundary({
            children: () => h(Child, null),
            fallback: () => h("i", null, "fallback"),
          }),
          h("button", null, "sibling"),
        ];
      }
      const root = host(h(Demo, null));
      const sibling = root.querySelector("button");
      broken = clientFailure;
      const handle = hydrate(h(Demo, null), root, options());
      expect(handle).not.toBeNull();
      expect(root.querySelector("button")).toBe(sibling);
      expect(root.querySelector(clientFailure ? "i" : "b")?.textContent).toBe(
        clientFailure ? "fallback" : "healthy",
      );
      expect(diagnostics).toEqual([]);
      handle!.unmount();
      expect(root.childNodes).toHaveLength(0);
    },
  );
  it("retries explicitly and on changed reset input without remounting the sibling", async () => {
    const resetKey = signal(0);
    const text = signal("live");
    let broken = true;
    let retries = 0;
    let cleaned = 0;
    let activated = 0;
    let reset = () => {};
    function Child() {
      retries++;
      getScope().onCleanup(() => cleaned++);
      getScope().onActivate(() => {
        activated++;
      });
      if (broken) throw Error("broken");
      return h("b", null, text);
    }
    let siblingSetups = 0;
    function Good() {
      siblingSetups++;
      const count = signal(0);
      return h("button", { "on:click": () => count.value++, children: count });
    }
    function Demo() {
      return [
        ErrorBoundary({
          resetKey,
          children: () => h(Child, null),
          fallback: (_, retry) => {
            reset = retry;
            return h("i", null, "fallback");
          },
        }),
        h(Good, null),
      ];
    }
    const root = host(h(Demo, null), true);
    const handle = mount(h(Demo, null), root, options())!;
    const sibling = root.querySelector("button")!;
    sibling.click();
    await flush();
    resetKey.value = 0;
    await flush();
    expect(retries).toBe(1);
    reset();
    reset();
    await flush();
    expect(retries).toBe(2);
    expect(cleaned).toBe(2);
    broken = false;
    resetKey.value = 1;
    await flush();
    expect(retries).toBe(3);
    expect(activated).toBe(1);
    expect(root.querySelector("b")?.textContent).toBe("live");
    expect(root.querySelector("button")).toBe(sibling);
    expect(sibling.textContent).toBe("1");
    expect(siblingSetups).toBe(1);
    expect(subscriberCount(text)).toBe(1);
    handle.unmount();
    expect(cleaned).toBe(3);
    expect(subscriberCount(text)).toBe(0);
    reset();
    resetKey.value = 2;
    await flush();
    expect(retries).toBe(3);
    expect(diagnostics).toEqual([]);
  });
  it.each(["Show", "For", "For.by"])(
    "recovers from scheduled %s rendering and cancels stale bindings",
    async (kind) => {
      const changed = signal(false);
      const each = signal(["a"]);
      const text = signal("before");
      let cleanup = 0;
      function Child() {
        getScope().onCleanup(() => cleanup++);
        return [
          h("b", null, text),
          kind === "Show"
            ? Show({
                when: changed,
                children: () => {
                  throw Error("structural");
                },
              })
            : For({
                each,
                by: (item) => {
                  if (kind === "For.by" && item === "bad") throw Error("structural key");
                  return item;
                },
                children: (item) => {
                  if (item.value === "bad") throw Error("structural");
                  return h("span", null, item);
                },
              }),
        ];
      }
      function Demo() {
        return [
          ErrorBoundary({
            children: () => h(Child, null),
            fallback: () => h("i", null, "fallback"),
          }),
          h("button", null, "good"),
        ];
      }
      const root = host(h(Demo, null), true);
      const handle = mount(h(Demo, null), root, options())!;
      const sibling = root.querySelector("button");
      changed.value = true;
      each.value = ["a", "bad"];
      text.value = "stale";
      await flush();
      expect(root.querySelector("i")?.textContent).toBe("fallback");
      expect(root.querySelector("b")).toBeNull();
      expect(root.querySelector("button")).toBe(sibling);
      expect(subscriberCount(text)).toBe(0);
      expect(cleanup).toBe(1);
      expect(diagnostics).toEqual([]);
      handle.unmount();
      expect(cleanup).toBe(1);
    },
  );
});

describe("ErrorBoundary phase and hydration invariants", () => {
  it("routes a scheduled fallback failure to the outer boundary", async () => {
    const when = signal(false);
    const received: unknown[] = [];
    const failure = { reason: "fallback" };
    function Demo() {
      return ErrorBoundary({
        children: () =>
          ErrorBoundary({
            children: () =>
              Show({
                when,
                children: () => {
                  throw Error("render");
                },
              }),
            fallback: () => {
              throw failure;
            },
          }),
        fallback: (error) => {
          received.push(error);
          return h("b", null, "outer");
        },
      });
    }
    const root = host(h(Demo, null), true);
    const handle = mount(h(Demo, null), root, options())!;
    when.value = true;
    await flush();
    expect(received).toEqual([failure]);
    expect(root.querySelector("b")?.textContent).toBe("outer");
    expect(diagnostics).toEqual([]);
    handle.unmount();
  });
  it.each(["text", "attribute", "rawHtml"])(
    "handles reactive %s errors by their phase",
    async (kind) => {
      const value = signal<unknown>("ok");
      function Demo() {
        return ErrorBoundary({
          children: () =>
            kind === "text"
              ? h("b", null, value as never)
              : kind === "attribute"
                ? h("b", { title: value })
                : h("b", { rawHtml: value }),
          fallback: () => h("i", null, "fallback"),
        });
      }
      const root = host(h(Demo, null), true);
      const handle = mount(h(Demo, null), root, options())!;
      value.value = {};
      await flush();
      expect(root.querySelector("i")?.textContent).toBe("fallback");
      expect(diagnostics).toEqual([]);
      expect(subscriberCount(value)).toBe(0);
      handle.unmount();
    },
  );
  it("does not interpret a user error message as an integrity classification", () => {
    function Demo() {
      return ErrorBoundary({
        children: () => {
          throw Error("ZR_IDENTITY: user text");
        },
        fallback: () => h("b", null, "caught"),
      });
    }
    const root = host(h(Demo, null), true);
    const handle = mount(h(Demo, null), root, options())!;
    expect(handle).not.toBeNull();
    expect(root.textContent).toBe("caught");
    handle.unmount();
  });
  it("keeps event, effect and cleanup failures diagnostic and cleans remaining resources", async () => {
    let cleaned = 0;
    function Demo() {
      return ErrorBoundary({
        children: () => {
          const scope = getScope();
          scope.onCleanup(() => cleaned++);
          scope.onCleanup(() => {
            throw Error("cleanup");
          });
          scope.effect(() => {
            throw Error("effect");
          });
          return h(
            "button",
            {
              "on:click": () => {
                throw Error("event");
              },
            },
            "good",
          );
        },
        fallback: () => h("i", null, "wrong"),
      });
    }
    const root = host(h(Demo, null), true);
    const handle = mount(h(Demo, null), root, options())!;
    root.querySelector("button")!.click();
    await flush();
    expect(root.querySelector("i")).toBeNull();
    handle.unmount();
    expect(cleaned).toBe(1);
    expect(diagnostics.map((entry) => entry.code).sort()).toEqual([
      "ZR_CLEANUP_ERROR",
      "ZR_EFFECT_ERROR",
      "ZR_EVENT_ERROR",
    ]);
  });
  it("does not catch initial or structural activation errors", async () => {
    for (const structural of [false, true]) {
      const when = signal(!structural);
      let caught = 0;
      function Broken() {
        getScope().onActivate(() => {
          throw Error("activation");
        });
        return h("b", null, "child");
      }
      function Demo() {
        return ErrorBoundary({
          children: () => Show({ when, children: () => h(Broken, null) }),
          fallback: () => {
            caught++;
            return "wrong";
          },
        });
      }
      const root = host(h(Demo, null), true);
      const handle = mount(h(Demo, null), root, options());
      if (structural) {
        expect(handle).not.toBeNull();
        when.value = true;
        await flush();
      } else expect(handle).toBeNull();
      expect(caught).toBe(0);
      expect(diagnostics.some((entry) => entry.code === "ZR_ACTIVATION_ERROR")).toBe(true);
      handle?.unmount();
    }
  });
  it("preserves dirty sibling forms while fresh boundary controls use their model", () => {
    const inside = signal("inside server");
    const outside = signal("outside server");
    function Demo() {
      return h(
        "form",
        null,
        ErrorBoundary({
          children: () => h("input", { name: "inside", modelValue: inside }),
          fallback: () => "fallback",
        }),
        h("input", { name: "outside", modelValue: outside }),
      );
    }
    const root = host(h(Demo, null));
    const oldInside = root.querySelector<HTMLInputElement>('[name="inside"]')!;
    const oldOutside = root.querySelector<HTMLInputElement>('[name="outside"]')!;
    oldInside.value = "inside dirty";
    oldOutside.value = "outside dirty";
    inside.value = "inside client";
    const handle = hydrate(h(Demo, null), root, options())!;
    expect(handle).not.toBeNull();
    expect(root.querySelector('[name="inside"]')).not.toBe(oldInside);
    expect(root.querySelector<HTMLInputElement>('[name="inside"]')!.value).toBe("inside client");
    expect(root.querySelector('[name="outside"]')).toBe(oldOutside);
    expect(outside.value).toBe("outside dirty");
    expect(inside.value).toBe("inside client");
    expect(diagnostics).toEqual([]);
    handle.unmount();
  });
  it("reconciles a radio group shared across a boundary using the retained form identity", () => {
    const model = signal<string | null>("inside");
    function Demo() {
      return h(
        "form",
        null,
        ErrorBoundary({
          children: () =>
            h("input", { type: "radio", name: "group", value: "inside", modelValue: model }),
          fallback: () => "fallback",
        }),
        h("input", { type: "radio", name: "group", value: "outside", modelValue: model }),
      );
    }
    const root = host(h(Demo, null));
    const outside = root.querySelector<HTMLInputElement>('[value="outside"]')!;
    outside.checked = true;
    const form = root.querySelector("form");
    const handle = hydrate(h(Demo, null), root, options())!;
    expect(handle).not.toBeNull();
    expect(root.querySelector("form")).toBe(form);
    expect(root.querySelector('[value="outside"]')).toBe(outside);
    expect(model.value).toBe("outside");
    expect(root.querySelector<HTMLInputElement>('[value="inside"]')!.checked).toBe(false);
    expect(outside.checked).toBe(true);
    expect(diagnostics).toEqual([]);
    handle.unmount();
  });
  it("retains root diagnostics for aggregate form failures", () => {
    const a = signal("a");
    const b = signal("b");
    let caught = 0;
    function Demo() {
      return ErrorBoundary({
        children: () => [
          h("input", { type: "radio", name: "group", value: "a", modelValue: a }),
          h("input", { type: "radio", name: "group", value: "b", modelValue: b }),
        ],
        fallback: () => {
          caught++;
          return "wrong";
        },
      });
    }
    const root = host(h(Demo, null), true);
    expect(mount(h(Demo, null), root, options())).toBeNull();
    expect(caught).toBe(0);
    expect(diagnostics[0]?.code).toBe("ZR_MODEL_CONFLICT");
  });
  it.each(["crossed", "namespace", "duplicate", "element", "fake-text"])(
    "rejects %s markers without replacing DOM",
    (kind) => {
      function Demo() {
        return [
          ErrorBoundary({
            children: () =>
              h(
                "section",
                null,
                h(() => h("b", null, "child"), null),
              ),
            fallback: () => "fallback",
          }),
          h("button", null, "good"),
        ];
      }
      const root = host(h(Demo, null));
      const section = root.querySelector("section")!;
      if (kind === "crossed")
        section.innerHTML = "<!--zr:1:2.0:c--><!--zr:1:2.1:f--><!--/zr:1:2.0--><!--/zr:1:2.1-->";
      if (kind === "namespace") section.innerHTML = "<!--zr:1:99:c--><!--/zr:1:99-->";
      if (kind === "duplicate")
        section.innerHTML = "<!--zr:1:2.0:c--><!--/zr:1:2.0--><!--zr:1:2.0:c--><!--/zr:1:2.0-->";
      if (kind === "element") section.innerHTML = "<!--zr:1:2.0:c--><b><!--/zr:1:2.0--></b>";
      if (kind === "fake-text") {
        const close = section.lastChild!;
        close.replaceWith(document.createTextNode(close.textContent!));
      }
      const before = root.innerHTML;
      expect(hydrate(h(Demo, null), root, options())).toBeNull();
      expect(root.innerHTML).toBe(before);
      expect(diagnostics[0]?.code).toBe("ZR_HYDRATION_MISMATCH");
    },
  );
  it("stages replacement until the following sibling also passes preflight", () => {
    let changed = false;
    function Demo() {
      return [
        ErrorBoundary({
          children: () => h("b", null, changed ? "new" : "old"),
          fallback: () => "fallback",
        }),
        h("button", null, changed ? "mismatch" : "good"),
      ];
    }
    const root = host(h(Demo, null));
    const before = root.innerHTML;
    changed = true;
    expect(hydrate(h(Demo, null), root, options())).toBeNull();
    expect(root.innerHTML).toBe(before);
  });
});

describe("ErrorBoundary ownership and reset", () => {
  it("starts a fresh healthy attempt only when Object.is sees a new reset key", async () => {
    const first = {};
    const resetKey = signal(first);
    let setups = 0;
    let cleaned = 0;
    function Demo() {
      return ErrorBoundary({
        resetKey,
        children: () => {
          setups++;
          getScope().onCleanup(() => cleaned++);
          return h("b", null, "healthy");
        },
        fallback: () => "fallback",
      });
    }
    const root = host(h(Demo, null), true);
    const handle = mount(h(Demo, null), root, options())!;
    const before = root.querySelector("b");
    resetKey.value = first;
    await flush();
    expect(root.querySelector("b")).toBe(before);
    resetKey.value = {};
    await flush();
    expect(root.querySelector("b")).not.toBe(before);
    expect(setups).toBe(2);
    expect(cleaned).toBe(1);
    handle.unmount();
    expect(cleaned).toBe(2);
    expect(subscriberCount(resetKey)).toBe(0);
  });
  it("preserves a selected fresh radio when adopted group members are unchecked", () => {
    const model = signal<string | null>("inside");
    function Demo() {
      return h(
        "form",
        null,
        ErrorBoundary({
          children: () =>
            h("input", { type: "radio", name: "group", value: "inside", modelValue: model }),
          fallback: () => "fallback",
        }),
        h("input", { type: "radio", name: "group", value: "outside", modelValue: model }),
      );
    }
    const root = host(h(Demo, null));
    const handle = hydrate(h(Demo, null), root, options())!;
    expect(handle).not.toBeNull();
    expect(model.value).toBe("inside");
    expect(root.querySelector<HTMLInputElement>('[value="inside"]')!.checked).toBe(true);
    expect(root.querySelector<HTMLInputElement>('[value="outside"]')!.checked).toBe(false);
    expect(diagnostics).toEqual([]);
    handle.unmount();
  });
  it("keeps reserved rawHtml updates diagnostic", async () => {
    const rawHtml = signal("safe");
    function Demo() {
      return ErrorBoundary({
        children: () => h("b", { rawHtml }),
        fallback: () => h("i", null, "wrong"),
      });
    }
    const root = host(h(Demo, null), true);
    const handle = mount(h(Demo, null), root, options())!;
    rawHtml.value = "<!--zr:1:99:c-->";
    await flush();
    expect(root.querySelector("i")).toBeNull();
    expect(root.querySelector("b")!.textContent).toBe("safe");
    expect(diagnostics[0]?.actual).toContain("ZR_RAW_HTML");
    handle.unmount();
  });
  it("rolls back additions to an existing server radio option set", () => {
    const model = signal<string | null>("b");
    const radio = (value: string) =>
      h("input", { type: "radio", name: "group", value, modelValue: model });
    const output = renderToString(
      h(
        "form",
        null,
        radio("a"),
        ErrorBoundary({
          children: () => [
            radio("b"),
            h(() => {
              throw Error("late");
            }, null),
          ],
          fallback: () => radio("b"),
        }),
      ),
    );
    expect(output).toContain('value="b" checked');
    expect(output.match(/value="b"/g)).toHaveLength(1);
  });
  it("accepts valid nested boundary and rawHtml markers", () => {
    function Demo() {
      return ErrorBoundary({
        children: () =>
          ErrorBoundary({
            children: () => h("div", { rawHtml: "<b>trusted</b>" }),
            fallback: () => "inner",
          }),
        fallback: () => "outer",
      });
    }
    const root = host(h(Demo, null));
    const handle = hydrate(h(Demo, null), root, options())!;
    expect(handle).not.toBeNull();
    expect(root.querySelector("b")?.textContent).toBe("trusted");
    expect(diagnostics).toEqual([]);
    handle.unmount();
  });
});

it("coalesces explicit and key resets in either scheduling order", async () => {
  for (const keyFirst of [true, false]) {
    const resetKey = signal(0);
    let reset = () => {};
    let attempts = 0;
    function Demo() {
      return ErrorBoundary({
        resetKey,
        children: () => {
          attempts++;
          throw Error("fail");
        },
        fallback: (_, retry) => {
          reset = retry;
          return "fallback";
        },
      });
    }
    const root = host(h(Demo, null), true);
    const handle = mount(h(Demo, null), root, options())!;
    if (keyFirst) {
      resetKey.value = 1;
      reset();
    } else {
      reset();
      resetKey.value = 1;
    }
    await flush();
    expect(attempts).toBe(2);
    handle.unmount();
  }
});

it("removes nested form listeners after an outer reset followed by inner recovery", async () => {
  const resetKey = signal(0);
  const when = signal(false);
  const model = signal("value");
  function Demo() {
    return ErrorBoundary({
      resetKey,
      children: () =>
        ErrorBoundary({
          children: () => [
            h("input", { modelValue: model }),
            Show({
              when,
              children: () => {
                throw Error("fail");
              },
            }),
          ],
          fallback: () => h("b", null, "inner fallback"),
        }),
      fallback: () => "outer fallback",
    });
  }
  const root = host(h(Demo, null), true);
  const handle = mount(h(Demo, null), root, options())!;
  resetKey.value = 1;
  await flush();
  const stale = root.querySelector("input")!;
  expect(subscriberCount(model)).toBe(1);
  when.value = true;
  await flush();
  expect(root.textContent).toBe("inner fallback");
  expect(subscriberCount(model)).toBe(0);
  stale.value = "stale";
  stale.dispatchEvent(new Event("input"));
  expect(model.value).toBe("value");
  handle.unmount();
});

it("adopts ordinary rawHtml comments with marker-like text outside boundaries", () => {
  function Demo() {
    return h("div", { rawHtml: "<!-- documentation: zr:example --><b>safe</b>" });
  }
  const root = host(h(Demo, null));
  const before = root.querySelector("b");
  const handle = hydrate(h(Demo, null), root, options())!;
  expect(handle).not.toBeNull();
  expect(root.querySelector("b")).toBe(before);
  expect(diagnostics).toEqual([]);
  handle.unmount();
});

it("resolves explicit form associations across fresh and adopted radios", () => {
  const model = signal<string | null>("inside");
  function Demo() {
    return [
      h("form", { id: "named-form" }),
      ErrorBoundary({
        children: () =>
          h("input", {
            type: "radio",
            form: "named-form",
            name: "group",
            value: "inside",
            modelValue: model,
          }),
        fallback: () => "fallback",
      }),
      h("input", {
        type: "radio",
        form: "named-form",
        name: "group",
        value: "outside",
        modelValue: model,
      }),
    ];
  }
  const root = host(h(Demo, null));
  const outside = root.querySelector('[value="outside"]');
  const handle = hydrate(h(Demo, null), root, options())!;
  expect(handle).not.toBeNull();
  expect(root.querySelector('[value="outside"]')).toBe(outside);
  expect(model.value).toBe("inside");
  expect(diagnostics).toEqual([]);
  handle.unmount();
});

it("resets a radio subgroup while the model selects a healthy sibling", async () => {
  const resetKey = signal(0);
  const model = signal<string | null>("inside");
  function Demo() {
    return h(
      "form",
      null,
      ErrorBoundary({
        resetKey,
        children: () =>
          h("input", { type: "radio", name: "group", value: "inside", modelValue: model }),
        fallback: () => "fallback",
      }),
      h("input", { type: "radio", name: "group", value: "outside", modelValue: model }),
    );
  }
  const root = host(h(Demo, null));
  const handle = hydrate(h(Demo, null), root, options())!;
  const outside = root.querySelector<HTMLInputElement>('[value="outside"]')!;
  model.value = "outside";
  resetKey.value = 1;
  await flush();
  expect(root.querySelector('[value="outside"]')).toBe(outside);
  expect(subscriberCount(model)).toBe(2);
  expect(outside.checked).toBe(true);
  const inside = root.querySelector<HTMLInputElement>('[value="inside"]')!;
  inside.checked = true;
  inside.dispatchEvent(new Event("change"));
  await flush();
  expect(model.value).toBe("inside");
  expect(outside.checked).toBe(false);
  expect(diagnostics).toEqual([]);
  handle.unmount();
});

it("preflights invalid replacement groups before disposing the live attempt", async () => {
  const resetKey = signal(0);
  const model = signal<string | null>("outside");
  let option = "inside";
  let cleanups = 0;
  function Demo() {
    return h(
      "form",
      null,
      ErrorBoundary({
        resetKey,
        children: () => {
          getScope().onCleanup(() => cleanups++);
          return h("input", { type: "radio", name: "group", value: option, modelValue: model });
        },
        fallback: () => "fallback",
      }),
      h("input", { type: "radio", name: "group", value: "outside", modelValue: model }),
    );
  }
  const root = host(h(Demo, null), true);
  const handle = mount(h(Demo, null), root, options())!;
  const inside = root.querySelector('[value="inside"]');
  option = "outside";
  resetKey.value++;
  await flush();
  expect(root.querySelector('[value="inside"]')).toBe(inside);
  expect(subscriberCount(model)).toBe(2);
  expect(cleanups).toBe(1); // only the rejected provisional attempt
  expect(diagnostics[0]?.actual).toContain("ZR_MODEL_CONFLICT");
  option = "new";
  resetKey.value++;
  await flush();
  expect(root.querySelector('[value="new"]')).not.toBeNull();
  expect(root.querySelector('[value="inside"]')).toBeNull();
  expect(cleanups).toBe(2);
  expect(subscriberCount(model)).toBe(2);
  handle.unmount();
  expect(cleanups).toBe(3);
  expect(subscriberCount(model)).toBe(0);
});

it("uses current radio members after repeated replacement and rejects removed options", async () => {
  const resetKey = signal(0);
  const model = signal<string | null>("outside");
  let option = "inside";
  function Demo() {
    return h(
      "form",
      null,
      ErrorBoundary({
        resetKey,
        children: () =>
          h("input", { type: "radio", name: "group", value: option, modelValue: model }),
        fallback: () => "fallback",
      }),
      h("input", { type: "radio", name: "group", value: "outside", modelValue: model }),
    );
  }
  const root = host(h(Demo, null), true);
  const handle = mount(h(Demo, null), root, options())!;
  for (const next of ["new", "newer", "last"]) {
    option = next;
    resetKey.value++;
    await flush();
    expect(subscriberCount(model)).toBe(2);
  }
  expect(diagnostics).toEqual([]);
  model.value = "inside";
  await flush();
  expect(diagnostics).toHaveLength(2);
  expect(diagnostics.every((entry) => entry.code === "ZR_MODEL_VALUE")).toBe(true);
  model.value = "last";
  await flush();
  expect(root.querySelector<HTMLInputElement>('[value="last"]')!.checked).toBe(true);
  expect(diagnostics).toHaveLength(2);
  handle.unmount();
  expect(subscriberCount(model)).toBe(0);
});

it("retains the cross-control kind invariant during reset", async () => {
  const resetKey = signal(0);
  const model = signal("inside");
  let radio = false;
  function Demo() {
    return [
      ErrorBoundary({
        resetKey,
        children: () =>
          h(
            "input",
            radio
              ? { type: "radio", name: "group", value: "inside", modelValue: model }
              : { modelValue: model },
          ),
        fallback: () => "fallback",
      }),
      h("input", { modelValue: model }),
    ];
  }
  const root = host(h(Demo, null), true);
  const handle = mount(h(Demo, null), root, options())!;
  const before = root.querySelector("input");
  radio = true;
  resetKey.value++;
  await flush();
  expect(root.querySelector("input")).toBe(before);
  expect(diagnostics[0]?.actual).toContain("radio and non-radio share model");
  expect(subscriberCount(model)).toBe(2);
  handle.unmount();
});

it("does not subscribe disposed rendering jobs to fallback model reads", async () => {
  const when = signal(false);
  const model = signal("fallback");
  function Demo() {
    return ErrorBoundary({
      children: () =>
        Show({
          when,
          children: () => {
            throw Error("fail");
          },
        }),
      fallback: () => h("input", { modelValue: model }),
    });
  }
  const root = host(h(Demo, null), true);
  const handle = mount(h(Demo, null), root, options())!;
  when.value = true;
  await flush();
  expect(subscriberCount(when)).toBe(0);
  expect(subscriberCount(model)).toBe(1);
  handle.unmount();
  expect(subscriberCount(model)).toBe(0);
});
