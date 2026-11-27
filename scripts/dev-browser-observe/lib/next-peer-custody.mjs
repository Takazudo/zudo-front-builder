// Linux fd3 is a preconnected Unix socket. The controller passes its endpoint
// only to fresh sampling helpers. Each helper sends a private reply socket.
import { accessSync, constants, readlinkSync, realpathSync } from "node:fs";
import { join, delimiter } from "node:path";
import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { fileURLToPath } from "node:url";
import { linuxBirth } from "./linux-birth.mjs";

const path = fileURLToPath(new URL("./next-peer-custody.py", import.meta.url));

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

import assert from "node:assert/strict";
import { verifyNextBootstrapPair, verifyNextBootstrapCurrent } from "./fixed-next-bootstrap.mjs";
const key = (p) => [p.bootId, p.pid, p.ppid, p.pgid, p.startTicks].join("/");
const same = (a, b) => Boolean(a && b && key(a) === key(b));
export function startNextPeerCustody({ contract, append, abort, intent }) {
  const session = randomBytes(16).toString("hex"),
    python = interpreter();
  const child = spawn(python, ["-I", "-B", path, String(process.pid)], {
    stdio: ["pipe", "pipe", "pipe", "pipe"],
  });
  const birth = child.pid && linuxBirth(child.pid),
    ready = Promise.withResolvers(),
    closed = Promise.withResolvers();
  let seq = 0,
    cycles = 0,
    bytes = 0,
    buf = "",
    stderr = "",
    firstFailure,
    seen,
    accepted,
    command,
    window,
    stopPromise,
    stopReceipt,
    exit,
    armed = false;
  let watchdog;
  let terminalVerified = false;
  let windowEnd = null;
  const secondaryErrors = [];
  const resetWatchdog = () => {
    clearTimeout(watchdog);
    watchdog = setTimeout(() => refuse(Error("identity scout stalled")), 200);
  };
  const notify = (type, data) => append({ type: "next-custody-" + type, session, ...data });
  const refuse = (e) => {
    if (firstFailure) return;
    clearTimeout(watchdog);
    firstFailure = e;
    ready.reject(e);
    command?.reject(e);
    try {
      notify("failure", { error: e.message });
    } catch (error) {
      secondaryErrors.push(String(error));
    }
    try {
      abort(e);
    } catch (error) {
      secondaryErrors.push(String(error));
    } finally {
      if (!exit) {
        try {
          child.kill("SIGKILL");
        } catch (error) {
          secondaryErrors.push(String(error));
        }
      }
    }
  };
  const write = (data) => child.stdin.write(JSON.stringify(data) + "\n");
  const req = (op, name, deadline) => {
    assert(!firstFailure && !command && !stopPromise, "scout unavailable");
    const wait = Promise.withResolvers();
    command = { ...wait, op, name, deadline };
    const timer = setTimeout(
      () => refuse(Error("scout window ACK timeout")),
      Math.max(1, Math.min(750, Number((BigInt(deadline) - process.hrtime.bigint()) / 1000000n))),
    );
    try {
      write({ session, op, name, deadline });
    } catch (e) {
      refuse(e);
    }
    return wait.promise.finally(() => clearTimeout(timer));
  };
  const frame = (m) => {
    assert(
      m.session === session && m.seq === ++seq && /^\d+$/.test(m.ns),
      "wrong scout sequence/session",
    );
    if (m.type === "ready") {
      const current = linuxBirth(child.pid);
      assert(
        !armed &&
          same(m.birth, birth) &&
          same(current, birth) &&
          birth.ppid === process.pid &&
          m.timeNamespace === readlinkSync("/proc/self/ns/time") &&
          readlinkSync(`/proc/${child.pid}/exe`) === python,
        "unverified scout",
      );
      armed = true;
      ready.resolve();
    } else if (m.type === "window-open" || m.type === "window-close") {
      assert(
        command &&
          command.op === (m.type === "window-open" ? "open" : "close") &&
          command.name === m.name,
        "scout window mismatch",
      );
      window = m.type === "window-open" ? m.name : null;
      windowEnd = window ? BigInt(command.deadline) : null;
      if (window) resetWatchdog();
      else clearTimeout(watchdog);
      command.resolve();
      command = null;
    } else if (m.type === "bound") {
      assert(
        window &&
          !seen &&
          m.birth.ppid === contract.ancestors[2].pid &&
          m.birth.pgid === contract.ancestors[0].pid &&
          m.birth.bootId === birth.bootId &&
          Array.isArray(m.descriptors) &&
          m.descriptors.length === 2,
        "unverified scout candidate",
      );
      seen = m;
    } else if (m.type === "qualified") {
      assert(
        window && same(seen?.birth, m.birth) && !accepted && /^[a-f0-9]{32}$/.test(m.token),
        "unbound or duplicate qualification",
      );
      assert.deepEqual(m.descriptors, seen.descriptors);
      assert(
        BigInt(m.ns) <= windowEnd && BigInt(m.views.at(-1).view.endNs) <= windowEnd,
        "qualification after window deadline",
      );
      const role = verifyNextBootstrapPair(contract, m.views);
      assert(same(role.birth, m.birth));
      accepted = m;
    } else if (m.type === "sweep") {
      assert(
        window &&
          Number.isSafeInteger(m.index) &&
          m.index >= 1 &&
          m.index <= 600 &&
          BigInt(m.endNs) >= BigInt(m.startNs) &&
          BigInt(m.endNs) - BigInt(m.startNs) <= 50000000n,
        "invalid scout sweep",
      );
      resetWatchdog();
    } else if (m.type === "stopped") {
      assert(
        stopPromise &&
          !window &&
          m.token === (accepted?.token ?? null) &&
          m.closedPairs === 3 + Number(Boolean(seen)),
        "unreconciled scout stop",
      );
      stopReceipt = m;
    } else throw Error("unexpected scout frame");
    notify(m.type, { evidence: m });
  };
  child.on("error", refuse);
  for (const stream of child.stdio) stream?.on("error", refuse);
  child.stdout.on("data", (chunk) => {
    try {
      bytes += chunk.length;
      assert(bytes <= 8 * 1024 * 1024);
      buf += chunk.toString("utf8");
      let end;
      while ((end = buf.indexOf("\n")) >= 0) {
        assert(end <= 65536);
        const m = JSON.parse(buf.slice(0, end));
        buf = buf.slice(end + 1);
        frame(m);
      }
      assert(buf.length <= 65536);
    } catch (e) {
      refuse(e);
    }
  });
  child.stderr.on("data", (c) => {
    if (Buffer.byteLength(stderr) + c.length > 4096) refuse(Error("scout stderr cap"));
    else stderr += c.toString();
  });
  child.once("exit", (code, signal) => {
    exit = { code, signal };
    child.stdio[3]?.destroy();
    if (!stopPromise) refuse(Error("scout exited: " + stderr));
  });
  child.once("close", (code, signal) => closed.resolve({ code, signal }));
  const timer = setTimeout(() => refuse(Error("scout readiness timeout")), 750);
  ready.promise.then(
    () => clearTimeout(timer),
    () => clearTimeout(timer),
  );
  try {
    intent?.update({
      phase: "next-custody-spawn",
      nextCustody: { session, birth, pid: child.pid },
    });
    notify("spawn", { birth });
    write({ contract, session });
  } catch (e) {
    refuse(e);
  }
  return {
    ready: ready.promise,
    channel: child.stdio[3],
    identity: { session, birth, secondaryErrors },
    openWindow(name, remainingMs) {
      const dt = Math.min(30000, remainingMs);
      assert(Number.isFinite(dt) && dt > 0);
      return req("open", name, String(process.hrtime.bigint() + BigInt(Math.floor(dt)) * 1000000n));
    },
    closeWindow(name) {
      return req("close", name, String(process.hrtime.bigint() + 750000000n));
    },
    snapshot(b) {
      // A cleanup census must still be able to signal its verified group if
      // the observer rejects an unfamiliar birth or the journal is unavailable.
      if (firstFailure || stopPromise) return;
      try {
        if (b.ppid !== contract.ancestors[2].pid) return;
        if (seen) {
          assert(same(seen.birth, b), "snapshot contradicts bound candidate");
          return;
        }
        assert(window, "unbound worker outside scout window");
        write({ session, op: "snapshot", birth: b });
      } catch (error) {
        refuse(error);
      }
    },
    begin(handle) {
      if (firstFailure) throw firstFailure;
      if (seen) {
        const b = seen.birth,
          old = handle.known.get(b.pid);
        if (old) assert(same(old, b));
        else {
          assert(handle.known.size < 64);
          const entry = { ...b, source: "next-custody-discovery" };
          handle.known.set(b.pid, entry);
          handle.onNewOwned?.(entry);
        }
      }
      return {
        session,
        cycle: ++cycles,
        holderPid: birth.pid,
        contract,
        tokens: accepted ? { [accepted.birth.pid]: accepted.token } : {},
      };
    },
    verifyCurrent(b) {
      assert(same(b, accepted?.birth), "unqualified Next bootstrap");
      return verifyNextBootstrapCurrent(contract, b);
    },
    isWorker(b) {
      return same(b, accepted?.birth);
    },
    validate(b, ack, cycle, helper) {
      assert(
        accepted &&
          same(b, ack.birth) &&
          same(b, accepted.birth) &&
          ack.token === accepted.token &&
          ack.cycle === cycle &&
          ack.session === session &&
          ack.holderPid === birth.pid &&
          ack.helperPid === helper.pid,
        "unverified scout handoff",
      );
      assert.deepEqual(ack.descriptors, accepted.descriptors);
    },
    terminal(b) {
      assert(same(b, accepted?.birth));
      terminalVerified = true;
    },
    get terminalVerified() {
      return terminalVerified;
    },
    get qualified() {
      return accepted?.birth;
    },
    stop() {
      if (stopPromise) return stopPromise;
      clearTimeout(watchdog);
      const timer = setTimeout(() => {
        if (!exit) child.kill("SIGKILL");
      }, 150);
      stopPromise = (async () => {
        let limit;
        try {
          write({ op: "stop", session });
          child.stdin.end();
          const result = await Promise.race([
            closed.promise,
            new Promise((_, rej) => {
              limit = setTimeout(() => rej(Error("scout closure unverified")), 350);
            }),
          ]);
          assert(
            !firstFailure && !buf && stopReceipt && result.code === 0 && !result.signal,
            "unverified scout cleanup: " + stderr,
          );
          const row = { result, birth, evidence: stopReceipt };
          notify("cleanup", row);
          return row;
        } finally {
          clearTimeout(timer);
          clearTimeout(limit);
        }
      })();
      return stopPromise;
    },
  };
}
