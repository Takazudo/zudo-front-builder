// The controller becomes a child subreaper (PR_SET_CHILD_SUBREAPER) for fixed
// v4 startup custody, so a descendant that escapes its process group and
// outlives its parent is reparented here, where the startup census sees it,
// instead of to init (#4862 product-completion premise).
import { readFileSync, readdirSync } from "node:fs";

const PR_SET_CHILD_SUBREAPER = 36;
const PR_GET_CHILD_SUBREAPER = 37;

async function loadFfi() {
  try {
    return await import("node:ffi");
  } catch (error) {
    throw new Error("controller subreaper requires node:ffi (pinned Node 26): " + error.message);
  }
}

/** Set PR_SET_CHILD_SUBREAPER to `value` and prove it with PR_GET_CHILD_SUBREAPER. */
export async function setChildSubreaper(value, { ffi } = {}) {
  ffi ??= await loadFfi();
  const set = ffi.dlopen("libc.so.6", {
    prctl: { arguments: ["int32", "uint64", "uint64", "uint64", "uint64"], return: "int32" },
  });
  try {
    const get = ffi.dlopen("libc.so.6", {
      prctl: { arguments: ["int32", "buffer", "uint64", "uint64", "uint64"], return: "int32" },
    });
    try {
      const rc = set.functions.prctl(PR_SET_CHILD_SUBREAPER, BigInt(value), 0n, 0n, 0n);
      const read = Buffer.alloc(4);
      const got = get.functions.prctl(PR_GET_CHILD_SUBREAPER, read, 0n, 0n, 0n);
      if (rc !== 0 || got !== 0 || read.readInt32LE(0) !== value)
        throw new Error(`controller subreaper not ${value ? "enabled" : "cleared"}`);
      return { enabled: value === 1, pid: process.pid };
    } finally {
      get.lib.close();
    }
  } finally {
    set.lib.close();
  }
}

export const enableChildSubreaper = (options) => setChildSubreaper(1, options);
// Cleared right after the startup readiness census: later orphans (browser
// helpers, a stop that outlives the shim) go to init again, as before #4862.
export const clearChildSubreaper = (options) => setChildSubreaper(0, options);

/** Direct children of `pid`, from every task's children list. */
export function controllerChildren(pid = process.pid) {
  const children = new Set();
  for (const task of readdirSync(`/proc/${pid}/task`)) {
    try {
      for (const child of readFileSync(`/proc/${pid}/task/${task}/children`, "utf8").split(/\s+/))
        if (child) children.add(Number(child));
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
  }
  return [...children].sort((a, b) => a - b);
}
