// A separate authority for a pinned independent outer observer. The receive
// callback supplies kernel credentials. The decision consumes only cached proof.
import assert from "node:assert/strict";
import { sameLinuxBirth } from "./linux-birth.mjs";

const stamp = (s) => {
  assert(typeof s === "string" && /^\d{1,20}$/.test(s), "invalid outer clock");
  return BigInt(s);
};
const sameOwner = (a, b) => sameLinuxBirth(a, b) && a.ppid === b.ppid && a.pgid === b.pgid;

export function outerTerminalGate({ authority, now = process.hrtime.bigint }) {
  const expected = structuredClone(authority);
  assert(sameOwner(expected.outer, expected.outer) && sameOwner(expected.worker, expected.worker));
  assert(/^[0-9a-f]{32}$/.test(expected.nonce), "invalid session nonce");
  assert(/^[0-9a-f]{32}$/.test(expected.token), "invalid transfer token");
  assert.equal(expected.credentials.pid, expected.outer.pid);
  assert(
    [expected.credentials.uid, expected.credentials.gid].every(
      (id) => Number.isSafeInteger(id) && id >= 0,
    ),
  );
  assert(
    Array.isArray(expected.descriptors) &&
      expected.descriptors.length === 2 &&
      expected.descriptors.every(
        (pair) =>
          Array.isArray(pair) &&
          pair.length === 2 &&
          pair.every((s) => typeof s === "string" && /^\d{1,20}$/.test(s)),
      ) &&
      expected.descriptors[0].join("/") !== expected.descriptors[1].join("/"),
    "distinct original descriptor identities required",
  );
  const start = stamp(expected.startNs);
  const expires = stamp(expected.deadlineNs);
  assert(start < expires);
  let state = "waiting";
  let receipt;
  return {
    accept({ kernelSender, packet, currentOuter }) {
      try {
        assert.equal(state, "waiting", "conflicting outer receipt");
        assert.deepEqual(kernelSender, expected.credentials, "outer kernel sender changed");
        assert(sameOwner(currentOuter, expected.outer), "outer birth changed");
        assert.equal(packet.nonce, expected.nonce);
        assert.equal(packet.token, expected.token);
        assert(sameOwner(packet.worker, expected.worker), "wrong qualified worker");
        assert.deepEqual(packet.descriptors, expected.descriptors, "original pair changed");
        assert.equal(packet.pidfdFlags, 0);
        assert([1, 16, 17].includes(packet.pollMask), "whole group exit unavailable");
        const first = stamp(packet.firstLiveNs);
        const second = stamp(packet.secondLiveNs);
        const terminal = stamp(packet.terminalNs);
        const sent = stamp(packet.sentNs);
        const received = now();
        assert(
          start <= first &&
            first < second &&
            second <= terminal &&
            terminal <= sent &&
            sent <= received &&
            received < expires,
          "outer observation outside clock bounds",
        );
        receipt = { packet: structuredClone(packet), receivedNs: String(received) };
        state = "ready";
      } catch (error) {
        state = "refused";
        throw error;
      }
    },
    consume(verifyCurrentGraph) {
      const previous = state;
      state = "spent";
      assert.equal(previous, "ready", "outer receipt unavailable");
      // Synchronous fresh graph verification is the caller's normal gate. A
      // promise or any other non-true result cannot admit the decision.
      assert.equal(verifyCurrentGraph(), true, "current graph incomplete");
      const decision = now();
      assert(
        stamp(receipt.receivedNs) < decision && decision < expires,
        "receipt unavailable before decision",
      );
      return { ...receipt, decisionNs: String(decision) };
    },
    retire() {
      state = "refused";
    },
  };
}
