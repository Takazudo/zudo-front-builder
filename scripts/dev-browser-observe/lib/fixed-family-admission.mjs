import assert from "node:assert/strict";
import { openSync, readSync, closeSync, fstatSync, readlinkSync } from "node:fs";
import { linuxBirth, sameLinuxBirth } from "./linux-birth.mjs";
import { interruptible } from "./session-deadline.mjs";
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
  let watchedPage,
    pageChanged = false;
  const changedPage = () => {
    pageChanged = true;
  };
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
      ...(watchedPage && { pageChanged }),
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
    watchSettlementPage() {
      [watchedPage] = context.pages();
      context.on("page", changedPage);
      watchedPage.on("framenavigated", changedPage);
      watchedPage.on("close", changedPage);
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
      if (watchedPage) {
        context.off("page", changedPage);
        watchedPage.off("framenavigated", changedPage);
        watchedPage.off("close", changedPage);
      }
      return captured;
    },
  };
}

// Only the final synchronous transition can call runBatch. No snapshot,
// preparation navigation, restore or special-mode write can precede this gate.
export function runOriginalFamilyBatch(
  input,
  { safety, snapshot, known, browserAudit, signal, append, deadlineNs, verifyImages, strategy },
) {
  const row = { type: "original-family-admission", batchEntered: false };
  let journaled = false,
    admission;
  const requireBlank = (audit) =>
    assert(
      audit.tracked &&
        !audit.pageChanged &&
        audit.pageUrl === "about:blank" &&
        audit.httpRequests === 0 &&
        audit.workloadWriteCalls === 0 &&
        audit.snapshots === 0,
      "family browser left blank setup",
    );
  const setup =
    strategy === "blank-join-owned.v2"
      ? {
          strategy,
          initialNs: String(process.hrtime.bigint()),
          workDeadlineNs: String(deadlineNs),
          settlementOutcome: "not-started",
          addedSettlementNs: "0",
        }
      : null;
  const checkPopulation = (provisional) => {
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
    return owned;
  };
  const enter = (initial, provisional) => {
    append({
      ...row,
      ...initial,
      status: "pre-batch-proof",
      proof: provisional,
      ...(setup && { setup: { ...setup } }),
    });
    journaled = true;
    signal.throwIfAborted();
    const owned = checkPopulation(provisional);
    const proof = safety.verifyFamilyAdmission();
    if (setup) {
      assert.equal(proof.sample, provisional.sample, "family accepted sample changed");
      assert.deepEqual(proof.terminal, provisional.terminal, "family terminal changed");
      assert.deepEqual(proof.release, provisional.release, "family release changed");
    }
    assert(
      same(proof.root, provisional.root) &&
        same(proof.dev, provisional.dev) &&
        same(proof.worker, provisional.worker),
      "family startup ownership changed",
    );
    signal.throwIfAborted();
    const audit = browserAudit.inspect();
    requireBlank(audit);
    const stamp = process.hrtime.bigint();
    assert(stamp < deadlineNs, "family work deadline expired");
    if (setup) setup.totalAdmissionNs = String(stamp - BigInt(setup.initialNs));
    admission = {
      decisionNs: String(stamp),
      owned,
      known: [...known.values()],
      ...audit,
      ...(setup && { setup }),
    };
  };
  const refuse = (error) => {
    append({
      ...row,
      ...browserAudit.finish(),
      admitted: false,
      afterJournal: journaled,
      reason: String(error?.message ?? error),
      ...(setup && { setup, decisionNs: String(process.hrtime.bigint()) }),
    });
    throw error;
  };
  const run = () => {
    browserAudit.finish();
    browserAudit.enter();
    return runBatch({ ...input, deps: { ...input.deps, familyAdmission: admission } });
  };
  const join = async (captured) => {
    const { cycle } = setup;
    const core = BigInt(cycle.helper.deadlineNs);
    const cutoff = core < deadlineNs ? core : deadlineNs;
    const begin = process.hrtime.bigint();
    setup.joinStartedNs = String(begin);
    setup.remainingCoreNs = String(core - begin);
    setup.remainingWorkNs = String(deadlineNs - begin);
    try {
      assert(begin < cutoff, "family join deadline expired");
      const timeout = AbortSignal.timeout(Math.ceil(Number(cutoff - begin) / 1e6));
      setup.settlementOutcome = "waiting";
      try {
        await interruptible(captured.settlement, AbortSignal.any([signal, timeout]));
      } finally {
        const settled = process.hrtime.bigint();
        setup.settlementObservedNs = String(settled);
        setup.addedSettlementNs = String(settled - begin);
      }
      signal.throwIfAborted();
      assert(BigInt(setup.settlementObservedNs) < cutoff, "family settlement deadline expired");
      const current = safety.verifyFamilyAdmission();
      const accepted = current.sample;
      assert(
        accepted.epoch === cycle.epoch &&
          accepted.startedNs === cycle.startedNs &&
          accepted.helperPid === cycle.helper.pid &&
          accepted.helperSpawnNs === cycle.helper.spawnHrtimeNs &&
          accepted.entryNs === cycle.helper.entryNs &&
          accepted.deadlineNs === cycle.helper.deadlineNs &&
          BigInt(accepted.resultNs) >= BigInt(cycle.helper.spawnHrtimeNs) &&
          BigInt(accepted.resultNs) <= BigInt(setup.settlementObservedNs),
        "family settlement changed cycle",
      );
      for (const name of ["root", "dev", "worker", "terminal", "release"])
        assert.deepEqual(
          current[name],
          captured.before[name],
          "family custody changed during settlement",
        );
      setup.settlementOutcome = "accepted";
      const latest = browserAudit.inspect();
      requireBlank(latest);
      enter(latest, current);
    } catch (error) {
      if (setup.settlementOutcome !== "accepted") setup.settlementOutcome = "refused";
      return refuse(error);
    }
    return run();
  };
  try {
    signal.throwIfAborted();
    const initial = browserAudit.inspect();
    requireBlank(initial);
    if (setup) {
      assert(process.hrtime.bigint() < deadlineNs, "family work deadline expired");
      setup.initialBlank = initial;
      const captured = safety.captureFamilyCycle(setup);
      if (captured.settlement) {
        setup.priorCustody = captured.before;
        browserAudit.watchSettlementPage();
        checkPopulation(captured.before);
        return join(captured);
      }
      setup.settlementOutcome = "no-pending";
      enter(initial, captured.before);
    } else enter(initial, safety.verifyFamilyAdmission());
  } catch (error) {
    return refuse(error);
  }
  return run();
}
