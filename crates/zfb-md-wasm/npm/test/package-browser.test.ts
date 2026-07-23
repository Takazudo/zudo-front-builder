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
import { fileURLToPath, pathToFileURL } from "node:url";
import { afterAll, describe, expect, it } from "vitest";

import { assertPackedContents, packedPaths } from "../scripts/assert-packed.mjs";
import { createWasmApi } from "../src/runtime.js";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const tempRoot = mkdtempSync(join(tmpdir(), "zfb-md-wasm-package-"));
let packedArchive: string | undefined;

function packPackage(): string {
  const packDir = join(tempRoot, "pack");
  execFileSync("pnpm", ["pack", "--pack-destination", packDir], {
    cwd: packageRoot,
    stdio: "pipe",
  });
  const archive = readdirSync(packDir).find((name) => name.endsWith(".tgz"));
  if (!archive) {
    throw new Error("pnpm pack did not create a tarball");
  }
  return join(packDir, archive);
}

function unpackForConsumer(archivePath: string): string {
  const unpackRoot = join(tempRoot, "unpacked");
  mkdirSync(unpackRoot, { recursive: true });
  execFileSync("tar", ["-xzf", archivePath, "-C", unpackRoot], { stdio: "pipe" });

  const fixtureRoot = join(tempRoot, "browser-fixture");
  const packageDestination = join(fixtureRoot, "node_modules", "@takazudo", "zfb-md-wasm");
  cpSync(join(unpackRoot, "package"), packageDestination, { recursive: true });
  return fixtureRoot;
}

function installTarballConsumer(archivePath: string): string {
  const fixtureRoot = join(tempRoot, "declaration-fixture");
  mkdirSync(fixtureRoot, { recursive: true });
  writeFileSync(
    join(fixtureRoot, "package.json"),
    JSON.stringify({
      private: true,
      type: "module",
      dependencies: { "@takazudo/zfb-md-wasm": `file:${archivePath}` },
    }),
  );
  execFileSync("pnpm", ["install", "--ignore-workspace", "--no-frozen-lockfile"], {
    cwd: fixtureRoot,
    stdio: "pipe",
  });
  return fixtureRoot;
}

function esbuildBin(): string {
  return resolve(packageRoot, "node_modules", ".bin", "esbuild");
}

function packedPackage(): string {
  packedArchive ??= packPackage();
  return packedArchive;
}

describe("packed browser conditional entry", () => {
  afterAll(() => {
    rmSync(tempRoot, { recursive: true, force: true });
  });

  it("contains exactly the required generated resources in its tarball", () => {
    const archive = packedPackage();
    assertPackedContents(packedPaths(archive));
  });

  it("composes recovery markers with an emitted glue URL's existing query and hash", async () => {
    const importedSpecifiers: string[] = [];
    const api = createWasmApi({
      glueUrl: new URL("https://cdn.example.test/glue.mjs?asset=packed#module"),
      loadWasmBytes: async () => new ArrayBuffer(0),
      compileWasm: async () => ({}) as WebAssembly.Module,
      importGlue: async (specifier) => {
        importedSpecifiers.push(specifier);
        return {
          initSync() {},
          highlightCode() {
            return JSON.stringify({ html: "", diagnostics: [] });
          },
          version() {
            return "fixture";
          },
          __forceTrapForTests() {},
        };
      },
    });

    await api.init();

    expect(importedSpecifiers).toHaveLength(1);
    const imported = new URL(importedSpecifiers[0]!);
    expect(imported.searchParams.get("asset")).toBe("packed");
    expect(imported.searchParams.get("zfbMdWasmGen")).toBe("0");
    expect(imported.searchParams.get("zfbMdWasmAttempt")).toBe("1");
    expect(imported.hash).toBe("#module");
  });

  it("resolves every public declaration from a tarball-only isolated consumer", () => {
    const fixtureRoot = installTarballConsumer(packedPackage());
    writeFileSync(
      join(fixtureRoot, "consumer.ts"),
      [
        'import { MdastAdapterError, toMdastRoot, type MdastRoot } from "@takazudo/zfb-md-wasm";',
        'import { highlightCode, type HighlightCodeResult } from "@takazudo/zfb-md-wasm/highlight";',
        "declare const raw: MdastRoot;",
        "const root: ReturnType<typeof toMdastRoot> = toMdastRoot(raw);",
        "const error: MdastAdapterError | undefined = undefined;",
        'const highlighted: Promise<HighlightCodeResult> = highlightCode("x", { language: "text" });',
        "void root; void error; void highlighted;",
      ].join("\n"),
    );
    writeFileSync(
      join(fixtureRoot, "tsconfig.json"),
      JSON.stringify({
        compilerOptions: {
          strict: true,
          noEmit: true,
          module: "NodeNext",
          moduleResolution: "NodeNext",
          target: "ES2022",
          skipLibCheck: false,
        },
        include: ["consumer.ts"],
      }),
    );
    execFileSync(resolve(packageRoot, "node_modules", ".bin", "tsc"), ["-p", "tsconfig.json"], {
      cwd: fixtureRoot,
      stdio: "pipe",
    });
  });

  it("selects browser.js and keeps relative glue/wasm resources through generic esbuild", async () => {
    const archive = packedPackage();
    const fixtureRoot = unpackForConsumer(archive);
    const nodeConsumerPath = join(fixtureRoot, "node-consumer.mjs");
    writeFileSync(
      nodeConsumerPath,
      [
        'import { init, toMdastRoot, version } from "@takazudo/zfb-md-wasm";',
        "await init();",
        'if (typeof toMdastRoot !== "function") throw new Error("missing Node adapter export");',
        "console.log(await version());",
      ].join("\n"),
    );
    const nodeVersion = execFileSync(process.execPath, [nodeConsumerPath], {
      cwd: fixtureRoot,
      encoding: "utf8",
    }).trim();
    expect(nodeVersion).toMatch(/^\d+\.\d+\.\d+/);

    const entryPath = join(fixtureRoot, "entry.mjs");
    const outDir = join(fixtureRoot, "out");
    const metafilePath = join(outDir, "meta.json");
    writeFileSync(
      entryPath,
      [
        'export { MdastAdapterError, __forceTrapForTests, __getTrapRecoveryStateForTests, highlightCode, init, parseToAst, toMdastRoot } from "@takazudo/zfb-md-wasm";',
      ].join("\n"),
    );

    execFileSync(
      esbuildBin(),
      [
        entryPath,
        "--bundle",
        "--format=esm",
        "--platform=browser",
        "--outdir=" + outDir,
        "--metafile=" + metafilePath,
        "--loader:.zfb-resource.mjs=file",
        "--loader:.wasm=file",
        "--asset-names=islands-resource-[name]-[hash]",
      ],
      { cwd: fixtureRoot, stdio: "pipe" },
    );

    const metafile = JSON.parse(readFileSync(metafilePath, "utf8")) as {
      inputs: Record<string, unknown>;
    };
    const inputNames = Object.keys(metafile.inputs);
    expect(inputNames.some((path) => path.endsWith("/dist/browser.js"))).toBe(true);
    expect(inputNames.some((path) => path.endsWith("/dist/index.js"))).toBe(false);

    const outputFiles = readdirSync(outDir);
    const resources = outputFiles.filter((name) => name.startsWith("islands-resource-"));
    expect(resources).toHaveLength(2);
    expect(resources.some((name) => name.endsWith(".mjs"))).toBe(true);
    expect(resources.some((name) => name.endsWith(".wasm"))).toBe(true);

    const entryOutput = join(outDir, "entry.js");
    const entryText = readFileSync(entryOutput, "utf8");
    for (const resource of resources) {
      expect(entryText).toContain(`./${resource}`);
    }
    expect(entryText).toContain("zfbMdWasmGen");
    expect(entryText).toContain("zfbMdWasmAttempt");

    // Exercise the emitted browser branch without a browser: its fetch only
    // needs a file: adapter in Node. The dynamic glue import remains exactly
    // the emitted URL with a query, which Node treats as a fresh ESM record.
    const originalFetch = globalThis.fetch;
    let wasmFetchAttempts = 0;
    globalThis.fetch = async (input) => {
      const url = new URL(input instanceof Request ? input.url : input.toString());
      if (url.protocol !== "file:") {
        return originalFetch(input);
      }
      if (url.pathname.endsWith(".wasm")) {
        wasmFetchAttempts += 1;
        if (wasmFetchAttempts === 1) {
          return new Response("temporary wasm outage", {
            status: 503,
            statusText: "Transient Failure",
          });
        }
      }
      return new Response(readFileSync(fileURLToPath(url)), { status: 200 });
    };
    try {
      const bundled = await import(pathToFileURL(entryOutput).href);
      const before = bundled.__getTrapRecoveryStateForTests();
      await expect(bundled.init()).rejects.toThrow("503 Transient Failure");

      const afterFailure = bundled.__getTrapRecoveryStateForTests();
      expect(afterFailure).toMatchObject({
        compiledModuleLoads: before.compiledModuleLoads + 1,
        currentGeneration: before.currentGeneration,
        freshInstanceStarts: before.freshInstanceStarts + 1,
        glueImportAttempts: before.glueImportAttempts + 1,
        terminal: false,
        trapRecoveriesStarted: before.trapRecoveriesStarted,
      });

      await Promise.all([bundled.init(), bundled.init()]);
      const afterRetry = bundled.__getTrapRecoveryStateForTests();
      expect(wasmFetchAttempts).toBe(2);
      expect(afterRetry).toMatchObject({
        compiledModuleLoads: before.compiledModuleLoads + 2,
        currentGeneration: before.currentGeneration,
        freshInstanceStarts: before.freshInstanceStarts + 2,
        glueImportAttempts: before.glueImportAttempts + 2,
        terminal: false,
        trapRecoveriesStarted: before.trapRecoveriesStarted,
      });

      await expect(bundled.__forceTrapForTests()).rejects.toMatchObject({
        name: "ZfbMdWasmTrapError",
      });
      const highlighted = await bundled.highlightCode("const recovered = true;", {
        language: "javascript",
      });
      const after = bundled.__getTrapRecoveryStateForTests();

      expect(highlighted.diagnostics).toEqual([]);

      // parseToAst (zfb#1857) through the SAME bundled browser entry --
      // proves the raw-mdast export survives the esbuild `file`-loader
      // resource graph and glue wiring exercised above, not just the
      // already-covered highlightCode path.
      const parsed = await bundled.parseToAst(":::note[Hi]\nBody :mark[here].\n:::\n", {
        filename: "post.mdx",
        directives: true,
      });
      expect(parsed.diagnostics).toEqual([]);
      expect(parsed.ast).toMatchObject({
        type: "root",
        children: [{ type: "containerDirective", name: "note" }],
      });
      const adapted = bundled.toMdastRoot(parsed.ast);
      expect(adapted).toMatchObject({
        type: "root",
        children: [{ type: "containerDirective", name: "note" }],
      });
      expect(bundled.MdastAdapterError).toBeTypeOf("function");
      expect(after.currentGeneration).toBe(before.currentGeneration + 1);
      expect(after.trapRecoveriesStarted).toBe(before.trapRecoveriesStarted + 1);
      expect(after.freshInstanceStarts).toBe(before.freshInstanceStarts + 3);
      expect(after.glueImportAttempts).toBe(before.glueImportAttempts + 3);
      expect(after.compiledModuleLoads).toBe(before.compiledModuleLoads + 2);
    } finally {
      globalThis.fetch = originalFetch;
    }
  });
});
