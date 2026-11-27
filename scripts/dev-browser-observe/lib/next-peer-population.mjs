// Reconcile the completed cleanup census without influencing its signal decisions.
import assert from "node:assert/strict";
import { openSync, closeSync, fstatSync, readSync, statSync } from "node:fs";
import { sameLinuxBirth } from "./linux-birth.mjs";

export function markNextRegistry(path) {
  try {
    const st = statSync(path);
    return { size: st.size, device: st.dev, inode: st.ino };
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
    return { size: 0 };
  }
}

export function readNextRegistry(path, mark, label) {
  const fd = openSync(path, "r");
  try {
    const st = fstatSync(fd);
    assert(
      mark.inode === undefined || (st.ino === mark.inode && st.dev === mark.device),
      "registry file changed",
    );
    const length = st.size - mark.size;
    assert(
      Number.isSafeInteger(length) && length > 0 && length <= 512 * 1024,
      "registry suffix missing or excessive",
    );
    const data = Buffer.alloc(length);
    let n = 0;
    while (n < length) {
      const count = readSync(fd, data, n, length - n, mark.size + n);
      assert(count > 0, "truncated registry");
      n += count;
    }
    assert(data.at(-1) === 10, "unterminated registry row");
    return data
      .toString("utf8")
      .trimEnd()
      .split("\n")
      .map((line) => JSON.parse(line))
      .filter((r) => r.label === label);
  } finally {
    closeSync(fd);
  }
}

export function reconcileNextPopulation({ chain, worker, terminalProof, stop, registry }) {
  const violations = [];
  const reject = (source, reason, b) =>
    violations.push({ source, reason, ...(b && { pid: b.pid, birth: b }) });
  assert(Array.isArray(chain) && chain.length === 3, "missing three qualified ancestors");
  const expected = new Map();
  for (const b of [...chain, ...(worker ? [worker] : [])]) {
    assert(b && !expected.has(b.pid) && sameLinuxBirth(b, b), "duplicate or unverified role birth");
    expected.set(b.pid, b);
  }
  const same = (a, b) => sameLinuxBirth(a, b) && a.ppid === b.ppid && a.pgid === b.pgid;
  if (
    worker &&
    !(
      same(terminalProof?.birth, worker) &&
      terminalProof.pidfdFlags === 0 &&
      [1, 16, 17].includes(terminalProof.pollMask)
    )
  )
    reject("terminal", "missing matching official terminal proof", worker);
  const inventory = (source, entries) => {
    assert(Array.isArray(entries) && entries.length <= 64, "missing or excessive " + source);
    const ids = new Set();
    for (const b of entries) {
      if (ids.has(b.pid)) reject(source, "duplicate PID", b);
      ids.add(b.pid);
      if (!same(expected.get(b.pid), b)) reject(source, "unqualified birth", b);
      if (
        source === "ownedBefore" &&
        same(worker, b) &&
        terminalProof &&
        !["Z", "X", "x"].includes(b.state)
      )
        reject(source, "worker reported live after terminal proof", b);
    }
    return ids;
  };
  const known = inventory("known", stop.known);
  inventory("ownedBefore", stop.ownedBefore);
  const remaining = inventory("remaining", stop.remaining);
  if (!stop.absent || remaining.size) reject("remaining", "server cleanup incomplete");
  const spawn = registry.filter((r) => r.type === "spawn");
  if (spawn.length !== 1 || !same(chain[0], spawn[0]))
    reject("registry", "missing or conflicting root spawn");
  const stopped = registry.filter((r) => r.type === "stopped");
  if (stopped.length !== 1 || stopped[0].pid !== chain[0].pid || stopped[0].absent !== stop.absent)
    reject("registry", "missing or inconsistent stopped receipt");
  const registered = inventory(
    "registry",
    registry.filter((r) => r.type === "owned"),
  );
  for (const b of expected.values()) {
    if (!known.has(b.pid)) reject("known", "qualified birth missing from stop inventory", b);
    if (b.pid !== chain[0].pid && !registered.has(b.pid))
      reject("registry", "missing owned birth", b);
  }
  assert(
    Array.isArray(stop.endings) && stop.endings.length <= 64,
    "missing or excessive stop endings",
  );
  const ended = new Set();
  for (const e of stop.endings) {
    if (!known.has(e.pid) || ended.has(e.pid)) reject("endings", "unmatched or duplicate PID", e);
    ended.add(e.pid);
  }
  for (const pid of known) if (!ended.has(pid)) reject("endings", "missing final PID", { pid });
  return {
    ok: violations.length === 0,
    resourceAbsent: stop.absent,
    qualifiedBirths: [...expected.values()],
    violations,
  };
}
