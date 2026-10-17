import { transformSync } from "esbuild";
import { describe, expect, it } from "vite-plus/test";

import * as runtime from "../../../../packages/zfb/src/zudo-react/jsx-runtime.js";
import type { Description } from "../../../../packages/zfb/src/zudo-react/description.js";
import { compile } from "../dist/index.js";

async function execute(
  source: string,
  components: Record<string, unknown>,
  options = {},
): Promise<Description> {
  const out = await compile(source, options);
  expect(out.diagnostics).toEqual([]);
  expect(out.code).toBeTypeOf("string");
  const module = { exports: {} as { default: (props: unknown) => Description } };
  const code = transformSync(out.code!, { format: "cjs" }).code;
  new Function("require", "module", "exports", code)(
    (specifier: string) => {
      expect(specifier).toBe("@takazudo/zfb/zudo-react/jsx-runtime");
      return runtime;
    },
    module,
    module.exports,
  );
  return module.exports.default({ components });
}

function find(root: Description, type: unknown): Description {
  if (root.type === type) return root;
  const children = [root.props.children].flat(Infinity);
  for (const child of children) {
    if (child && typeof child === "object" && "$$zudo" in child) {
      try {
        return find(child as Description, type);
      } catch {
        // Search the remaining siblings.
      }
    }
  }
  throw new Error("Expected component description is missing");
}

const Preview = () => null;
const Note = () => null;

const payloads = [
  [
    "spaces",
    "`const x = 1;\nif (x<1.5) {\n  console.log(x);\n}`",
    "const x = 1;\nif (x<1.5) {\n  console.log(x);\n}",
  ],
  ["tabs", "`a\n\tb\n \t c\n  `", "a\n\tb\n \t c\n  "],
  ["Unicode and interpolation", "`雪\n  café ${1 + 2}\n\t終`", "雪\n  café 3\n\t終"],
  ["blank lines", "`a\n  \n\t\n  z`", "a\n  \n\t\n  z"],
] as const;

describe("compile authored expression whitespace (#4278)", () => {
  for (const [label, expression, expected] of payloads) {
    const element = `<Preview js={${expression}} />`;
    const placements = [
      ["attribute", element],
      ["spread", `<Preview {...{js: ${expression}}} />`],
      ["flow expression", `{true ? ${element} : null}`],
      ["nested JSX", `<Note>\n\n${element}\n\n</Note>`],
    ];
    // Blank lines terminate text expressions in the pinned Markdown grammar.
    if (label !== "blank lines") placements.push(["text expression", `text {${element}} end`]);
    for (const [placement, source] of placements) {
      it(`preserves ${label} in ${placement}`, async () => {
        const tree = await execute(source, { Preview, Note });
        expect(find(tree, Preview).props.js).toBe(expected);
      });
    }
  }

  for (const [label, first, continuation] of [
    ["list", "- ", "  "],
    ["ordered list", "123. ", "     "],
    ["indented list", "  - ", "    "],
    ["tabbed list", "-\t", "\t"],
    ["tabbed nested list", "-\t- ", "\t  "],
    ["quote", "> ", "> "],
    ["list in quote", "> - ", ">   "],
    ["quote in list", "- > ", "  > "],
    ["footnote", "[^a]: ", "    "],
    ["footnote in quote", "> [^a]: ", ">     "],
    ["footnote in list", "- [^a]: ", "      "],
  ]) {
    it(`removes only structural ${label} indentation`, async () => {
      const element = "<Preview js={`雪\n  alpha\n\tbeta\n  \n  ${2 + 3}\n  `} />";
      const source = `${first}${element.replaceAll("\n", `\n${continuation}`)}\n\nreference[^a]`;
      const tree = await execute(
        source,
        { Preview },
        { pipeline: { gfm: { footnoteDefinition: true } } },
      );
      expect(find(tree, Preview).props.js).toBe("雪\n  alpha\n\tbeta\n  \n  5\n  ");
    });
  }

  it("preserves remaining columns of a tab split by a list container", async () => {
    const tree = await execute("- <Preview js={`a\n\tb\n\t`} />", { Preview });
    expect(find(tree, Preview).props.js).toBe("a\n  b\n  ");
  });

  it("preserves whitespace around a nested JSX interpolation", async () => {
    const tree = await execute("<Preview js={`a\n  ${<Note />}\n\tb`} />", { Preview, Note });
    expect(find(tree, Preview).props.js).toBe("a\n  [object Object]\n\tb");
  });
});
