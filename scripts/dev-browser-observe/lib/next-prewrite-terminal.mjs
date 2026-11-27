// This one-shot is separate from the RSS series. Its result is usable only
// after the original birth, transfer, poll and helper/pipe closure all agree.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { readlinkSync } from "node:fs";
import { linuxBirth, sameLinuxBirth } from "./linux-birth.mjs";

export const CORE_NS = 150_000_000n,
  CLOSE_NS = 50_000_000n;
const keyMatch = (a, b) => sameLinuxBirth(a, b) && a.ppid === b.ppid && a.pgid === b.pgid;
function validatePrewriteAck({ row, birth, qualified, session, holderPid, token, descriptors }) {
  assert(
    row.purpose === "prewrite-terminal" &&
      keyMatch(row.helper, birth) &&
      birth.ppid === process.pid,
    "wrong identity helper",
  );
  const ack = row.ack;
  assert(
    ack.purpose === row.purpose &&
      ack.cycle === 0 &&
      ack.session === session &&
      ack.helperPid === birth.pid &&
      ack.holderPid === holderPid &&
      ack.token === token &&
      keyMatch(ack.birth, qualified) &&
      /^[a-f0-9]{24}$/.test(ack.nonce),
    "wrong pre-write handoff",
  );
  assert.deepEqual(ack.descriptors, descriptors);
  assert(
    [ack.acknowledgedNs, row.observedNs].every(
      (v) => typeof v === "string" && /^\d{1,20}$/.test(v),
    ),
    "invalid pre-write monotonic stamp",
  );
}

export function validatePrewritePoll(options) {
  validatePrewriteAck(options);
  const { row, qualified, start, closed } = options;
  const ack = row.ack;
  assert(
    row.pidfdFlags === 0 && [1, 16, 17].includes(row.pollMask),
    "worker not proved terminal at pre-write poll",
  );
  assert(
    start <= BigInt(ack.acknowledgedNs) &&
      BigInt(ack.acknowledgedNs) <= BigInt(row.observedNs) &&
      BigInt(row.observedNs) <= closed &&
      closed <= start + CORE_NS,
    "pre-write helper outside core",
  );
  return { birth: qualified, pidfdFlags: 0, pollMask: row.pollMask, observedNs: row.observedNs };
}

// Timers can fire early or late; all acceptance uses absolute monotonic clocks.
function atDeadline(at, callback) {
  let timer;
  const check = () => {
    const left = at - process.hrtime.bigint();
    if (left <= 0n) callback();
    else timer = setTimeout(check, Math.ceil(Number(left) / 1e6));
  };
  check();
  return () => clearTimeout(timer);
}

export function pollPrewriteTerminal(options) {
  const {
    python,
    script,
    channel,
    session,
    holderPid,
    qualified,
    token,
    append,
    signal,
    deadlineNs,
  } = options;
  const start = options.enteredNs ?? process.hrtime.bigint();
  const coreEnd = start + CORE_NS;
  const reserveEnd = coreEnd + CLOSE_NS;
  assert(
    reserveEnd <= deadlineNs && process.hrtime.bigint() < coreEnd,
    "insufficient pre-write decision budget",
  );
  signal?.throwIfAborted();
  const spawnNs = process.hrtime.bigint();
  const child = spawn(python, ["-I", "-B", script, String(process.pid), "--prewrite-terminal"], {
    stdio: ["pipe", "pipe", "pipe", channel],
  });
  const birth = child.pid ? linuxBirth(child.pid) : null;
  let failure,
    exit,
    output = "",
    stderr = "",
    bytes = 0,
    settled = false;
  const pipes = { stdin: false, stdout: false, stderr: false };
  return new Promise((resolve, reject) => {
    let clearTimer = () => {};
    const stamp = () => ({
      purpose: "prewrite-terminal",
      session,
      helper: birth,
      pid: child.pid ?? null,
      startedNs: String(start),
      spawnNs: String(spawnNs),
      coreDeadlineNs: String(coreEnd),
      reserveDeadlineNs: String(reserveEnd),
    });
    const finish = (closed) => {
      if (settled) return;
      settled = true;
      clearTimer();
      signal?.removeEventListener("abort", interrupt);
      const at = process.hrtime.bigint();
      const receipt = {
        ...stamp(),
        closed,
        exit,
        pipes: { ...pipes },
        stderr,
        stdout: output,
        closedObservedNs: String(at),
      };
      try {
        // Log raw physical results once, separately from any authority to write.
        // Even a late, malformed or nonterminal reply remains useful evidence.
        if (output.endsWith("\n")) {
          try {
            receipt.row = JSON.parse(output);
            validatePrewriteAck({ ...options, row: receipt.row, birth });
            receipt.authenticatedAck = true;
          } catch (error) {
            receipt.validationError = String(error);
          }
        }
        append({ ...receipt, status: "transport-observation" });
        if (failure) throw failure;
        assert(
          closed && Object.values(pipes).every(Boolean) && exit?.code === 0 && !exit.signal,
          "pre-write helper closure incomplete",
        );
        assert(receipt.authenticatedAck, "pre-write handoff could not be authenticated");
        receipt.proof = validatePrewritePoll({
          ...options,
          row: receipt.row,
          birth,
          start,
          closed: at,
        });
        const verified = process.hrtime.bigint();
        assert(verified <= coreEnd, "pre-write evidence or validation exceeded core");
        receipt.verifiedNs = String(verified);
        // Full acceptance is published by the caller only after a fresh graph
        // check; this transport row alone can never authorize mutation.
        resolve(receipt);
      } catch (error) {
        receipt.error = String(error.message ?? error);
        try {
          append({
            ...stamp(),
            status: "refused",
            observedNs: String(process.hrtime.bigint()),
            reason: receipt.error,
          });
        } catch (writeError) {
          receipt.evidenceFailure = String(writeError);
        }
        reject(Object.assign(error, { decisionReceipt: receipt }));
      }
    };
    const cancel = (e) => {
      if (settled || failure) return;
      failure = e instanceof Error ? e : Error(String(e));
      clearTimer();
      if (child.exitCode === null && child.signalCode === null) {
        try {
          child.kill("SIGKILL");
        } catch (error) {
          failure.killError = String(error);
        }
      }
      // Early cancellation gets at most 50 ms closure, and never outlives the
      // original absolute reserve even if a callback was dispatched late.
      const closeEnd = process.hrtime.bigint() + CLOSE_NS;
      clearTimer = atDeadline(closeEnd < reserveEnd ? closeEnd : reserveEnd, () => finish(false));
    };
    const interrupt = () => cancel(signal?.reason ?? Error("pre-write decision interrupted"));
    for (const [name, stream] of [
      ["stdin", child.stdin],
      ["stdout", child.stdout],
      ["stderr", child.stderr],
    ]) {
      stream.prependOnceListener("close", () => {
        pipes[name] = true;
      });
      stream.once("error", cancel);
    }
    child.once("error", cancel);
    child.once("exit", (code, term) => {
      exit = { code, signal: term };
    });
    child.once("close", () => {
      if (!settled) {
        finish(true);
        return;
      }
      // A late closure never changes the original result or lifts its hold.
      try {
        append({
          ...stamp(),
          status: "late-closure",
          observedNs: String(process.hrtime.bigint()),
          exit,
          pipes: { ...pipes },
        });
      } catch {
        /* The original unresolved receipt already requires reconciliation. */
      }
    });
    child.stdout.on("data", (chunk) => {
      bytes += chunk.length;
      if (bytes > 4096) cancel(Error("pre-write reply cap"));
      else output += chunk;
    });
    child.stderr.on("data", (chunk) => {
      if (Buffer.byteLength(stderr) + chunk.length > 4096) cancel(Error("pre-write stderr cap"));
      else stderr += chunk;
    });
    signal?.addEventListener("abort", interrupt, { once: true });
    clearTimer = atDeadline(coreEnd, () => cancel(Error("pre-write decision core expired")));
    try {
      if (failure || settled) return;
      assert(
        birth &&
          !birth.birthUnverifiable &&
          birth.ppid === process.pid &&
          readlinkSync(`/proc/${child.pid}/exe`) === python,
        "unverified decision helper birth/image",
      );
      append({ ...stamp(), status: "spawn" });
      child.stdin.end(
        JSON.stringify({
          purpose: "prewrite-terminal",
          session,
          holderPid,
          cycle: 0,
          birth: qualified,
          tokens: { [qualified.pid]: token },
        }) + "\n",
      );
    } catch (error) {
      cancel(error);
    }
  });
}
