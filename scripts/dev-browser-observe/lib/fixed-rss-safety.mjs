// Only the direct Linux zfb distribution uses this contract: Node runs the
// pinned shim; the materialized, pinned native binary is its direct child.
import { closeSync, fstatSync, openSync, readSync, realpathSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";
import assert from "node:assert/strict";
import {
  samplePeerRoles,
  peerProcessIdentity,
  FIXED_ASTRO_FOOTER_PROTOCOL,
  FIXED_PEER_PROTOCOL,
} from "./fixed-peer-roles.mjs";
import { sampleRendererMemory } from "./linux-rss-sample.mjs";
import { interruptible } from "./session-deadline.mjs";

// Owner decision on #4870 (2026-11-27): a pre-write decision that finds an
// ordinary sample in flight waits for it to settle, bounded by its 750 ms core
// plus the 200 ms closure, and then decides on that fresh sample.
const ORDINARY_SETTLE_BOUND_NS = 950_000_000n;

function pin(path, digest) {
  const real = realpathSync(path);
  const fd = openSync(real, "r");
  try {
    const info = fstatSync(fd, { bigint: true });
    const hash = createHash("sha256"),
      chunk = Buffer.allocUnsafe(256 * 1024);
    for (let bytes; (bytes = readSync(fd, chunk)) > 0;) hash.update(chunk.subarray(0, bytes));
    if (hash.digest("hex") !== digest)
      throw new Error(`fixed safety executable digest mismatch: ${real}`);
    return { path: real, device: String(info.dev), inode: String(info.ino) };
  } finally {
    closeSync(fd);
  }
}

export function fixedAstroExecutables(facts, protocol = FIXED_ASTRO_FOOTER_PROTOCOL) {
  return fixedPeerExecutables(facts, protocol);
}

export function fixedPeerExecutables(facts, protocol = FIXED_PEER_PROTOCOL) {
  if (facts.installReceipt.node.realpath !== realpathSync(process.execPath))
    throw new Error("fixed peer Node executable does not match install receipt");
  return {
    protocol,
    root: pin(process.execPath, facts.installReceipt.node.sha256),
  };
}

export function fixedZfbExecutables(appDir, facts, protocol = "linux-fixed-zfb.v1") {
  const child = facts.install.executables.find((exe) => exe.role === "zfb-native" && exe.ok);
  if (!child || facts.installReceipt.node.realpath !== realpathSync(process.execPath))
    throw new Error("fixed safety requires the verified Node launcher and packaged native binary");
  let launch;
  if (["linux-fixed-zfb.v2", "linux-fixed-zfb.v3", "linux-fixed-zfb.v4"].includes(protocol)) {
    const shim = facts.install.executables.find((exe) => exe.role === "zfb-shim" && exe.ok);
    if (!shim) throw new Error("fixed v2 requires the verified distribution shim");
    launch = { cwd: realpathSync(appDir), shim: pin(join(appDir, shim.path), shim.sha256) };
  }
  return {
    protocol,
    ...(launch && { launch }),
    root: pin(process.execPath, facts.installReceipt.node.sha256),
    child: pin(join(appDir, child.path), child.sha256),
  };
}

// Legacy ps samples remain contextual, separate from the pidfd safety receipts.
// A missing row cannot turn an unknown live contribution into zero RSS.
export function fixedContextSample(result) {
  if (
    result.processes?.length &&
    result.processes.every((p) => Number.isSafeInteger(p.rssKiB) && p.rssKiB >= 0)
  )
    return result;
  return { ...result, partialSummedRssKiB: result.summedRssKiB, summedRssKiB: null };
}

// Explains each unexpected direct child against the sampler's own recorded
// helper births. A full match (pid, startTicks, bootId) is still a refusal:
// this record attributes the cause and never admits the child.
export function attributeGraphRefusal(refusal, helpers) {
  const same = (a, b) =>
    Boolean(a && b) && a.pid === b.pid && a.startTicks === b.startTicks && a.bootId === b.bootId;
  return {
    ...refusal,
    extra: refusal.extra.map((child) => {
      let attribution = null;
      if (!child.birth) attribution = "no-birth";
      else if (same(child.birth, helpers.pending)) attribution = "pending-ordinary-helper";
      else if (same(child.birth, helpers.lastAccepted)) attribution = "closed-ordinary-helper";
      else if ([helpers.pending, helpers.lastAccepted].some((h) => h && h.pid === child.pid))
        attribution = "helper-pid-different-birth";
      return { ...child, attribution };
    }),
  };
}

export function startFixedRssSafety({
  server,
  fixedExecutables,
  limits,
  append,
  abort,
  sample = sampleRendererMemory,
  custody = null,
  peer,
  checkRoles = samplePeerRoles,
  nextOwner = () => null,
}) {
  let pending = null;
  let pendingOuterCycle = null,
    acceptedOuterCycle = null;
  let decision = null,
    decisionAbort;
  const intervalNs = BigInt(limits.sampleEveryMs) * 1000000n;
  let nextDispatchNs = process.hrtime.bigint() + intervalNs;
  let failure = null;
  let secondaryHelperFailure = null;
  let nativeBirth = null;
  let unresolvedHelper = null;
  let readyCheck = null;
  let stopped = false;
  let measuredCycles = 0;
  let peakKiB = null;
  const stable = new Map();
  let qualifiedPeer;
  let readyNs,
    firstSpawn,
    postCycles = 0,
    expiry;
  const fail = (error) => {
    if (error?.details?.helper?.closed === false || error?.details?.helper?.pipesClosed === false)
      unresolvedHelper = error.details.helper;
    const partial =
      fixedExecutables.protocol === "linux-fixed-zfb.v4" &&
      error?.details?.rssCoverage === "partial"
        ? error.details.observedSubtotalKiB
        : error?.details?.summedRssKiB;
    if (Number.isSafeInteger(partial) && partial >= 0) peakKiB = Math.max(peakKiB ?? 0, partial);
    const issue = { error: String(error?.message ?? error), details: error?.details ?? null };
    // At most one helper can still be in flight after the periodic dispatcher
    // refuses an overlap. Preserve its later settlement independently.
    if (failure) {
      if (!secondaryHelperFailure) {
        secondaryHelperFailure = issue;
        try {
          append({ type: "fixed-rss-secondary-helper-failure", failure: issue });
        } catch (writeError) {
          issue.receiptError = String(writeError);
        }
      }
      return;
    }
    failure = issue;
    readyCheck?.resolve();
    abort(new Error(failure.error));
    try {
      append({ type: "fixed-rss-failure", failure });
    } catch (error) {
      failure.receiptError = String(error);
    }
  };
  const take = () => {
    if (failure || stopped) return;
    nextDispatchNs = process.hrtime.bigint() + intervalNs;
    if (decision) {
      const error = new Error("periodic RSS dispatch overlapped pre-write identity check");
      decisionAbort.abort(error);
      fail(error);
      return;
    }
    if (pending) {
      fail(new Error("fixed safety sampler still occupied at next periodic dispatch"));
      return;
    }
    // A startup sample already in flight when the readiness line arrives can
    // legitimately predate the native child. Qualify a new periodic sample.
    const readyAtEntry = readyCheck;
    const startedNs = String(process.hrtime.bigint());
    const custodyAtEntry = nextOwner();
    const outerCycle =
      fixedExecutables.protocol === "linux-fixed-peer.v4"
        ? {
            epoch: measuredCycles + 1,
            startedNs,
            cadenceDeadlineNs: String(nextDispatchNs - 5000000n),
            helperBirth: null,
          }
        : null;
    pendingOuterCycle = outerCycle;
    const args = peer
      ? custodyAtEntry
        ? [
            server,
            fixedExecutables.root.path,
            { nextCustody: custodyAtEntry, ...(outerCycle && { outerCycle }) },
          ]
        : [server, fixedExecutables.root.path]
      : [server, fixedExecutables.root.path, { fixedExecutables, custody }];
    pending = sample(...args)
      .then((result) => {
        measuredCycles++;
        if (Number.isSafeInteger(result.summedRssKiB))
          peakKiB = Math.max(peakKiB ?? 0, result.summedRssKiB);
        append({
          type: "fixed-rss-sample",
          startedNs,
          result,
          ...(failure && { afterStopDecision: true }),
        });
        if (peer) {
          if (failure) return;
          const t = result.timing.parent;
          firstSpawn ??= BigInt(result.helper.spawnHrtimeNs);
          assert(
            result.helper.closed && result.helper.pipesClosed && result.timing.clock.qualified,
            "incomplete peer sampler",
          );
          assert(
            BigInt(t.entryNs) <= BigInt(t.resultNs) &&
              BigInt(t.resultNs) <= BigInt(t.deadlineNs) &&
              BigInt(t.deadlineNs) === BigInt(t.entryNs) + 750_000_000n,
            "invalid peer sampler bracket",
          );
          const custody = custodyAtEntry;
          const worker = custody?.qualified;
          const isWorker = (p) => worker && custody.isWorker(p);
          const sum = result.processes.reduce((n, p) => {
            // V1 performs metric aggregation here and validates live disposition
            // through its unchanged peer classifier. Preserve that callback contract.
            if (!custody) return n + Math.max(p.statusKiB.VmRSS, p.smapsRollupKiB.Rss);
            if (isWorker(p) && p.disposition !== "live") {
              assert(
                ["proved-departure", "custodied-departure", "retained-terminal-proof"].includes(
                  p.disposition,
                ) &&
                  p.terminalProof?.pidfdFlags === 0 &&
                  [1, 16, 17].includes(p.terminalProof.pollMask),
                "unverified Next exit",
              );
              if (p.rssKiB === null) return n;
              assert(Number.isSafeInteger(p.rssKiB) && p.rssKiB >= 0);
              return n + p.rssKiB;
            }
            assert(
              p.disposition === "live" &&
                Number.isSafeInteger(p.statusKiB?.VmRSS) &&
                Number.isSafeInteger(p.smapsRollupKiB?.Rss),
              "missing live peer metrics",
            );
            return n + Math.max(p.statusKiB.VmRSS, p.smapsRollupKiB.Rss);
          }, 0);
          assert(
            Number.isSafeInteger(sum) &&
              sum > 0 &&
              sum === result.summedRssKiB &&
              sum <= limits.rssCeilingKiB,
            "invalid peer owned RSS",
          );
          const roles = checkRoles(
            custody
              ? { ...result, processes: result.processes.filter((p) => !isWorker(p)) }
              : result,
            peer,
            Boolean(readyAtEntry),
          );
          for (const row of roles.chain) {
            const key = [row.pid, row.ppid, row.pgid, row.bootId, row.startTicks].join("/");
            assert(!stable.has(row.role) || stable.get(row.role) === key, "peer role replaced");
            stable.set(row.role, key);
          }
          assert.equal(roles.chain.length, stable.size, "previous peer role disappeared");
          append({
            type: "fixed-peer-roles",
            decisionNs: String(process.hrtime.bigint()),
            startedNs,
            readyAtEntry: Boolean(readyAtEntry),
            ...roles,
          });
          if (readyAtEntry) qualifiedPeer = roles;
          if (readyAtEntry && postCycles < 2) {
            const decision = process.hrtime.bigint();
            assert(
              firstSpawn <= readyNs &&
                readyNs <= BigInt(startedNs) &&
                BigInt(startedNs) <= decision &&
                decision <= readyNs + 3_000_000_000n,
              "peer readiness sampling window missed",
            );
            append({
              type: "fixed-peer-post-accepted",
              startedNs,
              readyNs: String(readyNs),
              decisionNs: String(decision),
            });
            if (++postCycles === 2) {
              clearTimeout(expiry);
              readyAtEntry.resolve();
            }
          }
          if (outerCycle)
            acceptedOuterCycle = {
              epoch: outerCycle.epoch,
              startedNs,
              coreResultNs: result.timing.parent.resultNs,
              cadenceDeadlineNs: outerCycle.cadenceDeadlineNs,
              helperBirth: outerCycle.helperBirth && { ...outerCycle.helperBirth },
            };
          return;
        }
        const children = result.processes.filter(
          (row) => row.disposition === "live" && row.ppid === server.handle.native.pid,
        );
        if (children.length > 1 || ((readyAtEntry || nativeBirth) && children.length !== 1))
          throw new Error("fixed safety native child absent or ambiguous");
        if (children.length) {
          const child = children[0];
          const key = [child.pid, child.startTicks, child.bootId].join("/");
          if (nativeBirth && nativeBirth !== key)
            throw new Error("fixed safety native child replaced");
          nativeBirth = key;
        }
        if (
          !Number.isSafeInteger(result.summedRssKiB) ||
          result.summedRssKiB <= 0 ||
          result.summedRssKiB > limits.rssCeilingKiB
        )
          throw Object.assign(
            new Error(`fixed safety RSS invalid or over ceiling: ${result.summedRssKiB}`),
            { details: result },
          );
        readyAtEntry?.resolve();
      })
      .catch(fail)
      .finally(() => {
        pending = null;
        pendingOuterCycle = null;
      });
    return pending;
  };
  take();
  const timer = setInterval(take, limits.sampleEveryMs);
  async function observeReady(actualReadyNs) {
    if (failure) throw new Error(failure.error);
    if (stopped) throw new Error("fixed safety sampling stopped before readiness");
    readyCheck ??= Promise.withResolvers();
    if (peer) {
      readyNs = actualReadyNs;
      const expire = () => {
        const left = readyNs + 3_000_000_000n - process.hrtime.bigint();
        if (left < 0n) fail(new Error("peer readiness sampling window expired"));
        else expiry = setTimeout(expire, Math.max(1, Math.ceil(Number(left) / 1e6)));
      };
      expire();
    }
    await readyCheck.promise;
    if (peer && process.hrtime.bigint() > readyNs + 3_000_000_000n)
      throw new Error("peer readiness sampling window expired");
    if (failure) throw new Error(failure.error);
    if (stopped) throw new Error("fixed safety sampling stopped before readiness");
  }
  return {
    verifyAstroCurrent(owned, known) {
      const describe = (p) => [p.pid, p.ppid, p.pgid, p.bootId, p.startTicks].join("/");
      const root = stable.get("astro-cli");
      assert(peer && root && !failure && !stopped && postCycles === 2);
      assert(
        [owned, known].every((rows) => rows.length === 1 && describe(rows[0]) === root),
        "unexpected current or previously observed Astro process",
      );
      return samplePeerRoles(
        { processes: owned.map((p) => ({ ...p, disposition: "live", executable: peer.image })) },
        peer,
        true,
      );
    },
    get qualifiedPeer() {
      return qualifiedPeer;
    },
    verifyOuterGraph(getOwned, checkObserver, stage = "outer-decision") {
      const entered = process.hrtime.bigint();
      let coverage = "not-checked",
        productGraph = "not-checked",
        observerGraph = "not-checked";
      try {
        // A live ordinary helper is an expected controller child. It still
        // prevents admission: refuse on the original cycle before invoking
        // either census, without waiting or assigning a birth from a PID.
        let reason = null;
        if (failure) reason = "sampler-failed";
        else if (stopped) reason = "sampler-stopped";
        else if (pending) reason = "ordinary-sample-pending";
        else if (decision) reason = "identity-decision-pending";
        else if (postCycles !== 2 || !acceptedOuterCycle) reason = "ready-coverage-incomplete";
        else if (entered >= BigInt(acceptedOuterCycle.cadenceDeadlineNs)) reason = "cadence-due";
        if (reason) {
          coverage = reason;
          throw new Error("outer decision outside current RSS coverage: " + reason);
        }
        coverage = "accepted";
        productGraph = "checking";
        const custody = nextOwner();
        const worker = custody?.qualified;
        assert(worker, "outer decision requires a qualified candidate");
        const owned = getOwned();
        assert(!owned.some((p) => custody.isWorker(p)), "outer decision requires a missing worker");
        const known = [...server.handle.known.values()];
        assert(
          known.some((p) => custody.isWorker(p)),
          "ordinary sampler has not recorded qualified birth",
        );
        peerProcessIdentity(
          owned,
          known.filter((p) => !custody.isWorker(p)),
          qualifiedPeer,
          peer,
        );
        productGraph = "accepted";
        if (checkObserver) {
          observerGraph = "checking";
          checkObserver();
          observerGraph = "accepted";
        }
        return {
          epoch: acceptedOuterCycle.epoch,
          cadenceDeadlineNs: acceptedOuterCycle.cadenceDeadlineNs,
        };
      } catch (error) {
        // Snapshot before asynchronous disarm/teardown. Later receipt or
        // closure from this same cycle cannot rewrite the refusal.
        error.outerRefusal = {
          stage,
          entryNs: String(entered),
          refusedNs: String(process.hrtime.bigint()),
          coverage,
          productGraph: productGraph === "checking" ? "failed" : productGraph,
          observerGraph: observerGraph === "checking" ? "failed" : observerGraph,
          lastAccepted: acceptedOuterCycle && {
            ...acceptedOuterCycle,
            helperBirth: acceptedOuterCycle.helperBirth && { ...acceptedOuterCycle.helperBirth },
          },
          observerGraphDetail: error.graphRefusal
            ? attributeGraphRefusal(error.graphRefusal, {
                pending: pendingOuterCycle?.helperBirth ?? null,
                lastAccepted: acceptedOuterCycle?.helperBirth ?? null,
              })
            : null,
          pending: pendingOuterCycle && {
            ...pendingOuterCycle,
            helperBirth: pendingOuterCycle.helperBirth && { ...pendingOuterCycle.helperBirth },
          },
          firstSamplerFailure: failure?.error ?? null,
        };
        throw error;
      }
    },
    // Waits only on the ordinary sample already in flight; it never dispatches,
    // never accepts the older cycle and leaves every refusal to verifyOuterGraph.
    async settleOrdinaryBeforeDecision({ deadlineNs, signal }) {
      const enteredNs = process.hrtime.bigint();
      const cycle = pendingOuterCycle;
      const record = {
        pendingAtDecision: Boolean(pending),
        enteredNs: String(enteredNs),
        waitedNs: "0",
        pendingEpoch: cycle?.epoch ?? null,
        settledEpoch: null,
        boundNs: null,
        outcome: pending ? "waiting" : "not-pending",
      };
      if (!pending || failure || stopped || !cycle) return record;
      const core = BigInt(cycle.startedNs) + ORDINARY_SETTLE_BOUND_NS;
      const boundNs = core < deadlineNs ? core : deadlineNs;
      record.boundNs = String(boundNs);
      let timer;
      try {
        if (enteredNs >= boundNs) record.outcome = "bound-expired";
        else {
          const expired = new Promise((resolve) => {
            timer = setTimeout(
              () => resolve("bound-expired"),
              Math.ceil(Number(boundNs - enteredNs) / 1e6),
            );
          });
          record.outcome = await interruptible(
            Promise.race([pending.then(() => "settled"), expired]),
            signal,
          );
        }
      } finally {
        clearTimeout(timer);
        const observed = process.hrtime.bigint();
        record.waitedNs = String(observed - enteredNs);
        if (record.outcome === "waiting") record.outcome = "aborted";
        record.settledEpoch = acceptedOuterCycle?.epoch ?? null;
      }
      return record;
    },
    async verifyPrewriteTerminal({ getOwned, workDeadlineNs, signal }) {
      const custody = nextOwner();
      assert(custody?.pollBeforeMutation && !failure && !stopped, "pre-write safety unavailable");
      const worker = custody.qualified;
      if (!worker || custody.terminalVerified || custody.decisionProof) return;
      const enteredNs = process.hrtime.bigint();
      const owned = getOwned();
      if (owned.some((p) => custody.isWorker(p))) return;
      // Check all other observed and remembered births first. The new helper is
      // eligible only for this single, otherwise-valid missing-worker decision.
      peerProcessIdentity(
        owned,
        [...server.handle.known.values()].filter((p) => !custody.isWorker(p)),
        qualifiedPeer,
        peer,
      );
      assert(!pending && !decision, "ordinary RSS helper is busy");
      decisionAbort = new AbortController();
      // The final caller takes another complete census. Aligning with the next
      // periodic timer would change the workload, so insufficient slack refuses.
      const deadlineNs =
        workDeadlineNs < nextDispatchNs - 5000000n ? workDeadlineNs : nextDispatchNs - 5000000n;
      decision = custody.pollBeforeMutation({
        deadlineNs,
        enteredNs,
        signal: signal ? AbortSignal.any([signal, decisionAbort.signal]) : decisionAbort.signal,
      });
      try {
        await decision;
        if (failure) throw new Error(failure.error);
      } finally {
        decision = null;
        decisionAbort = null;
      }
    },
    verifyPeerProcesses(owned, known, outerDecisionBirth) {
      if (failure || stopped) throw new Error(failure?.error ?? "fixed peer stopped");
      const custody = nextOwner();
      if (!custody) return peerProcessIdentity(owned, known, qualifiedPeer, peer);
      const isWorker = (p) => custody.isWorker(p);
      // Scout qualification can precede the next sampler's known-map commit.
      if (custody.qualified && !owned.some(isWorker))
        assert(
          custody.terminalVerified ||
            custody.decisionProof ||
            (fixedExecutables.protocol === "linux-fixed-peer.v4" &&
              outerDecisionBirth &&
              custody.isWorker(outerDecisionBirth)),
          "known Next worker missing before official terminal decision",
        );
      const results = peerProcessIdentity(
        owned.filter((p) => !isWorker(p)),
        known.filter((p) => !isWorker(p)),
        qualifiedPeer,
        peer,
      );
      for (const p of owned.filter(isWorker)) results.push(custody.verifyCurrent(p));
      return results;
    },
    report() {
      return {
        protocol: fixedExecutables.protocol ?? "linux-fixed-zfb.v1",
        measuredCycles,
        peakSummedOwnedRssKiB: peakKiB,
        failure,
        secondaryHelperFailure,
        fixedExecutables,
      };
    },
    get unresolvedHelper() {
      return unresolvedHelper;
    },
    // Only the zero-edit stock adapter uses this read-only milestone. All
    // normal workload entry points still require full readiness below.
    async verifyZeroEditObservationReady() {
      if (fixedExecutables.protocol !== "linux-fixed-zfb.v4")
        throw new Error("observation-only readiness requires startup custody v4");
      await observeReady();
    },
    async verifyReady(actualReadyNs) {
      await observeReady(actualReadyNs);
      custody?.verifyStartup?.();
    },
    async stop() {
      stopped = true;
      readyCheck?.resolve();
      clearInterval(timer);
      clearTimeout(expiry);
      decisionAbort?.abort(new Error("fixed safety is stopping"));
      if (decision) await Promise.allSettled([decision, pending]);
      else await pending;
      return failure;
    },
  };
}
