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

export function startFixedCustody({ append, abort }) {
  const session = randomBytes(16).toString("hex");
  const python = interpreter();
  const child = spawn(python, ["-I", "-B", path, String(process.pid)], {
    stdio: ["pipe", "pipe", "pipe", "pipe"],
  });
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
  const fail = (error) => {
    if (fault) return;
    fault = error;
    ready.reject(error);
    abort(error);
    if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
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
      if (outputBytes > 2048) throw issue("response cap");
      pending += chunk.toString();
      while (pending.includes("\n")) {
        const end = pending.indexOf("\n");
        const msg = JSON.parse(pending.slice(0, end));
        pending = pending.slice(end + 1);
        if (!handshake && msg.ready === true && msg.pid === birth?.pid && msg.session === session) {
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
          stopping &&
          !stopReport &&
          msg.stopped === true &&
          msg.epochs === epoch &&
          Number.isSafeInteger(msg.pairsClosed) &&
          msg.pairsClosed >= 0 &&
          msg.pairsClosed <= 64
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
    begin() {
      if (!handshake || stopping || fault) throw fault ?? issue("holder unavailable");
      return {
        session,
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
    record(b, token) {
      if (typeof token !== "string" || !/^[a-f0-9]{32}$/.test(token)) throw issue("invalid token");
      if (retained.has(b.pid)) {
        if (!this.matches(b, token)) throw issue("changed birth or token");
      } else {
        if (retained.size >= 64) throw issue("ownership cap");
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
        });
      }
    },
    forget(b) {
      retained.delete(b.pid);
    },
    stop() {
      if (closePromise) return closePromise;
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
          if (!birth || !stopReport || ended.code !== 0 || ended.signal || pending)
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
