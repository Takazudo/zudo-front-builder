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
