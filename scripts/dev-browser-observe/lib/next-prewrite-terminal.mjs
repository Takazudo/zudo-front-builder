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
  const start = process.hrtime.bigint(),
    coreEnd = start + CORE_NS;
  assert(start + CORE_NS + CLOSE_NS <= deadlineNs, "insufficient pre-write decision budget");
  signal?.throwIfAborted();
  const child = spawn(python, ["-I", "-B", script, String(process.pid), "--prewrite-terminal"], {
    stdio: ["pipe", "pipe", "pipe", channel],
  });
  const birth = child.pid ? linuxBirth(child.pid) : null;
  let failure,
    output = "",
    stderr = "",
    bytes = 0,
    exit,
    settled = false;
  return new Promise((resolve, reject) => {
    let timer;
    const finish = (closed) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      signal?.removeEventListener("abort", interrupt);
      const at = process.hrtime.bigint();
      const receipt = {
        purpose: "prewrite-terminal",
        helper: birth,
        pid: child.pid ?? null,
        session,
        startedNs: String(start),
        finishedNs: String(at),
        closed,
        exit,
        stderr,
        stdout: output,
      };
      try {
        if (failure) throw failure;
        assert(
          closed && at <= coreEnd && exit?.code === 0 && !exit.signal && output.endsWith("\n"),
          "pre-write helper incomplete",
        );
        const row = JSON.parse(output);
        receipt.row = row;
        validatePrewriteAck({ ...options, row, birth });
        receipt.authenticatedAck = true;
        receipt.proof = validatePrewritePoll({ ...options, row, birth, start, closed: at });
        assert(process.hrtime.bigint() <= coreEnd, "pre-write validation exceeded core");
        append(receipt);
        assert(process.hrtime.bigint() <= coreEnd, "pre-write evidence commit exceeded core");
        resolve(receipt);
      } catch (error) {
        receipt.error = String(error.message ?? error);
        try {
          append(receipt);
        } catch (writeError) {
          receipt.evidenceFailure = String(writeError);
        }
        reject(Object.assign(error, { decisionReceipt: receipt }));
      }
    };
    const cancel = (e) => {
      if (settled || failure) return;
      failure = e instanceof Error ? e : new Error(String(e));
      clearTimeout(timer);
      if (child.exitCode === null && child.signalCode === null) {
        try {
          child.kill("SIGKILL");
        } catch (killError) {
          failure.killError = String(killError);
        }
      }
      timer = setTimeout(() => finish(false), Number(CLOSE_NS / 1_000_000n));
    };
    const interrupt = () => cancel(signal?.reason ?? Error("pre-write decision interrupted"));
    const expire = () => {
      const left = coreEnd - process.hrtime.bigint();
      if (left > 0n) timer = setTimeout(expire, Math.ceil(Number(left) / 1e6));
      else cancel(Error("pre-write decision core expired"));
    };
    for (const stream of child.stdio) stream?.on("error", cancel);
    child.once("error", cancel);
    child.once("exit", (code, term) => {
      exit = { code, signal: term };
    });
    child.once("close", () => finish(true));
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
    expire();
    try {
      assert(
        birth &&
          !birth.birthUnverifiable &&
          birth.ppid === process.pid &&
          readlinkSync(`/proc/${child.pid}/exe`) === python,
        "unverified decision helper birth/image",
      );
      append({
        purpose: "prewrite-terminal-spawn",
        session,
        helper: birth,
        startedNs: String(start),
      });
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
    } catch (e) {
      cancel(e);
    }
  });
}
