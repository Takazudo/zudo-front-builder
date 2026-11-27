import assert from "node:assert/strict";
import { openSync, readSync, closeSync, fstatSync, readlinkSync } from "node:fs";
import { linuxBirth, sameLinuxBirth } from "./linux-birth.mjs";
import { runBatch } from "./state-machine.mjs";

const same = (a, b) => sameLinuxBirth(a, b) && a.ppid === b.ppid && a.pgid === b.pgid;
function population(rows, expected) {
  return (
    rows.length === expected.length &&
    expected.every((want) => rows.filter((row) => same(row, want)).length === 1)
  );
}

// Verify the currently running images, not just the cached successful sampler
// row. Both Linux birth reads surround all accesses for the same PID.
export function verifyFamilyImages(proof, pins) {
  for (const [birth, pin, args] of [
    [proof.root, pins.root, [pins.root.path, ...proof.root.argv]],
    [proof.dev, pins.child, [pins.child.path, ...proof.root.argv.slice(1)]],
  ]) {
    const prefix = "/proc/" + birth.pid + "/";
    const live = () => {
      const p = linuxBirth(birth.pid);
      assert(same(birth, p) && !["X", "x", "Z"].includes(p.state), "family live birth mismatch");
    };
    live();
    const fd = openSync(prefix + "exe", "r");
    try {
      const st = fstatSync(fd, { bigint: true }),
        magic = Buffer.alloc(4);
      assert.equal(String(st.dev), pin.device);
      assert.equal(String(st.ino), pin.inode);
      assert.equal(readSync(fd, magic, 0, 4, 0), 4);
      assert.equal(magic.toString("hex"), "7f454c46");
      assert.equal(readlinkSync(prefix + "exe"), pin.path);
      assert.equal(readlinkSync(prefix + "cwd"), pins.launch.cwd);
      const cmd = openSync(prefix + "cmdline", "r"),
        buf = Buffer.alloc(16385);
      try {
        let offset = 0;
        while (offset < buf.length) {
          const n = readSync(cmd, buf, offset, buf.length - offset, null);
          if (!n) break;
          offset += n;
        }
        assert(
          offset < buf.length &&
            buf.subarray(0, offset).equals(Buffer.from(args.join("\0") + "\0")),
          "family role argv mismatch",
        );
      } finally {
        closeSync(cmd);
      }
      live();
    } finally {
      closeSync(fd);
    }
  }
}

// The browser context exists while the ordinary RSS cadence continues. Install
// observation before its first page so an early browser request is visible.
export function blankBrowserAudit() {
  let context;
  let httpRequests = 0;
  let workloadWriteCalls = 0,
    snapshots = 0,
    entered = false,
    captured = null;
  const onRequest = (request) => {
    if (/^https?:/i.test(request.url())) httpRequests++;
  };
  return {
    attach(value) {
      assert(!context && value.pages().length === 0, "family context must begin empty");
      context = value;
      context.on("request", onRequest);
    },
    trackFixture(fs) {
      const guarded = { ...fs };
      for (const name of ["snapshot", "performWrite", "performWrites", "restore"]) {
        if (!fs[name]) continue;
        guarded[name] = (...args) => {
          if (name === "snapshot") snapshots++;
          else workloadWriteCalls++;
          assert(entered, "fixture operation before family admission");
          return fs[name](...args);
        };
      }
      return guarded;
    },
    enter() {
      entered = true;
    },
    get finished() {
      return captured !== null;
    },
    finish() {
      if (captured) return captured;
      const pages = context?.pages();
      const pageUrl = pages?.length === 1 ? pages[0].url() : null;
      context?.off("request", onRequest);
      captured = {
        tracked: Boolean(context),
        httpRequests: context ? httpRequests : null,
        pageUrl,
        workloadWriteCalls,
        snapshots,
      };
      return captured;
    },
  };
}

// Only the final synchronous transition can call runBatch. No snapshot,
// preparation navigation, restore or special-mode write can precede this gate.
export function runOriginalFamilyBatch(
  input,
  { safety, snapshot, known, browserAudit, signal, append, deadlineNs, verifyImages },
) {
  const audit = browserAudit.finish();
  const row = {
    type: "original-family-admission",
    ...audit,
    batchEntered: false,
  };
  try {
    signal.throwIfAborted();
    assert(
      audit.tracked &&
        audit.pageUrl === "about:blank" &&
        audit.httpRequests === 0 &&
        audit.workloadWriteCalls === 0 &&
        audit.snapshots === 0,
      "family browser left blank setup",
    );
    const owned = snapshot();
    const proof = safety.verifyFamilyAdmission();
    assert(
      population(proof.sample.live, [proof.root, proof.dev]) &&
        population(owned, [proof.root, proof.dev]) &&
        population([...known.values()], [proof.root, proof.dev, proof.worker]),
      "family ownership population changed",
    );
    assert(
      owned.every((p) => !["X", "x", "Z"].includes(p.state)),
      "family root/native unavailable",
    );
    signal.throwIfAborted();
    assert(process.hrtime.bigint() < deadlineNs, "family work deadline expired");
    verifyImages(proof);
    row.proof = proof;
    row.owned = owned;
    row.known = [...known.values()];
  } catch (error) {
    append({ ...row, admitted: false, reason: String(error?.message ?? error) });
    throw error;
  }
  append({ ...row, status: "pre-batch-proof" });
  // Journaling can itself exhaust the clock. Refuse without entering the batch.
  try {
    signal.throwIfAborted();
    safety.verifyFamilyAdmission();
    assert(process.hrtime.bigint() < deadlineNs, "family work deadline expired after journal");
  } catch (error) {
    append({
      ...row,
      admitted: false,
      reason: String(error?.message ?? error),
      afterJournal: true,
    });
    throw error;
  }
  browserAudit.enter();
  return runBatch({
    ...input,
    deps: { ...input.deps, familyAdmissionDecisionNs: String(process.hrtime.bigint()) },
  });
}
