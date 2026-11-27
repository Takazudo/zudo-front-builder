import { spawn } from "node:child_process";
import { closeSync, fstatSync, openSync } from "node:fs";
import { linuxBirth } from "./linux-birth.mjs";

// macOS ships no flock(1) and Node has no flock binding, so a perl child holds
// flock(LOCK_EX|LOCK_NB) for as long as its stdin stays open. The lock dies
// with the controller (stdin EOF) or the helper; deleting the file proves nothing.
const PERL_HELPER = String.raw`
use strict; use Fcntl qw(:flock); use Errno qw(EAGAIN EWOULDBLOCK);
$| = 1;
my $path = shift @ARGV;
open(my $fh, ">>", $path) or do { print "ERROR open $!\n"; exit 4; };
if (!flock($fh, LOCK_EX | LOCK_NB)) {
  if ($! == EAGAIN || $! == EWOULDBLOCK) { print "BUSY\n"; exit 3; }
  print "ERROR flock $!\n"; exit 4;
}
print "LOCKED $$\n";
while (my $l = <STDIN>) {}
exit 0;
`;

export class LockBusyError extends Error {
  constructor(path) {
    super(`lock busy: ${path}`);
    this.code = "LOCK_BUSY";
  }
}

const ACQUIRE_MS = 3000;
const RELEASE_MS = 2000;
const CLOSE_MS = 1000;
const OUTPUT_BYTES = 1024;

async function within(promise, ms) {
  let timer;
  try {
    return await Promise.race([
      promise,
      new Promise((resolve) => {
        timer = setTimeout(() => resolve(null), ms);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

/** An acknowledgement admits a batch only when the original child reports its own PID in time. */
export async function acquireLock(path, { perl = "perl", signal, spawnHelper = spawn } = {}) {
  const start = process.hrtime.bigint();
  const deadline = start + BigInt(ACQUIRE_MS) * 1_000_000n;
  const evidence = {
    path,
    startNs: String(start),
    deadlineNs: String(deadline),
    pid: null,
    birth: null,
    response: null,
    stdoutBytes: 0,
    stderrBytes: 0,
    exit: null,
    close: null,
    signalRequest: null,
    pipesClosed: null,
    cleanupVerified: false,
  };
  let child;
  try {
    signal?.throwIfAborted();
    const fd = openSync(path, "a");
    try {
      const stat = fstatSync(fd, { bigint: true });
      evidence.parentOpenedFile = { device: String(stat.dev), inode: String(stat.ino) };
    } finally {
      closeSync(fd);
    }
    signal?.throwIfAborted();
    if (process.hrtime.bigint() >= deadline) throw new Error("lock helper acquisition timed out");
    child = spawnHelper(perl, ["-e", PERL_HELPER, path], { stdio: ["pipe", "pipe", "pipe"] });
  } catch (error) {
    evidence.decisionNs = evidence.finishedNs = String(process.hrtime.bigint());
    evidence.cleanupVerified = true; // No ChildProcess was returned.
    throw Object.assign(error instanceof Error ? error : new Error(String(error)), {
      lockHelper: structuredClone(evidence),
    });
  }

  evidence.pid = child.pid ?? null;
  if (process.platform === "linux" && child.pid) evidence.birth = linuxBirth(child.pid);
  const ack = Promise.withResolvers();
  let response = "",
    diagnostic = "",
    firstError = null,
    acquired = false;
  function refuse(error) {
    firstError ??= error;
    ack.reject(firstError);
  }
  function receive(chunk) {
    evidence.stdoutBytes += chunk.length;
    if (response.length + chunk.length > OUTPUT_BYTES) {
      response += chunk.subarray(0, OUTPUT_BYTES - response.length).toString("latin1");
      refuse(new Error("lock helper response exceeded its limit"));
      return;
    }
    response += chunk.toString("latin1");
    if (firstError) return;
    if (acquired || evidence.response !== null) {
      refuse(new Error("unexpected data after lock helper response"));
      return;
    }
    if (!response.includes("\n")) return;
    const received = process.hrtime.bigint();
    evidence.responseNs = String(received);
    if (received > deadline) return refuse(new Error("lock helper acquisition timed out"));
    if (response === "BUSY\n") evidence.response = "busy";
    else if (child.pid && response === `LOCKED ${child.pid}\n`) evidence.response = "locked";
    else if (/^ERROR [^\r\n]+\n$/.test(response)) {
      evidence.response = "error";
      return refuse(new Error("lock helper failed: " + response.trimEnd()));
    } else return refuse(new Error("invalid lock helper response: " + JSON.stringify(response)));
    ack.resolve(evidence.response);
  }
  child.stdout.on("data", receive);
  child.stderr.on("data", (chunk) => {
    evidence.stderrBytes += chunk.length;
    diagnostic += chunk
      .subarray(0, Math.max(0, OUTPUT_BYTES - diagnostic.length))
      .toString("latin1");
  });
  for (const stream of child.stdio) stream.on("error", refuse);
  child.on("error", refuse);
  const exited = new Promise((resolve) =>
    child.once("exit", (code, signal) => {
      evidence.exitNs = String(process.hrtime.bigint());
      evidence.exit = { code, signal };
      resolve(evidence.exit);
    }),
  );
  const closed = new Promise((resolve) =>
    child.once("close", (code, signal) => {
      evidence.closeNs = String(process.hrtime.bigint());
      evidence.close = { code, signal };
      if (!evidence.response)
        refuse(new Error("lock helper closed before reporting: " + JSON.stringify(response)));
      resolve(evidence.close);
    }),
  );
  function terminate() {
    if (child.pid && !evidence.exit && !evidence.close) {
      try {
        evidence.signalRequest = { signal: "SIGKILL", sent: child.kill("SIGKILL") };
      } catch (error) {
        evidence.signalRequest = { signal: "SIGKILL", error: error.message };
      }
    }
  }
  function snapshot() {
    evidence.finishedNs = String(process.hrtime.bigint());
    evidence.stdout = response;
    evidence.stderr = diagnostic;
    evidence.pipesClosed = child.stdio.map((s) => s.closed);
    evidence.cleanupVerified = Boolean(
      evidence.close && (!child.pid || evidence.exit) && evidence.pipesClosed.every(Boolean),
    );
    return evidence;
  }
  async function finish(force) {
    child.stdin.end();
    if (force) terminate();
    else if (!(await within(closed, RELEASE_MS))) terminate();
    await within(closed, CLOSE_MS);
    snapshot();
    if (!evidence.cleanupVerified) {
      // A descendant can inherit a pipe. Record the missing remote closure
      // before detaching our local handles; local destruction is not its proof.
      evidence.localHandlesDetached = true;
      for (const pipe of child.stdio) pipe.destroy();
      child.unref();
    }
    return evidence;
  }
  const cancel = () =>
    refuse(new Error("lock helper acquisition cancelled", { cause: signal.reason }));
  signal?.addEventListener("abort", cancel, { once: true });
  if (signal?.aborted) cancel();
  let timer;
  const armDeadline = () => {
    const ns = deadline - process.hrtime.bigint();
    if (ns <= 0n) refuse(new Error("lock helper acquisition timed out"));
    else timer = setTimeout(armDeadline, Math.ceil(Number(ns) / 1e6));
  };
  armDeadline();
  let result;
  try {
    result = await ack.promise;
    evidence.decisionNs = String(process.hrtime.bigint());
    if (BigInt(evidence.decisionNs) > deadline)
      throw new Error("lock helper acquisition timed out");
    if (firstError) throw firstError;
    if (
      result === "locked" &&
      (child.exitCode !== null || child.signalCode !== null || evidence.close)
    ) {
      throw new Error("lock helper exited before acquisition decision");
    }
  } catch (error) {
    evidence.decisionNs ??= String(process.hrtime.bigint());
    await finish(true);
    throw Object.assign(error instanceof Error ? error : new Error(String(error)), {
      lockHelper: structuredClone(evidence),
    });
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener("abort", cancel);
  }
  if (result === "busy") {
    await finish(false);
    if (evidence.cleanupVerified && evidence.exit?.code === 3 && !firstError) {
      throw Object.assign(new LockBusyError(path), { lockHelper: structuredClone(evidence) });
    }
    throw Object.assign(firstError ?? new Error("unverified lock helper contention"), {
      lockHelper: structuredClone(evidence),
    });
  }
  acquired = true;
  let releasing;
  return {
    path,
    helperPid: child.pid,
    exited,
    release() {
      releasing ??= (async () => {
        await finish(false);
        if (!evidence.cleanupVerified || evidence.exit?.code !== 0 || firstError) {
          throw Object.assign(firstError ?? new Error("lock helper stopped abnormally"), {
            lockHelper: structuredClone(evidence),
          });
        }
        return evidence.exit;
      })();
      return releasing;
    },
  };
}
