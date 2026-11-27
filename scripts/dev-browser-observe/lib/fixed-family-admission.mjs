import assert from "node:assert/strict";
import { openSync, readSync, closeSync, fstatSync, readlinkSync } from "node:fs";
import { linuxBirth, sameLinuxBirth } from "./linux-birth.mjs";
import { interruptible } from "./session-deadline.mjs";
import { runBatch } from "./state-machine.mjs";

// An idle capture admits on the already-accepted sample only while one full
// cadence period (limits.sampleEveryMs) is still ahead, which no real sample
// (43-53 ms to settle) leaves. Every other idle capture joins the next original
// cadence sample, so the decision sits at a fixed point after that dispatch
// instead of wherever browser readiness landed (#4862 slot 01 refused 527.1 ms
// after an idle capture at T+877.3 ms).
//
// Admission-cost bound, with S = sample settle time after dispatch, P = one
// population check (snapshot + verifyImages), J = the journal append, c = the
// capture offset inside an in-flight sample, the decision precedes the joined
// sample's cadence deadline (dispatch + 1000 ms) when
//   deferred: S + J + P < 1000 ms
//   pending:  max(S, c + P) + J + P < 1000 ms, and c + P < 750 ms (helper core)
// Both branches admit for every c < S when 2P + J < 1000 ms - S (about 947 ms
// at S = 53 ms) and P < 750 ms - S. At P = 490 ms, J = 400 ms the deferred
// branch admits (943 ms) and the pending branch refuses (1400 ms at c = 20 ms).
const FAMILY_HELPER_CORE_NS = 750_000_000n;

const same = (a, b) => sameLinuxBirth(a, b) && a.ppid === b.ppid && a.pgid === b.pgid;
function population(rows, expected) {
  return (
    rows.length === expected.length &&
    expected.every((want) => rows.filter((row) => same(row, want)).length === 1)
  );
}

export function zfbFooterStopPopulation(cap, stop) {
  if (
    !cap ||
    !stop?.rootAlive ||
    !stop.absent ||
    stop.error ||
    stop.remaining?.length !== 0 ||
    !Array.isArray(stop.known) ||
    !Array.isArray(stop.ownedBefore) ||
    stop.endings?.length !== 3
  )
    return false;
  const root = cap.root,
    dev = cap.dev,
    worker = cap.startupBirth;
  if (!population(stop.known, [root, dev, worker]) || !population(stop.ownedBefore, [root, dev]))
    return false;
  return [root, dev, worker].every((b) => {
    const ending = stop.endings.find((r) => r.pid === b.pid);
    if (!ending) return false;
    if (b.pid === worker.pid) return ending.endedAfter === "before-stop";
    return (
      (b.pid !== root.pid ||
        (stop.exit && JSON.stringify(ending.exit) === JSON.stringify(stop.exit))) &&
      ["SIGINT", "SIGTERM", "SIGKILL"].includes(ending.endedAfter) &&
      stop.steps?.some(
        (step) =>
          step.delivery === "sent" &&
          step.target === "group:" + root.pgid &&
          step.signal === ending.endedAfter,
      )
    );
  });
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
          stages: [],
        }
      : null;
  // Monotonic setup-stage stamps, so an admission refusal is attributable from
  // the receipt. They are setup evidence, never workload timing.
  const stage = (name, phase, work) => {
    if (!setup) return work();
    const row = { stage: name, phase, startNs: String(process.hrtime.bigint()) };
    setup.stages.push(row);
    try {
      return work();
    } catch (error) {
      row.threw = true;
      throw error;
    } finally {
      row.endNs = String(process.hrtime.bigint());
    }
  };
  const checkPopulation = (provisional, phase) => {
    const owned = stage("snapshot", phase, snapshot);
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
    stage("verify-images", phase, () => verifyImages(provisional));
    return owned;
  };
  const enter = (initial, provisional) => {
    stage("journal-append", "enter", () =>
      append({
        ...row,
        ...initial,
        status: "pre-batch-proof",
        proof: provisional,
        ...(setup && { setup: { ...setup, stages: structuredClone(setup.stages) } }),
      }),
    );
    journaled = true;
    signal.throwIfAborted();
    const owned = checkPopulation(provisional, "enter");
    const proof = stage("verify-family-admission", "enter", () => safety.verifyFamilyAdmission());
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
    if (setup)
      assert(stamp < BigInt(proof.sample.cadenceDeadlineNs), "cadence due at family decision");
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
  const joinPending = (captured) => {
    setup.priorCustody = captured.before;
    checkPopulation(captured.before, "pre-settlement");
    return join(captured);
  };
  const deferToNextCycle = async (first) => {
    const cadenceDeadlineNs = BigInt(first.before.sample.cadenceDeadlineNs);
    const boundNs =
      cadenceDeadlineNs + FAMILY_HELPER_CORE_NS < deadlineNs
        ? cadenceDeadlineNs + FAMILY_HELPER_CORE_NS
        : deadlineNs;
    const deferral = {
      reserveNs: String(first.deferred.reserveNs),
      remainingCadenceNs: String(first.deferred.remainingCadenceNs),
      firstCapture: structuredClone(
        Object.fromEntries(
          ["capturedNs", "pending", "failed", "stopped", "strict", "previousEpoch", "cycle"].map(
            (key) => [key, setup[key]],
          ),
        ),
      ),
      cadenceDeadlineNs: String(cadenceDeadlineNs),
      waitBoundNs: String(boundNs),
      outcome: "waiting",
    };
    setup.idleDeferral = deferral;
    setup.settlementOutcome = "deferred-next-cycle";
    let timer;
    try {
      const begin = process.hrtime.bigint();
      deferral.waitStartedNs = String(begin);
      assert(begin < boundNs, "family next cadence dispatch expired");
      const bounded = new Promise((_, reject) => {
        timer = setTimeout(
          () => reject(Error("family next cadence dispatch expired")),
          Math.ceil(Number(boundNs - begin) / 1e6),
        );
      });
      try {
        await interruptible(Promise.race([safety.nextFamilyHelper(), bounded]), signal);
      } finally {
        clearTimeout(timer);
        const observed = process.hrtime.bigint();
        deferral.helperObservedNs = String(observed);
        deferral.addedWaitNs = String(observed - begin);
      }
      signal.throwIfAborted();
      assert(BigInt(deferral.helperObservedNs) < boundNs, "family next cadence dispatch expired");
      requireBlank(browserAudit.inspect());
      const captured = safety.captureFamilyCycle(setup);
      assert(captured.settlement, "family next cadence cycle not joinable");
      for (const name of ["root", "dev", "worker", "terminal", "release"])
        assert.deepEqual(
          captured.before[name],
          first.before[name],
          "family custody changed during deferral",
        );
      deferral.outcome = "joined";
      // enter() checks population against the joined sample; a check here
      // would only add synchronous cost before the settlement wait.
      setup.priorCustody = captured.before;
      return join(captured);
    } catch (error) {
      deferral.outcome = "refused";
      setup.settlementOutcome = "refused";
      return refuse(error);
    }
  };
  try {
    signal.throwIfAborted();
    const initial = browserAudit.inspect();
    requireBlank(initial);
    if (setup) {
      assert(process.hrtime.bigint() < deadlineNs, "family work deadline expired");
      setup.initialBlank = initial;
      browserAudit.watchSettlementPage();
      setup.pageWatchStartedNs = String(process.hrtime.bigint());
      const captured = safety.captureFamilyCycle(setup, { idleNeedsFullPeriod: true });
      if (captured.settlement) return joinPending(captured);
      if (captured.deferred) return deferToNextCycle(captured);
      setup.settlementOutcome = "no-pending";
      enter(initial, captured.before);
    } else enter(initial, safety.verifyFamilyAdmission());
  } catch (error) {
    return refuse(error);
  }
  return run();
}
