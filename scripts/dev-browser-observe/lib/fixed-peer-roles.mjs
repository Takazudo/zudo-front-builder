import assert from "node:assert/strict";
import { openSync, closeSync, readSync, readlinkSync, statSync, opendirSync } from "node:fs";
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

export function identifyPeerRoles(rows, { framework, rootPid, appDir, node, port }, ready) {
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
  const sameBirth = (a, b) => sameLinuxBirth(a, b) && a.ppid === b.ppid && a.pgid === b.pgid;
  const live = sample.processes;
  const rows = live.map((p) => {
    assert.equal(p.disposition, "live", "departed or unresolved role before closure");
    assert.deepEqual(p.executable, cfg.image);
    const before = linuxBirth(p.pid);
    assert(sameBirth(p, before));
    const base = "/proc/" + p.pid + "/",
      argv = boundedText(base + "cmdline", 65536).split("\0");
    while (argv.at(-1) === "") argv.pop();
    const cwd = readlinkSync(base + "cwd");
    assert.equal(cwd, cfg.appDir);
    const exe = statSync(base + "exe", { bigint: true });
    assert.equal(readlinkSync(base + "exe"), cfg.image.path);
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
      privateWorker,
      devServer,
    };
  });
  const chain = identifyPeerRoles(rows, cfg, ready);
  let socket = null;
  if (ready) {
    const serving = chain.at(-1);
    socket = ownsListener(serving.pid, cfg.port);
    assert(sameBirth(serving, linuxBirth(serving.pid)));
  }
  return { chain, socket };
}
