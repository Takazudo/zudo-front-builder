import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  openSync,
  closeSync,
  fstatSync,
  readSync,
  readlinkSync,
  statSync,
  opendirSync,
} from "node:fs";
import { join } from "node:path";
import { isDeepStrictEqual } from "node:util";
import { linuxBirth, sameLinuxBirth } from "./linux-birth.mjs";

export const FIXED_PEER_PROTOCOL = "linux-fixed-peer.v1";

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

export function peerArgv(raw, nextServer) {
  assert(raw.endsWith("\0"), "unterminated peer argv");
  if (nextServer && /^next-server \(v16\.4\.6\)\x00+$/.test(raw)) return ["next-server (v16.4.6)"];
  return raw.slice(0, -1).split("\0");
}

export function identifyPeerRoles(rows, cfg, ready) {
  try {
    return classifyPeerRoles(rows, cfg, ready);
  } catch (error) {
    // The sampler already owns these births. Preserve observations separately
    // from accepted roles; in particular, a Node image alone is ambiguous.
    let remaining = 64 * 1024;
    const observed = rows.map((row) => {
      const {
        pid,
        ppid,
        pgid,
        startTicks,
        bootId,
        birthVersion,
        before,
        after,
        cwd,
        image,
        elfHeader,
        privateWorker,
        devServer,
      } = row;
      const argvBytes = Buffer.from(JSON.stringify(row.argv));
      const include = argvBytes.length <= remaining;
      if (include) remaining -= argvBytes.length;
      return {
        pid,
        ppid,
        pgid,
        startTicks,
        bootId,
        birthVersion,
        before,
        after,
        cwd,
        image,
        elfHeader,
        privateWorker,
        devServer,
        ...(include
          ? { argv: row.argv }
          : {
              argvOmitted: true,
              argvJsonBytes: argvBytes.length,
              argvJsonSha256: createHash("sha256").update(argvBytes).digest("hex"),
            }),
      };
    });
    error.details = {
      stage: "peer-role-classification",
      roleQualified: false,
      framework: cfg.framework,
      rootPid: cfg.rootPid,
      port: cfg.port,
      ready,
      observed,
    };
    throw error;
  }
}

function classifyPeerRoles(rows, { framework, rootPid, appDir, node, port }, ready) {
  const root = rows.find((r) => r.pid === rootPid);
  assert(root, "root missing");
  const prefix =
    framework === "astro"
      ? [
          join(appDir, "node_modules/astro/bin/astro.mjs"),
          "dev",
          "--ignore-lock",
          "--host",
          "127.0.0.1",
          "--port",
        ]
      : [join(appDir, "scripts/dev-content.mjs"), "--hostname", "127.0.0.1", "--port"];
  assert(isDeepStrictEqual(root.argv, [node, ...prefix, String(port)]), "unexpected root argv");
  const chain = [{ role: framework === "astro" ? "astro-cli" : "content-sidecar", ...root }];
  if (framework === "next") {
    const kids = rows.filter((r) => r.ppid === root.pid);
    assert(kids.length <= 1, "ambiguous sidecar child");
    if (kids.length) {
      const cli = kids[0];
      assert(
        isDeepStrictEqual(cli.argv, [
          node,
          join(appDir, "node_modules/next/dist/bin/next"),
          "dev",
          "--hostname",
          "127.0.0.1",
          "--port",
          String(port),
        ]),
        "next cli argv",
      );
      chain.push({ role: "next-cli", ...cli });
      const servers = rows.filter((r) => r.ppid === cli.pid);
      assert(servers.length <= 1, "ambiguous server");
      if (servers.length) {
        const s = servers[0],
          entry = join(appDir, "node_modules/next/dist/server/lib/start-server.js");
        const original = isDeepStrictEqual(s.argv, [node, entry]),
          retitled = isDeepStrictEqual(s.argv, ["next-server (v16.4.6)"]);
        assert(
          (original || retitled) && s.privateWorker && s.devServer,
          "unexpected fork identity",
        );
        chain.push({ role: "next-server", retitled, ...s });
      }
    }
  }
  assert.equal(rows.length, chain.length, "unknown role or parent");
  if (ready) assert.equal(chain.length, framework === "astro" ? 1 : 3, "missing ready role");
  return chain;
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
  for (const p of sample.processes) {
    assert.equal(p.disposition, "live", "departed or unresolved role before closure");
    assert.deepEqual(p.executable, cfg.image);
  }
  return readPeerRoles(sample.processes, cfg, ready);
}

// Fresh proc reads for both periodic and independently requested identity checks.
export function readPeerExecutable(path) {
  const fd = openSync(path, "r");
  try {
    const bytes = Buffer.alloc(4),
      info = fstatSync(fd, { bigint: true });
    assert.equal(readSync(fd, bytes, 0, 4, 0), 4, "unreadable peer ELF header");
    const header = bytes.toString("hex");
    assert(info.isFile() && header === "7f454c46", "invalid peer executable");
    return { info, imagePath: readlinkSync("/proc/self/fd/" + fd), elfHeader: header };
  } finally {
    closeSync(fd);
  }
}

export function readPeerRoles(births, cfg, ready, verifyFile = false) {
  const sameBirth = (a, b) =>
    sameLinuxBirth(a, b) &&
    !["Z", "X", "x"].includes(b.state) &&
    a.ppid === b.ppid &&
    a.pgid === b.pgid;
  const rows = births.map((p) => {
    const before = linuxBirth(p.pid);
    assert(sameBirth(p, before));
    const base = "/proc/" + p.pid + "/",
      argv = peerArgv(
        boundedText(base + "cmdline", 65536),
        cfg.framework === "next" && p.ppid !== cfg.rootPid && p.pid !== cfg.rootPid,
      );
    const cwd = readlinkSync(base + "cwd");
    assert.equal(cwd, cfg.appDir);
    const {
      info: exe,
      imagePath,
      elfHeader,
    } = verifyFile
      ? readPeerExecutable(base + "exe")
      : { info: statSync(base + "exe", { bigint: true }), imagePath: readlinkSync(base + "exe") };
    assert.equal(imagePath, cfg.image.path);
    assert.equal(String(exe.dev), cfg.image.device);
    assert.equal(String(exe.ino), cfg.image.inode);
    let privateWorker = false,
      devServer = false;
    if (cfg.framework === "next" && p.ppid !== cfg.rootPid && p.pid !== cfg.rootPid) {
      const flags = boundedText(base + "environ", 131072).split("\0");
      privateWorker = flags.includes("NEXT_PRIVATE_WORKER=1");
      devServer = flags.includes("__NEXT_DEV_SERVER=1");
    }
    const after = linuxBirth(p.pid);
    assert(sameBirth(p, after));
    return {
      cwd,
      image: { path: imagePath, device: String(exe.dev), inode: String(exe.ino) },
      ...(elfHeader && { elfHeader }),
      before,
      after,
      birthVersion: p.birthVersion,
      pid: p.pid,
      ppid: p.ppid,
      pgid: p.pgid,
      startTicks: p.startTicks,
      bootId: p.bootId,
      argv,
      privateWorker,
      devServer,
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

export function peerProcessIdentity(owned, known, qualified, cfg, read = readPeerRoles) {
  assert(qualified?.chain.length, "no qualified peer identity");
  const startNs = String(process.hrtime.bigint());
  const actual = read(owned, cfg, true, true);
  assert.equal(
    new Set(actual.chain.map((row) => row.pid)).size,
    actual.chain.length,
    "duplicate peer process",
  );
  for (const row of actual.chain) {
    assert.equal(row.elfHeader, "7f454c46", "fresh peer ELF evidence missing");
    assert.deepEqual(row.image, cfg.image, "fresh peer image mismatch");
  }
  for (const collection of [known, owned, qualified.chain]) {
    assert.equal(
      new Set(collection.map((row) => row.pid)).size,
      collection.length,
      "duplicate peer process",
    );
    assert.equal(collection.length, actual.chain.length, "peer process set changed");
    for (const prior of collection) {
      const row = actual.chain.find((p) => p.pid === prior.pid);
      assert(
        row &&
          sameLinuxBirth(prior, row) &&
          prior.ppid === row.ppid &&
          prior.pgid === row.pgid &&
          (!prior.role || prior.role === row.role),
        "peer role or birth changed",
      );
    }
  }
  return actual.chain.map((row) => ({
    name: "process-executable:" + row.pid + ":" + row.role,
    pid: row.pid,
    role: row.role,
    method: "linux-proc-birth-role-executable",
    ok: true,
    expected: cfg.image,
    actual: row.image,
    evidence: row,
    socket: row.pid === actual.socket.pid ? actual.socket : null,
    startNs,
    endNs: String(process.hrtime.bigint()),
  }));
}
