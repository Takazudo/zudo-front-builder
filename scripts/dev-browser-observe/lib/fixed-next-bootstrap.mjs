// The Next 16.4.6 Jest child is a generic bootstrap. The loaded worker module
// is sent separately over IPC and cannot be identified from this role.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  closeSync,
  constants,
  fstatSync,
  openSync,
  readSync,
  readlinkSync,
  realpathSync,
} from "node:fs";
import { join } from "node:path";
import { identifyPeerRoles, peerArgv } from "./fixed-peer-roles.mjs";

export const NEXT_BOOTSTRAP_PROTOCOL = "linux-fixed-peer.v2";
const SCRIPT_SHA256 = "463a33ccdf45500486481c697ca9b7445fae5998410b82d3d5485c0b85b770eb";
const SCRIPT_BYTES = 2868;
const fields = ["pid", "ppid", "pgid", "startTicks", "bootId"];

function birth(row) {
  for (const field of ["pid", "ppid", "pgid"])
    assert(Number.isSafeInteger(row[field]) && row[field] > 0, "invalid peer " + field);
  assert(
    /^[1-9]\d*$/.test(row.startTicks) && typeof row.startTicks === "string",
    "invalid start ticks",
  );
  assert(/^[a-f\d]{8}(?:-[a-f\d]{4}){3}-[a-f\d]{12}$/i.test(row.bootId), "invalid boot ID");
  return Object.fromEntries(fields.map((field) => [field, row[field]]));
}
function live(row) {
  assert(
    ["R", "S", "D", "T", "t", "W", "K", "P", "I"].includes(row.state),
    "unknown or terminal peer",
  );
  return birth(row);
}

function pinBootstrap(appDir, expected) {
  assert.equal(expected, SCRIPT_SHA256, "wrong packaged bootstrap digest");
  const path = join(appDir, "node_modules/next/dist/compiled/jest-worker/processChild.js");
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    assert.equal(readlinkSync("/proc/self/fd/" + fd), path, "unexpected bootstrap backing path");
    const before = fstatSync(fd, { bigint: true });
    assert(
      before.isFile() && before.size === BigInt(SCRIPT_BYTES),
      "wrong bootstrap file type or size",
    );
    const bytes = Buffer.alloc(SCRIPT_BYTES);
    let offset = 0;
    while (offset < bytes.length) {
      const count = readSync(fd, bytes, offset, bytes.length - offset, offset);
      assert(count > 0, "truncated bootstrap file");
      offset += count;
    }
    const sha256 = createHash("sha256").update(bytes).digest("hex");
    assert.equal(sha256, expected, "wrong bootstrap bytes");
    const after = fstatSync(fd, { bigint: true });
    assert(
      after.size === before.size &&
        after.ctimeNs === before.ctimeNs &&
        after.mtimeNs === before.mtimeNs,
      "bootstrap changed during verification",
    );
    return {
      path,
      device: String(before.dev),
      inode: String(before.ino),
      size: bytes.length,
      sha256,
    };
  } finally {
    closeSync(fd);
  }
}

// Build only after the ordinary three-role readiness and live socket proof.
export function buildNextBootstrapContract({
  protocol,
  framework,
  appDir,
  image,
  chain,
  socket,
  bootstrapSha256,
}) {
  assert.equal(protocol, NEXT_BOOTSTRAP_PROTOCOL);
  assert.equal(framework, "next");
  assert.equal(realpathSync(appDir), appDir);
  assert.equal(chain.length, 3, "three ready roles required");
  const [root, cli, server] = chain;
  const port = socket.port;
  assert(Number.isInteger(port) && port > 0 && port <= 65535);
  assert.equal(socket.pid, server.pid);
  assert(/^[1-9]\d*$/.test(socket.inode));
  assert(
    (socket.family === "tcp" && socket.addr === "0100007F") ||
      (socket.family === "tcp6" && socket.addr === "0000000000000000FFFF00000100007F"),
    "invalid serving listener",
  );
  assert.equal(root.pid, root.pgid);
  assert.equal(cli.ppid, root.pid);
  assert.equal(server.ppid, cli.pid);
  assert.equal(new Set(chain.map((p) => p.pid)).size, 3);
  const roles = identifyPeerRoles(
    chain.map(({ role, ...rest }) => rest),
    { framework, rootPid: root.pid, appDir, node: image.path, port },
    true,
  );
  const ancestors = roles.map((row, i) => {
    assert.equal(row.pid, chain[i].pid, "unexpected role order");
    assert.equal(row.role, chain[i].role, "unexpected prior role");
    assert.equal(row.pgid, root.pgid);
    assert.equal(row.bootId, root.bootId);
    assert.equal(row.cwd, appDir);
    assert.equal(row.elfHeader, "7f454c46");
    assert.deepEqual(row.image, image);
    const key = birth(row);
    for (const bound of [row.before, row.after, row.finalBirth]) assert.deepEqual(live(bound), key);
    return Object.freeze({ ...key, role: row.role });
  });
  const script = pinBootstrap(appDir, bootstrapSha256);
  return Object.freeze({
    protocol,
    framework,
    app: appDir,
    image: Object.freeze({ ...image }),
    script: Object.freeze(script),
    ancestors: Object.freeze(ancestors),
    port,
    expectedJestId: null,
  });
}

function verifyView(view, contract) {
  assert.deepEqual(view.image, contract.image, "peer image mismatch");
  assert.equal(view.elf, "7f454c46");
  assert.equal(view.cwd, contract.app);
  assert.equal(view.jestId, null, "JEST_WORKER_ID must be observed absent");
  assert.equal(view.private, true);
  assert.equal(view.dev, true);
  for (const stamp of [view.startNs, view.endNs])
    assert(typeof stamp === "string" && /^\d+$/.test(stamp), "missing view timestamp");
  assert(BigInt(view.endNs) >= BigInt(view.startNs), "inverted view interval");
  return live(view.birth);
}

// Acquisition owns pidfd/procdir pairing and all I/O. This function validates
// the two independently acquired live views; it provides neither RSS nor exit proof.
export function verifyNextBootstrapPair(contract, observations) {
  assert.equal(contract.protocol, NEXT_BOOTSTRAP_PROTOCOL);
  assert.equal(contract.expectedJestId, null);
  assert.equal(contract.framework, "next");
  assert.equal(
    contract.script.path,
    join(contract.app, "node_modules/next/dist/compiled/jest-worker/processChild.js"),
  );
  assert.equal(contract.script.sha256, SCRIPT_SHA256);
  assert.equal(contract.script.size, SCRIPT_BYTES);
  assert.equal(observations.length, 2);
  const parent = contract.ancestors[2];
  let previous;
  for (const row of observations) {
    assert(
      Number.isSafeInteger(row.index) && row.index >= 0 && row.candidateCount === 1,
      "ambiguous peer sweep",
    );
    const { view, parentView } = row;
    const { role, ...parentBirth } = parent;
    assert.equal(role, "next-server");
    assert.deepEqual(verifyView(parentView, contract), parentBirth, "serving birth changed");
    assert(
      typeof parentView.argvHex === "string" && /^(?:[0-9a-f]{2})+$/.test(parentView.argvHex),
      "invalid serving argv bytes",
    );
    const argv = peerArgv(Buffer.from(parentView.argvHex, "hex").toString("utf8"), true);
    assert(
      (argv.length === 1 && argv[0] === "next-server (v16.4.6)") ||
        (argv.length === 2 &&
          argv[0] === contract.image.path &&
          argv[1] === join(contract.app, "node_modules/next/dist/server/lib/start-server.js")),
      "unexpected serving argv",
    );
    assert(BigInt(parentView.endNs) <= BigInt(view.startNs), "parent observed after child");
    const child = verifyView(view, contract);
    assert.equal(child.ppid, parent.pid);
    assert.equal(child.pgid, parent.pgid);
    assert.equal(child.bootId, parent.bootId);
    assert(
      !contract.ancestors.some((a) => a.pid === child.pid),
      "ancestor cannot be the bootstrap",
    );
    assert.equal(
      view.argvHex,
      Buffer.from(contract.image.path + "\0" + contract.script.path + "\0").toString("hex"),
      "unexpected bootstrap argv",
    );
    if (previous) {
      assert.equal(row.index, previous.index + 1, "nonconsecutive sweeps");
      assert(BigInt(parentView.startNs) > BigInt(previous.view.endNs), "overlapping observations");
      assert.deepEqual(child, birth(previous.view.birth), "candidate birth changed");
    }
    previous = row;
  }
  return { role: "next-jest-bootstrap", birth: birth(previous.view.birth) };
}
