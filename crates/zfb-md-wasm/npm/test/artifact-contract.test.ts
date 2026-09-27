import { mkdtempSync, rmSync, truncateSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { ARTIFACTS } from "../scripts/build.mjs";
import {
  MAX_PACKED_BYTES,
  REQUIRED_PACKED_FILES,
  WASM_RESOURCE_SETS,
  assertPackedArchive,
  assertPackedContents,
} from "../scripts/assert-packed.mjs";
import packageJson from "../package.json";
import * as parseEntry from "../src/parse.js";
import * as renderEntry from "../src/render.js";
import { createWasmApi } from "../src/runtime.js";

const temporaryDirectories: string[] = [];

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

describe("slim artifact descriptors", () => {
  it("locks the Cargo feature matrix and Workerd factory artifacts", () => {
    expect(
      ARTIFACTS.map(({ entry, cargoFeatureArgs, dirName, outName, workerd, gzipCeiling }) => ({
        entry,
        cargoFeatureArgs,
        dirName,
        outName,
        workerd: workerd === true,
        gzipCeiling,
      })),
    ).toEqual([
      {
        entry: ".",
        cargoFeatureArgs: [],
        dirName: "wasm",
        outName: "zfb_md_wasm",
        workerd: false,
        gzipCeiling: 1_600_000,
      },
      {
        entry: "./highlight",
        cargoFeatureArgs: ["--no-default-features", "--features", "highlight"],
        dirName: "wasm-highlight",
        outName: "zfb_md_wasm_highlight",
        workerd: true,
        gzipCeiling: 880_000,
      },
      {
        entry: "./render",
        cargoFeatureArgs: ["--no-default-features", "--features", "render"],
        dirName: "wasm-render",
        outName: "zfb_md_wasm_render",
        workerd: false,
        gzipCeiling: 1_100_000,
      },
      {
        entry: "./parse",
        cargoFeatureArgs: ["--no-default-features", "--features", "parse"],
        dirName: "wasm-parse",
        outName: "zfb_md_wasm_parse",
        workerd: true,
        gzipCeiling: 325_000,
      },
    ]);
  });

  it("publishes matching source and dist conditional exports", () => {
    for (const entry of ["./parse", "./highlight"] as const) {
      expect(Object.keys(packageJson.exports[entry])).toEqual([
        "types",
        "workerd",
        "browser",
        "default",
      ]);
      expect(Object.keys(packageJson.publishConfig.exports[entry])).toEqual([
        "types",
        "workerd",
        "browser",
        "default",
      ]);
    }
    expect(packageJson.exports["./highlight"]).toEqual({
      types: "./src/highlight.ts",
      workerd: "./src/highlight-workerd.ts",
      browser: "./src/highlight-browser.ts",
      default: "./src/highlight.ts",
    });
    expect(packageJson.exports["./render"]).toEqual({
      types: "./src/render.ts",
      browser: "./src/render-browser.ts",
      default: "./src/render.ts",
    });
    expect(packageJson.exports["./parse"]).toEqual({
      types: "./src/parse.ts",
      workerd: "./src/parse-workerd.ts",
      browser: "./src/parse-browser.ts",
      default: "./src/parse.ts",
    });
    expect(packageJson.publishConfig.exports["./highlight"]).toEqual({
      types: "./dist/highlight.d.ts",
      workerd: "./dist/highlight-workerd.js",
      browser: "./dist/highlight-browser.js",
      default: "./dist/highlight.js",
    });
    expect(packageJson.publishConfig.exports["./render"]).toEqual({
      types: "./dist/render.d.ts",
      browser: "./dist/render-browser.js",
      default: "./dist/render.js",
    });
    expect(packageJson.publishConfig.exports["./parse"]).toEqual({
      types: "./dist/parse.d.ts",
      workerd: "./dist/parse-workerd.js",
      browser: "./dist/parse-browser.js",
      default: "./dist/parse.js",
    });
  });

  it("keeps slim source value surfaces closed", () => {
    expect(Object.keys(renderEntry).sort()).toEqual([
      "ZfbMdWasmTrapError",
      "ZfbMdWasmTrapRecoveryLimitError",
      "__forceTrapForTests",
      "__getTrapRecoveryStateForTests",
      "init",
      "renderHtml",
      "version",
    ]);
    expect(Object.keys(parseEntry).sort()).toEqual([
      "MdastAdapterError",
      "ZfbMdWasmTrapError",
      "ZfbMdWasmTrapRecoveryLimitError",
      "__forceTrapForTests",
      "__getTrapRecoveryStateForTests",
      "init",
      "parseToAst",
      "toMdastRoot",
      "version",
    ]);
  });
});

describe("closed packed layout", () => {
  it("describes the closed four-file and six-file resource sets", () => {
    expect(WASM_RESOURCE_SETS).toHaveLength(4);
    expect(WASM_RESOURCE_SETS.map((set) => [set.dirName, set.requiredFiles.size])).toEqual([
      ["wasm", 4],
      ["wasm-highlight", 6],
      ["wasm-render", 4],
      ["wasm-parse", 6],
    ]);
    expect(() => assertPackedContents(REQUIRED_PACKED_FILES)).not.toThrow();
  });

  it("fails closed for a missing file, an extra sidecar, and stray resources", () => {
    expect(() => assertPackedContents(REQUIRED_PACKED_FILES.slice(1))).toThrow(/missing/);
    expect(() =>
      assertPackedContents([
        ...REQUIRED_PACKED_FILES,
        "package/dist/wasm-render/zfb_md_wasm_render_bg.extra",
      ]),
    ).toThrow(/unexpected resources/);
    expect(() =>
      assertPackedContents([...REQUIRED_PACKED_FILES, "package/dist/copied_bg.wasm"]),
    ).toThrow(/unapproved runtime resources/);
    expect(() =>
      assertPackedContents([...REQUIRED_PACKED_FILES, "package/dist/copied_glue.zfb-factory.mjs"]),
    ).toThrow(/unapproved runtime resources/);
    expect(() =>
      assertPackedContents(
        REQUIRED_PACKED_FILES.filter((path) => !path.endsWith("_glue.zfb-factory.mjs")),
      ),
    ).toThrow(/missing/);
  });

  it("rejects an oversized archive before attempting to inspect it", () => {
    const directory = mkdtempSync(join(tmpdir(), "zfb-md-wasm-archive-limit-"));
    temporaryDirectories.push(directory);
    const archive = join(directory, "oversized.tgz");
    writeFileSync(archive, "");
    truncateSync(archive, MAX_PACKED_BYTES + 1);
    expect(() => assertPackedArchive(archive)).toThrow(/ceiling/);
  });
});

describe("runtime capability isolation", () => {
  it("round-trips representative render and parse calls with independent state", async () => {
    const makeApi = (capability: "render" | "parse") =>
      createWasmApi({
        glueUrl: new URL(`https://example.test/${capability}.mjs`),
        loadWasmBytes: async () => new ArrayBuffer(0),
        compileWasm: async () => ({}) as WebAssembly.Module,
        importGlue: async () => ({
          initSync() {},
          ...(capability === "render"
            ? {
                renderHtml: () =>
                  JSON.stringify({ html: "<p>render</p>", frontmatter: null, diagnostics: [] }),
              }
            : {
                parseToAst: () =>
                  JSON.stringify({ ast: { type: "root", children: [] }, diagnostics: [] }),
              }),
          version: () => "fixture",
          __forceTrapForTests() {
            if (capability === "render") {
              throw new WebAssembly.RuntimeError("fixture trap");
            }
          },
        }),
      });

    const renderApi = makeApi("render");
    const parseApi = makeApi("parse");
    await expect(renderApi.renderHtml("render")).resolves.toMatchObject({ html: "<p>render</p>" });
    await expect(parseApi.parseToAst("parse")).resolves.toMatchObject({
      ast: { type: "root", children: [] },
    });
    expect(renderApi.__getTrapRecoveryStateForTests().compiledModuleLoads).toBe(1);
    expect(parseApi.__getTrapRecoveryStateForTests().compiledModuleLoads).toBe(1);
    await expect(renderApi.__forceTrapForTests()).rejects.toThrow("automatically re-instantiated");
    expect(renderApi.__getTrapRecoveryStateForTests().currentGeneration).toBe(1);
    expect(parseApi.__getTrapRecoveryStateForTests().currentGeneration).toBe(0);
  });

  it("reports structural glue mismatches without naming another artifact", async () => {
    const api = createWasmApi({
      glueUrl: new URL("https://example.test/parse.mjs"),
      loadWasmBytes: async () => new ArrayBuffer(0),
      compileWasm: async () => ({}) as WebAssembly.Module,
      importGlue: async () => ({
        initSync() {},
        parseToAst: () => JSON.stringify({ ast: null, diagnostics: [] }),
        version: () => "fixture",
        __forceTrapForTests() {},
      }),
    });
    await expect(api.renderHtml("wrong artifact")).rejects.toThrow(
      "renderHtml() is not available in this wasm artifact",
    );
  });
});
