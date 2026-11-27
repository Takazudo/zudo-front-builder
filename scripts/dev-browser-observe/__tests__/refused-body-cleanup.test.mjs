import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync, renameSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "vitest";
import { readPeerTarget, snapshotPaths } from "../lib/fixture.mjs";
import { runBatch } from "../lib/state-machine.mjs";
import { deps, fakeFs, fakePage } from "./fakes.mjs";

test.each([
  "unchanged",
  "late-before-open",
  "changed-hash",
  "changed-inode",
  "prior-write",
  "failed-write",
])("a refused body gate preserves cleanup appropriate to %s", async (mode) => {
  const dir = mkdtempSync(join(tmpdir(), "peer-no-write-"));
  try {
    const path = join(dir, "article-00005.md");
    writeFileSync(path, "v0");
    const fs = fakeFs();
    fs.snapshot = () => snapshotPaths({ files: [path] });
    fs.readPeerTarget = readPeerTarget;
    let diskCalls = 0;
    fs.performWrite = (w, now) => {
      const t = now();
      diskCalls++;
      writeFileSync(path, w.bytes);
      if (mode === "failed-write") throw Error("controlled error after write");
      return t;
    };
    let restorations = 0,
      visits = 0,
      settles = 0,
      gate = 0;
    fs.restore = (original) => {
      restorations++;
      writeFileSync(path, original.files[path].bytes);
      return { mismatches: [], actions: [{ path, method: "in-place" }] };
    };
    fs.prepareAttempt = (op) => ({
      token: op.id,
      targetPath: "/articles/article-00005/",
      predicate: { kind: "text", selector: ".body", includes: op.id },
      write: { method: "in-place", path, bytes: Buffer.from(op.id) },
      supplemental: [],
    });
    const page = fakePage({
      behaviors: [{ kind: "notify", t1: 2_000_000_000n }],
      liveCheck: () => {
        visits++;
        return { ok: true };
      },
    });
    page.settle = async () => {
      settles++;
      return { settled: true, nativeUpdateObserved: true };
    };
    const d = {
      ...deps({ page, fs }),
      checkPeerWriteTime: () => {
        if (mode === "late-before-open") throw Error("late after graph verification");
      },
      beforePeerMutation: () => {
        gate++;
        if (mode === "late-before-open") return;
        if ((mode === "prior-write" && gate === 1) || mode === "failed-write") return;
        if (mode === "changed-hash") writeFileSync(path, "XX");
        if (mode === "changed-inode") {
          writeFileSync(join(dir, "replacement"), "v0");
          renameSync(join(dir, "replacement"), path);
        }
        throw Error("known Next worker missing before official terminal decision");
      },
    };
    const op = { id: "edit-05", op: "edit-body", article: "article-00005", expect: "classify" };
    const { accounting, stopped } = await runBatch({
      deps: d,
      scenarios: [
        { id: "fn-edit-body", ops: mode === "prior-write" ? [op, { ...op, id: "edit2" }] : [op] },
      ],
    });
    assert(stopped);
    const untouched = ["unchanged", "late-before-open"].includes(mode);
    assert.equal(diskCalls, ["prior-write", "failed-write"].includes(mode) ? 1 : 0);
    assert.equal(restorations, untouched ? 0 : 1);
    assert.equal(visits, untouched ? 0 : 1);
    assert.equal(settles, untouched ? 0 : 1);
    if (untouched) {
      assert.equal(accounting.stopReason, "identity-rejected");
      const audit = d.recorder.records.find((x) => x.type === "fixed-peer-refused-disk-audit");
      assert(audit.unchanged && audit.before.sha256 === audit.after.sha256);
      assert.deepEqual(audit.before, audit.after);
      const cleanup = d.recorder.records.find((x) => x.type === "cleanup");
      assert.equal(cleanup.ok, false);
      assert.equal(cleanup.restored, false);
      assert.equal(cleanup.liveSkipped, "before-mutation-no-write");
      assert.deepEqual(cleanup.live, []);
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
