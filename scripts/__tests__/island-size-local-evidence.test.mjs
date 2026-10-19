import { realpathSync } from "node:fs";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, symlinkSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vite-plus/test";
import {
  localBundleDigest,
  readLocalEvidence,
  validateLocalReceipt,
  verifyLocalStageEvidence,
} from "../island-size-local-evidence.mjs";
import { parseArgs } from "../rebaseline-island-size-darwin.mjs";
import { parseArgs as linuxArgs } from "../rebaseline-island-size-linux.mjs";
const source = "a".repeat(40);
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const roots = [];
afterEach(() => roots.splice(0).forEach((root) => rmSync(root, { recursive: true, force: true })));
function fixture() {
  const root = mkdtempSync(join(realpathSync(tmpdir()), "zfb-local-evidence-"));
  roots.push(root);
  const bundle = join(root, "bundle");
  mkdirSync(bundle);
  writeFileSync(join(bundle, "evidence"), "retained bytes");
  const receipt = {
    schemaVersion: 1,
    kind: "local-native-darwin",
    operator: "owner",
    collectedAt: "2026-01-01T00:00:00.000Z",
    source: { commit: source, clean: true },
    platform: { os: "darwin", arch: "arm64" },
    commands: [["node", "collect.mjs"]],
    bundle: { location: "owner archive / bundle", sha256: localBundleDigest(bundle) },
  };
  const path = join(root, "receipt.json");
  writeFileSync(path, JSON.stringify(receipt));
  return { root, bundle, receipt, path, digest: sha(readFileSync(path)) };
}
describe("local native receipt boundary", () => {
  it("accepts exact retained bytes without interpreting recorded commands", () => {
    const f = fixture();
    expect(readLocalEvidence(f.path, f.bundle, f.digest, source).receipt).toEqual(f.receipt);
  });
  it.each([
    ["operator", ""],
    ["collectedAt", "2999-01-01T00:00:00.000Z"],
    ["collectedAt", "2026-02-30T00:00:00.000Z"],
    ["source", { commit: source, clean: false }],
    ["source", { commit: "b".repeat(40), clean: true }],
    ["platform", { os: "linux", arch: "arm64" }],
    ["commands", []],
    ["commands", [["node\nunsafe"]]],
    ["runId", 123],
    ["bundle", { location: "", sha256: "a".repeat(64) }],
  ])("rejects invalid %s", (key, value) => {
    const { receipt } = fixture();
    receipt[key] = value;
    expect(() => validateLocalReceipt(receipt, source)).toThrow(/local Darwin evidence/);
  });
  it("binds filenames as well as bytes and rejects changed retained bytes", () => {
    const f = fixture();
    writeFileSync(join(f.bundle, "extra"), "retained bytes");
    expect(localBundleDigest(f.bundle)).not.toBe(f.receipt.bundle.sha256);
    expect(() => readLocalEvidence(f.path, f.bundle, f.digest, source)).toThrow(
      "bundle digest mismatch",
    );
    writeFileSync(f.path, "{}");
    expect(() => readLocalEvidence(f.path, f.bundle, f.digest, source)).toThrow(
      "receipt digest mismatch",
    );
  });
  it.each(["nested", "root", "receipt"])("rejects %s symlink escapes", (kind) => {
    const f = fixture();
    let { bundle, path } = f;
    if (kind === "nested") symlinkSync(f.path, join(bundle, "escape"));
    if (kind === "root") {
      bundle = join(f.root, "alias");
      symlinkSync(f.bundle, bundle);
    }
    if (kind === "receipt") {
      path = join(f.root, "alias");
      symlinkSync(f.path, path);
    }
    expect(() => readLocalEvidence(path, bundle, f.digest, source)).toThrow(/symlink/);
  });
  it("rejects a receipt inside its own bundle", () => {
    const f = fixture();
    const path = join(f.bundle, "receipt.json");
    writeFileSync(path, readFileSync(f.path));
    expect(() => readLocalEvidence(path, f.bundle, f.digest, source)).toThrow("outside its bundle");
  });
  it("rejects a binding without local contract provenance", () => {
    expect(() => verifyLocalStageEvidence({ localEvidence: {} }, {}, ".")).toThrow(
      "mixed or missing",
    );
  });
});
describe("local CLI scope", () => {
  const flags = [
    "--local-receipt",
    "/retained/receipt.json",
    "--local-receipt-sha",
    "a".repeat(64),
  ];
  it.each(["run-id", "job-id", "artifact-id", "artifact-expires-at"])(
    "rejects mixed Actions %s",
    (key) => {
      expect(() =>
        parseArgs([
          "--reason",
          "cargo-producer-lock",
          "--stage",
          "/stage",
          "--tool-sha",
          source,
          ...flags,
          "--" + key,
          "1",
        ]),
      ).toThrow("forbids Actions");
    },
  );
  it.each(["release-bump", "runtime-identity", "fingerprint-helper", "cargo-internal-rewire"])(
    "rejects %s scope escape",
    (reason) => {
      expect(() => parseArgs(["--reason", reason, ...flags])).toThrow("requires staged");
    },
  );
  it("rejects partial receipt arguments, duplicate flags, apply and Linux scope escapes", () => {
    expect(() =>
      parseArgs([
        "--reason",
        "cargo-producer-lock",
        "--stage",
        "/stage",
        "--tool-sha",
        source,
        ...flags.slice(0, 2),
      ]),
    ).toThrow("required together");
    expect(() =>
      parseArgs([
        "--reason",
        "cargo-producer-lock",
        "--stage",
        "/stage",
        "--tool-sha",
        source,
        ...flags.slice(2),
      ]),
    ).toThrow("required together");
    expect(() =>
      parseArgs([
        "--reason",
        "cargo-producer-lock",
        "--stage",
        "/stage",
        "--tool-sha",
        source,
        ...flags,
        ...flags,
      ]),
    ).toThrow("duplicate");
    expect(() => parseArgs(["--apply", "/stage", ...flags])).toThrow("requires staged");
    expect(() => linuxArgs(flags)).toThrow("Darwin adapter");
  });
});
