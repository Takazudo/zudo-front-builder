// Only the direct Linux zfb distribution uses this contract: Node runs the
// pinned shim; the materialized, pinned native binary is its direct child.
import { closeSync, fstatSync, openSync, readSync, realpathSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";
import assert from "node:assert/strict";
import { samplePeerRoles, peerProcessIdentity, FIXED_PEER_PROTOCOL } from "./fixed-peer-roles.mjs";
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

export function fixedPeerExecutables(facts) {
  if (facts.installReceipt.node.realpath !== realpathSync(process.execPath))
    throw new Error("fixed peer Node executable does not match install receipt");
  return {
    protocol: FIXED_PEER_PROTOCOL,
    root: pin(process.execPath, facts.installReceipt.node.sha256),
  };
}

export function fixedZfbExecutables(appDir, facts) {
  const child = facts.install.executables.find((exe) => exe.role === "zfb-native" && exe.ok);
  if (!child || facts.installReceipt.node.realpath !== realpathSync(process.execPath))
    throw new Error("fixed safety requires the verified Node launcher and packaged native binary");
  return {
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
  peer,
  checkRoles = samplePeerRoles,
}) {
  let pending = null;
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
    const partial = error?.details?.summedRssKiB;
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
    pending = (
      peer
        ? sample(server, fixedExecutables.root.path)
        : sample(server, fixedExecutables.root.path, { fixedExecutables })
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
          return;
        }
        const children = result.processes.filter(
          (row) => row.disposition === "live" && row.pid !== server.handle.native.pid,
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
      });
    return pending;
  };
  take();
  const timer = setInterval(take, limits.sampleEveryMs);
  return {
    verifyPeerProcesses(owned, known) {
      if (failure || stopped) throw new Error(failure?.error ?? "fixed peer stopped");
      return peerProcessIdentity(owned, known, qualifiedPeer, peer);
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
    async verifyReady(actualReadyNs) {
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
