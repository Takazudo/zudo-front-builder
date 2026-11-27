// Linux fd3 is a preconnected Unix socket. The controller passes its endpoint
// only to fresh sampling helpers. Each helper sends a private reply socket.
import { accessSync, constants, readlinkSync, realpathSync } from "node:fs";
import { join, delimiter } from "node:path";
import { spawn } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { fileURLToPath } from "node:url";
import { linuxBirth, sameLinuxBirth } from "./linux-birth.mjs";

const path = fileURLToPath(new URL("./linux-rss-custody.py", import.meta.url));

function interpreter() {
  for (const directory of process.env.PATH.split(delimiter)) {
    const candidate = join(directory, "python3");
    try {
      accessSync(candidate, constants.X_OK);
      return realpathSync(candidate);
    } catch (error) {
      if (error.code !== "ENOENT" && error.code !== "EACCES") throw error;
    }
  }
  throw new Error("fixed custody requires Python 3");
}

// This evidence comes from a subsequent normal sample. A scan, direct live
// binding, an earlier terminal proof, or different PID/epoch cannot qualify.
export function startupTerminalObservation(processes, cycle, birth, acquiredNs) {
  if (!birth || cycle?.startupPollPid !== birth.pid) return null;
  return (
    processes.find(
      (p) =>
        p.disposition === "custodied-departure" &&
        sameLinuxBirth(p, birth) &&
        p.ppid === birth.ppid &&
        p.pgid === birth.pgid &&
        p.rssKiB === null &&
        p.statusKiB === null &&
        p.smapsRollupKiB === null &&
        p.custodyReceipt?.epoch === cycle.epoch &&
        p.custodyReceipt?.session === cycle.session &&
        BigInt(p.startedNs) >= BigInt(acquiredNs),
    ) ?? null
  );
}

export function startFixedCustody({ append: record, abort, intent, startup = false }) {
  const append = (row) => {
    if (row.type === "fixed-custody-spawn" || row.type === "fixed-custody-ready")
      intent?.update({
        phase: row.type,
        fixedCustody: { pid: row.pid ?? row.birth?.pid, birth: row.birth, session: row.session },
      });
    record(row);
  };
  const session = randomBytes(16).toString("hex");
  const python = interpreter();
  const child = spawn(
    python,
    ["-I", "-B", path, String(process.pid), ...(startup ? ["--startup"] : [])],
    {
      stdio: ["pipe", "pipe", "pipe", "pipe"],
    },
  );
  const ready = Promise.withResolvers();
  const closed = Promise.withResolvers();
  const retained = new Map();
  const birth = child.pid && linuxBirth(child.pid);
  let stopping = false,
    handshake = false,
    epoch = 0,
    pending = "",
    stderr = "",
    outputBytes = 0;
  let stopReport, exit, fault, closePromise;
  let seed, startupBirth, acquiredNs, scoutReport, scoutTimer, confirmedStartup;
  const fail = (error) => {
    if (fault) return;
    fault = error;
    ready.reject(error);
    clearTimeout(scoutTimer);
    try {
      abort(error);
    } finally {
      if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
    }
  };
  const issue = (reason) => new Error(`fixed descriptor custody: ${reason}`);
  // Install all callbacks before sending the first byte. The readiness promise
  // is consumed by the controller even when spawning fails synchronously.
  child.on("error", fail);
  for (const stream of child.stdio) stream?.on("error", fail);
  child.stderr.on("data", (chunk) => {
    if (Buffer.byteLength(stderr) + chunk.length > 4096) fail(issue("stderr cap"));
    else stderr += chunk.toString();
  });
  child.stdout.on("data", (chunk) => {
    try {
      outputBytes += chunk.length;
      if (outputBytes > (startup ? 8192 : 2048)) throw issue("response cap");
      pending += chunk.toString();
      while (pending.includes("\n")) {
        const end = pending.indexOf("\n");
        const msg = JSON.parse(pending.slice(0, end));
        pending = pending.slice(end + 1);
        if (!handshake && msg.ready === true && msg.pid === birth?.pid && msg.session === session) {
          if (msg.timeNamespace !== readlinkSync("/proc/self/ns/time"))
            throw issue("holder time namespace mismatch");
          const live = linuxBirth(child.pid);
          if (
            !sameLinuxBirth(live, birth) ||
            live.ppid !== process.pid ||
            live.pgid !== birth.pgid ||
            readlinkSync(`/proc/${child.pid}/exe`) !== python
          )
            throw issue("unverified holder birth or image");
          handshake = true;
          append({ type: "fixed-custody-ready", birth, session });
          ready.resolve();
        } else if (
          startup &&
          handshake &&
          seed &&
          msg.session === session &&
          msg.startupBirth &&
          !startupBirth &&
          !scoutReport
        ) {
          const b = msg.startupBirth;
          if (
            !sameLinuxBirth(seed, msg.root) ||
            msg.root.ppid !== process.pid ||
            msg.root.pgid !== seed.pid ||
            msg.dev.ppid !== seed.pid ||
            msg.dev.pgid !== seed.pgid ||
            msg.dev.bootId !== seed.bootId ||
            b.ppid !== msg.dev.pid ||
            b.pgid !== b.pid ||
            ![msg.dev, b].every(
              (row) =>
                row.birthVersion === "linux-proc-v1" &&
                Number.isSafeInteger(row.pid) &&
                row.pid > 0 &&
                /^\d+$/.test(row.startTicks) &&
                row.bootId === seed.bootId,
            ) ||
            !/^[a-f0-9]{32}$/.test(msg.token) ||
            !Array.isArray(msg.descriptors) ||
            msg.descriptors.length !== 2 ||
            !msg.descriptors.every(
              (pair) =>
                Array.isArray(pair) &&
                pair.length === 2 &&
                pair.every((v) => typeof v === "string" && /^\d+$/.test(v)),
            )
          )
            throw issue("invalid startup compiler role or descriptor tuple");
          const prior = retained.get(b.pid);
          if (
            (retained.size && !prior) ||
            (prior &&
              (!sameLinuxBirth(prior.birth, b) ||
                prior.token !== msg.token ||
                JSON.stringify(prior.descriptors) !== JSON.stringify(msg.descriptors)))
          )
            throw issue("startup custody conflicts with ordinary safety");
          if (!/^\d{1,20}$/.test(msg.acquiredNs)) throw issue("invalid startup acquisition time");
          acquiredNs = msg.acquiredNs;
          startupBirth = b;
          retained.set(b.pid, { birth: b, token: msg.token, descriptors: msg.descriptors });
          append({ type: "fixed-startup-live-custody", ...msg });
        } else if (
          startup &&
          handshake &&
          seed &&
          msg.session === session &&
          msg.startupReport &&
          !scoutReport
        ) {
          scoutReport = msg.startupReport;
          clearTimeout(scoutTimer);
          append({ type: "fixed-startup-scout-end", session, ...scoutReport });
          if (
            scoutReport.status !== "retained" ||
            !startupBirth ||
            scoutReport.error ||
            !Number.isSafeInteger(scoutReport.sweeps) ||
            scoutReport.sweeps < 1 ||
            scoutReport.sweeps > 150 ||
            !Number.isSafeInteger(scoutReport.reads) ||
            scoutReport.reads > 8192 ||
            !Number.isSafeInteger(scoutReport.procBytes) ||
            scoutReport.procBytes > 4 << 20 ||
            BigInt(scoutReport.startNs) > BigInt(acquiredNs) ||
            BigInt(acquiredNs) > BigInt(scoutReport.finishedNs) ||
            BigInt(scoutReport.deadlineNs) - BigInt(scoutReport.startNs) !== 3_000_000_000n ||
            BigInt(scoutReport.finishedNs) > BigInt(scoutReport.deadlineNs)
          )
            throw issue("startup scan did not complete a bounded live binding");
        } else if (
          stopping &&
          handshake &&
          !stopReport &&
          msg.stopped === true &&
          msg.epochs === epoch &&
          Number.isSafeInteger(msg.pairsClosed) &&
          msg.pairsClosed >= 0 &&
          msg.pairsClosed <= 1 &&
          Number.isSafeInteger(msg.maxPairs) &&
          msg.maxPairs >= msg.pairsClosed &&
          msg.maxPairs <= 1 &&
          [msg.retains, msg.gets, msg.releases].every((n) => Number.isSafeInteger(n) && n >= 0) &&
          msg.retains >= msg.maxPairs &&
          msg.retains - msg.releases === msg.pairsClosed &&
          Number.isSafeInteger(msg.maxRssKiB) &&
          msg.maxRssKiB > 0 &&
          Number.isFinite(msg.userSeconds) &&
          msg.userSeconds >= 0 &&
          Number.isFinite(msg.systemSeconds) &&
          msg.systemSeconds >= 0
        ) {
          const lines = [...retained.values()]
            .map(({ birth: b, token }) =>
              [b.bootId, b.pid, b.ppid, b.pgid, b.startTicks, token].join("|"),
            )
            .sort();
          const digest = createHash("sha256").update(lines.join("\n")).digest("hex");
          if (msg.pairsClosed !== retained.size || msg.retainedDigest !== digest)
            throw issue("holder/controller ownership mismatch");
          stopReport = msg;
        } else throw issue("unexpected handoff response");
      }
    } catch (error) {
      fail(error);
    }
  });
  child.once("exit", (code, signal) => {
    exit = { code, signal };
    // After this socket is inherited by a sampling helper, Node no longer
    // consumes its EOF. Close the controller endpoint after holder exit.
    child.stdio[3]?.destroy();
    if (!stopping) fail(issue(`holder exited ${code ?? signal}`));
  });
  child.once("close", (code, signal) => {
    closed.resolve({ code, signal });
    if (!stopping) fail(issue("holder closed unexpectedly"));
  });
  const timer = setTimeout(() => fail(issue("readiness timeout")), 750);
  ready.promise.then(
    () => clearTimeout(timer),
    () => clearTimeout(timer),
  );
  try {
    append({ type: "fixed-custody-spawn", session, birth, pid: child.pid ?? null });
    child.stdin.write(session + "\n");
  } catch (error) {
    fail(error);
  }
  return {
    ready: ready.promise,
    identity: { session, birth, pid: child.pid ?? null },
    channel: child.stdio[3],
    attachStartup(handle, fixed) {
      if (!startup || !handshake || seed || stopping || fault)
        throw issue("invalid startup attachment");
      seed = handle.native;
      if (
        !sameLinuxBirth(seed, linuxBirth(seed.pid)) ||
        seed.ppid !== process.pid ||
        seed.pgid !== seed.pid
      )
        throw issue("unverified startup root");
      const line = JSON.stringify({ session, root: seed, fixed }) + "\n";
      if (Buffer.byteLength(line) > 16384) throw issue("startup seed cap");
      append({ type: "fixed-startup-scout-seed", session, root: seed, fixed });
      scoutTimer = setTimeout(() => fail(issue("startup scout deadline or closure")), 3200);
      child.stdin.write(line);
    },
    begin(handle) {
      if (!handshake || stopping || fault) throw fault ?? issue("holder unavailable");
      if (startupBirth) {
        const existing = handle.known.get(startupBirth.pid);
        if (
          existing &&
          (!sameLinuxBirth(existing, startupBirth) ||
            existing.ppid !== startupBirth.ppid ||
            existing.pgid !== startupBirth.pgid)
        )
          throw issue("startup birth conflicts with known inventory");
        if (!existing) {
          if (handle.known.size >= 64) throw issue("startup inventory cap");
          const item = { ...startupBirth, source: "fixed-startup-scout" };
          handle.known.set(item.pid, item);
          handle.onNewOwned?.(item);
        }
      }
      return {
        session,
        ...(startupBirth && !confirmedStartup && { startupPollPid: startupBirth.pid }),
        holderPid: birth.pid,
        epoch: ++epoch,
        tokens: Object.fromEntries([...retained].map(([pid, item]) => [pid, item.token])),
      };
    },
    matches(b, token) {
      const prev = retained.get(b.pid);
      return Boolean(
        prev &&
        prev.token === token &&
        sameLinuxBirth(prev.birth, b) &&
        prev.birth.ppid === b.ppid &&
        prev.birth.pgid === b.pgid,
      );
    },
    sameDescriptors(b) {
      const prior = retained.get(b.pid)?.descriptors;
      return Boolean(
        prior &&
        b.custodyReceipt?.descriptors?.every((pair, i) =>
          pair.every((value, j) => value === prior[i]?.[j]),
        ),
      );
    },
    record(b, token) {
      if (typeof token !== "string" || !/^[a-f0-9]{32}$/.test(token)) throw issue("invalid token");
      if (retained.has(b.pid)) {
        if (!this.matches(b, token) || !this.sameDescriptors(b))
          throw issue("changed birth, token or descriptors");
      } else {
        if (retained.size >= 1) throw issue("ownership cap");
        retained.set(b.pid, {
          birth: {
            pid: b.pid,
            ppid: b.ppid,
            pgid: b.pgid,
            startTicks: b.startTicks,
            bootId: b.bootId,
            birthVersion: b.birthVersion,
          },
          token,
          descriptors: b.custodyReceipt.descriptors,
        });
      }
    },
    acceptStartupCycle(processes, cycle) {
      if (!startupBirth || confirmedStartup) return;
      confirmedStartup = startupTerminalObservation(processes, cycle, startupBirth, acquiredNs);
    },
    verifyStartup() {
      if (startup && (!startupBirth || scoutReport?.status !== "retained" || !confirmedStartup))
        throw issue("startup live-to-official-terminal sequence incomplete");
    },
    forget(b) {
      retained.delete(b.pid);
    },
    stop() {
      if (closePromise) return closePromise;
      clearTimeout(scoutTimer);
      stopping = true;
      child.stdin.end();
      closePromise = (async () => {
        let kill, deadline;
        try {
          kill = setTimeout(() => {
            if (!exit) child.kill("SIGKILL");
          }, 150);
          const ended = await Promise.race([
            closed.promise,
            new Promise((_, reject) => {
              deadline = setTimeout(() => reject(issue("closure unverified")), 350);
            }),
          ]);
          const row = {
            type: "fixed-custody-stop",
            birth,
            session,
            exit: ended,
            stopReport: stopReport ?? null,
            stderr,
            pendingBytes: Buffer.byteLength(pending),
            retainedRecords: retained.size,
            fault: fault?.message ?? null,
          };
          append(row);
          if (
            fault ||
            !handshake ||
            !birth ||
            !stopReport ||
            ended.code !== 0 ||
            ended.signal ||
            pending
          )
            throw issue("incomplete holder shutdown");
          return row;
        } catch (error) {
          append({
            type: "fixed-custody-stop-failure",
            birth,
            session,
            exit,
            stopReport: stopReport ?? null,
            retainedRecords: retained.size,
            firstFailure: fault?.message ?? null,
            stderr,
            reason: error.message,
          });
          throw error;
        } finally {
          clearTimeout(kill);
          clearTimeout(deadline);
        }
      })();
      return closePromise;
    },
  };
}
