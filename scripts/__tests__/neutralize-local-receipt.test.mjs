import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vite-plus/test";
import * as evidence from "../island-size-local-evidence.mjs";
import { main, neutralizeReceipt, readMap } from "../neutralize-local-receipt.mjs";

const source = "b".repeat(40);
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const roots = [];
afterEach(() => roots.splice(0).forEach((root) => rmSync(root, { recursive: true, force: true })));

function fixture() {
  const root = mkdtempSync(join(realpathSync(tmpdir()), "zfb-neutral-receipt-"));
  roots.push(root);
  const bundle = join(root, "bundle");
  mkdirSync(bundle);
  writeFileSync(join(bundle, "evidence"), "retained bytes");
  const receipt = {
    schemaVersion: 1,
    kind: "local-native-darwin",
    operator: "Example Operator (synthetic Mac)",
    collectedAt: "2026-01-01T00:00:00.000Z",
    source: { commit: source, clean: true },
    platform: { os: "darwin", arch: "arm64" },
    commands: [
      ["/Users/example/tools/heavy-guard.sh", "--max-run", "14400", "--", "cargo", "build"],
      ["/Users/example/node/bin/node", "/Users/example/work/src/scripts/measure-real.mjs"],
      ["pnpm", "install", "--frozen-lockfile"],
    ],
    bundle: {
      location: "/Users/example/work/retain/bundle",
      sha256: evidence.localBundleDigest(bundle),
    },
  };
  const bytes = Buffer.from(JSON.stringify(receipt, null, 2) + "\n");
  const map = {
    prefixes: [
      { from: "/Users/example/work/src", to: "<SOURCE>" },
      { from: "/Users/example/work/retain", to: "<RETAIN>" },
      { from: "/Users/example/tools/heavy-guard.sh", to: "<HEAVY_GUARD>" },
      { from: "/Users/example/node/bin/node", to: "<ZFB_NODE>" },
    ],
    operator: "zfb owner, local Darwin arm64 collection",
    forbidden: ["example"],
  };
  return { root, bundle, receipt, bytes, map };
}

const run = (f, overrides = {}) =>
  neutralizeReceipt({
    originalBytes: f.bytes,
    expectedSha: sha(f.bytes),
    sourceSha: source,
    bundleDigest: f.receipt.bundle.sha256,
    map: f.map,
    evidence,
    identityFragments: [],
    ...overrides,
  });

describe("neutralize-local-receipt", () => {
  it("replaces only listed prefixes and the operator, keeping every attested fact", () => {
    const f = fixture();
    const { bytes, report } = run(f);
    const neutral = JSON.parse(bytes.toString("utf8"));
    expect(neutral.commands).toEqual([
      ["<HEAVY_GUARD>", "--max-run", "14400", "--", "cargo", "build"],
      ["<ZFB_NODE>", "<SOURCE>/scripts/measure-real.mjs"],
      ["pnpm", "install", "--frozen-lockfile"],
    ]);
    expect(neutral.bundle).toEqual({
      location: "<RETAIN>/bundle",
      sha256: f.receipt.bundle.sha256,
    });
    expect(neutral.operator).toBe("zfb owner, local Darwin arm64 collection");
    for (const key of ["schemaVersion", "kind", "collectedAt", "source", "platform"])
      expect(neutral[key]).toEqual(f.receipt[key]);
    expect(report).toEqual({
      originalSha256: sha(f.bytes),
      neutralSha256: sha(bytes),
      bundleSha256: f.receipt.bundle.sha256,
      sourceSha: source,
      placeholders: { "<HEAVY_GUARD>": 1, "<RETAIN>": 1, "<SOURCE>": 1, "<ZFB_NODE>": 1 },
      changedCommandArgs: 3,
      bundleLocationReplaced: true,
      operatorReplaced: true,
    });
    expect(report.neutralSha256).not.toBe(report.originalSha256);
    expect(JSON.stringify(report)).not.toContain("example");
  });

  it("keeps the operator when the map does not replace it", () => {
    const f = fixture();
    const { operator, ...map } = f.map;
    const { bytes, report } = run(f, { map: { ...map, forbidden: [] } });
    expect(JSON.parse(bytes.toString("utf8")).operator).toBe(f.receipt.operator);
    expect(report.operatorReplaced).toBe(false);
  });

  it("refuses a wrong original digest, a moved bundle, a leftover path or a non-placeholder", () => {
    const f = fixture();
    expect(() => run(f, { expectedSha: "0".repeat(64) })).toThrow(
      "original receipt digest mismatch",
    );
    expect(() => run(f, { bundleDigest: "1".repeat(64) })).toThrow(
      "retained bundle digest mismatch",
    );
    expect(() => run(f, { map: { ...f.map, prefixes: f.map.prefixes.slice(1) } })).toThrow(
      "forbidden fragment(s) remain",
    );
    expect(() => run(f, { identityFragments: ["Darwin arm64"] })).toThrow(
      "forbidden fragment(s) remain",
    );
    expect(() => readMap({ prefixes: [{ from: "/Users/x", to: "SOURCE" }] })).toThrow(
      "<PLACEHOLDER>",
    );
    expect(() => readMap({ prefixes: [{ from: "/x", to: "<X>" }], extra: 1 })).toThrow(
      "unknown map key extra",
    );
  });

  it("writes once through the CLI, never overwrites, and leaves the original untouched", async () => {
    const f = fixture();
    const receiptPath = join(f.root, "receipt.json");
    const mapPath = join(f.root, "map.json");
    const out = join(f.root, "receipt-neutral.json");
    writeFileSync(receiptPath, f.bytes);
    writeFileSync(mapPath, JSON.stringify(f.map));
    const args = [
      "--tools",
      resolve(import.meta.dirname, "../.."),
      "--receipt",
      receiptPath,
      "--expected-sha",
      sha(f.bytes),
      "--source-sha",
      source,
      "--bundle",
      f.bundle,
      "--map",
      mapPath,
      "--out",
      out,
    ];
    const log = console.log;
    const lines = [];
    console.log = (line) => lines.push(line);
    try {
      await main(args);
    } finally {
      console.log = log;
    }
    expect(sha(readFileSync(receiptPath))).toBe(sha(f.bytes));
    expect(JSON.parse(lines.join("\n")).neutralSha256).toBe(sha(readFileSync(out)));
    await expect(main(args)).rejects.toThrow("never overwrite");
  });
});
