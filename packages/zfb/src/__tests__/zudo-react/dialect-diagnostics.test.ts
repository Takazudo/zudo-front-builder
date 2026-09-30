import { beforeEach, describe, expect, it } from "vitest";

import { h, signal, type Child, type Diagnostic } from "../../zudo-react/index.js";
import { mount } from "../../zudo-react/client.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";

const identity = { component: "DialectDiagnostics", build: "b1" };

type Case = {
  readonly name: string;
  readonly build: () => Child;
  readonly html?: string;
  readonly error?: {
    readonly code: string;
    readonly authored: string;
    readonly serverDetail: string;
    readonly clientExpected: string;
    readonly clientActual: string;
  };
};

const cases: readonly Case[] = [
  {
    name: "meta charSet suggests charset",
    build: () => h("meta", { charSet: "utf-8" }),
    error: {
      code: "ZR_PROP_DIALECT",
      authored: "charSet",
      serverDetail: "meta.charSet (use `charset` instead of `charSet`)",
      clientExpected: "charset",
      clientActual: "charSet",
    },
  },
  {
    name: "input autoComplete suggests autocomplete",
    build: () => h("input", { autoComplete: "name" }),
    error: {
      code: "ZR_PROP_DIALECT",
      authored: "autoComplete",
      serverDetail: "input.autoComplete (use `autocomplete` instead of `autoComplete`)",
      clientExpected: "autocomplete",
      clientActual: "autoComplete",
    },
  },
  {
    name: "form acceptCharset suggests accept-charset",
    build: () => h("form", { acceptCharset: "utf-8" }),
    error: {
      code: "ZR_PROP_DIALECT",
      authored: "acceptCharset",
      serverDetail: "form.acceptCharset (use `accept-charset` instead of `acceptCharset`)",
      clientExpected: "accept-charset",
      clientActual: "acceptCharset",
    },
  },
  {
    name: "button onClick suggests an event listener",
    build: () => h("button", { onClick: () => undefined }),
    error: {
      code: "ZR_PROP_DIALECT",
      authored: "onClick",
      serverDetail: "button.onClick (use `on:click` instead of `onClick`)",
      clientExpected: "on:click",
      clientActual: "onClick",
    },
  },
  {
    name: "form onSubmit suggests an event listener",
    build: () => h("form", { onSubmit: () => undefined }),
    error: {
      code: "ZR_PROP_DIALECT",
      authored: "onSubmit",
      serverDetail: "form.onSubmit (use `on:submit` instead of `onSubmit`)",
      clientExpected: "on:submit",
      clientActual: "onSubmit",
    },
  },
  {
    name: "onDoubleClick suggests the native dblclick event",
    build: () => h("button", { onDoubleClick: () => undefined }),
    error: {
      code: "ZR_PROP_DIALECT",
      authored: "onDoubleClick",
      serverDetail: "button.onDoubleClick (use `on:dblclick` instead of `onDoubleClick`)",
      clientExpected: "on:dblclick",
      clientActual: "onDoubleClick",
    },
  },
  {
    name: "div className suggests class",
    build: () => h("div", { className: "notice" }),
    error: {
      code: "ZR_PROP_DIALECT",
      authored: "className",
      serverDetail: "div.className (use `class` instead of `className`)",
      clientExpected: "class",
      clientActual: "className",
    },
  },
  {
    name: "label htmlFor suggests for",
    build: () => h("label", { htmlFor: "name" }),
    error: {
      code: "ZR_PROP_DIALECT",
      authored: "htmlFor",
      serverDetail: "label.htmlFor (use `for` instead of `htmlFor`)",
      clientExpected: "for",
      clientActual: "htmlFor",
    },
  },
  {
    name: "SVG path strokeWidth suggests stroke-width",
    build: () => h("svg", null, h("path", { strokeWidth: 2 })),
    error: {
      code: "ZR_PROP_DIALECT",
      authored: "strokeWidth",
      serverDetail: "path.strokeWidth (use `stroke-width` instead of `strokeWidth`)",
      clientExpected: "stroke-width",
      clientActual: "strokeWidth",
    },
  },
  {
    name: "legacy strokeWidth stays a dialect error outside SVG",
    build: () => h("div", { strokeWidth: 2 }),
    error: {
      code: "ZR_PROP_DIALECT",
      authored: "strokeWidth",
      serverDetail: "div.strokeWidth",
      clientExpected: "HTML-spelled prop",
      clientActual: "strokeWidth",
    },
  },
  {
    name: "SVG-only aliases do not apply to HTML elements",
    build: () => h("div", { fillRule: "evenodd" }),
    error: {
      code: "ZR_ATTRIBUTE",
      authored: "fillRule",
      serverDetail: "div.fillRule",
      clientExpected: "supported attribute",
      clientActual: "fillRule",
    },
  },
  {
    name: "SVG viewBox remains a valid camelCase attribute",
    build: () => h("svg", { viewBox: "0 0 1 1" }),
    html: '<svg viewBox="0 0 1 1"></svg>',
  },
  {
    name: "inputMode remains an unknown attribute without a suggestion",
    build: () => h("input", { inputMode: "text" }),
    error: {
      code: "ZR_ATTRIBUTE",
      authored: "inputMode",
      serverDetail: "input.inputMode",
      clientExpected: "supported attribute",
      clientActual: "inputMode",
    },
  },
  {
    name: "custom elements continue to accept arbitrary prop names",
    build: () => h("my-el", { someProp: "kept" }),
    html: '<my-el someProp="kept"></my-el>',
  },
  {
    name: "custom elements keep accepting new alias names",
    build: () => h("my-el", { autoComplete: "name" }),
    html: '<my-el autoComplete="name"></my-el>',
  },
  {
    name: "custom elements still reject legacy dialect names",
    build: () => h("my-el", { className: "notice" }),
    error: {
      code: "ZR_PROP_DIALECT",
      authored: "className",
      serverDetail: "my-el.className (use `class` instead of `className`)",
      clientExpected: "class",
      clientActual: "className",
    },
  },
  {
    name: "progress value requires a static value",
    build: () => h("progress", { value: signal(0.5) }),
    error: {
      code: "ZR_MODEL_UNSUPPORTED",
      authored: "value",
      serverDetail: "progress.value requires a static value",
      clientExpected: "static value",
      clientActual: "value",
    },
  },
  {
    name: "radio checked uses modelValue",
    build: () => h("input", { type: "radio", checked: signal(true) }),
    error: {
      code: "ZR_MODEL_UNSUPPORTED",
      authored: "checked",
      serverDetail: "input.checked requires modelValue",
      clientExpected: "modelValue",
      clientActual: "checked",
    },
  },
  {
    name: "radio value remains static",
    build: () => h("input", { type: "radio", value: signal("a") }),
    error: {
      code: "ZR_MODEL_UNSUPPORTED",
      authored: "value",
      serverDetail: "input.value requires a static value",
      clientExpected: "static value",
      clientActual: "value",
    },
  },
  {
    name: "checkbox checked uses modelChecked",
    build: () => h("input", { type: "checkbox", checked: signal(true) }),
    error: {
      code: "ZR_MODEL_UNSUPPORTED",
      authored: "checked",
      serverDetail: "input.checked requires modelChecked",
      clientExpected: "modelChecked",
      clientActual: "checked",
    },
  },
  {
    name: "text input value uses modelValue",
    build: () => h("input", { type: "text", value: signal("name") }),
    error: {
      code: "ZR_MODEL_UNSUPPORTED",
      authored: "value",
      serverDetail: "input.value requires modelValue",
      clientExpected: "modelValue",
      clientActual: "value",
    },
  },
  {
    name: "textarea value uses modelValue",
    build: () => h("textarea", { value: signal("text") }),
    error: {
      code: "ZR_MODEL_UNSUPPORTED",
      authored: "value",
      serverDetail: "textarea.value requires modelValue",
      clientExpected: "modelValue",
      clientActual: "value",
    },
  },
  {
    name: "select value uses modelValue",
    build: () => h("select", { value: signal("a") }),
    error: {
      code: "ZR_MODEL_UNSUPPORTED",
      authored: "value",
      serverDetail: "select.value requires modelValue",
      clientExpected: "modelValue",
      clientActual: "value",
    },
  },
  {
    name: "button value requires a static value",
    build: () => h("button", { value: signal("run") }),
    error: {
      code: "ZR_MODEL_UNSUPPORTED",
      authored: "value",
      serverDetail: "button.value requires a static value",
      clientExpected: "static value",
      clientActual: "value",
    },
  },
  {
    name: "custom element value requires a static value",
    build: () => h("my-el", { value: signal("run") }),
    error: {
      code: "ZR_MODEL_UNSUPPORTED",
      authored: "value",
      serverDetail: "my-el.value requires a static value",
      clientExpected: "static value",
      clientActual: "value",
    },
  },
];

let diagnostics: Diagnostic[];

function mountCase(node: Child) {
  let child: Child = h("div");
  function DialectDiagnostics() {
    return child;
  }
  const host = document.createElement("div");
  host.innerHTML = renderToString(islandRoot(h(DialectDiagnostics, {}), { identity }));
  document.body.append(host);
  const container = host.firstElementChild!;
  child = node;
  const handle = mount(h(DialectDiagnostics, {}), container, {
    identity,
    report: (item) => diagnostics.push(item),
  });
  return handle;
}

beforeEach(() => {
  document.body.replaceChildren();
  diagnostics = [];
});

describe("SSR and client attribute diagnostics", () => {
  it.each(cases)("$name", (testCase) => {
    const serverNode = testCase.build();
    if (testCase.error) {
      let message = "";
      try {
        renderToString(serverNode);
      } catch (error) {
        message = error instanceof Error ? error.message : String(error);
      }
      expect(message).toContain(`${testCase.error.code}:`);
      expect(message).toContain(testCase.error.serverDetail);
      expect(message).toContain(testCase.error.authored);

      const handle = mountCase(testCase.build());
      expect(handle).toBeNull();
      expect(diagnostics).toHaveLength(1);
      expect(diagnostics[0]).toMatchObject({
        code: testCase.error.code,
        expected: testCase.error.clientExpected,
        actual: testCase.error.clientActual,
      });
      return;
    }

    expect(renderToString(serverNode)).toBe(testCase.html);
    const handle = mountCase(testCase.build());
    expect(handle).not.toBeNull();
    expect(diagnostics).toEqual([]);
    handle!.unmount();
  });
});
