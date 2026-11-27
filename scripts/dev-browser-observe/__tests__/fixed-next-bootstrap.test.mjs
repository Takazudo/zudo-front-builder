import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "vitest";
import { readSession } from "../lib/aggregate.mjs";
import { NEXT_BOOTSTRAP_PROTOCOL, verifyNextBootstrapPair } from "../lib/fixed-next-bootstrap.mjs";

const image = { path: "/pinned/node", device: "20", inode: "63" };
const bootId = "1edafe52-089e-4f16-a478-1143d29052bd";
const birth = (pid, ppid, ticks) => ({ pid, ppid, pgid: 100, startTicks: String(ticks), bootId });
const parent = birth(103, 102, 12),
  child = birth(106, 103, 52);
const app = "/work/apps/next";
const script = join(app, "node_modules/next/dist/compiled/jest-worker/processChild.js");
const argv = (...words) => Buffer.from(words.join("\0") + "\0").toString("hex");
const contract = {
  protocol: NEXT_BOOTSTRAP_PROTOCOL,
  framework: "next",
  app,
  image,
  script: {
    path: script,
    device: "20",
    inode: "41",
    size: 2868,
    sha256: "463a33ccdf45500486481c697ca9b7445fae5998410b82d3d5485c0b85b770eb",
  },
  ancestors: [
    { ...birth(100, 49, 10), role: "content-sidecar" },
    { ...birth(102, 100, 11), role: "next-cli" },
    { ...parent, role: "next-server" },
  ],
  port: 44000,
  expectedJestId: null,
};
function observations() {
  const view = (b, argvHex, t) => ({
    birth: { ...b, state: "R" },
    image: { ...image },
    elf: "7f454c46",
    cwd: app,
    private: true,
    dev: true,
    jestId: null,
    argvHex,
    startNs: String(t),
    endNs: String(t + 1),
  });
  return [0, 1].map((i) => ({
    index: 44 + i,
    candidateCount: 1,
    parentView: view(parent, argv("next-server (v16.4.6)"), 50_000_000 * i + 3),
    view: view(child, argv(image.path, script), 50_000_000 * i + 7),
  }));
}

test("a same-group live role requires two distinct, independently bracketed observations", () => {
  const result = verifyNextBootstrapPair(contract, observations());
  assert.deepEqual(result, { role: "next-jest-bootstrap", birth: child });
  assert(!("rssKiB" in result));
  assert(!("terminalProof" in result));
  assert(!("workerModule" in result));
});

test("unrecognized role, stale birth, argv, environment or measurement provenance refuses the pair", () => {
  const invalid = [
    (o) => o.pop(),
    (o) => o[1].index++,
    (o) => (o[1].candidateCount = 2),
    (o) => (o[1].view.birth.state = "Z"),
    (o) => o[1].view.birth.pid++,
    (o) => (o[1].view.birth.ppid = 100),
    (o) => (o[1].view.birth.pgid = o[1].view.birth.pid),
    (o) => (o[1].view.birth.startTicks = "53"),
    (o) => (o[1].view.birth.bootId = "foreign"),
    (o) => o[1].parentView.birth.pid++,
    (o) => (o[1].parentView.birth.startTicks = "9"),
    (o) => (o[1].parentView.birth.state = "Z"),
    (o) => (o[1].parentView.argvHex = argv("not-next")),
    (o) => (o[1].view.argvHex = argv(image.path, script, "")),
    (o) => (o[1].view.argvHex = argv(image.path, "/other/processChild.js")),
    (o) =>
      (o[1].view.argvHex = argv(
        image.path,
        join(app, "node_modules/next/dist/server/dev/static-paths-worker.js"),
      )),
    (o) => (o[1].view.cwd = "/foreign"),
    (o) => (o[1].view.image.inode = "wrong"),
    (o) => (o[1].view.elf = "0000"),
    (o) => (o[1].view.jestId = "1"),
    (o) => delete o[1].view.jestId,
    (o) => (o[1].parentView.jestId = "8"),
    (o) => (o[1].view.private = false),
    (o) => (o[1].parentView.dev = false),
    (o) => (o[1].parentView.endNs = o[1].view.endNs),
    (o) => (o[1].parentView.startNs = o[0].view.startNs),
  ];
  for (const change of invalid) {
    const rows = observations();
    change(rows);
    assert.throws(() => verifyNextBootstrapPair(contract, rows), String(change));
  }
});

test("a Next bootstrap diagnostic remains excluded from pooled timing", () => {
  const dir = mkdtempSync(join(tmpdir(), "next-role-aggregation-"));
  try {
    writeFileSync(
      join(dir, "receipt.json"),
      JSON.stringify({ fixedRssSafety: NEXT_BOOTSTRAP_PROTOCOL }),
    );
    const result = readSession({ dir, label: "next-bootstrap" });
    assert.equal(result.timingExcluded, true);
    assert.deepEqual(result.blocks, []);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
