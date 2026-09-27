import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { transformWasmBindgenGlueToFactory } from "../scripts/build.mjs";

const FACTORY_ARTIFACTS = [
  {
    capability: "parseToAst" as const,
    dirName: "wasm-parse",
    stem: "zfb_md_wasm_parse",
  },
  {
    capability: "highlightCode" as const,
    dirName: "wasm-highlight",
    stem: "zfb_md_wasm_highlight",
  },
];

type FactoryGlue = {
  initSync(input: { module: WebAssembly.Module }): unknown;
  parseToAst?(source: string, optionsJson: string): string;
  highlightCode?(code: string, optionsJson: string): string;
  version(): string;
  __forceTrapForTests(): void;
};

function factoryFixture(capability: "parseToAst" | "highlightCode") {
  return `let wasm;
function initSync(module) {
  const instance = new WebAssembly.Instance(module, {});
  wasm = instance.exports;
  if (wasm.__wbindgen_start !== undefined) wasm.__wbindgen_start();
  return wasm;
}
export function ${capability}(input, options) { return input + options; }
export function version() { return "fixture"; }
export function __forceTrapForTests() { throw new WebAssembly.RuntimeError("fixture trap"); }
async function __wbg_load(module, imports) {
  return await WebAssembly.instantiate(await module.arrayBuffer(), imports);
}
async function __wbg_init(moduleOrPath) {
  const url = import.meta.url;
  await fetch(url);
  return await import(url);
}
export { initSync, __wbg_init as default };
`;
}

describe("wasm-bindgen factory transform", () => {
  it.each(FACTORY_ARTIFACTS)(
    "matches the emitted factory for real $capability glue",
    (artifact) => {
      const source = readFileSync(
        new URL(
          `../src/${artifact.dirName}/${artifact.stem}_glue.zfb-resource.mjs`,
          import.meta.url,
        ),
        "utf8",
      );
      const actual = transformWasmBindgenGlueToFactory(source, artifact.capability);

      expect(actual.javascript).toBe(
        readFileSync(
          new URL(
            `../src/${artifact.dirName}/${artifact.stem}_glue.zfb-factory.mjs`,
            import.meta.url,
          ),
          "utf8",
        ),
      );
      expect(actual.declaration).toBe(
        readFileSync(
          new URL(
            `../src/${artifact.dirName}/${artifact.stem}_glue.zfb-factory.d.mts`,
            import.meta.url,
          ),
          "utf8",
        ),
      );
    },
  );

  it("fails closed when wasm-bindgen changes the loader function shape", () => {
    const unexpected = factoryFixture("parseToAst").replace(
      "async function __wbg_load(",
      "async function __wbg_load_v2(",
    );
    expect(() => transformWasmBindgenGlueToFactory(unexpected, "parseToAst")).toThrow(
      /expected one function declaration for __wbg_load; found 0/,
    );
  });

  it("fails closed when a forbidden import.meta remains outside the loader", () => {
    const leftover = factoryFixture("parseToAst").replace(
      "let wasm;",
      "let wasm;\nconst resourceBase = import.meta.url;",
    );
    expect(() => transformWasmBindgenGlueToFactory(leftover, "parseToAst")).toThrow(
      /forbidden token import\.meta/,
    );
  });

  it("fails closed when another export statement remains", () => {
    const extraExport = factoryFixture("parseToAst").replace(
      "export function parseToAst(input, options)",
      "export const surprise = true;\nexport function parseToAst(input, options)",
    );
    expect(() => transformWasmBindgenGlueToFactory(extraExport, "parseToAst")).toThrow(
      /left a residual export/,
    );
  });
});

describe("built Workerd factories", () => {
  it.each(FACTORY_ARTIFACTS)(
    "keeps $capability state separate across instances of one compiled module",
    async (artifact) => {
      const wasm = new WebAssembly.Module(
        readFileSync(
          new URL(`../dist/${artifact.dirName}/${artifact.stem}_bg.wasm`, import.meta.url),
        ),
      );
      const factory = (await import(
        new URL(
          `../dist/${artifact.dirName}/${artifact.stem}_glue.zfb-factory.mjs`,
          import.meta.url,
        ).href
      )) as { createGlue(): FactoryGlue };
      const first = factory.createGlue();
      const second = factory.createGlue();
      first.initSync({ module: wasm });
      second.initSync({ module: wasm });

      const call = (glue: FactoryGlue, token: string) => {
        const json =
          artifact.capability === "parseToAst"
            ? glue.parseToAst!(`# ${token}\n`, "{}")
            : glue.highlightCode!(`const ${token} = 1;`, '{"language":"javascript"}');
        return JSON.parse(json) as { ast?: unknown; html?: string | null; diagnostics?: unknown[] };
      };

      expect(JSON.stringify(call(first, "firstInstance"))).toContain("firstInstance");
      expect(JSON.stringify(call(second, "secondInstance"))).toContain("secondInstance");
      expect(JSON.stringify(call(first, "firstAgain"))).toContain("firstAgain");

      expect(() => first.__forceTrapForTests()).toThrow(WebAssembly.RuntimeError);
      expect(JSON.stringify(call(second, "secondAfterTrap"))).toContain("secondAfterTrap");
    },
  );
});
