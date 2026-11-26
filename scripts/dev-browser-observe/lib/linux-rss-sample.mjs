import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { LINUX_BIRTH, sameLinuxBirth } from "./linux-birth.mjs";

const samplerVersion = "linux-pidfd-rss.v1";
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

/** A single direct child scans, pins and samples the owned set in one bounded cycle. */
export function sampleRendererMemory(
  server,
  expectedExecutablePath,
  {
    launch = () =>
      spawn("python3", ["-I", "-B", script, String(process.pid)], {
        stdio: ["pipe", "pipe", "pipe"],
      }),
  } = {},
) {
  const at = new Date().toISOString();
  const start = process.hrtime.bigint();
  const deadline = start + BigInt(budgetMs) * 1_000_000n;
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
    endHrtimeNs: String(process.hrtime.bigint()),
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
      if (child && !closeTimer)
        closeTimer = setTimeout(() => {
          fail("helper-close", "sampling helper closure unverified");
          // Leave the failed resource identity in the receipt for admission.
          for (const pipe of [child.stdin, child.stdout, child.stderr]) pipe.destroy();
          child.unref();
          finish();
        }, closeMs);
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
      };
      spawnHrtimeNs = String(process.hrtime.bigint());
      child = launch();
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
    const consume = (row) => {
      requireValue(!summary, "sample output after terminal row");
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
            ticks(exe.inode),
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
            handle.onNewOwned(entry);
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
                p.executable?.path === executable.path &&
                p.executable?.device === executable.device &&
                p.executable?.inode === executable.inode &&
                uint(p.threads?.listed) &&
                Array.isArray(p.threads?.rendererHosts),
              "invalid live process observation",
            );
          }
        }
        observed.add(p.pid);
        processes.push(p);
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
        summary = { helperResult: "invalid" };
      } else {
        requireValue(
          row.type === "complete" &&
            uint(row.helperMaxRssKiB) &&
            uint(row.readCalls) &&
            uint(row.procInputBytes) &&
            observed.size === handle.known.size &&
            processes.some((p) => p.pid === root.pid && p.disposition === "live"),
          "invalid or incomplete sampler completion",
        );
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
      if (protocolBroken || finished) return;
      try {
        const allowed = Math.max(0, maxOutput - bytes);
        bytes += chunk.length;
        partial = Buffer.concat([partial, chunk.subarray(0, allowed)]);
        let end;
        while ((end = partial.indexOf(10)) !== -1) {
          requireValue(end < maxLine, "helper line byte cap");
          consume(JSON.parse(partial.subarray(0, end).toString("utf8")));
          partial = partial.subarray(end + 1);
        }
        requireValue(bytes <= maxOutput, "helper output byte cap");
        requireValue(partial.length < maxLine, "helper line byte cap");
      } catch (error) {
        protocolBroken = true;
        fail("protocol", String(error.message));
        kill();
      }
    });
    child.stderr.on("data", (chunk) => {
      if (stderr.length + chunk.length > 4096) {
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
    child.once("close", (code, signal) => {
      if (finished) return;
      helper = { ...helper, closed: true, pipesClosed: true, code, signal };
      if (partial.length || !summary || code !== 0 || signal)
        fail("helper-close", "sampling helper failed or returned incomplete output");
      if (process.hrtime.bigint() > deadline) fail("deadline", "sampling helper deadline reached");
      finish();
    });
    timer = setTimeout(
      () => {
        fail("deadline", "sampling helper deadline reached");
        kill();
      },
      Math.max(0, Number(deadline - process.hrtime.bigint()) / 1e6),
    );
    child.stdin.end(JSON.stringify(request));
  });
}
