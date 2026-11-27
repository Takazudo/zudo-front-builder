import { spawn } from "node:child_process";
import { readFileSync, readlinkSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { LINUX_BIRTH, sameLinuxBirth } from "./linux-birth.mjs";
import { LINUX_RSS_SAMPLER_VERSION as samplerVersion } from "./renderer-memory-protocol.mjs";

const script = fileURLToPath(new URL("./linux-rss-sample.py", import.meta.url));
const maxOwned = 64;
const maxLine = 128 * 1024;
const maxOutput = 512 * 1024;
const budgetMs = 750;
const closeMs = 200;
const terminalProofs = new WeakMap();
const uint = (n) => Number.isSafeInteger(n) && n >= 0;
const ticks = (s) => typeof s === "string" && /^\d{1,20}$/.test(s);
const same = (a, b) => sameLinuxBirth(a, b) && a.pgid === b.pgid;

function matchesExecutable(observed, expected) {
  return (
    observed?.path === expected.path &&
    observed.device === expected.device &&
    observed.inode === expected.inode
  );
}

function requireValue(ok, msg) {
  if (!ok) throw new Error(msg);
}

function compactBirth(p) {
  return {
    pid: p.pid,
    pgid: p.pgid,
    ppid: p.ppid,
    bootId: p.bootId,
    startTicks: p.startTicks,
    birthVersion: p.birthVersion,
  };
}

function checkBirth(row, bootId) {
  requireValue(
    row?.birthVersion === LINUX_BIRTH &&
      row.bootId === bootId &&
      uint(row.pid) &&
      row.pid > 0 &&
      uint(row.pgid) &&
      row.pgid > 0 &&
      uint(row.ppid) &&
      ticks(row.startTicks) &&
      row.birthUnverifiable === undefined &&
      typeof row.comm === "string" &&
      Buffer.byteLength(row.comm) <= 64 &&
      /^[RSDZTtXxKWPI]$/.test(row.state),
    "malformed sample birth",
  );
}

function checkProof(proof, birth) {
  requireValue(
    same(proof?.birth, birth) &&
      proof.pidfdFlags === 0 &&
      [1, 16, 17].includes(proof.pollMask) &&
      ticks(proof.observedNs),
    "malformed group-pidfd proof",
  );
}

const timingVersion = "linux-rss-timing.v1";
const now = () => process.hrtime.bigint().toString();
const nullFields = (names) => Object.fromEntries(names.map((name) => [name, null]));
const requestTimes = ["entryNs", "requestReadStartNs", "requestReadEndNs", "requestDecodedNs"];
const outputTimes = ["serializeStartNs", "serializedNs", "writtenNs", "flushedNs"];
const stageTimes = {
  request: requestTimes,
  finalRootBegin: ["finalRootBeginNs"],
  finalRoot: ["finalRootStartNs", "finalRootEndNs"],
  cleanupBegin: ["cleanupBeginNs"],
  fdCleanup: ["cleanupStartNs", "cleanupEndNs"],
  terminalOutput: [
    "terminalPrepareStartNs",
    "terminalPreparedNs",
    "terminalSerializeStartNs",
    "terminalSerializedNs",
    "terminalWrittenNs",
    "terminalFlushedNs",
  ],
  preExit: ["preExitNs"],
  observationOutput: outputTimes,
};

function parentClock() {
  const clock = {
    implementation: "process.hrtime.bigint/uv_hrtime",
    clockId: process.platform === "linux" ? "CLOCK_MONOTONIC" : null,
    platform: process.platform,
    libuv: process.versions.uv,
    bootId: null,
    timeNamespace: null,
  };
  if (process.platform === "linux") {
    try {
      clock.bootId = readFileSync("/proc/sys/kernel/random/boot_id", "ascii").trim();
      clock.timeNamespace = readlinkSync("/proc/self/ns/time");
    } catch {
      // Missing clock evidence leaves both per-process timelines usable.
    }
  }
  return clock;
}

function newTiming(start, deadline) {
  const parent = {
    ...nullFields([
      "spawnCallNs",
      "spawnReturnNs",
      "spawnEventNs",
      "requestStartNs",
      "requestEndNs",
      "requestFinishedNs",
      "firstDataNs",
      "lastDataNs",
      "lastDataEndNs",
      "parsingNs",
      "journalNs",
      "exitNs",
      "exitCode",
      "exitSignal",
      "stdinCloseNs",
      "stdoutEndNs",
      "stdoutCloseNs",
      "stderrEndNs",
      "stderrCloseNs",
      "closeNs",
      "deadlineDispatchNs",
      "closureDeadlineNs",
      "closureDispatchNs",
      "terminalReceiptNs",
      "terminalFrameCallbackNs",
      "preExitFrameCallbackNs",
      "terminalOutputFrameCallbackNs",
      "dataHandlingNs",
      "parseAndValidateNs",
      "requestBytes",
      "resultNs",
    ]),
    entryNs: String(start),
    deadlineNs: String(deadline),
    dataCallbacks: 0,
    parsedRows: 0,
    journalCalls: 0,
    journal: [],
    stdoutBytes: 0,
    stderrBytes: 0,
  };
  return {
    version: timingVersion,
    parent,
    helper: {
      ...nullFields(
        Object.entries(stageTimes)
          .filter(([stage]) => stage !== "observationOutput")
          .flatMap(([, names]) => names),
      ),
      observations: [],
      frames: 0,
    },
    clock: {
      parent: parentClock(),
      helper: null,
      qualified: false,
      reason: "missing-helper-clock",
      preExitToExitDeliveryResidualNs: null,
    },
  };
}

function qualify(timing, envelopeOk) {
  const { parent, helper, clock } = timing;
  const p = clock.parent;
  const h = clock.helper;
  if (!h) return;
  if (p.platform !== "linux" || !p.libuv || h.implementation !== "clock_gettime(CLOCK_MONOTONIC)")
    clock.reason = "clock-implementation";
  else if (!p.bootId || p.bootId !== h.bootId) clock.reason = "boot-identity";
  else if (!/^time:\[\d+\]$/.test(p.timeNamespace) || p.timeNamespace !== h.timeNamespace)
    clock.reason = "time-namespace";
  else if (
    !envelopeOk ||
    (helper.preExitNs !== null &&
      parent.exitNs !== null &&
      BigInt(helper.preExitNs) > BigInt(parent.exitNs))
  )
    clock.reason = "causal-envelope";
  else {
    clock.qualified = true;
    clock.reason = null;
    // Includes the last timing frame, interpreter shutdown, OS exit, and parent delivery.
    if (helper.preExitNs !== null && parent.exitNs !== null)
      clock.preExitToExitDeliveryResidualNs = String(
        BigInt(parent.exitNs) - BigInt(helper.preExitNs),
      );
  }
}

/** A single direct child scans, pins and samples the owned set in one bounded cycle. */
export function sampleRendererMemory(
  server,
  expectedExecutablePath,
  {
    fixedExecutables = null,
    launch = () =>
      spawn("python3", ["-I", "-B", script, String(process.pid)], {
        stdio: ["pipe", "pipe", "pipe"],
      }),
  } = {},
) {
  const at = new Date().toISOString();
  const start = process.hrtime.bigint();
  const deadline = start + BigInt(budgetMs) * 1_000_000n;
  const timing = newTiming(start, deadline);
  const parent = timing.parent;
  const stamps = timing.helper;
  let envelopeOk = true;
  const { handle } = server;
  const root = handle.native;
  const proofs = terminalProofs.get(handle) ?? new Map();
  terminalProofs.set(handle, proofs);
  const processes = [];
  const discoveries = [];
  const journalFailures = [];
  const bound = new Map();
  const inTree = new Set();
  const rss = new Map();
  let summedRssKiB = null;
  const observed = new Set();
  const rssSources = new Map();
  let helper = null;
  let executable = null;
  let summary = null;
  let fault = null;
  let stderr = "";
  let finished = false;

  const details = () => ({
    samplerVersion,
    at,
    hrtimeNs: String(start),
    endHrtimeNs: parent.resultNs,
    timing,
    helper,
    verifiedDiscoveries: discoveries,
    journalFailures,
    processes,
    summedRssKiB,
    observedRssKiB: Object.fromEntries(rssSources),
    rssBasis: "per-birth maximum of VmRSS and smaps_rollup.Rss observed in this cycle",
    ...summary,
  });

  const fail = (stage, message) => {
    fault ??= { stage, error: message };
  };
  return new Promise((resolve, reject) => {
    let child, timer, closeTimer;
    const finish = () => {
      if (finished) return;
      finished = true;
      clearTimeout(timer);
      clearTimeout(closeTimer);
      qualify(timing, envelopeOk);
      parent.resultNs = now();
      if (BigInt(parent.resultNs) > deadline) fail("deadline", "sampling helper deadline reached");
      const record = details();
      if (fault)
        reject(
          Object.assign(new Error(fault.error), {
            details: {
              expected: root ? compactBirth(root) : null,
              observed: null,
              terminalProof: null,
              boundBirths: [...bound.values()].map(compactBirth),
              ...record,
              ...fault,
              stderr,
            },
          }),
        );
      else resolve(record);
    };

    const kill = () => {
      if (child?.pid && child.exitCode === null && child.signalCode === null) child.kill("SIGKILL"); // direct, unreaped ChildProcess handle only
      if (child && !closeTimer) {
        parent.closureDeadlineNs = String(process.hrtime.bigint() + BigInt(closeMs) * 1_000_000n);
        closeTimer = setTimeout(() => {
          parent.closureDispatchNs = now();
          fail("helper-close", "sampling helper closure unverified");
          // Leave the failed resource identity in the receipt for admission.
          for (const pipe of [child.stdin, child.stdout, child.stderr]) pipe.destroy();
          child.unref();
          finish();
        }, closeMs);
      }
    };
    let request, spawnHrtimeNs;
    try {
      requireValue(
        server.isAlive().alive &&
          root?.birthVersion === LINUX_BIRTH &&
          !root.birthUnverifiable &&
          root.pgid === root.pid &&
          /^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(root.bootId) &&
          same(root, handle.known?.get(root.pid)) &&
          handle.known.size <= maxOwned,
        "missing or unverifiable live sampling root",
      );
      requireValue(
        typeof expectedExecutablePath === "string" &&
          expectedExecutablePath.startsWith("/") &&
          Buffer.byteLength(expectedExecutablePath) <= 4096,
        "invalid expected executable path",
      );
      request = {
        root: compactBirth(root),
        known: [...handle.known.values()].map(compactBirth),
        terminalProofs: [...proofs.values()],
        executablePath: expectedExecutablePath,
        ...(fixedExecutables && { fixedChildExecutable: fixedExecutables.child }),
      };
      spawnHrtimeNs = parent.spawnCallNs = now();
      child = launch();
      parent.spawnReturnNs = now();
    } catch (error) {
      fail("start", String(error.message));
      finish();
      return;
    }
    helper = {
      pid: child.pid ?? null,
      spawnHrtimeNs,
      birth: null,
      closed: false,
      pipesClosed: false,
    };

    let partial = Buffer.alloc(0);
    let bytes = 0;
    let protocolBroken = false;
    let requiredStage = "request";
    let pendingObservation = null;
    let callbackNs;
    let lastFrameNs = 0n;
    const consumeTiming = (row) => {
      const { stage, ns } = row;
      requireValue(
        typeof stage === "string" &&
          Object.hasOwn(stageTimes, stage) &&
          row.version === timingVersion &&
          row.seq === stamps.frames &&
          row.seq < maxOwned + 8 &&
          Array.isArray(ns) &&
          ns.length === stageTimes[stage].length &&
          ns.every(ticks) &&
          Object.keys(row).length === (["request", "observationOutput"].includes(stage) ? 6 : 5) &&
          ns.every((n, i) => !i || BigInt(n) >= BigInt(ns[i - 1])),
        "invalid helper timing frame",
      );
      requireValue(
        (requiredStage === null || requiredStage === stage) &&
          stamps.preExitNs === null &&
          (row.seq === 0) === (stage === "request") &&
          (pendingObservation !== null) === (stage === "observationOutput") &&
          (stage === "observationOutput" || stamps[stageTimes[stage][0]] === null),
        "unexpected helper timing frame",
      );
      // A deferred failure can be prepared before earlier rows are flushed. Its
      // actual serialization still follows every previously received timing span.
      const first = stage === "terminalOutput" ? ns[2] : ns[0];
      requireValue(BigInt(first) >= lastFrameNs, "out-of-order helper timing span");
      lastFrameNs = BigInt(ns.at(-1));
      if (lastFrameNs > BigInt(callbackNs)) envelopeOk = false;
      if (stage === "request") {
        const c = row.clock;
        const optionalString = (s) => s === null || (typeof s === "string" && s.length <= 80);
        requireValue(
          c &&
            Object.keys(c).length === 3 &&
            typeof c.implementation === "string" &&
            c.implementation.length <= 80 &&
            optionalString(c.timeNamespace) &&
            optionalString(c.bootId),
          "malformed helper clock",
        );
        timing.clock.helper = c;
        if (
          BigInt(ns[0]) < BigInt(parent.spawnCallNs) ||
          BigInt(ns[2]) < BigInt(parent.requestStartNs)
        )
          envelopeOk = false;
      } else if (stage === "observationOutput") {
        requireValue(
          pendingObservation.pid === row.pid &&
            BigInt(ns[0]) >= BigInt(pendingObservation.metadataEndNs),
          "unmatched observation timing",
        );
        for (const [i, name] of outputTimes.entries()) pendingObservation[name] = ns[i];
        pendingObservation.outputFrameCallbackNs = callbackNs;
        pendingObservation = null;
      } else if (stage === "finalRootBegin") {
        requireValue(
          !summary && stamps.cleanupBeginNs === null && observed.size === handle.known.size,
          "unexpected final root timing",
        );
      } else if (stage === "finalRoot") {
        requireValue(
          !summary && stamps.finalRootBeginNs !== null && stamps.cleanupBeginNs === null,
          "unexpected final root span",
        );
      } else if (stage === "cleanupBegin") {
        requireValue(
          stamps.finalRootBeginNs === null || stamps.finalRootEndNs !== null,
          "unexpected cleanup timing",
        );
      } else if (stage === "fdCleanup") {
        requireValue(stamps.cleanupBeginNs !== null, "unexpected cleanup span");
      } else if (stage === "terminalOutput") {
        requireValue(summary, "terminal timing without a terminal result");
        parent.terminalOutputFrameCallbackNs = callbackNs;
      } else {
        parent.preExitFrameCallbackNs = callbackNs;
        requireValue(
          stamps.terminalFlushedNs !== null &&
            (stamps.cleanupBeginNs === null || stamps.cleanupEndNs !== null),
          "incomplete pre-exit timing",
        );
      }
      if (stage !== "observationOutput")
        for (const [i, name] of stageTimes[stage].entries()) stamps[name] = ns[i];
      stamps.frames++;
      // Paired checkpoints cannot be interleaved. A failure can precede or
      // follow descriptor cleanup, depending on where it was observed.
      requiredStage =
        { finalRootBegin: "finalRoot", finalRoot: "cleanupBegin", cleanupBegin: "fdCleanup" }[
          stage
        ] ?? null;
    };
    const consume = (row) => {
      if (row.type === "timing") return consumeTiming(row);
      requireValue(requiredStage === null, "missing helper timing frame");
      requireValue(!summary, "sample output after terminal row");
      requireValue(
        (stamps.finalRootBeginNs === null && stamps.cleanupBeginNs === null) ||
          ["complete", "failure"].includes(row.type),
        "sample row during finalization",
      );
      if (row.type === "helper") {
        const birth = row.birth;
        checkBirth(birth, root.bootId);
        requireValue(
          !helper.birth && birth.pid === child.pid && birth.ppid === process.pid,
          "incorrect direct sampling helper identity",
        );
        helper.birth = { ...compactBirth(birth), state: birth.state, comm: birth.comm };
        return;
      }
      if (row.type === "executable") {
        const exe = row.executable;
        requireValue(
          helper.birth &&
            !executable &&
            row.pathInput === expectedExecutablePath &&
            typeof exe?.path === "string" &&
            exe.path.startsWith("/") &&
            Buffer.byteLength(exe.path) <= 4096 &&
            ticks(exe.device) &&
            ticks(exe.inode) &&
            (!fixedExecutables || matchesExecutable(exe, fixedExecutables.root)),
          "invalid expected executable identity",
        );
        executable = exe;
        return;
      }
      requireValue(row.type === "failure" || executable, "sample data before helper identity");
      if (row.type === "bound") {
        const p = row.birth;
        checkBirth(p, root.bootId);
        const b = row.binding;
        requireValue(
          b?.pidfdFlags === 0 && b.fdinfoPid === p.pid && ticks(b.verifiedNs),
          "unverified pidfd discovery",
        );
        requireValue(p.pid === root.pid || bound.has(root.pid), "discovery before bound root");
        const old = handle.known.get(p.pid);
        requireValue(
          !bound.has(p.pid) && (!old || same(old, p)),
          "duplicate or conflicting bound birth",
        );
        const reachable = p.pgid === root.pgid || inTree.has(p.ppid);
        requireValue(reachable || old, "unverified discovery ancestry");
        requireValue(old || handle.known.size < maxOwned, "verified ownership cap reached");
        if (reachable) inTree.add(p.pid);
        bound.set(p.pid, p);
        if (!old) {
          const entry = {
            ...compactBirth(p),
            state: p.state,
            comm: p.comm,
            source: samplerVersion,
            binding: { pidfdFlags: 0, fdinfoPid: p.pid, verifiedNs: b.verifiedNs },
          };
          handle.known.set(p.pid, entry);
          discoveries.push(entry);
          try {
            requireValue(typeof handle.onNewOwned === "function", "ownership journal unavailable");
            const journalStart = process.hrtime.bigint();
            parent.journalCalls++;
            try {
              handle.onNewOwned(entry);
            } finally {
              const end = process.hrtime.bigint();
              parent.journal.push({
                pid: p.pid,
                startNs: String(journalStart),
                endNs: String(end),
              });
              parent.journalNs = String(BigInt(parent.journalNs ?? 0) + end - journalStart);
            }
          } catch (error) {
            const message = String(error.message).slice(0, 256);
            journalFailures.push({ pid: p.pid, error: message });
            fail("ownership-journal", message);
            kill();
          }
        }
      } else if (row.type === "rss") {
        const p = row.birth;
        const values = rssSources.get(p?.pid) ?? {};
        requireValue(
          same(bound.get(p?.pid), p) &&
            ["VmRSS", "rollup.Rss"].includes(row.source) &&
            !Object.hasOwn(values, row.source) &&
            uint(row.rssKiB) &&
            ticks(row.observedNs),
          "invalid bound RSS observation",
        );
        values[row.source] = row.rssKiB;
        rssSources.set(p.pid, values);
        const before = rss.get(p.pid) ?? 0;
        const after = Math.max(before, row.rssKiB);
        const total = (summedRssKiB ?? 0) + (after - before);
        requireValue(Number.isSafeInteger(total), "summed RSS exceeds safe integer range");
        rss.set(p.pid, after);
        summedRssKiB = total;
      } else if (row.type === "observation") {
        const p = row.process;
        const prior = proofs.get(p?.pid);
        requireValue(
          same(handle.known.get(p?.pid), p) &&
            !observed.has(p.pid) &&
            ticks(p.startedNs) &&
            ticks(p.endNs) &&
            BigInt(p.endNs) >= BigInt(p.startedNs) &&
            p.rssKiB === (rss.get(p.pid) ?? null),
          "inconsistent process observation",
        );
        if (p.disposition === "retained-terminal-proof") {
          checkProof(p.terminalProof, p);
          requireValue(
            prior &&
              prior.observedNs === p.terminalProof.observedNs &&
              prior.pollMask === p.terminalProof.pollMask &&
              !bound.has(p.pid),
            "unverified retained terminal proof",
          );
        } else {
          requireValue(same(bound.get(p.pid), p), "observation without a matching binding");
          if (p.disposition === "proved-departure") {
            requireValue(p.pid !== root.pid, "sampling root terminated");
            checkProof(p.terminalProof, p);
            proofs.set(p.pid, p.terminalProof);
          } else {
            requireValue(
              p.disposition === "live" &&
                !prior &&
                uint(p.rssKiB) &&
                uint(p.statusKiB?.VmRSS) &&
                p.statusKiB.VmRSS === rssSources.get(p.pid)?.VmRSS &&
                uint(p.smapsRollupKiB?.Rss) &&
                p.smapsRollupKiB.Rss === rssSources.get(p.pid)?.["rollup.Rss"] &&
                (fixedExecutables && p.pid !== root.pid
                  ? p.ppid === root.pid && matchesExecutable(p.executable, fixedExecutables.child)
                  : matchesExecutable(p.executable, executable)) &&
                uint(p.threads?.listed) &&
                Array.isArray(p.threads?.rendererHosts),
              "invalid live process observation",
            );
          }
        }
        observed.add(p.pid);
        processes.push(p);
        pendingObservation = {
          pid: p.pid,
          metadataStartNs: p.startedNs,
          metadataEndNs: p.endNs,
          receivedNs: callbackNs,
          outputFrameCallbackNs: null,
          ...nullFields(outputTimes),
        };
        stamps.observations.push(pendingObservation);
        requiredStage = "observationOutput";
      } else if (row.type === "failure") {
        requireValue(
          typeof row.stage === "string" && typeof row.error === "string",
          "invalid sampler failure row",
        );
        const {
          stage,
          error,
          errno,
          syscallError,
          expected,
          observed,
          terminalProof,
          partialProcess,
          startedNs,
          endNs,
          readCalls,
          procInputBytes,
          helperMaxRssKiB,
        } = row;
        requireValue(
          stage.length <= 64 &&
            error.length <= 256 &&
            (errno === null || uint(errno)) &&
            (syscallError === null || typeof syscallError === "string") &&
            ticks(startedNs) &&
            ticks(endNs) &&
            uint(readCalls) &&
            uint(procInputBytes) &&
            uint(helperMaxRssKiB),
          "invalid sampler failure fields",
        );
        fault ??= {
          stage,
          error,
          errno,
          syscallError,
          expected,
          observed,
          terminalProof,
          partialProcess,
          startedNs,
          endNs,
          readCalls,
          procInputBytes,
          helperMaxRssKiB,
        };
        parent.terminalFrameCallbackNs = callbackNs;
        parent.terminalReceiptNs = now();
        requiredStage = "terminalOutput";
        summary = { helperResult: "invalid" };
      } else {
        requireValue(
          row.type === "complete" &&
            stamps.finalRootEndNs !== null &&
            stamps.cleanupEndNs !== null &&
            uint(row.helperMaxRssKiB) &&
            uint(row.readCalls) &&
            uint(row.procInputBytes) &&
            observed.size === handle.known.size &&
            processes.some((p) => p.pid === root.pid && p.disposition === "live"),
          "invalid or incomplete sampler completion",
        );
        parent.terminalFrameCallbackNs = callbackNs;
        parent.terminalReceiptNs = now();
        requiredStage = "terminalOutput";
        summary = {
          helperResult: "complete",
          readCalls: row.readCalls,
          procInputBytes: row.procInputBytes,
          helperMaxRssKiB: row.helperMaxRssKiB,
        };
      }
    };
    child.stdout.on("data", (chunk) => {
      // Even after an operational fault, drain complete, verified birth and RSS
      // rows already flushed to the pipe. A protocol violation ends this prefix.
      if (finished) return;
      parent.stdoutBytes += chunk.length;
      if (protocolBroken) return;
      callbackNs = now();
      parent.firstDataNs ??= callbackNs;
      parent.lastDataNs = callbackNs;
      parent.dataCallbacks++;
      try {
        const allowed = Math.max(0, maxOutput - bytes);
        bytes += chunk.length;
        partial = Buffer.concat([partial, chunk.subarray(0, allowed)]);
        let end;
        while ((end = partial.indexOf(10)) !== -1) {
          requireValue(end < maxLine, "helper line byte cap");
          const parseStart = process.hrtime.bigint();
          try {
            let row;
            try {
              row = JSON.parse(partial.subarray(0, end).toString("utf8"));
              parent.parsedRows++;
            } finally {
              parent.parsingNs = String(
                BigInt(parent.parsingNs ?? 0) + process.hrtime.bigint() - parseStart,
              );
            }
            consume(row);
          } finally {
            parent.parseAndValidateNs = String(
              BigInt(parent.parseAndValidateNs ?? 0) + process.hrtime.bigint() - parseStart,
            );
          }
          partial = partial.subarray(end + 1);
        }
        requireValue(bytes <= maxOutput, "helper output byte cap");
        requireValue(partial.length < maxLine, "helper line byte cap");
      } catch (error) {
        protocolBroken = true;
        fail("protocol", String(error.message));
        kill();
      } finally {
        parent.lastDataEndNs = now();
        parent.dataHandlingNs = String(
          BigInt(parent.dataHandlingNs ?? 0) + BigInt(parent.lastDataEndNs) - BigInt(callbackNs),
        );
      }
    });
    child.stderr.on("data", (chunk) => {
      if (finished) return;
      parent.stderrBytes += chunk.length;
      if (parent.stderrBytes > 4096) {
        fail("protocol", "helper stderr byte cap");
        kill();
      } else stderr += chunk.toString("utf8");
    });
    for (const stream of [child.stdin, child.stdout, child.stderr]) {
      stream.on("error", (error) => {
        fail("pipe", String(error.message));
        kill();
      });
    }
    child.once("error", (error) => {
      fail("spawn", String(error.message));
      kill();
    });
    const markEvent = (emitter, event, field) => {
      const record = () => {
        if (!finished) parent[field] = now();
      };
      // Node may emit ChildProcess.close inside its own last pipe close listener.
      if (event === "close") emitter.prependOnceListener(event, record);
      else emitter.once(event, record);
    };
    markEvent(child, "spawn", "spawnEventNs");
    markEvent(child.stdin, "finish", "requestFinishedNs");
    markEvent(child.stdin, "close", "stdinCloseNs");
    for (const [name, stream] of [
      ["stdout", child.stdout],
      ["stderr", child.stderr],
    ]) {
      markEvent(stream, "end", name + "EndNs");
      markEvent(stream, "close", name + "CloseNs");
    }
    child.once("exit", (code, signal) => {
      if (finished) return;
      parent.exitNs = now();
      parent.exitCode = code;
      parent.exitSignal = signal;
    });
    child.once("close", (code, signal) => {
      if (finished) return;
      parent.closeNs = now();
      helper = { ...helper, closed: true, pipesClosed: true, code, signal };
      if (partial.length || !summary || stamps.preExitNs === null || code !== 0 || signal)
        fail("helper-close", "sampling helper failed or returned incomplete output");
      finish();
    });
    // A millisecond timer may wake before the monotonic nanosecond cutoff.
    // An early wake must not start the separate cancellation reserve.
    const checkDeadline = () => {
      const remaining = deadline - process.hrtime.bigint();
      if (remaining > 0n) {
        timer = setTimeout(checkDeadline, Math.ceil(Number(remaining) / 1e6));
        return;
      }
      parent.deadlineDispatchNs = now();
      fail("deadline", "sampling helper deadline reached");
      kill();
    };
    checkDeadline();
    parent.requestStartNs = now();
    const encodedRequest = JSON.stringify(request);
    parent.requestBytes = Buffer.byteLength(encodedRequest);
    child.stdin.end(encodedRequest);
    parent.requestEndNs = now();
  });
}
