import assert from "node:assert/strict";
import { chmod, mkdtemp, mkdir, rm, symlink, unlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import {
  inputIdentityForPath,
  sameTestedInputs,
  testedInputIdentity,
} from "../../../scripts/wind-compatibility/reference-identity.mjs";
import { digest, sha256 } from "../../../scripts/wind-compatibility/reference.mjs";

test("input identity records broken and directory symlinks without following targets", async () => {
  const dir = await mkdtemp(join(tmpdir(), "wind-input-links-"));
  try {
    const link = join(dir, "link");
    await symlink("missing-target", link);
    const first = await inputIdentityForPath(link);
    assert.deepEqual(first, { kind: "symlink", sha256: sha256("missing-target") });
    await mkdir(join(dir, "missing-target"));
    assert.deepEqual(await inputIdentityForPath(link), first);
    await unlink(link);
    await symlink("different-missing-target", link);
    assert.notDeepEqual(await inputIdentityForPath(link), first);
    await unlink(link);
    await writeFile(link, "missing-target");
    assert.notDeepEqual(await inputIdentityForPath(link), first);
    await assert.rejects(() => inputIdentityForPath(join(dir, "missing-target")), /Unsupported/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("file bytes and executable permission both bind the input identity", async () => {
  const dir = await mkdtemp(join(tmpdir(), "wind-input-file-"));
  try {
    const path = join(dir, "runner");
    await writeFile(path, "original", { mode: 0o644 });
    const original = await inputIdentityForPath(path);
    await chmod(path, 0o755);
    assert.notDeepEqual(await inputIdentityForPath(path), original);
    await chmod(path, 0o644);
    await writeFile(path, "changed");
    assert.notDeepEqual(await inputIdentityForPath(path), original);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("repository snapshot includes tracked link identity and rejects the prior schema", async () => {
  const identity = await testedInputIdentity();
  assert.equal(identity.schemaVersion, 2);
  assert.equal(
    identity.files.find(([path]) => path === "docs/.claude/skills/zfb-wisdom/docs")?.[1].kind,
    "symlink",
  );
  assert.equal(sameTestedInputs(identity, structuredClone(identity)), true);
  const old = { ...identity, schemaVersion: 1 };
  assert.equal(sameTestedInputs(old, old), false);
  const changed = structuredClone(identity);
  changed.files[0][1].sha256 = sha256("changed");
  changed.digest = `sha256:${digest(changed.files)}`;
  assert.equal(sameTestedInputs(identity, changed), false);
});
