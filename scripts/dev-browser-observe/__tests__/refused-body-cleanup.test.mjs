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
  "outer-decision-only",
  "outer-fails-at-t0",
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
      ...(mode.startsWith("outer-") && {
        outerDecisionOnly: true,
        beforePeerDecision: () => {},
        outerPeerWriteTime: (now) => {
          if (mode === "outer-fails-at-t0") throw new Error("actual outer cutoff");
          throw Object.assign(new Error("declared diagnostic"), {
            code: "NEXT_OUTER_DECISION_ONLY",
            proof: { t0Ns: String(now()) },
          });
        },
      }),
      checkPeerWriteTime: () => {
        if (mode === "late-before-open") throw Error("late after graph verification");
      },
      beforePeerMutation: () => {
        gate++;
        if (mode === "late-before-open" || mode.startsWith("outer-")) return;
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
    const untouched = [
      "unchanged",
      "late-before-open",
      "outer-decision-only",
      "outer-fails-at-t0",
    ].includes(mode);
    assert.equal(diskCalls, ["prior-write", "failed-write"].includes(mode) ? 1 : 0);
    assert.equal(restorations, untouched ? 0 : 1);
    assert.equal(visits, untouched ? 0 : 1);
    assert.equal(settles, untouched ? 0 : 1);
    if (untouched) {
      assert.equal(
        accounting.stopReason,
        mode === "outer-decision-only" ? "outer-decision-only" : "identity-rejected",
      );
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

test.each(["prepare-failed", "drift-after-snapshot", "unexpected-return", "unexpected-multiwrite"])(
  "the no-write experiment cannot edit, restore or visit recovery routes on %s",
  async (mode) => {
    const dir = mkdtempSync(join(tmpdir(), "outer-no-recovery-"));
    try {
      const path = join(dir, "article-00005.md");
      writeFileSync(path, "v0");
      const fs = fakeFs(),
        page = fakePage();
      let writes = 0,
        restores = 0,
        visits = 0;
      fs.snapshot = () => snapshotPaths({ files: [path] });
      fs.readPeerTarget = readPeerTarget;
      fs.performWrite = (w, t0) => {
        const stamp = t0();
        writes++;
        return stamp;
      };
      fs.performWrites = async () => {
        writes++;
        return { t0: 0n };
      };
      fs.restore = () => {
        restores++;
        return { mismatches: [], actions: [] };
      };
      page.liveCheck = async () => {
        visits++;
        return { ok: true };
      };
      fs.prepareAttempt = () => {
        if (mode === "prepare-failed") throw Error("controlled early preparation failure");
        const write = { method: "in-place", path, bytes: Buffer.from("next") };
        return {
          token: "controlled",
          targetPath: "/articles/article-00005/",
          predicate: { kind: "text", selector: ".body", includes: "next" },
          write,
          ...(mode === "unexpected-multiwrite" && { writes: [write, write] }),
          supplemental: [],
        };
      };
      const d = {
        ...deps({ page, fs }),
        outerDecisionOnly: true,
        beforePeerMutation: () => {
          if (mode === "drift-after-snapshot") {
            writeFileSync(path, "external drift");
            throw Error("changed witness");
          }
        },
        outerPeerWriteTime: (now) => ({ t0Ns: String(now()) }),
      };
      const op = { id: "edit-05", op: "edit-body", article: "article-00005", expect: "classify" };
      const result = await runBatch({ deps: d, scenarios: [{ id: "fn-edit-body", ops: [op] }] });
      assert(result.stopped);
      assert.deepEqual({ writes, restores, visits }, { writes: 0, restores: 0, visits: 0 });
      const cleanup = d.recorder.records.find((r) => r.type === "cleanup");
      assert(cleanup && cleanup.restored === false);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  },
);
