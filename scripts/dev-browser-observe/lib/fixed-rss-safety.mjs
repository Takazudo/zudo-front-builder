// Only the direct Linux zfb distribution uses this contract: Node runs the
// pinned shim; the materialized, pinned native binary is its direct child.
import { closeSync, fstatSync, openSync, readSync, realpathSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";
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

export function fixedZfbExecutables(appDir, facts, protocol = "linux-fixed-zfb.v1") {
  const child = facts.install.executables.find((exe) => exe.role === "zfb-native" && exe.ok);
  if (!child || facts.installReceipt.node.realpath !== realpathSync(process.execPath))
    throw new Error("fixed safety requires the verified Node launcher and packaged native binary");
  let launch;
  if (protocol === "linux-fixed-zfb.v2") {
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
    pending = sample(server, fixedExecutables.root.path, { fixedExecutables })
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
      });
    return pending;
  };
  take();
  const timer = setInterval(take, limits.sampleEveryMs);
  return {
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
    async verifyReady() {
      if (failure) throw new Error(failure.error);
      if (stopped) throw new Error("fixed safety sampling stopped before readiness");
      readyCheck ??= Promise.withResolvers();
      await readyCheck.promise;
      if (failure) throw new Error(failure.error);
      if (stopped) throw new Error("fixed safety sampling stopped before readiness");
    },
    async stop() {
      stopped = true;
      readyCheck?.resolve();
      clearInterval(timer);
      await pending;
      return failure;
    },
  };
}
