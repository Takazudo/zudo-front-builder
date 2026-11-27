// Only the direct Linux zfb distribution uses this contract: Node runs the
// pinned shim; the materialized, pinned native binary is its direct child.
import { closeSync, fstatSync, openSync, readSync, realpathSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";
import assert from "node:assert/strict";
import { samplePeerRoles, FIXED_ASTRO_FOOTER_PROTOCOL } from "./fixed-peer-roles.mjs";
import { sampleRendererMemory } from "./linux-rss-sample.mjs";

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

export function startFixedRssSafety({
  server,
  fixedExecutables,
  limits,
  append,
  abort,
  sample = sampleRendererMemory,
  joinOwnedFamily = false,
  custody = null,
  peer,
  checkRoles = samplePeerRoles,
}) {
  let pending = null;
  let active = null;
  let failure = null;
  let secondaryHelperFailure = null;
  let nativeBirth = null;
  let unresolvedHelper = null;
  let readyCheck = null;
  let stopped = false;
  let measuredCycles = 0;
  let lastAccepted;
  let peakKiB = null;
  const stable = new Map();
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
    if (pending) {
      fail(new Error("fixed safety sampler still occupied at next periodic dispatch"));
      return;
    }
    // A startup sample already in flight when the readiness line arrives can
    // legitimately predate the native child. Qualify a new periodic sample.
    const readyAtEntry = readyCheck;
    const startedNs = String(process.hrtime.bigint());
    const cycle = joinOwnedFamily ? { epoch: measuredCycles + 1, startedNs, helper: null } : null;
    active = cycle;
    pending = (
      peer
        ? sample(server, fixedExecutables.root.path)
        : sample(server, fixedExecutables.root.path, {
            fixedExecutables,
            custody,
            ...(cycle && {
              onHelperSpawn: (h) => {
                cycle.helper = h;
              },
            }),
          })
    )
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
          const sum = result.processes.reduce(
            (n, p) => n + Math.max(p.statusKiB.VmRSS, p.smapsRollupKiB.Rss),
            0,
          );
          assert(
            Number.isSafeInteger(sum) &&
              sum > 0 &&
              sum === result.summedRssKiB &&
              sum <= limits.rssCeilingKiB,
            "invalid peer owned RSS",
          );
          const roles = checkRoles(result, peer, Boolean(readyAtEntry));
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
        if (fixedExecutables.protocol === "linux-fixed-zfb.v4")
          lastAccepted = {
            startedNs,
            resultNs: result.timing?.parent?.resultNs,
            epoch: measuredCycles,
            ...(joinOwnedFamily && {
              helperPid: result.helper?.pid,
              helperSpawnNs: result.helper?.spawnHrtimeNs,
              entryNs: result.timing?.parent?.entryNs,
              deadlineNs: result.timing?.parent?.deadlineNs,
            }),
            strict:
              result.helperResult === "complete" &&
              result.helper?.closed &&
              result.helper?.pipesClosed &&
              result.timing?.clock?.qualified &&
              result.startupEpochCoverage?.complete &&
              Number.isSafeInteger(result.summedRssKiB) &&
              result.summedRssKiB > 0 &&
              result.summedRssKiB <= limits.rssCeilingKiB,
            live: result.processes
              .filter((row) => row.disposition === "live")
              .map(({ pid, ppid, pgid, bootId, startTicks, birthVersion }) => ({
                pid,
                ppid,
                pgid,
                bootId,
                startTicks,
                birthVersion,
              })),
          };
        readyAtEntry?.resolve();
      })
      .catch(fail)
      .finally(() => {
        pending = null;
        active = null;
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
    async verifyFamilyBrowserReady() {
      if (fixedExecutables.protocol !== "linux-fixed-zfb.v4")
        throw Error("family observation requires v4");
      await observeReady();
      if (!lastAccepted?.strict || lastAccepted.live.length !== 2 || !nativeBirth)
        throw Error("family blank setup requires complete root/native RSS observation");
    },
    captureFamilyCycle(receipt) {
      const proof = lastAccepted;
      Object.assign(receipt, {
        capturedNs: String(process.hrtime.bigint()),
        v4: fixedExecutables.protocol === "linux-fixed-zfb.v4",
        failed: Boolean(failure),
        stopped,
        pending: Boolean(pending),
        strict: Boolean(proof?.strict && proof.resultNs),
        previousEpoch: proof?.epoch ?? null,
        cycle: active ? structuredClone(active) : null,
      });
      assert(
        joinOwnedFamily && receipt.v4 && !failure && !stopped && receipt.strict && nativeBirth,
        "family observation unavailable before settlement",
      );
      const before = { sample: proof, ...structuredClone(custody.familyProof()) };
      assert(
        Number.isSafeInteger(before.release?.receipt?.epoch) &&
          before.release.receipt.epoch <= proof.epoch,
        "family release lacks a previously accepted epoch",
      );
      if (!pending) return { before: this.verifyFamilyAdmission(), settlement: null };
      const { helper, epoch, startedNs } = active;
      assert(
        helper &&
          Number.isSafeInteger(helper.pid) &&
          helper.pid > 0 &&
          epoch === proof.epoch + 1 &&
          BigInt(startedNs) <= BigInt(helper.entryNs) &&
          BigInt(helper.entryNs) <= BigInt(helper.spawnHrtimeNs) &&
          BigInt(helper.spawnHrtimeNs) <= BigInt(receipt.capturedNs) &&
          BigInt(helper.deadlineNs) === BigInt(helper.entryNs) + 750_000_000n &&
          BigInt(receipt.capturedNs) < BigInt(helper.deadlineNs),
        "invalid or expired family helper",
      );
      return { before, settlement: pending };
    },
    verifyFamilyAdmission() {
      if (
        fixedExecutables.protocol !== "linux-fixed-zfb.v4" ||
        failure ||
        stopped ||
        pending ||
        !lastAccepted?.strict ||
        !lastAccepted.resultNs
      )
        throw Error("family RSS observation missing, failed, pending or stopped");
      if (
        process.hrtime.bigint() >=
        BigInt(lastAccepted.startedNs) + BigInt(limits.sampleEveryMs) * 1000000n
      )
        throw Error("family RSS cadence due before admission");
      return { sample: lastAccepted, ...custody.familyProof() };
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
      await pending;
      return failure;
    },
  };
}
