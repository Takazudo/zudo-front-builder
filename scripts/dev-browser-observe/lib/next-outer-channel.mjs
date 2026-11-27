// Only an explicitly qualified outer observer may supply this private bundle.
// fd3 remains the controller's ordinary IPC. fd4 is separately forwarded to
// the holder. Receipt/wakeup descriptors never reach a product or sampler.
import assert from "node:assert/strict";
import { closeSync, fstatSync, readlinkSync } from "node:fs";
import { linuxBirth, sameLinuxBirth } from "./linux-birth.mjs";

export const NEXT_OUTER_PROVIDER = "linux-next-outer-receipt.v1";

export function takeNextOuterChannel({ provider, outer, fds, sockets }) {
  assert.equal(provider, NEXT_OUTER_PROVIDER);
  assert.equal(process.platform, "linux");
  assert.equal(process.arch, "x64");
  assert.equal(process.version, "v26.13.1");
  assert.deepEqual(process.execArgv, []);
  const current = linuxBirth(process.ppid);
  assert(
    sameLinuxBirth(current, outer) && current.ppid === outer.ppid && current.pgid === outer.pgid,
    "changed outer supervisor parent",
  );
  assert(Array.isArray(fds) && fds.length === 3 && new Set(fds).size === 3);
  assert(Array.isArray(sockets) && sockets.length === 3 && new Set(sockets).size === 3);
  // Ownership is transferred only after validating the entire set. A malformed
  // declaration can name unrelated numbers: never close them on failed validation.
  for (let i = 0; i < fds.length; i++) {
    const fd = fds[i];
    assert(Number.isSafeInteger(fd) && fd > 2 && fd <= 1024);
    assert(/^socket:\[\d{1,20}\]$/.test(sockets[i]));
    assert.equal(readlinkSync("/proc/self/fd/" + fd), sockets[i]);
    assert(fstatSync(fd).isSocket());
  }
  const owned = [...fds];
  return {
    descriptors: [...fds],
    close() {
      const errors = [];
      for (const fd of owned.splice(0)) {
        try {
          closeSync(fd);
        } catch (error) {
          errors.push(error);
        }
      }
      if (errors.length) throw new AggregateError(errors, "outer channel close failed");
    },
  };
}

// These fixed descriptors belong to the controller after the normal fork.
// The listener runs independently of the mutation gate. It accepts no queued
// data retroactively if the holder's qualified token is still unavailable.
export async function observeNextOuterReceipt({
  peer,
  gateAtDelivery,
  onFailure,
  record,
  signal,
  deadlineNs,
}) {
  const { Socket } = await import("node:net");
  const { openNextOuterReceiver } = await import("./next-outer-receive.mjs");
  let reader;
  try {
    reader = await openNextOuterReceiver({ signal, deadlineNs });
  } catch (error) {
    try {
      closeSync(5);
    } finally {
      closeSync(6);
    }
    throw error;
  }
  let wake;
  const wakeIdentity = readlinkSync("/proc/self/fd/6");
  try {
    wake = new Socket({ fd: 6, readable: true, writable: false });
  } catch (error) {
    try {
      // libuv may already have disposed of the given fd. Never close an
      // unrelated descriptor that reused its number.
      if (readlinkSync("/proc/self/fd/6") === wakeIdentity) closeSync(6);
    } catch (checkError) {
      if (checkError.code !== "ENOENT") throw new AggregateError([error, checkError]);
    } finally {
      try {
        closeSync(5);
      } finally {
        reader.close();
      }
    }
    throw error;
  }
  let delivered = false;
  let stopped = false;
  let failure;
  const closed = new Promise((resolve) => wake.once("close", resolve));
  const fail = (error) => {
    if (failure || stopped) return;
    failure = error;
    wake.destroy();
    onFailure(error);
  };
  wake.on("error", fail);
  wake.on("end", () => {
    if (!delivered) fail(new Error("outer closed before terminal receipt"));
  });
  wake.on("data", (bytes) => {
    try {
      assert(!delivered && !failure && !stopped, "conflicting outer wake");
      assert.deepEqual(bytes, Buffer.from([71]), "invalid outer wake");
      const { payload, peer: kernelSender, receivedNs } = reader.receive(5, peer);
      const gate = gateAtDelivery();
      assert(gate, "outer receipt preceded official holder qualification");
      const currentOuter = linuxBirth(peer.pid);
      gate.accept({ kernelSender, packet: payload, currentOuter });
      delivered = true;
      record({ kernelSender, payload, receivedNs, acceptedNs: String(process.hrtime.bigint()) });
    } catch (error) {
      fail(error);
    }
  });
  return {
    async stop() {
      if (!stopped) {
        stopped = true;
        wake.destroy();
        try {
          closeSync(5);
        } finally {
          reader.close();
        }
      }
      await closed;
      if (failure) throw failure;
    },
  };
}
