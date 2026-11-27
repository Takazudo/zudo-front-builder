import assert from "node:assert/strict";
import { test } from "vitest";
import { outerTerminalGate } from "../lib/next-outer-terminal.mjs";

const birth = (pid) => ({
  birthVersion: "linux-proc-v1",
  pid,
  startTicks: "1234",
  pgid: 10,
  ppid: 11,
  bootId: "8116e36c-eeea-4ca0-973f-b1af4f6144d6",
});

function fixture() {
  let clock = 80n;
  const authority = {
    outer: birth(300),
    worker: birth(301),
    credentials: { pid: 300, uid: 1000, gid: 1000 },
    nonce: "ab".repeat(16),
    token: "cd".repeat(16),
    descriptors: [
      ["6", "123"],
      ["7", "456"],
    ],
    startNs: "10",
    deadlineNs: "100",
  };
  const receipt = {
    kernelSender: { ...authority.credentials },
    currentOuter: { ...authority.outer },
    packet: {
      worker: { ...authority.worker },
      nonce: authority.nonce,
      token: authority.token,
      descriptors: structuredClone(authority.descriptors),
      pidfdFlags: 0,
      pollMask: 1,
      firstLiveNs: "20",
      secondLiveNs: "30",
      terminalNs: "50",
      sentNs: "60",
    },
  };
  return {
    gate: outerTerminalGate({ authority, now: () => clock }),
    receipt,
    set: (t) => {
      clock = t;
    },
  };
}

test("one original receipt can authorize only one later decision inside the bound", () => {
  const { gate, receipt, set } = fixture();
  gate.accept(receipt);
  receipt.packet.pollMask = 0;
  set(90n);
  const result = gate.consume(() => true);
  assert.equal(result.packet.pollMask, 1);
  assert.equal(result.receivedNs, "80");
  assert.equal(result.decisionNs, "90");
  assert.throws(() => gate.consume(() => true));
});
test.each([
  ["sibling", (r) => r.kernelSender.pid++],
  ["different parent", (r) => r.currentOuter.ppid++],
  ["worker rebirth", (r) => (r.packet.worker.startTicks = "1235")],
  ["changed original pair", (r) => r.packet.descriptors.reverse()],
  ["live group", (r) => (r.packet.pollMask = 0)],
  ["thread pidfd", (r) => (r.packet.pidfdFlags = 1)],
  ["same live observation", (r) => (r.packet.secondLiveNs = r.packet.firstLiveNs)],
  ["future send", (r) => (r.packet.sentNs = "90")],
  ["other nonce", (r) => (r.packet.nonce = "00".repeat(16))],
  ["other token", (r) => (r.packet.token = "00".repeat(16))],
])("%s permanently refuses", (_name, change) => {
  const { gate, receipt } = fixture();
  change(receipt);
  assert.throws(() => gate.accept(receipt));
  assert.throws(() => gate.accept(fixture().receipt));
  assert.throws(() => gate.consume(() => true));
});
test.each([
  "late",
  "missing",
  "replayed",
  "retired",
  "stale-graph",
  "async-graph",
  "no-prior-receipt",
])("%s leaves no usable authority", (mode) => {
  const { gate, receipt, set } = fixture();
  if (mode !== "missing") gate.accept(receipt);
  if (mode === "retired") gate.retire();
  if (mode === "replayed") assert.throws(() => gate.accept(receipt));
  set(mode === "late" ? 100n : mode === "no-prior-receipt" ? 80n : 90n);
  assert.throws(() =>
    gate.consume(() => (mode === "async-graph" ? Promise.resolve(true) : mode !== "stale-graph")),
  );
  assert.throws(() => gate.accept(receipt));
});
