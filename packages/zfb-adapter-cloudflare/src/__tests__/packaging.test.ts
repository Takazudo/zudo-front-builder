// Packaging regression test for the published-tarball file set.
//
// 0.1.0-next.27 shipped broken: the shared-emitWorker refactor made
// `bin/cli.mjs` import `../src/emit-worker.mjs` and `dist/build.js`
// import `./emit-worker.mjs`, but neither the `files` allowlist nor the
// build script's `cp` step was updated — so the published tarball lacked
// the file at both paths and EVERY adapter consumer crashed at build
// time with ERR_MODULE_NOT_FOUND. The runtime tests run against the repo
// working tree, where `src/emit-worker.mjs` always exists, so they could
// not catch the omission.
//
// These checks are static (package.json + source text), so they hold in
// any environment without requiring `npm pack` or a prior `pnpm build`:
//
// 1. Every relative `../src/*.mjs` import in `bin/cli.mjs` must be
//    covered by the `files` allowlist (the published `bin/` resolves
//    those imports against the shipped `src/` entries).
// 2. Every relative `./*.mjs` import in `src/build.ts` (compiled to
//    `dist/build.js`, which keeps the specifier verbatim) must have a
//    matching `cp src/<name> dist/<name>` in the build script, since
//    `tsc` does not copy non-TS assets into `dist/`.

import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const PKG_DIR = join(dirname(fileURLToPath(import.meta.url)), "../..");

async function read(rel: string): Promise<string> {
  return readFile(join(PKG_DIR, rel), "utf8");
}

function relativeMjsImports(source: string, prefix: string): string[] {
  // Matches static `import ... from "<prefix><name>.mjs"` specifiers.
  const re = new RegExp(
    `from\\s+["'](${prefix.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}[\\w-]+\\.mjs)["']`,
    "g",
  );
  return [...source.matchAll(re)].map((m) => m[1]);
}

describe("packaging — published tarball must ship every imported .mjs", () => {
  it("files allowlist covers every src/*.mjs imported by bin/cli.mjs", async () => {
    const cli = await read("bin/cli.mjs");
    const imports = relativeMjsImports(cli, "../src/");
    expect(imports.length).toBeGreaterThan(0);

    const pkg = JSON.parse(await read("package.json")) as {
      files: string[];
    };
    for (const spec of imports) {
      // `../src/foo.mjs` resolved from `bin/` → tarball path `src/foo.mjs`.
      const tarballPath = spec.replace(/^\.\.\//, "");
      expect(
        pkg.files,
        `bin/cli.mjs imports ${spec} but "${tarballPath}" is not in package.json "files" — the published bin would crash with ERR_MODULE_NOT_FOUND`,
      ).toContain(tarballPath);
    }
  });

  it("build script copies every ./*.mjs imported by src/build.ts into dist/", async () => {
    const buildTs = await read("src/build.ts");
    const imports = relativeMjsImports(buildTs, "./");
    expect(imports.length).toBeGreaterThan(0);

    const pkg = JSON.parse(await read("package.json")) as {
      scripts: { build: string };
    };
    for (const spec of imports) {
      const name = spec.replace(/^\.\//, "");
      expect(
        pkg.scripts.build,
        `src/build.ts imports ${spec} but the build script never runs "cp src/${name} dist/${name}" — dist/build.js would crash with ERR_MODULE_NOT_FOUND in the published package`,
      ).toContain(`cp src/${name} dist/${name}`);
    }
  });
});
