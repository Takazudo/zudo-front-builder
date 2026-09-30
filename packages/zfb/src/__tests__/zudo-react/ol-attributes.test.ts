import { beforeEach, describe, expect, it } from "vitest";
import { h } from "../../zudo-react/index.js";
import { hydrate, mount } from "../../zudo-react/client.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";
import type { Diagnostic } from "../../zudo-react/index.js";

const identity = { component: "Demo", build: "b1" };
let diagnostics: Diagnostic[];

function Demo({ attributes }: { attributes: Record<string, unknown> }) {
  return h("ol", attributes);
}

function list(attributes: Record<string, unknown> = {}) {
  return h(Demo, { attributes });
}

function server(attributes: Record<string, unknown>): Element {
  const host = document.createElement("div");
  document.body.append(host);
  host.innerHTML = renderToString(islandRoot(list(attributes), { identity }));
  return host.firstElementChild!;
}

const options = () => ({ identity, report: (item: Diagnostic) => diagnostics.push(item) });

beforeEach(() => {
  document.body.replaceChildren();
  diagnostics = [];
});

describe("ordered list attributes", () => {
  it("renders numeric and string start values and boolean reversed values on SSR", () => {
    expect(renderToString(h("ol", { start: 3 }))).toBe('<ol start="3"></ol>');
    expect(renderToString(h("ol", { start: "3" }))).toBe('<ol start="3"></ol>');
    expect(renderToString(h("ol", { reversed: true }))).toBe("<ol reversed></ol>");
    expect(renderToString(h("ol", { reversed: false }))).toBe("<ol></ol>");
  });

  it("rejects a boolean start value on SSR", () => {
    expect(() => renderToString(h("ol", { start: false }))).toThrow("ZR_ATTRIBUTE");
  });

  it.each([
    ["numeric start", { start: 3 }, "3", null],
    ["string start", { start: "3" }, "3", null],
    ["reversed true", { reversed: true }, null, ""],
    ["reversed false", { reversed: false }, null, null],
  ])("mounts and hydrates %s with no diagnostics", (_label, attributes, start, reversed) => {
    for (const attach of [hydrate, mount]) {
      const container = server(attributes);
      const handle = attach(list(attributes), container, options());
      const orderedList = container.querySelector("ol");

      expect(handle).not.toBeNull();
      expect(orderedList?.getAttribute("start")).toBe(start);
      expect(orderedList?.getAttribute("reversed")).toBe(reversed);
      expect(diagnostics).toEqual([]);
      handle?.dispose();
      diagnostics = [];
    }
  });

  it.each([hydrate, mount])("rejects boolean start during client setup", (attach) => {
    const container = server({});
    const handle = attach(list({ start: false }), container, options());

    expect(handle).toBeNull();
    expect(diagnostics).toHaveLength(1);
    expect(diagnostics[0]?.code).toBe("ZR_ATTRIBUTE");
  });
});
