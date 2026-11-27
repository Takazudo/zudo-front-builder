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
        let eof = false;
        for (let read = 0; read < 8 && offset < buf.length; read++) {
          const n = readSync(cmd, buf, offset, buf.length - offset, null);
          if (!n) {
            eof = true;
            break;
          }
          offset += n;
        }
        assert(
          eof &&
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
  const inspect = () => {
    const pages = context?.pages();
    return {
      tracked: Boolean(context),
      httpRequests: context ? httpRequests : null,
      pageUrl: pages?.length === 1 ? pages[0].url() : null,
      workloadWriteCalls,
      snapshots,
    };
  };
  return {
    inspect,
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
      captured = inspect();
      context?.off("request", onRequest);
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
  const row = { type: "original-family-admission", batchEntered: false };
  let journaled = false,
    admission;
  const requireBlank = (audit) =>
    assert(
      audit.tracked &&
        audit.pageUrl === "about:blank" &&
        audit.httpRequests === 0 &&
        audit.workloadWriteCalls === 0 &&
        audit.snapshots === 0,
      "family browser left blank setup",
    );
  try {
    signal.throwIfAborted();
    const initial = browserAudit.inspect();
    requireBlank(initial);
    const provisional = safety.verifyFamilyAdmission();
    append({ ...row, ...initial, status: "pre-batch-proof", proof: provisional });
    journaled = true;
    // The filesystem journal may stall. Make the authoritative census and
    // live image/argv checks only after it returns; no extra scheduling.
    signal.throwIfAborted();
    const owned = snapshot();
    assert(
      population(provisional.sample.live, [provisional.root, provisional.dev]) &&
        population(owned, [provisional.root, provisional.dev]) &&
        population([...known.values()], [provisional.root, provisional.dev, provisional.worker]),
      "family ownership population changed",
    );
    assert(
      owned.every((p) => !["X", "x", "Z"].includes(p.state)),
      "family root/native unavailable",
    );
    verifyImages(provisional);
    const audit = browserAudit.finish();
    requireBlank(audit);
    const proof = safety.verifyFamilyAdmission();
    assert(
      same(proof.root, provisional.root) &&
        same(proof.dev, provisional.dev) &&
        same(proof.worker, provisional.worker),
      "family startup ownership changed",
    );
    signal.throwIfAborted();
    const stamp = process.hrtime.bigint();
    assert(stamp < deadlineNs, "family work deadline expired");
    admission = { decisionNs: String(stamp), owned, known: [...known.values()], ...audit };
  } catch (error) {
    append({
      ...row,
      ...browserAudit.finish(),
      admitted: false,
      afterJournal: journaled,
      reason: String(error?.message ?? error),
    });
    throw error;
  }
  browserAudit.enter();
  return runBatch({ ...input, deps: { ...input.deps, familyAdmission: admission } });
}
