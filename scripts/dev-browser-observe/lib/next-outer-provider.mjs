// Stock v4 admission binds the independently running Python role before any
// server/browser or FFIs start. All original file operations remain owned
// through settlement underneath the normal independent supervisor.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  constants,
  closeSync,
  openSync,
  readFileSync,
  readSync,
  readlinkSync,
  opendirSync,
} from "node:fs";
import { open, readlink } from "node:fs/promises";
import { linuxBirth, sameLinuxBirth, parseLinuxStat } from "./linux-birth.mjs";

export async function verifyNextOuterProvider({
  profile,
  decl,
  profilePath,
  label,
  deadlineNs,
  signal,
}) {
  const cutoff = [deadlineNs, process.hrtime.bigint() + 2_000_000_000n].reduce((a, b) =>
    a < b ? a : b,
  );
  const check = () => {
    signal?.throwIfAborted();
    assert(process.hrtime.bigint() < cutoff, "outer provider verification expired");
  };
  const outer = decl.outer;
  const verifyBirth = () => {
    const parent = linuxBirth(process.ppid),
      current = linuxBirth(outer.pid);
    assert(
      parent?.ppid === outer.pid &&
        sameLinuxBirth(current, outer) &&
        current.ppid === outer.ppid &&
        current.pgid === outer.pgid,
      "outer ancestor changed",
    );
  };
  check();
  verifyBirth();
  const root = "/proc/" + outer.pid;
  assert.equal(await readlink(root + "/cwd"), profile.cwd);
  assert.equal(await readlink(root + "/exe"), profile.pythonPath);
  const expected = Buffer.from(
    [profile.pythonPath, "-IB", profile.scriptPath, profilePath, label, ""].join("\0"),
  );
  const cmd = await open(root + "/cmdline", "r");
  try {
    const buf = Buffer.alloc(16385);
    const { bytesRead } = await cmd.read(buf, 0, buf.length, 0);
    assert(
      bytesRead < buf.length && buf.subarray(0, bytesRead).equals(expected),
      "outer argv mismatch",
    );
  } finally {
    await cmd.close();
  }
  const pins = [];
  for (const [path, sha256, cap] of [
    [profile.pythonPath, profile.pythonSha256, 16 * 1024 * 1024],
    [profile.scriptPath, profile.scriptSha256, 128 * 1024],
  ]) {
    check();
    const f = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
    try {
      const before = await f.stat({ bigint: true });
      assert(before.isFile() && before.size > 0n && before.size <= BigInt(cap));
      const block = Buffer.alloc(64 * 1024),
        digest = createHash("sha256");
      for (let n = 0; n < Number(before.size);) {
        check();
        const want = Math.min(block.length, Number(before.size) - n);
        const { bytesRead } = await f.read(block, 0, want, n);
        check();
        assert.equal(bytesRead, want);
        digest.update(block.subarray(0, want));
        n += want;
      }
      assert.equal(digest.digest("hex"), sha256);
      const after = await f.stat({ bigint: true });
      assert(
        before.size === after.size &&
          before.ctimeNs === after.ctimeNs &&
          before.mtimeNs === after.mtimeNs,
      );
      if (path === profile.pythonPath) {
        const exe = await open(root + "/exe", "r");
        try {
          const st = await exe.stat({ bigint: true });
          assert(st.dev === before.dev && st.ino === before.ino);
        } finally {
          await exe.close();
        }
      }
      pins.push({ path, sha256, device: String(before.dev), inode: String(before.ino) });
    } finally {
      await f.close();
    }
  }
  verifyBirth();
  check();
  return { outer, pins, verifiedNs: String(process.hrtime.bigint()) };
}

export function nextOuterChildren(pid, counts = { tasks: 0, bytes: 0 }) {
  const dir = opendirSync("/proc/" + pid + "/task"),
    ids = new Set();
  try {
    for (let ent; (ent = dir.readSync());) {
      assert(++counts.tasks <= 512, "outer graph thread cap");
      const fd = openSync(
        "/proc/" + pid + "/task/" + ent.name + "/children",
        constants.O_RDONLY | constants.O_NONBLOCK,
      );
      try {
        const buf = Buffer.alloc(4097);
        const n = readSync(fd, buf, 0, buf.length, null);
        counts.bytes += n;
        assert(n < buf.length && counts.bytes <= 32768, "outer graph child input cap");
        const raw = buf.toString("ascii", 0, n).trim();
        for (const s of raw ? raw.split(/\s+/) : []) {
          assert(/^[1-9]\d{0,9}$/.test(s));
          ids.add(Number(s));
          assert(ids.size <= 64, "outer graph child cap");
        }
      } finally {
        closeSync(fd);
      }
    }
  } finally {
    dir.closeSync();
  }
  return [...ids].sort((a, b) => a - b);
}

// Refusal-time identity of one unexpected direct child, read once on the refusal
// path. It explains a refusal and never admits the child.
export function describeGraphChild(pid, now = process.hrtime.bigint) {
  const observedNs = String(now());
  const birth = linuxBirth(pid);
  const read = (f) => {
    try {
      return f();
    } catch (error) {
      return { error: String(error.code ?? error) };
    }
  };
  return {
    pid,
    observedNs,
    birth: birth && !birth.birthUnverifiable ? birth : null,
    birthError: !birth ? "absent" : birth.birthUnverifiable ? String(birth.error) : null,
    exe: read(() => readlinkSync("/proc/" + pid + "/exe")),
    argv: read(() =>
      readFileSync("/proc/" + pid + "/cmdline")
        .subarray(0, 512)
        .toString("utf8")
        .split("\0")
        .filter(Boolean),
    ),
  };
}

export function exactChildren(prior, expected, counts, describe = describeGraphChild) {
  const actual = nextOuterChildren(prior.pid, counts);
  const want = expected.map((p) => p.pid).sort((a, b) => a - b);
  try {
    assert.deepEqual(actual, want, "unexpected outer graph child");
  } catch (error) {
    error.graphRefusal = {
      parent: { pid: prior.pid, startTicks: prior.startTicks, bootId: prior.bootId },
      expected: want,
      actual,
      extra: actual.filter((pid) => !want.includes(pid)).map((pid) => describe(pid)),
      missing: want.filter((pid) => !actual.includes(pid)),
    };
    throw error;
  }
}

// The benchmark lock and Chromium are ordinary controller children in this
// WSL launch. A new child cannot become an accepted worker by sharing its name.
export function verifyNextOuterWiring({
  outer,
  supervisor,
  controller,
  lock,
  server,
  browser,
  holder,
  browserPath,
}) {
  const checks = [];
  const counts = { tasks: 0, bytes: 0 };
  const see = (prior) => {
    const live = linuxBirth(prior.pid);
    assert(
      sameLinuxBirth(live, prior) &&
        live.ppid === prior.ppid &&
        live.pgid === prior.pgid &&
        !["Z", "X", "x"].includes(live.state),
      "outer graph birth changed",
    );
    return live;
  };
  const exact = (prior, expected) => {
    see(prior);
    exactChildren(prior, expected, counts);
    checks.push(prior.pid);
  };
  exact(outer, [supervisor]);
  exact(supervisor, [controller]);
  exact(controller, [lock, server, browser, holder]);
  exact(lock, []);
  exact(holder, []);
  const pending = [browser],
    browserBirths = [],
    known = new Set();
  while (pending.length) {
    const next = see(pending.pop());
    assert(!known.has(next.pid) && known.size < 64, "outer browser population cap");
    known.add(next.pid);
    assert.equal(readlinkSync("/proc/" + next.pid + "/exe"), browserPath);
    for (const id of nextOuterChildren(next.pid, counts)) {
      const child = linuxBirth(id);
      assert(child && !child.birthUnverifiable && child.ppid === next.pid);
      pending.push(child);
    }
    see(next);
    browserBirths.push(next);
  }
  see(outer);
  see(controller);
  return { checked: checks, browserBirths, ...counts };
}

// No child process is dispatched from the real write gate. Include all members
// of the product process group, including a reparented or unfamiliar birth.
export function readNextOuterProductGraph(root) {
  const entries = opendirSync("/proc");
  const table = new Map();
  let count = 0,
    bytes = 0;
  const buffer = Buffer.alloc(4097);
  try {
    for (let ent; (ent = entries.readSync());) {
      if (!/^[1-9]\d*$/.test(ent.name)) continue;
      assert(++count <= 4096, "outer process census cap");
      let fd;
      try {
        fd = openSync("/proc/" + ent.name + "/stat", constants.O_RDONLY | constants.O_NONBLOCK);
        const n = readSync(fd, buffer, 0, buffer.length, null);
        bytes += n;
        assert(n > 0 && n < buffer.length && bytes <= 8 * 1024 * 1024, "outer stat input cap");
        const birth = parseLinuxStat(buffer.toString("utf8", 0, n), Number(ent.name));
        table.set(birth.pid, birth);
      } catch (error) {
        if (error.code !== "ENOENT" && error.code !== "ESRCH") throw error;
      } finally {
        if (fd !== undefined) closeSync(fd);
      }
    }
  } finally {
    entries.closeSync();
  }
  const ids = new Set([root.pid]);
  // Add descendants even if they left the group. Both unexpected descendants
  // and unattached group members must surface in the normal role verifier.
  for (const b of table.values()) if (b.pgid === root.pgid) ids.add(b.pid);
  for (let prev = -1; prev !== ids.size;) {
    assert(ids.size <= 64, "outer product population cap");
    prev = ids.size;
    for (const b of table.values()) if (ids.has(b.ppid)) ids.add(b.pid);
  }
  const result = [];
  for (const pid of ids) {
    const original = table.get(pid),
      current = linuxBirth(pid);
    assert(
      original &&
        current &&
        current.bootId === root.bootId &&
        original.ppid === current.ppid &&
        original.pgid === current.pgid &&
        original.startTicks === current.startTicks,
      "outer product census changed",
    );
    result.push(current);
  }
  assert(sameLinuxBirth(result[0], root), "outer product root changed");
  return result;
}
