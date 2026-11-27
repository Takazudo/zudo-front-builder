// The controller becomes a child subreaper (PR_SET_CHILD_SUBREAPER) for fixed
// v4 startup custody, so a descendant that escapes its process group and
// outlives its parent is reparented here, where the startup census sees it,
// instead of to init (#4862 product-completion premise).
import { readFileSync, readdirSync } from "node:fs";

const PR_SET_CHILD_SUBREAPER = 36;
const PR_GET_CHILD_SUBREAPER = 37;

export async function enableChildSubreaper() {
  let ffi;
  try {
    ffi = await import("node:ffi");
  } catch (error) {
    throw new Error("controller subreaper requires node:ffi (pinned Node 26): " + error.message);
  }
  const set = ffi.dlopen("libc.so.6", {
    prctl: { arguments: ["int32", "uint64", "uint64", "uint64", "uint64"], return: "int32" },
  });
  const get = ffi.dlopen("libc.so.6", {
    prctl: { arguments: ["int32", "buffer", "uint64", "uint64", "uint64"], return: "int32" },
  });
  try {
    const rc = set.functions.prctl(PR_SET_CHILD_SUBREAPER, 1n, 0n, 0n, 0n);
    const value = Buffer.alloc(4);
    const read = get.functions.prctl(PR_GET_CHILD_SUBREAPER, value, 0n, 0n, 0n);
    const enabled = rc === 0 && read === 0 && value.readInt32LE(0) === 1;
    if (!enabled) throw new Error("controller subreaper not enabled");
    return { enabled, pid: process.pid };
  } finally {
    set.lib.close();
    get.lib.close();
  }
}

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
