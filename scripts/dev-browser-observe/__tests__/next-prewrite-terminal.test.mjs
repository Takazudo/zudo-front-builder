import assert from "node:assert/strict";
import { test } from "vitest";
import {
  CORE_NS,
  CLOSE_NS,
  pollPrewriteTerminal,
  validatePrewritePoll,
} from "../lib/next-prewrite-terminal.mjs";

const birth = (pid, ppid) => ({
  pid,
  ppid,
  pgid: 20,
  startTicks: String(pid),
  bootId: "1edafe52-089e-4f16-a478-1143d29052bd",
  birthVersion: "linux-proc-v1",
});
function fixture() {
  const helper = birth(22, process.pid),
    qualified = birth(25, 24),
    descriptors = [
      ["61", "101"],
      ["32", "202"],
    ];
  const args = {
    birth: helper,
    qualified,
    session: "original",
    holderPid: 20,
    token: "positive",
    descriptors,
    start: 100n,
    closed: 160n,
  };
  const ack = {
    purpose: "prewrite-terminal",
    session: args.session,
    cycle: 0,
    nonce: "ab".repeat(12),
    helperPid: 22,
    holderPid: 20,
    birth: qualified,
    token: "positive",
    descriptors,
    acknowledgedNs: "120",
  };
  return {
    ...args,
    row: { purpose: ack.purpose, helper, ack, pollMask: 17, observedNs: "130", pidfdFlags: 0 },
  };
}
test("only a contemporaneous terminal poll on the original authenticated pair is eligible", () => {
  const positive = fixture();
  assert.deepEqual(validatePrewritePoll(positive), {
    birth: positive.qualified,
    pidfdFlags: 0,
    pollMask: 17,
    observedNs: "130",
  });
  const corruptions = [
    (x) => (x.row.pollMask = null),
    (x) => (x.row.pollMask = 8),
    (x) => (x.row.pidfdFlags = 1),
    (x) => (x.row.ack.cycle = 1),
    (x) => x.row.ack.holderPid++,
    (x) => (x.row.ack.session = "stale"),
    (x) => (x.row.ack.nonce = "invalid"),
    (x) =>
      (x.row.ack.descriptors = [
        ["61", "404"],
        ["32", "202"],
      ]),
    (x) => (x.row.ack.birth = { ...x.qualified, startTicks: "224" }),
    (x) => (x.row.helper = { ...x.birth, ppid: 1 }),
    (x) => (x.row.ack.acknowledgedNs = "140"),
    (x) => (x.row.observedNs = "161"),
    (x) => (x.closed = x.start + CORE_NS + 1n),
  ];
  for (const corrupt of corruptions) {
    const x = fixture();
    corrupt(x);
    assert.throws(() => validatePrewritePoll(x));
  }
});
test("insufficient existing time refuses before spawning a helper", () => {
  assert.throws(
    () => pollPrewriteTerminal({ deadlineNs: process.hrtime.bigint() + CORE_NS + CLOSE_NS - 1n }),
    /insufficient/,
  );
});
