// A separate authority for a pinned independent outer observer. The receive
// callback supplies kernel credentials. The decision consumes only cached proof.
import assert from "node:assert/strict";
import { sameLinuxBirth } from "./linux-birth.mjs";

const stamp = (s) => {
  assert(typeof s === "string" && /^\d{1,20}$/.test(s), "invalid outer clock");
  return BigInt(s);
};
const sameOwner = (a, b) => sameLinuxBirth(a, b) && a.ppid === b.ppid && a.pgid === b.pgid;

// A rejection can precede the cover/graph callback (for example, no cached
// receipt). A timestamp on that rejection cannot certify a graph we did not
// observe. Preserve the first detailed failure whenever one already exists.
export function outerCheckpoint(stage, operation, now = process.hrtime.bigint) {
  const entryNs = String(now());
  try {
    return operation();
  } catch (error) {
    if (!error.outerRefusal && error.code !== "NEXT_OUTER_DECISION_ONLY") {
      error.outerRefusal = {
        stage,
        entryNs,
        refusedNs: String(now()),
        coverage: "unknown",
        productGraph: "unknown",
        observerGraph: "unknown",
        lastAccepted: null,
        pending: null,
      };
    }
    throw error;
  }
}

/**
 * The outer pre-write decision. An ordinary sample in flight is first allowed
 * to settle (bounded; see settleOrdinaryBeforeDecision), then the one-use
 * receipt is consumed against a fresh coverage, graph and census check. The
 * settle record travels with the decision or the refusal, outside workload timing.
 */
export async function outerPrewriteDecision({
  safety,
  gate,
  readProductGraph,
  verifyWiring,
  attemptDeadlineNs,
  workDeadlineNs,
  signal,
  now = process.hrtime.bigint,
}) {
  const entryNs = String(now());
  let preDecisionSettle;
  try {
    signal?.throwIfAborted();
    preDecisionSettle = await safety.settleOrdinaryBeforeDecision({
      deadlineNs: attemptDeadlineNs < workDeadlineNs ? attemptDeadlineNs : workDeadlineNs,
      signal,
    });
  } catch (error) {
    error.outerRefusal ??= {
      stage: "prewrite-settle",
      entryNs,
      refusedNs: String(now()),
      coverage: "unknown",
      productGraph: "unknown",
      observerGraph: "unknown",
      lastAccepted: null,
      pending: null,
    };
    throw error;
  }
  try {
    return outerCheckpoint(
      "prewrite-decision",
      () => {
        signal?.throwIfAborted();
        assert(gate, "independent outer receipt unavailable");
        let graph, outerCadence, outerDecision;
        try {
          outerDecision = gate.consume(() => {
            outerCadence = safety.verifyOuterGraph(
              readProductGraph,
              () => {
                graph = verifyWiring();
              },
              "prewrite-decision",
            );
            return true;
          });
          outerDecision.attemptDeadlineNs = String(attemptDeadlineNs);
          const stamp = BigInt(outerDecision.decisionNs);
          assert(
            stamp < attemptDeadlineNs && stamp < BigInt(outerCadence.cadenceDeadlineNs),
            "outer decision expired",
          );
        } catch (error) {
          // A receipt consumed before a later refusal stays attributable.
          if (outerDecision) error.outerDecision = outerDecision;
          throw error;
        }
        return { outerDecision, outerCadence, graph, preDecisionSettle };
      },
      now,
    );
  } catch (error) {
    if (error.outerRefusal) error.outerRefusal.preDecisionSettle = preDecisionSettle;
    throw error;
  }
}

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
  let decisionNs;
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
      state = "decided";
      decisionNs = decision;
      return { ...receipt, decisionNs: String(decision) };
    },
    checkWriteTime(t0, verifyCurrentGraph) {
      const previous = state;
      state = "spent";
      assert.equal(previous, "decided", "outer decision unavailable at T0");
      // Evaluate the graph before drawing T0. No operation inside this callback
      // can open a helper, wait on an event or manufacture an earlier receipt.
      assert.equal(verifyCurrentGraph(), true, "current graph incomplete at T0");
      const actual = t0();
      assert(
        typeof actual === "bigint" && decisionNs <= actual && actual < expires,
        "outer receipt expired at T0",
      );
      return { ...receipt, decisionNs: String(decisionNs), t0Ns: String(actual) };
    },
    retire() {
      state = "refused";
    },
  };
}
