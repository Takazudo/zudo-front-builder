import { transformSync } from "esbuild";
import { describe, expect, it } from "vite-plus/test";

import * as runtime from "../../../../packages/zfb/src/zudo-react/jsx-runtime.js";
import type { Description } from "../../../../packages/zfb/src/zudo-react/description.js";
import { compile } from "../dist/index.js";

async function execute(source: string, components: Record<string, unknown>): Promise<Description> {
  const out = await compile(source);
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
  ["unspaced", "`x<1.5`", "x<1.5"],
  ["spaced", "`x < 1.5`", "x < 1.5"],
  ["multiple", "`x<1.5; y<2.25; z < 3.75`", "x<1.5; y<2.25; z < 3.75"],
  [
    "multiline JS",
    "`const x = 1;\nif (x<1.5) {\nconsole.log(x<2.25);\n}`",
    "const x = 1;\nif (x<1.5) {\nconsole.log(x<2.25);\n}",
  ],
  [
    "multiline GLSL",
    "`void main() {\nif (uv.x<1.5 && uv.y<2.25) {\ngl_FragColor = vec4(1.0);\n}\n}`",
    "void main() {\nif (uv.x<1.5 && uv.y<2.25) {\ngl_FragColor = vec4(1.0);\n}\n}",
  ],
  ["single quote", "'x<1.5'", "x<1.5"],
  ["double quote", '"x<1.5"', "x<1.5"],
  ["nested object", "({ nested: { js: `x<1.5` } }).nested.js", "x<1.5"],
] as const;

describe("compile decimal comparisons (#4275)", () => {
  for (const [label, expression, expected] of payloads) {
    const element = `<Preview js={${expression}} />`;
    for (const [placement, source] of [
      ["attribute", element],
      ["nested element", `<Note>\n\n${element}\n\n</Note>`],
      ["conditional", `{true ? ${element} : null}`],
    ]) {
      it(`preserves ${label} bytes in ${placement}`, async () => {
        const tree = await execute(source, { Preview, Note });
        expect(find(tree, Preview).props.js).toBe(expected);
      });
    }
  }

  it("resolves member, Unicode and combining heads inside expressions and interpolation", async () => {
    const Anchor = () => null;
    const tree = await execute(
      "<Preview upper={<UI.Group.Anchor />} lower={<ui.Anchor />} unicode={<Δ.Anchor />} combined={<Á.Anchor />} dollar={<UI$Kit.An$chor />} js={`x<1.5 ${<Inner.Anchor />}`} />",
      {
        Preview,
        UI: { Group: { Anchor } },
        ui: { Anchor },
        Δ: { Anchor },
        Á: { Anchor },
        UI$Kit: { An$chor: Anchor },
        Inner: { Anchor },
      },
    );
    const props = find(tree, Preview).props;
    for (const key of ["upper", "lower", "unicode", "combined", "dollar"]) {
      expect((props[key] as Description).type).toBe(Anchor);
    }
    expect(props.js).toBe("x<1.5 [object Object]");
  });
});
