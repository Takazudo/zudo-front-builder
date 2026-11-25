import { fork } from "node:child_process";
import { RENDERER_MEMORY_MAPPING_LIMITS as limits } from "./renderer-memory-protocol.mjs";

const worker = new URL("./renderer-memory-mapping-worker.mjs", import.meta.url);

/**
 * A direct child isolates procfs reads and parsing from the one-second safety sampler.
 * Only its unreaped child handle is signalled. A deadline failure returns immediately
 * for restoration; finalization separately verifies actual child/pipe closure.
 */
export function createMappingCollector({
  launch = (args) =>
    fork(worker, [JSON.stringify(args)], {
      execArgv: [],
      stdio: ["ignore", "ignore", "ignore", "ipc"],
    }),
} = {}) {
  let active = null;

  function collect({ expected, executablePath, artifactDir, boundary, timeoutMs, signal }) {
    signal.throwIfAborted();
    if (active) throw new Error("mapping collector still active");
    const clampedMs = Math.min(timeoutMs, limits.timeMs);
    const deadline = process.hrtime.bigint() + BigInt(clampedMs) * 1_000_000n;
    const child = launch({
      expected,
      executablePath,
      artifactDir,
      boundary,
      deadlineNs: String(deadline),
    });
    const close = new Promise((resolve) => {
      child.once("close", (code, sig) => {
        active = null;
        resolve({ code, sig });
      });
    });
    active = { child, close, boundary };
    return new Promise((resolve, reject) => {
      let report;
      const finish = (error, value) => {
        clearTimeout(timer);
        signal.removeEventListener("abort", abort);
        if (error) reject(error);
        else resolve(value);
      };
      const cancel = (reason) => {
        if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
        finish(new Error(reason));
      };
      const abort = () => cancel(`mapping capture interrupted: ${signal.reason}`);
      const remainingMs = Number(deadline - process.hrtime.bigint()) / 1e6;
      const timer = setTimeout(
        () => cancel("mapping capture deadline reached"),
        Math.max(0, remainingMs),
      );
      signal.addEventListener("abort", abort, { once: true });
      child.once("message", (message) => {
        report = message;
      });
      child.once("error", (error) => finish(error));
      close.then(({ code, sig }) => {
        if (process.hrtime.bigint() > deadline)
          finish(new Error("mapping capture deadline reached"));
        else if (report?.error) {
          finish(
            Object.assign(new Error(report.error), { mappingEvidence: report.evidence ?? null }),
          );
        } else if (code !== 0 || !report?.result) {
          finish(
            new Error(
              `mapping collector exited without a valid result (code ${code}, signal ${sig})`,
            ),
          );
        } else finish(null, { ...report.result, collectorPid: child.pid, collectorClosed: true });
      });
      if (signal.aborted) abort();
    });
  }

  return {
    collect,
    get activePid() {
      return active?.child.pid ?? null;
    },
    get activeBoundary() {
      return active?.boundary ?? null;
    },
    releaseUnresolvedHandle() {
      const child = active?.child;
      if (child?.connected) child.disconnect();
      child?.unref();
    },
    stop() {
      if (!active) return Promise.resolve();
      const { child, close } = active;
      if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
      return close;
    },
  };
}
