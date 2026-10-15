import { describe, expect, it } from "vite-plus/test";
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
      "explicit-table-compact",
      "explicit-table-sections",
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

    if (name === "explicit-table-compact") {
      // Check raw server output as well as the parsed DOM: HTML parsers
      // repair p > table by moving the table and inserting empty paragraphs.
      expect(html).not.toMatch(/<p[^>]*>\s*<table/);
      expect(Array.from(container.children).map((child) => child.tagName)).toEqual([
        "TABLE",
        "P",
        "TABLE",
        "P",
      ]);
      expect(container.querySelector("table > thead > tr > th")?.textContent).toBe("A");
      expect(container.querySelector("table > tbody > tr > td")?.textContent).toBe("B");
      expect(container.querySelector("table.compact td")?.textContent).toBe("C");
      expect(Array.from(container.querySelectorAll("p")).map((p) => p.textContent?.trim())).toEqual(
        ["Before bold inline.", "After."],
      );
      expect(container.querySelector("p > strong")?.textContent).toBe("bold");
      expect(container.querySelector("p > span")?.textContent).toBe("inline");
    }

    if (name === "explicit-table-sections") {
      // The renderer throws ZR_PARSER_CONTEXT on `table > p`, and the DOM
      // parser would foster-parent a stray `p` out of the table, so count
      // every paragraph: two of prose plus the one cell paragraph.
      const paragraphs = Array.from(container.querySelectorAll("p"));
      expect(paragraphs.map((p) => p.textContent?.trim())).toEqual([
        "Before the table.",
        "Cell paragraph.",
        "After the table.",
      ]);
      expect(paragraphs[1]?.parentElement?.tagName).toBe("TD");
      expect(
        container.querySelectorAll(
          "table > p, thead > p, tbody > p, tfoot > p, tr > p, colgroup > p",
        ),
      ).toHaveLength(0);
      expect(container.querySelector("table > caption")?.textContent).toBe("Scores");
      expect(container.querySelectorAll("table > colgroup > col")).toHaveLength(2);
      const texts = (selector: string) =>
        Array.from(container.querySelectorAll(selector)).map((cell) => cell.textContent?.trim());
      expect(texts("table > thead > tr > th")).toEqual(["Name", "Score"]);
      expect(texts("table > tbody > tr > td")).toEqual([
        "Ann",
        "10",
        "Bob",
        "9",
        "Cy",
        "Cell paragraph.",
      ]);
      expect(texts("table > tfoot > tr > td")).toEqual(["Total", "19"]);
      expect(container.querySelector("tbody td > strong")?.textContent).toBe("10");
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
