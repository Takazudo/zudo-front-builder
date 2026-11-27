import assert from "node:assert/strict";
import { openSync, closeSync, readSync, readlinkSync, fstatSync, opendirSync } from "node:fs";
import { join } from "node:path";
import { linuxBirth, sameLinuxBirth } from "./linux-birth.mjs";

export const FIXED_ASTRO_CSS_PROTOCOL = "linux-fixed-astro-css.v1";

// Each live member runs the same pinned Node image. Its actual argv, stable
// birth and parent, and the serving role's loopback socket distinguish roles.

function boundedText(path, cap) {
  const fd = openSync(path, "r"),
    buf = Buffer.alloc(cap + 1);
  try {
    let n = 0;
    while (n < buf.length) {
      const size = readSync(fd, buf, n, buf.length - n, null);
      if (!size) break;
      n += size;
    }
    assert(n <= cap, "proc field exceeds its cap");
    return buf.toString("utf8", 0, n);
  } finally {
    closeSync(fd);
  }
}

export function identifyPeerRoles(rows, { framework, rootPid, appDir, node, port }) {
  assert.equal(framework, "astro");
  assert.equal(rows.length, 1, "unknown Astro process");
  const [root] = rows;
  assert.equal(root.pid, rootPid);
  assert.deepEqual(root.argv, [
    node,
    join(appDir, "node_modules/astro/bin/astro.mjs"),
    "dev",
    "--ignore-lock",
    "--host",
    "127.0.0.1",
    "--port",
    String(port),
  ]);
  return [{ ...root, role: "astro-cli" }];
}
function listener(port) {
  const matching = [];
  for (const family of ["tcp", "tcp6"]) {
    const data = boundedText("/proc/net/" + family, 2 << 20);
    for (const line of data.trim().split("\n").slice(1)) {
      const cells = line.trim().split(/\s+/),
        [addr, p] = cells[1].split(":");
      if (cells[3] === "0A" && parseInt(p, 16) === port)
        matching.push({ family, addr, inode: cells[9] });
    }
  }
  return matching;
}
function ownsListener(pid, port) {
  const matches = listener(port);
  assert.equal(matches.length, 1, "requested port must have one listener");
  const m = matches[0];
  assert(
    ["0100007F", "0000000000000000FFFF00000100007F"].includes(m.addr),
    "non-loopback listener",
  );
  const fds = opendirSync("/proc/" + pid + "/fd");
  let count = 0,
    owned = null;
  try {
    for (let entry; (entry = fds.readSync());) {
      assert(++count <= 4096, "fd cap");
      try {
        if (readlinkSync("/proc/" + pid + "/fd/" + entry.name) === "socket:[" + m.inode + "]")
          owned = { ...m, pid, fd: entry.name, port };
      } catch (e) {
        if (e.code !== "ENOENT") throw e;
      }
    }
  } finally {
    fds.closeSync();
  }
  if (owned) return owned;
  throw Error("expected serving role does not own printed port");
}
export function samplePeerRoles(sample, cfg, ready) {
  const sameBirth = (a, b) =>
    sameLinuxBirth(a, b) &&
    !["Z", "X", "x"].includes(b.state) &&
    a.ppid === b.ppid &&
    a.pgid === b.pgid;
  const live = sample.processes;
  const rows = live.map((p) => {
    assert.equal(p.disposition, "live", "departed or unresolved role before closure");
    assert.deepEqual(p.executable, cfg.image);
    const before = linuxBirth(p.pid);
    assert(sameBirth(p, before));
    const base = "/proc/" + p.pid + "/",
      cmdline = boundedText(base + "cmdline", 65536);
    assert(cmdline.endsWith("\0"), "unterminated Astro argument vector");
    const argv = cmdline.slice(0, -1).split("\0");
    const cwd = readlinkSync(base + "cwd");
    assert.equal(cwd, cfg.appDir);
    const fd = openSync(base + "exe", "r");
    let exe;
    try {
      exe = fstatSync(fd, { bigint: true });
      const header = Buffer.alloc(4);
      assert.equal(readSync(fd, header, 0, 4, 0), 4);
      assert.equal(header.toString("hex"), "7f454c46");
    } finally {
      closeSync(fd);
    }
    assert.equal(readlinkSync(base + "exe"), cfg.image.path);
    assert.equal(String(exe.dev), cfg.image.device);
    assert.equal(String(exe.ino), cfg.image.inode);
    const after = linuxBirth(p.pid);
    assert(sameBirth(p, after));
    return {
      cwd,
      image: { path: cfg.image.path, device: String(exe.dev), inode: String(exe.ino) },
      before,
      after,
      birthVersion: p.birthVersion,
      pid: p.pid,
      ppid: p.ppid,
      pgid: p.pgid,
      startTicks: p.startTicks,
      bootId: p.bootId,
      argv,
    };
  });
  const chain = identifyPeerRoles(rows, cfg, ready);
  let socket = null;
  if (ready) {
    const serving = chain.at(-1);
    socket = ownsListener(serving.pid, cfg.port);
  }
  for (const row of chain) {
    row.finalBirth = linuxBirth(row.pid);
    assert(sameBirth(row, row.finalBirth), "peer role changed during ownership checks");
  }
  return { chain, socket };
}

export function verifyAstroPopulation(root, stop) {
  const exact = (p) => sameLinuxBirth(root, p) && root.ppid === p.ppid && root.pgid === p.pgid;
  return Boolean(
    root &&
    stop.rootAlive &&
    stop.absent &&
    !stop.error &&
    stop.remaining?.length === 0 &&
    [stop.known, stop.ownedBefore].every((rows) => rows?.length === 1 && exact(rows[0])) &&
    stop.endings?.length === 1 &&
    stop.endings[0].pid === root.pid &&
    stop.steps?.some(
      (step) =>
        step.delivery === "sent" &&
        step.target === "group:" + root.pgid &&
        step.signal === stop.endings[0].endedAfter,
    ) &&
    stop.exit &&
    JSON.stringify(stop.endings[0].exit) === JSON.stringify(stop.exit),
  );
}
