import assert from "node:assert/strict";
import { test } from "vitest";
import {
  clearChildSubreaper,
  enableChildSubreaper,
  setChildSubreaper,
} from "../lib/linux-subreaper.mjs";

// A fake node:ffi: prctl(36, v) sets the flag (rc `setRc`), prctl(37, buf) writes it.
function fakeFfi({ setRc = 0, getRc = 0, sticky = null } = {}) {
  const state = { flag: 0, calls: [], closed: 0 };
  const ffi = {
    dlopen(lib, symbols) {
      assert.equal(lib, "libc.so.6");
      const reads = symbols.prctl.arguments[1] === "buffer";
      return {
        lib: { close: () => state.closed++ },
        functions: {
          prctl(option, arg, ...rest) {
            state.calls.push([option, reads ? "buffer" : arg, ...rest]);
            if (option === 36) {
              if (setRc === 0) state.flag = sticky ?? Number(arg);
              return setRc;
            }
            assert.equal(option, 37);
            arg.writeInt32LE(state.flag, 0);
            return getRc;
          },
        },
      };
    },
  };
  return { ffi, state };
}

test("enable and clear set the flag and prove it with PR_GET", async () => {
  const f = fakeFfi();
  assert.deepEqual(await enableChildSubreaper({ ffi: f.ffi }), { enabled: true, pid: process.pid });
  assert.equal(f.state.flag, 1);
  assert.deepEqual(await clearChildSubreaper({ ffi: f.ffi }), {
    enabled: false,
    pid: process.pid,
  });
  assert.equal(f.state.flag, 0);
  assert.deepEqual(f.state.calls, [
    [36, 1n, 0n, 0n, 0n],
    [37, "buffer", 0n, 0n, 0n],
    [36, 0n, 0n, 0n, 0n],
    [37, "buffer", 0n, 0n, 0n],
  ]);
  assert.equal(f.state.closed, 4);
});

test.each([
  ["PR_SET fails", { setRc: -1 }, 1, /not enabled/],
  ["PR_GET fails", { getRc: -1 }, 1, /not enabled/],
  ["PR_GET reads the old value", { sticky: 0 }, 1, /not enabled/],
  ["a clear that does not take", { sticky: 1 }, 0, /not cleared/],
])("the subreaper refuses when %s", async (_label, options, value, message) => {
  const f = fakeFfi(options);
  await assert.rejects(setChildSubreaper(value, { ffi: f.ffi }), message);
  assert.equal(f.state.closed, 2);
});

test("without node:ffi the subreaper refuses (system Node 24 path)", async () => {
  let ffi = null;
  try {
    ffi = await import("node:ffi");
  } catch {}
  if (ffi) return; // pinned Node 26: the real path runs in linux-subreaper.test.mjs
  await assert.rejects(enableChildSubreaper(), /requires node:ffi/);
});
