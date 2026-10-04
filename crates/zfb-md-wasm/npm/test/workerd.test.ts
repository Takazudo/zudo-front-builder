import { execFileSync } from "node:child_process";
import {
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { build } from "esbuild";
import { Miniflare } from "miniflare";
import { afterAll, describe, expect, it } from "vite-plus/test";

import { assertPackedArchive } from "../scripts/assert-packed.mjs";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const tempRoot = mkdtempSync(join(tmpdir(), "zfb-md-wasm-workerd-"));
let miniflare: Miniflare | undefined;

afterAll(async () => {
  await miniflare?.dispose();
  rmSync(tempRoot, { recursive: true, force: true });
});

function unpackPackage(name: string): string {
  const root = join(tempRoot, name);
  const packDir = join(root, "pack");
  mkdirSync(packDir, { recursive: true });
  execFileSync("pnpm", ["pack", "--pack-destination", packDir], { cwd: packageRoot });
  const archive = join(packDir, readdirSync(packDir).find((name) => name.endsWith(".tgz"))!);
  assertPackedArchive(archive);
  const unpacked = join(root, "unpacked");
  mkdirSync(unpacked);
  execFileSync("tar", ["-xzf", archive, "-C", unpacked]);
  const fixture = join(root, "fixture");
  const installed = join(fixture, "node_modules", "@takazudo", "zfb-md-wasm");
  cpSync(join(unpacked, "package"), installed, { recursive: true });
  return fixture;
}

const workerSource = `
import * as parse from "@takazudo/zfb-md-wasm/parse";
import * as highlight from "@takazudo/zfb-md-wasm/highlight";
export default {
  async fetch(request) {
    try {
      const command = new URL(request.url).pathname;
      if (command === "/parse") return Response.json(await parse.parseToAst("| A | B |\\n| --- | --- |\\n| one | two |"));
      if (command === "/mdast") {
        const result = await parse.parseToAst("| A | B |\\n| --- | --- |\\n| one | two |");
        return Response.json(parse.toMdastRoot(result.ast));
      }
      if (command === "/highlight") return Response.json(await highlight.highlightCode("const x = 1;", { language: "javascript" }));
      if (command === "/versions") return Response.json([await parse.version(), await highlight.version()]);
      if (command === "/surfaces") return Response.json([Object.keys(parse).sort(), Object.keys(highlight).sort()]);
      if (command === "/parse-trap" || command === "/highlight-trap") {
        const api = command === "/parse-trap" ? parse : highlight;
        try { await api.__forceTrapForTests(); }
        catch (error) { return Response.json({ name: error.name, isTrap: error instanceof api.ZfbMdWasmTrapError }); }
        throw new Error("forced trap did not throw");
      }
      throw new Error("unknown command");
    } catch (error) { return Response.json({ error: String(error), stack: error.stack }, { status: 500 }); }
  }
};
`;

describe("packed workerd entries", () => {
  it("needs no Wrangler module rules for a packed consumer", () => {
    const fixture = unpackPackage("wrangler");
    writeFileSync(
      join(fixture, "worker.mjs"),
      `import { parseToAst } from "@takazudo/zfb-md-wasm/parse";
import { highlightCode } from "@takazudo/zfb-md-wasm/highlight";
export default { async fetch() {
  const parsed = await parseToAst("# Hi");
  const highlighted = await highlightCode("const x = 1", { language: "javascript" });
  return Response.json({ parsed, highlighted });
} };`,
    );
    writeFileSync(
      join(fixture, "wrangler.jsonc"),
      JSON.stringify({
        name: "zfb-md-wasm-workerd-proof",
        main: "worker.mjs",
        compatibility_date: "2026-05-01",
      }),
    );
    const outDir = join(fixture, "out");
    const wrangler = resolve(packageRoot, "../../../node_modules/.bin/wrangler");
    const output = execFileSync(wrangler, ["deploy", "--dry-run", "--outdir", outDir], {
      cwd: fixture,
      encoding: "utf8",
    });
    expect(output).toContain("--dry-run: exiting now.");
    const wasmFiles = readdirSync(outDir).filter((name) => name.endsWith(".wasm"));
    expect(wasmFiles).toHaveLength(2);
    const bundle = readFileSync(join(outDir, "worker.js"), "utf8");
    for (const wasm of wasmFiles) {
      expect(bundle).toContain(`from "./${wasm}"`);
    }
  }, 30_000);

  // Local packed bundle + real Miniflare run measured at 1.9 s on a local run; 30 s gives CI startup room.
  it("bundles both compiled WASM modules and recovers isolated traps under workerd", async () => {
    const fixture = unpackPackage("miniflare");
    const entry = join(fixture, "worker.mjs");
    const outDir = join(fixture, "out");
    writeFileSync(entry, workerSource);
    const result = await build({
      entryPoints: [entry],
      outdir: outDir,
      bundle: true,
      platform: "browser",
      format: "esm",
      conditions: ["workerd", "worker"],
      loader: { ".wasm": "copy" },
      metafile: true,
    });
    const bundle = join(outDir, "worker.js");
    const text = readFileSync(bundle, "utf8");
    for (const forbidden of ["?url", "WebAssembly.compile", "node:", "import("]) {
      expect(text).not.toContain(forbidden);
    }
    const inputs = Object.keys(result.metafile!.inputs);
    expect(inputs.some((path) => path.endsWith("/dist/parse-workerd.js"))).toBe(true);
    expect(inputs.some((path) => path.endsWith("/dist/highlight-workerd.js"))).toBe(true);
    expect(inputs.filter((path) => path.endsWith("_bg.wasm"))).toHaveLength(2);
    const wasmFiles = readdirSync(outDir).filter((name) => name.endsWith(".wasm"));
    expect(wasmFiles).toHaveLength(2);
    for (const wasm of wasmFiles) expect(text).toContain(`./${wasm}`);

    miniflare = new Miniflare({
      modules: true,
      scriptPath: bundle,
      modulesRoot: outDir,
      modulesRules: [{ type: "CompiledWasm", include: ["**/*.wasm"] }],
      compatibilityDate: "2026-05-01",
    });
    async function request(path: string) {
      const response = await miniflare!.dispatchFetch(`http://localhost${path}`);
      const data = await response.json();
      expect(response.status, JSON.stringify(data)).toBe(200);
      return data;
    }

    expect(await request("/parse")).toMatchObject({
      ast: { type: "root", children: [{ type: "table" }] },
    });
    expect(await request("/mdast")).toMatchObject({ type: "root", children: [{ type: "table" }] });
    expect(await request("/highlight")).toMatchObject({
      diagnostics: [],
      html: expect.stringContaining("const"),
    });
    expect(await request("/surfaces")).toEqual([
      [
        "MdastAdapterError",
        "ZfbMdWasmTrapError",
        "ZfbMdWasmTrapRecoveryLimitError",
        "__forceTrapForTests",
        "__getTrapRecoveryStateForTests",
        "init",
        "parseToAst",
        "toMdastRoot",
        "version",
      ].sort(),
      [
        "ZfbMdWasmTrapError",
        "ZfbMdWasmTrapRecoveryLimitError",
        "__forceTrapForTests",
        "__getTrapRecoveryStateForTests",
        "highlightCode",
        "init",
        "version",
      ].sort(),
    ]);
    expect(await request("/versions")).toEqual([
      expect.stringMatching(/^\d+\.\d+\.\d+/),
      expect.stringMatching(/^\d+\.\d+\.\d+/),
    ]);
    expect(await request("/parse-trap")).toEqual({ name: "ZfbMdWasmTrapError", isTrap: true });
    // A parse trap does not disturb the highlight instance.
    expect(await request("/highlight")).toMatchObject({ diagnostics: [] });
    expect(await request("/parse")).toMatchObject({ ast: { type: "root" } });
    expect(await request("/highlight-trap")).toEqual({ name: "ZfbMdWasmTrapError", isTrap: true });
    expect(await request("/highlight")).toMatchObject({ diagnostics: [] });
  }, 30_000);
});
