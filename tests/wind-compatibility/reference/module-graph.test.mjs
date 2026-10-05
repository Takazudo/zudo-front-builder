import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, mkdir, writeFile, rm, symlink, rename } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  referenceModuleNames,
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
