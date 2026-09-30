import { describe, expect, it } from "vitest";
import { renderToString } from "../../zudo-react/server.js";

type GoldenModule = {
  default: (props: Record<string, never>) => Parameters<typeof renderToString>[0];
};

const goldens = import.meta.glob<GoldenModule>("./fixtures/md-roundtrip/*.mjs", { eager: true });
const cases = Object.entries(goldens)
  .map(([path, module]) => ({
    name: path.slice(path.lastIndexOf("/") + 1, -".mjs".length),
    render: module.default,
  }))
  .sort((left, right) => left.name.localeCompare(right.name));

describe("md-wasm compiler output rendered by zudo-react", () => {
  it("loads every checked-in roundtrip case", () => {
    expect(cases.map(({ name }) => name)).toEqual([
      "aligned-table",
      "footnote",
      "link-image-heading",
      "ordered-resumed",
      "ordered-start-1",
      "ordered-start-3",
      "task-list",
    ]);
  });

  it.each(cases)("renders $name without a consumer error", ({ name, render }) => {
    const html = renderToString(render({}));
    const container = document.createElement("div");
    container.innerHTML = html;

    if (name === "ordered-start-3") {
      expect(container.querySelector("ol")?.getAttribute("start")).toBe("3");
    }

    if (name === "ordered-resumed") {
      expect(container.querySelectorAll("ol")[1]?.getAttribute("start")).toBe("2");
    }

    if (name === "ordered-start-1") {
      expect(container.querySelector("ol")?.hasAttribute("start")).toBe(false);
    }

    if (name === "task-list") {
      const inputs = Array.from(container.querySelectorAll('input[type="checkbox"]'));
      expect(inputs).toHaveLength(2);
      expect(inputs.map((input) => input.disabled)).toEqual([true, true]);
      expect(inputs.map((input) => input.checked)).toEqual([true, false]);
    }

    if (name === "aligned-table") {
      const cells = Array.from(container.querySelectorAll("th, td"));
      expect(cells.map((cell) => cell.style.textAlign)).toEqual([
        "left",
        "center",
        "right",
        "",
        "left",
        "center",
        "right",
        "",
      ]);
      expect(cells[3]?.hasAttribute("style")).toBe(false);
      expect(cells[7]?.hasAttribute("style")).toBe(false);
    }

    if (name === "footnote") {
      const reference = container.querySelector<HTMLAnchorElement>("a[data-footnote-ref]");
      const definition = container.querySelector<HTMLElement>("li[id^='user-content-fn-']");
      expect(reference?.getAttribute("href")).toBe(`#${definition?.id}`);
    }

    if (name === "link-image-heading") {
      expect(container.querySelector("a[title]")?.getAttribute("title")).toBe("t");
      expect(container.querySelector("img[title]")?.getAttribute("title")).toBe("t");
      const heading = container.querySelector("h1");
      expect(heading?.textContent).toBe("Title");
      if (heading?.hasAttribute("id")) expect(heading.id).toBe("title");
    }
  });
});
