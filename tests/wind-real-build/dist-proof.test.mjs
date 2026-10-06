import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { verifyRustBuildExport } from "./dist-proof.mjs";
import { sha256 } from "../../scripts/wind-compatibility/reference.mjs";

test("build export re-reads its sidecar, native esbuild and dist bytes", async () => {
  const root = await mkdtemp(join(tmpdir(), "zfb-shipping-dist-proof-"));
  try {
    const dist = join(root, "dist");
    await mkdir(join(dist, "assets"), { recursive: true });
    await writeFile(join(dist, "index.html"), "<html></html>");
    await writeFile(join(dist, "assets/styles-test.css"), ".test{display:block}");
    const esbuild = join(root, "esbuild");
    await writeFile(esbuild, "native esbuild fixture bytes");
    const build = { binaryPath: join(root, "zfb"), binarySha256: "a".repeat(64) };
    const execution = {
      schemaVersion: 1,
      kind: "zfb-rust-build-export",
      binaryPath: build.binaryPath,
      binarySha256: build.binarySha256,
      indexSha256: sha256(await readFile(join(dist, "index.html"))),
      stylesheet: "assets/styles-test.css",
      stylesheetSha256: sha256(await readFile(join(dist, "assets/styles-test.css"))),
      buildEnvironment: {
        PATH: "",
        NODE_PATH: null,
        ZFB_ESBUILD_BIN: esbuild,
        esbuildSha256: sha256(await readFile(esbuild)),
      },
    };
    const sidecar = join(root, "execution.json");
    await writeFile(sidecar, JSON.stringify(execution));
    const originalSha = sha256(await readFile(sidecar));
    assert.deepEqual(
      await verifyRustBuildExport(sidecar, originalSha, execution, build, dist),
      execution,
    );
    await writeFile(sidecar, JSON.stringify({ ...execution, indexSha256: "0".repeat(64) }));
    await assert.rejects(
      verifyRustBuildExport(sidecar, originalSha, execution, build, dist),
      /sidecar hash changed/,
    );
    await writeFile(sidecar, JSON.stringify(execution));
    await writeFile(join(dist, "assets/styles-test.css"), ".test{display:none}");
    await assert.rejects(
      verifyRustBuildExport(sidecar, originalSha, execution, build, dist),
      /does not match/,
    );
    await rm(sidecar);
    await assert.rejects(
      verifyRustBuildExport(sidecar, originalSha, execution, build, dist),
      /ENOENT/,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
