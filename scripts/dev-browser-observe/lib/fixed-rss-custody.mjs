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
        !p.fromStartupHello &&
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

// Release is a second independently closed ordinary epoch. The earlier
// terminal row cannot establish that the holder actually closed both FDs.
export function startupReleaseObservation(releases, cycle, terminal) {
  if (!releases.length) return null;
  const [row] = releases;
  const first = terminal?.custodyReceipt;
  const ack = row.receipt;
  if (
    releases.length !== 1 ||
    !first ||
    !sameLinuxBirth(row.birth, terminal) ||
    row.birth.ppid !== terminal.ppid ||
    row.birth.pgid !== terminal.pgid ||
    row.token !== terminal.custodyToken ||
    ack.session !== first.session ||
    ack.session !== cycle.session ||
    ack.holderPid !== first.holderPid ||
    ack.helperPid === first.helperPid ||
    ack.epoch !== cycle.epoch ||
    ack.epoch <= first.epoch ||
    BigInt(ack.verifiedNs) <= BigInt(terminal.terminalProof.observedNs) ||
    JSON.stringify(ack.descriptors) !== JSON.stringify(first.descriptors)
  ) {
    throw new Error("unverified subsequent startup pair release");
  }
  return row;
}

export function requireStartupRelease(status, terminal, release, completion = null) {
  if (status === "worker-not-observed" && completion?.confirmed && !terminal && !release) return;
  if (status !== "retained" || !terminal || !release)
    throw new Error("startup custody requires accepted terminal and later release cycles");
}

const STAT = (line) => {
  const close = line.lastIndexOf(") ");
  const open = line.indexOf(" (");
  if (close < 0 || open < 0 || open >= close) return null;
  return {
    pid: Number(line.slice(0, open)),
    comm: line.slice(open + 2, close),
    fields: line.slice(close + 2).split(" "),
  };
};

// The scout's zombie record is the holder's word. Re-derive every field from
// its own stat line before accepting the custody as a completed transient role.
export function checkCompletedTransient(b, dev) {
  const t = b.completedTransient;
  const stat = typeof t?.stat === "string" ? STAT(t.stat) : null;
  return Boolean(
    stat &&
    t.role === "__zfb-compiler-worker" &&
    t.exitCode === 0 &&
    b.state === "Z" &&
    stat.pid === b.pid &&
    stat.comm === b.comm &&
    stat.fields[0] === "Z" &&
    Number(stat.fields[1]) === b.ppid &&
    b.ppid === dev.pid &&
    Number(stat.fields[2]) === b.pgid &&
    b.pgid === b.pid &&
    stat.fields[19] === b.startTicks &&
    BigInt(b.startTicks) >= BigInt(dev.startTicks) &&
    stat.fields.length >= 50 &&
    stat.fields[49] === "0" &&
    t.parent?.pid === dev.pid &&
    t.parent?.startTicks === dev.startTicks &&
    /^\d{1,20}$/.test(t.observedNs),
  );
}

// A product-completion record: zfb bound live, no compiler child ever bound,
// and zfb's own count of faults of waited-for children went above zero.
export function checkProductCompletion(record, seed) {
  const birth = (row) =>
    row &&
    row.birthVersion === "linux-proc-v1" &&
    Number.isSafeInteger(row.pid) &&
    row.pid > 0 &&
    /^\d+$/.test(row.startTicks) &&
    row.bootId === seed.bootId;
  const { root, native, bracket } = record ?? {};
  const upper = bracket?.upper;
  const lower = bracket?.lower;
  return Boolean(
    record?.outcome === "product-completion" &&
    birth(root) &&
    birth(native) &&
    sameLinuxBirth(seed, root) &&
    root.ppid === process.pid &&
    native.ppid === seed.pid &&
    native.pgid === seed.pgid &&
    Number.isSafeInteger(upper?.nativeCminflt) &&
    upper.nativeCminflt > 0 &&
    upper.children === 0 &&
    ((lower?.kind === "observed-zero" &&
      lower.nativeCminflt === 0 &&
      lower.children === 0 &&
      lower.sweep < upper.sweep) ||
      (lower?.kind === "native-birth" && lower.startTicks === native.startTicks)) &&
    Array.isArray(record.trace) &&
    record.trace.length <= 16 &&
    /^\d{1,20}$/.test(record.observedNs),
  );
}

// Install a positive private-PGID birth for cleanup before any fallible
// evidence write. A failed registry append still leaves the known map intact.
export function registerStartupBirth(handle, startupBirth) {
  const existing = handle.known.get(startupBirth.pid);
  if (
    existing &&
    (!sameLinuxBirth(existing, startupBirth) ||
      existing.ppid !== startupBirth.ppid ||
      existing.pgid !== startupBirth.pgid)
  )
    throw new Error("startup birth conflicts with known inventory");
  if (!existing) {
    if (handle.known.size >= 64) throw new Error("startup inventory cap");
    const item = { ...startupBirth, source: "fixed-startup-scout" };
    handle.known.set(item.pid, item);
    handle.onNewOwned?.(item);
  }
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
  const readinessDeadline = process.hrtime.bigint() + 750_000_000n;
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
  let spawnRecorded = false;
  let stopping = false,
    handshake = false,
    epoch = 0,
    pending = "",
    stderr = "",
    outputBytes = 0;
  let stopReport, exit, fault, closePromise;
  let targetHandle;
  let startupDev,
    helloTransfer,
    helloAccepted = false;
  let seed, startupBirth, acquiredNs, scoutReport, scoutTimer, confirmedStartup, releasedStartup;
  let startupCompletion = null,
    startupOutcome = null;
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
  const checkReadinessTime = () => {
    if (process.hrtime.bigint() >= readinessDeadline) throw issue("readiness timeout");
  };
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
          if (fault) return;
          if (!spawnRecorded) throw issue("holder replied before spawn ownership was recorded");
          checkReadinessTime();
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
          checkReadinessTime();
          append({ type: "fixed-custody-ready", birth, session });
          checkReadinessTime();
          handshake = true;
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
            (b.completedTransient !== undefined && !checkCompletedTransient(b, msg.dev)) ||
            (b.completedTransient === undefined && ["Z", "X", "x"].includes(b.state)) ||
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
          if (
            !/^\d{1,20}$/.test(msg.acquiredNs) ||
            !/^\d{1,20}$/.test(msg.liveVerifiedNs) ||
            BigInt(msg.liveVerifiedNs) > BigInt(msg.acquiredNs)
          )
            throw issue("invalid startup acquisition time");
          acquiredNs = msg.acquiredNs;
          startupDev = msg.dev;
          startupBirth = b;
          registerStartupBirth(targetHandle, b);
          retained.set(b.pid, { birth: b, token: msg.token, descriptors: msg.descriptors });
          startupOutcome = b.completedTransient ? "zombie-custody" : "live-custody";
          append({
            type: b.completedTransient
              ? "fixed-startup-transient-custody"
              : "fixed-startup-live-custody",
            ...msg,
          });
        } else if (
          startup &&
          handshake &&
          seed &&
          msg.session === session &&
          msg.startupCompletion &&
          !startupBirth &&
          !startupCompletion &&
          !scoutReport
        ) {
          if (
            retained.size ||
            !checkProductCompletion(msg.startupCompletion, seed) ||
            !/^\d{1,20}$/.test(msg.publishedNs) ||
            BigInt(msg.startupCompletion.observedNs) > BigInt(msg.publishedNs)
          )
            throw issue("invalid startup product-completion record");
          // The scan's bounded-binding check reads the decision time as acquiredNs.
          acquiredNs = msg.startupCompletion.observedNs;
          startupCompletion = { ...msg.startupCompletion, confirmed: null };
          startupDev = msg.startupCompletion.native;
          startupOutcome = "product-completion";
          append({ type: "fixed-startup-product-completion", session, ...msg });
        } else if (startup && msg.session === session && msg.startupHello && !helloTransfer) {
          const h = msg.startupHello;
          if (
            !startupBirth ||
            h.epoch !== epoch ||
            h.holderPid !== birth.pid ||
            !Number.isSafeInteger(h.helperPid) ||
            h.helperPid <= 0 ||
            !/^\d{1,20}$/.test(h.verifiedNs) ||
            BigInt(h.verifiedNs) < BigInt(acquiredNs) ||
            retained.get(startupBirth.pid)?.token !== h.token
          )
            throw issue("unverified holder startup hello");
          helloTransfer = h;
          append({ type: "fixed-startup-hello-transfer", session, ...h });
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
            !(
              (scoutReport.status === "retained" && startupBirth && !startupCompletion) ||
              (scoutReport.status === "worker-not-observed" && startupCompletion && !startupBirth)
            ) ||
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
          if (startup && msg.startupHelloTransfers !== Number(Boolean(helloTransfer)))
            throw issue("startup hello transfer not reconciled");
          const lines = [...retained.values()]
            .map(({ birth: b, token }) =>
              [b.bootId, b.pid, b.ppid, b.pgid, b.startTicks, token].join("|"),
            )
            .sort();
          const digest = createHash("sha256").update(lines.join("\n")).digest("hex");
          if (msg.pairsClosed !== retained.size || msg.retainedDigest !== digest)
            throw issue("holder/controller ownership mismatch");
          stopReport = startup ? { ...msg, startupHelloCorroborated: helloAccepted } : msg;
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
  let timer;
  const watchReadiness = () => {
    const left = readinessDeadline - process.hrtime.bigint();
    if (left <= 0n) fail(issue("readiness timeout"));
    else timer = setTimeout(watchReadiness, Math.ceil(Number(left) / 1e6));
  };
  watchReadiness();
  ready.promise.then(
    () => clearTimeout(timer),
    () => clearTimeout(timer),
  );
  // The greeting lets the helper initialize while the controller persists its
  // birth. No startup seed or descriptor exchange is permitted before ready.
  try {
    checkReadinessTime();
    child.stdin.write(session + "\n");
  } catch (error) {
    fail(error);
  }
  try {
    append({ type: "fixed-custody-spawn", session, birth, pid: child.pid ?? null });
    spawnRecorded = true;
    checkReadinessTime();
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
      targetHandle = handle;
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
      if (startupBirth) registerStartupBirth(handle, startupBirth);
      return {
        session,
        ...(startup && !startupBirth && retained.size === 0 && { allowStartupAddition: true }),
        ...(startupBirth && !confirmedStartup && { startupPollPid: startupBirth.pid }),
        holderPid: birth.pid,
        epoch: ++epoch,
        tokens: Object.fromEntries([...retained].map(([pid, item]) => [pid, item.token])),
      };
    },
    // Holder stdout and helper stdout are separate streams. An announcement
    // still uncorroborated at this decision must fail without waiting.
    validateStartupHello(addition, cycle, helper) {
      if (
        helloAccepted ||
        !helloTransfer ||
        !cycle.allowStartupAddition ||
        !startupBirth ||
        !this.matches(addition.birth, addition.token) ||
        addition.acquiredNs !== acquiredNs ||
        !sameLinuxBirth(addition.dev, startupDev) ||
        addition.dev.ppid !== startupDev.ppid ||
        addition.dev.pgid !== startupDev.pgid ||
        !sameLinuxBirth(addition.root, seed) ||
        addition.root.ppid !== seed.ppid ||
        addition.root.pgid !== seed.pgid ||
        helloTransfer.epoch !== cycle.epoch ||
        helloTransfer.helperPid !== helper.pid ||
        helloTransfer.verifiedNs !== addition.receipt.verifiedNs ||
        JSON.stringify(retained.get(addition.birth.pid)?.descriptors) !==
          JSON.stringify(addition.descriptors)
      )
        throw issue("startup hello contradicts independent holder publication");
      helloAccepted = true;
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
    selectStartupCycle(processes, releases, cycle) {
      if (!startupBirth || !cycle) return null;
      const terminal = startupTerminalObservation(processes, cycle, startupBirth, acquiredNs);
      if (terminal) return { terminal };
      const release = startupReleaseObservation(releases, cycle, confirmedStartup);
      return release ? { release } : null;
    },
    commitStartupCycle(selection) {
      if (selection?.terminal) confirmedStartup = selection.terminal;
      if (selection?.release) releasedStartup = selection.release;
    },
    // Product completion is confirmed once, at the first readiness-qualified
    // ordinary sample: zfb live with the scout's birth, no zfb child, a complete
    // census, and no unknown child of this (subreaper) controller.
    confirmStartupCompletion({ sample, controllerChildren, expectedChildren }) {
      if (!startup || !startupCompletion || startupCompletion.confirmed) return;
      const native = startupCompletion.native;
      const live = sample?.live ?? [];
      const unknown = controllerChildren.filter((pid) => !expectedChildren.includes(pid));
      const same = (a, b) => sameLinuxBirth(a, b) && a.ppid === b.ppid && a.pgid === b.pgid;
      const ok =
        sample?.strict &&
        Array.isArray(sample.departures) &&
        sample.departures.length === 0 &&
        live.length === 2 &&
        live.some((p) => same(p, seed)) &&
        live.some((p) => same(p, native)) &&
        unknown.length === 0;
      const row = {
        type: "fixed-startup-completion-confirmation",
        session,
        epoch: sample?.epoch ?? null,
        sampleStartedNs: sample?.startedNs ?? null,
        sampleResultNs: sample?.resultNs ?? null,
        live,
        departures: sample?.departures ?? null,
        controllerChildren,
        expectedChildren,
        unknownControllerChildren: unknown,
        ok: Boolean(ok),
      };
      append(row);
      if (!ok) throw issue("startup product completion unconfirmed at readiness");
      startupCompletion.confirmed = { epoch: sample.epoch, startedNs: sample.startedNs };
    },
    get startupCompletionPending() {
      return Boolean(startup && startupCompletion && !startupCompletion.confirmed);
    },
    startupOutcome() {
      const kinds = ["live-custody", "zombie-custody", "product-completion"];
      return {
        outcome: startupOutcome,
        scoutStatus: scoutReport?.status ?? null,
        counts: Object.fromEntries(kinds.map((k) => [k, Number(startupOutcome === k)])),
      };
    },
    verifyStartup() {
      if (startup)
        requireStartupRelease(
          scoutReport?.status,
          confirmedStartup,
          releasedStartup,
          startupCompletion,
        );
    },
    familyProof() {
      if (!startup || stopping || fault || !handshake || retained.size !== 0)
        throw issue("family startup custody unavailable");
      requireStartupRelease(
        scoutReport?.status,
        confirmedStartup,
        releasedStartup,
        startupCompletion,
      );
      return {
        root: seed,
        dev: startupDev,
        worker: startupBirth ?? null,
        terminal: confirmedStartup ?? null,
        release: releasedStartup ?? null,
        ...(startupCompletion && { completion: structuredClone(startupCompletion) }),
      };
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
