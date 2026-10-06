import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, mkdir, writeFile, rm, symlink, rename } from "node:fs/promises";
import { createHash } from "node:crypto";
import { execFileSync, spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
  referenceModuleNames,
  verifiedReferenceImport,
  verifyExtractedReferenceModules,
} from "../../../scripts/wind-compatibility/reference-module-graph.mjs";

test("the exact module graph rejects altered chunks, alternate entries and symlinks", async () => {
  const cache = await mkdtemp(join(tmpdir(), "wind-reference-graph-"));
  const dist = join(cache, "dist"),
    entry = join(dist, "lib.mjs");
  const modules = new Map(
    referenceModuleNames.map((name) => [
      name,
      Buffer.from(`export const identity = ${JSON.stringify(name)};\n`),
    ]),
  );
  try {
    await mkdir(dist);
    for (const [name, bytes] of modules) await writeFile(join(dist, name), bytes);
    const verified = await verifyExtractedReferenceModules(cache, entry, modules);
    assert.deepEqual(Object.keys(verified.moduleDigests).sort(), [...referenceModuleNames].sort());
    assert.match(verified.moduleGraphSha256, /^[0-9a-f]{64}$/);

    const chunk = "chunk-5JIJA4QV.mjs";
    await writeFile(join(dist, chunk), "tampered");
    await assert.rejects(
      verifyExtractedReferenceModules(cache, entry, modules),
      /differs from tarball/,
    );
    await writeFile(join(dist, chunk), modules.get(chunk));

    await writeFile(join(dist, "evil.mjs"), "export const evil = true;");
    await assert.rejects(verifyExtractedReferenceModules(cache, entry, modules), /module set/);
    await rm(join(dist, "evil.mjs"));

    const alternate = join(cache, "alternate-entry");
    await rename(entry, alternate);
    await symlink(alternate, entry);
    await assert.rejects(
      verifyExtractedReferenceModules(cache, entry, modules),
      /Unverified compiler module lib.mjs/,
    );
    await rm(entry);
    await rename(alternate, entry);

    const alternateDist = join(cache, "alternate-dist");
    await rename(dist, alternateDist);
    await symlink(alternateDist, dist);
    await assert.rejects(
      verifyExtractedReferenceModules(cache, entry, modules),
      /directory is a symlink/,
    );
  } finally {
    await rm(cache, { recursive: true, force: true });
  }
});

test("future graph only imports exact verified sibling modules", () => {
  const root = "file:///tmp/verified/dist/";
  const members = new Set(["lib.mjs", "chunk.mjs"]);
  assert.equal(verifiedReferenceImport("./chunk.mjs", `${root}lib.mjs`, root, members), true);
  for (const specifier of ["./helper.js", "./missing.mjs", "../escape.mjs",
    "tailwindcss", "node:fs", "./chunk.mjs?query"])
    assert.throws(() => verifiedReferenceImport(specifier, `${root}lib.mjs`, root, members),
      /Unsupported reference module import/);
  assert.equal(verifiedReferenceImport("node:fs", "file:///other/module.mjs", root, members), false);
});

test("future compiler rejects a transitive nonverified JS import at execution", async () => {
  const cache = await mkdtemp(join(tmpdir(), "wind-future-import-"));
  const packageDir = join(cache, "package");
  try {
    await mkdir(join(packageDir, "dist"), { recursive: true });
    await writeFile(join(packageDir, "package.json"), JSON.stringify({ name: "tailwindcss", version: "9.9.9" }));
    await writeFile(join(packageDir, "preflight.css"), "/* test */");
    await writeFile(join(packageDir, "dist/lib.mjs"),
      'import "./helper.js"; export const compile = async () => ({ build: () => "" });');
    await writeFile(join(packageDir, "dist/helper.js"), "export const helper = true;");
    const archive = join(cache, "artifact.tgz");
    execFileSync("tar", ["-czf", archive, "package"], { cwd: cache });
    const bytes = await import("node:fs/promises").then((fs) => fs.readFile(archive));
    const sha = createHash("sha256").update(bytes).digest("hex");
    const sri = `sha512-${createHash("sha512").update(bytes).digest("base64")}`;
    await writeFile(join(cache, `response-${sha}`), bytes);
    const runner = resolve("scripts/wind-compatibility/differential-runner.mjs");
    const result = spawnSync(process.execPath, ["--input-type=module", "-e",
      `import {loadReference} from ${JSON.stringify(new URL(`file://${runner}`).href)}; await loadReference(${JSON.stringify(cache)}, ${JSON.stringify({ package: "tailwindcss", version: "9.9.9", integrity: sri, artifactSha256: sha })});`],
    { encoding: "utf8" });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Unsupported reference module import/);
  } finally { await rm(cache, { recursive: true, force: true }); }
});
