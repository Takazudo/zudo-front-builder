import { describe, expect, it } from "vitest";
import { h, signal, flush } from "../../zudo-react/index.js";
import { renderToString, islandRoot } from "../../zudo-react/server.js";
import { hydrate } from "../../zudo-react/client.js";
import { styleText } from "../../zudo-react/dom-bindings.js";
import type { Diagnostic } from "../../zudo-react/index.js";

const valid = {
  left: 0,
  opacity: 0.5,
  "line-height": 1.5,
  "--space": 2,
  "-webkit-transform": "translateX(1px)",
  inset: "1rem",
  cursor: "pointer",
  mask: "none",
  visibility: "visible",
  outline: "none",
  filter: "blur(1px)",
};
const expected = Object.entries(valid)
  .map(([key, value]) => `${key}:${value};`)
  .join("");
const ssr = (value: unknown) => renderToString(h("div", { style: value as string }));

describe("literal styles", () => {
  it("uses the same serialized text for SSR and client", () => {
    expect(styleText(valid)).toBe(expected);
    expect(ssr(valid)).toBe(`<div style="${expected}"></div>`);
    expect(styleText({ left: null, color: undefined })).toBe("");
    expect(styleText({ "--X": 2 })).toBe("--X:2;");
    expect(ssr("left:10px;")).toBe('<div style="left:10px;"></div>');
  });

  it.each([
    [{ left: 10 }, /left.*explicit units/],
    [{ "-webkit-transform": 2 }, /-webkit-transform.*explicit units/],
    [{ filter: 1 }, /filter.*explicit units/],
    [{ opacity: Infinity }, /opacity.*finite number/],
    [{ "--x": NaN }, /--x.*finite number/],
    [{ left: true }, /left.*finite number/],
    [{ fontSize: "1rem" }, /unsupported property fontSize/],
    [{ Color: "red" }, /unsupported property Color/],
    [{ "invented-property": "x" }, /unsupported property invented-property/],
    [[], /plain object/],
    [Object.create(null), /plain object/],
    [
      new (class Style {
        left = 0;
      })(),
      /plain object/,
    ],
  ])("rejects invalid style %# across renderers", (value, message) => {
    expect(() => styleText(value)).toThrow(message);
    expect(() => ssr(value)).toThrow(message);
  });

  it("validates styles introduced by reactive updates", async () => {
    document.body.replaceChildren();
    const style = signal<unknown>({ left: 0 });
    const diagnostics: Diagnostic[] = [];
    function Demo() {
      return h("div", { style: style as never });
    }
    const identity = { component: "Demo", build: "b1" };
    const host = document.createElement("div");
    host.innerHTML = renderToString(islandRoot(h(Demo, {}), { identity }));
    document.body.append(host);
    const container = host.firstElementChild!;
    const element = container.querySelector("div")!;
    const handle = hydrate(h(Demo, {}), container, {
      identity,
      report: (item) => diagnostics.push(item),
    })!;
    style.value = { opacity: 0.5, "--space": 2 };
    await flush();
    expect(element.getAttribute("style")).toBe("opacity:0.5;--space:2;");
    style.value = { left: 10 };
    await flush();
    expect(diagnostics.at(-1)?.code).toBe("ZR_SUBSCRIBER_ERROR");
    expect(diagnostics.at(-1)?.actual).toContain("ZR_STYLE: nonzero numeric left");
    expect(element.getAttribute("style")).toBe("opacity:0.5;--space:2;");
    style.value = new (class Style {
      left = 0;
    })();
    await flush();
    expect(diagnostics.at(-1)?.actual).toContain(
      "ZR_STYLE: style must be a string or plain object",
    );
    expect(element.getAttribute("style")).toBe("opacity:0.5;--space:2;");
    handle.dispose();
  });
});
