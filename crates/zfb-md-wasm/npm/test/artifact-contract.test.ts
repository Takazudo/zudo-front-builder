import { mkdtempSync, rmSync, truncateSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vite-plus/test";

import { ARTIFACTS, buildShippedArtifactsManifest } from "../scripts/build.mjs";
import {
  MAX_PACKED_BYTES,
  REQUIRED_PACKED_FILES,
  WASM_RESOURCE_SETS,
  assertPackedArchive,
  assertPackedContents,
} from "../scripts/assert-packed.mjs";
import packageJson from "../package.json";
import { assertShippedArtifactsManifest } from "../../../../scripts/assert-zfb-md-wasm-release.mjs";
import * as parseEntry from "../src/parse.js";
import * as renderEntry from "../src/render.js";
import { createWasmApi } from "../src/runtime.js";

const temporaryDirectories: string[] = [];
const fixtureBytes = [
  new Uint8Array([0x61, 0x62, 0x63]),
  new TextEncoder().encode("highlight fixture bytes"),
  new TextEncoder().encode("render fixture bytes"),
  new TextEncoder().encode("parse fixture bytes"),
];
const fixturePaths = ARTIFACTS.map(({ entry, dirName, outName }, index) => ({
  entry,
  path: `dist/${dirName}/${outName}_bg.wasm`,
  bytes: fixtureBytes[index],
}));
const fixtureBytesByPath = new Map(fixturePaths.map(({ path, bytes }) => [path, bytes]));

function fixtureManifest() {
  return buildShippedArtifactsManifest({
    name: packageJson.name,
    version: packageJson.version,
    entries: fixturePaths,
  });
}

function fixtureReader(path: string) {
  const bytes = fixtureBytesByPath.get(path);
  if (!bytes) throw new Error(`missing fixture bytes for ${path}`);
  return bytes;
}

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

  it("exports the generated artifact manifest from source and published packages", () => {
    expect(packageJson.exports["./shipped-artifacts.json"]).toBe("./dist/shipped-artifacts.json");
    expect(packageJson.publishConfig.exports["./shipped-artifacts.json"]).toBe(
      "./dist/shipped-artifacts.json",
    );
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

describe("shipped artifact manifest", () => {
  it("hashes four distinct byte fixtures and validates the schema and bytes", () => {
    expect(new Set(fixtureBytes.map((bytes) => Buffer.from(bytes).toString("hex"))).size).toBe(4);

    const manifest = fixtureManifest();
    expect(manifest).toMatchObject({
      schemaVersion: 1,
      name: "@takazudo/zfb-md-wasm",
      version: packageJson.version,
    });
    expect(manifest.artifacts.map(({ entry, path, bytes }) => ({ entry, path, bytes }))).toEqual(
      fixturePaths.map(({ entry, path, bytes }) => ({ entry, path, bytes: bytes.byteLength })),
    );
    // Independent SHA-256 known answer for the first fixture ("abc").
    expect(manifest.artifacts[0].sha256).toBe(
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
    expect(() =>
      assertShippedArtifactsManifest(manifest, packageJson, fixtureReader, "fixture manifest"),
    ).not.toThrow();
  });

  it("rejects same-size byte corruption, wrong metadata, and duplicate or extra entries", () => {
    const manifest = fixtureManifest();
    const firstPath = manifest.artifacts[0].path;
    const originalBytes = fixtureBytesByPath.get(firstPath);
    if (!originalBytes) throw new Error(`missing fixture bytes for ${firstPath}`);
    const corruptedBytesByPath = new Map(fixtureBytesByPath);
    const corrupted = new Uint8Array(originalBytes);
    corrupted[0] ^= 0xff;
    corruptedBytesByPath.set(firstPath, corrupted);
    expect(corrupted.byteLength).toBe(originalBytes.byteLength);
    expect(() =>
      assertShippedArtifactsManifest(
        manifest,
        packageJson,
        (path) => corruptedBytesByPath.get(path) ?? new Uint8Array(),
        "corrupt fixture",
      ),
    ).toThrow(/sha256 does not match/);

    expect(() =>
      assertShippedArtifactsManifest(
        { ...manifest, version: "0.0.0" },
        packageJson,
        fixtureReader,
        "wrong-version fixture",
      ),
    ).toThrow(/version/);
    const wrongPath = fixtureManifest();
    wrongPath.artifacts[1].path = wrongPath.artifacts[0].path;
    expect(() =>
      assertShippedArtifactsManifest(wrongPath, packageJson, fixtureReader, "wrong-path fixture"),
    ).toThrow(/duplicate artifact path|expected/);
    const duplicateEntry = fixtureManifest();
    duplicateEntry.artifacts[1].entry = duplicateEntry.artifacts[0].entry;
    expect(() =>
      assertShippedArtifactsManifest(
        duplicateEntry,
        packageJson,
        fixtureReader,
        "duplicate fixture",
      ),
    ).toThrow(/duplicate artifact entry/);
    const missingEntry = fixtureManifest();
    missingEntry.artifacts.pop();
    expect(() =>
      assertShippedArtifactsManifest(missingEntry, packageJson, fixtureReader, "missing fixture"),
    ).toThrow(/exactly 4 artifacts/);
    const extraEntry = fixtureManifest();
    extraEntry.artifacts.push({ ...extraEntry.artifacts[0] });
    expect(() =>
      assertShippedArtifactsManifest(extraEntry, packageJson, fixtureReader, "extra fixture"),
    ).toThrow(/exactly 4 artifacts/);
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
      assertPackedContents([...REQUIRED_PACKED_FILES, "package/dist/shipped-artifacts.json"]),
    ).toThrow(/duplicate files/);
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
