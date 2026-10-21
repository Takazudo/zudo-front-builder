import { realpathSync } from "node:fs";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, symlinkSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vite-plus/test";
import {
  LOCAL_EVIDENCE_PROFILES,
  localBundleDigest,
  localEvidenceProfile,
  readLocalEvidence,
  validateLocalReceipt,
  verifyLocalStageEvidence,
} from "../island-size-local-evidence.mjs";
import { parseArgs } from "../rebaseline-island-size-darwin.mjs";
import { REASONS, parseArgs as linuxArgs } from "../rebaseline-island-size-linux.mjs";
import {
  DECISION_SHA256,
  NEW_LOCK_SHA256,
  OLD_LOCK_SHA256,
} from "../rustls-security-transition.mjs";
const source = "a".repeat(40);
const PROVENANCE_REPINS = [
  ["release-bump", "release-version-stamp"],
  ["runtime-identity", "runtime-identity-repin"],
];
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
  it.each([
    "metric-migration",
    "fingerprint-migration",
    "fingerprint-helper",
    "dependency-graph",
    "fingerprint-policy",
  ])("rejects %s scope escape", (reason) => {
    expect(() =>
      parseArgs(["--reason", reason, "--stage", "/stage", "--tool-sha", source, ...flags]),
    ).toThrow("requires staged");
  });
  it.each(["cargo-producer-lock", "cargo-internal-rewire", "release-bump", "runtime-identity"])(
    "requires --stage and an exact --tool-sha for %s",
    (reason) => {
      expect(() => parseArgs(["--reason", reason, "--tool-sha", source, ...flags])).toThrow(
        "requires staged",
      );
      expect(() => parseArgs(["--reason", reason, "--stage", "/stage", ...flags])).toThrow(
        "--tool-sha",
      );
      expect(() =>
        parseArgs([
          "--reason",
          reason,
          "--stage",
          "/stage",
          "--tool-sha",
          "A".repeat(40),
          ...flags,
        ]),
      ).toThrow("--tool-sha");
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
describe("negative controls: corrupt, swapped, wrong-SHA and wrong-platform evidence", () => {
  const flip = (hex) => (hex[0] === "0" ? "1" : "0") + hex.slice(1);
  it("rejects a corrupt receipt even when its digest is supplied for the corrupt bytes", () => {
    const f = fixture();
    writeFileSync(f.path, readFileSync(f.path, "utf8").slice(0, -7));
    expect(() => readLocalEvidence(f.path, f.bundle, sha(readFileSync(f.path)), source)).toThrow(
      "not valid JSON",
    );
  });
  it("rejects same-size corrupted and truncated retained bytes", () => {
    const f = fixture();
    writeFileSync(join(f.bundle, "evidence"), "retained bytez");
    expect(() => readLocalEvidence(f.path, f.bundle, f.digest, source)).toThrow(
      "bundle digest mismatch",
    );
    writeFileSync(join(f.bundle, "evidence"), "");
    expect(() => readLocalEvidence(f.path, f.bundle, f.digest, source)).toThrow(
      "bundle digest mismatch",
    );
  });
  it("rejects swapped receipts and bundles from two individually valid collections", () => {
    const a = fixture();
    const b = fixture();
    writeFileSync(join(b.bundle, "evidence"), "other retained bytes");
    b.receipt.bundle.sha256 = localBundleDigest(b.bundle);
    writeFileSync(b.path, JSON.stringify(b.receipt));
    b.digest = sha(readFileSync(b.path));
    expect(readLocalEvidence(b.path, b.bundle, b.digest, source).sha256).toBe(b.digest);
    expect(() => readLocalEvidence(a.path, b.bundle, a.digest, source)).toThrow(
      "bundle digest mismatch",
    );
    expect(() => readLocalEvidence(b.path, a.bundle, b.digest, source)).toThrow(
      "bundle digest mismatch",
    );
    expect(() => readLocalEvidence(a.path, a.bundle, b.digest, source)).toThrow(
      "receipt digest mismatch",
    );
  });
  it("rejects a wrong receipt SHA, a wrong source SHA and a renamed bundle entry", () => {
    const f = fixture();
    expect(() => readLocalEvidence(f.path, f.bundle, flip(f.digest), source)).toThrow(
      "receipt digest mismatch",
    );
    expect(() => readLocalEvidence(f.path, f.bundle, f.digest.toUpperCase(), source)).toThrow(
      "explicit receipt SHA-256",
    );
    expect(() => readLocalEvidence(f.path, f.bundle, f.digest, "b".repeat(40))).toThrow(
      "exact source",
    );
    writeFileSync(join(f.bundle, "evidencf"), readFileSync(join(f.bundle, "evidence")));
    rmSync(join(f.bundle, "evidence"));
    expect(() => readLocalEvidence(f.path, f.bundle, f.digest, source)).toThrow(
      "bundle digest mismatch",
    );
  });
  it.each([
    { os: "darwin", arch: "x64" },
    { os: "linux", arch: "x64" },
    { os: "windows", arch: "arm64" },
    { os: "Darwin", arch: "arm64" },
    { os: "darwin", arch: "arm64", rosetta: true },
  ])("rejects receipt platform %o", (platform) => {
    const f = fixture();
    f.receipt.platform = platform;
    writeFileSync(f.path, JSON.stringify(f.receipt));
    expect(() => readLocalEvidence(f.path, f.bundle, sha(readFileSync(f.path)), source)).toThrow(
      "native Darwin arm64",
    );
  });

  // Shape-only stage records: profile checks must reject these before any
  // retained file or git checkout is read.
  const security = {
    oldSha: OLD_LOCK_SHA256,
    newSha: NEW_LOCK_SHA256,
    review: { id: "rustls-security-4314", manifestSha256: DECISION_SHA256 },
  };
  const tool = { root: "/tool", commit: "c".repeat(40) };
  function staged(reason, evidence = {}, toolchain = { cargoLockSha256: NEW_LOCK_SHA256 }) {
    return {
      run: {
        reason,
        platform: "darwin-arm64",
        sourceSha: source,
        stagingTool: tool,
        localEvidence: {
          receiptPath: "/r",
          bundlePath: "/b",
          sha256: "a".repeat(64),
        },
      },
      contract: {
        baselineSourceSha: source,
        toolchain,
        baselineEvidence: {
          event: "local-native",
          sourceSha: source,
          stagingTool: { commit: tool.commit },
          ...evidence,
        },
      },
    };
  }
  const missing = /ENOENT|no such file/;
  it("accepts each profile's own transition shape as far as the retained-file read", () => {
    let s = staged("cargo-producer-lock", { cargoTransition: security });
    expect(() => verifyLocalStageEvidence(s.run, s.contract, ".")).toThrow(missing);
    s = staged("cargo-internal-rewire", {
      cargoRewire: { oldSha: "x", newSha: "y" },
    });
    expect(() => verifyLocalStageEvidence(s.run, s.contract, ".")).toThrow(missing);
    for (const [reason, outcome] of PROVENANCE_REPINS) {
      s = staged(reason);
      s.contract.outcome = outcome;
      expect(() => verifyLocalStageEvidence(s.run, s.contract, ".")).toThrow(missing);
    }
  });
  it.each(
    PROVENANCE_REPINS.flatMap(([reason, outcome]) => [
      [
        reason + " carrying the security transition",
        reason,
        outcome,
        { cargoTransition: security },
      ],
      [reason + " carrying a rewire proof", reason, outcome, { cargoRewire: { oldSha: "x" } }],
      [reason + " with another reason's outcome", reason, "cargo-producer-lock-transition", {}],
      [reason + " without an outcome", reason, undefined, {}],
    ]),
  )("rejects a provenance re-pin stage: %s", (_label, reason, outcome, evidence) => {
    const s = staged(reason, evidence);
    s.contract.outcome = outcome;
    expect(() => verifyLocalStageEvidence(s.run, s.contract, ".")).toThrow(/local Darwin evidence/);
    expect(() => verifyLocalStageEvidence(s.run, s.contract, ".")).not.toThrow(missing);
  });
  it.each([
    [
      "rewire stage carrying the security transition",
      "cargo-internal-rewire",
      { cargoTransition: security },
    ],
    [
      "security stage carrying a rewire proof",
      "cargo-producer-lock",
      { cargoTransition: security, cargoRewire: { oldSha: "x" } },
    ],
    ["rewire stage without its proof", "cargo-internal-rewire", {}],
    ["security stage without its transition", "cargo-producer-lock", {}],
    ["reason without a profile", "dependency-graph", { cargoTransition: security }],
  ])("rejects a swapped profile: %s", (_label, reason, evidence) => {
    const s = staged(reason, evidence);
    expect(() => verifyLocalStageEvidence(s.run, s.contract, ".")).toThrow(/local Darwin evidence/);
    expect(() => verifyLocalStageEvidence(s.run, s.contract, ".")).not.toThrow(missing);
  });
  it.each([
    ["old lock", { ...security, oldSha: flip(OLD_LOCK_SHA256) }],
    ["new lock", { ...security, newSha: flip(NEW_LOCK_SHA256) }],
    [
      "decision",
      {
        ...security,
        review: { ...security.review, manifestSha256: flip(DECISION_SHA256) },
      },
    ],
    [
      "review id",
      {
        ...security,
        review: { ...security.review, id: "rustls-security-4315" },
      },
    ],
    ["old/new swap", { ...security, oldSha: NEW_LOCK_SHA256, newSha: OLD_LOCK_SHA256 }],
  ])("rejects a wrong security %s SHA", (_label, transition) => {
    const s = staged("cargo-producer-lock", { cargoTransition: transition });
    expect(() => verifyLocalStageEvidence(s.run, s.contract, ".")).toThrow("exact rustls");
  });
  it("rejects the security profile with the old lock still pinned", () => {
    const s = staged(
      "cargo-producer-lock",
      { cargoTransition: security },
      { cargoLockSha256: OLD_LOCK_SHA256 },
    );
    expect(() => verifyLocalStageEvidence(s.run, s.contract, ".")).toThrow("wrong security lock");
  });
  it.each(["linux-x64", "darwin-x64", undefined])("rejects staged run platform %s", (platform) => {
    const s = staged("cargo-producer-lock", { cargoTransition: security });
    s.run.platform = platform;
    expect(() => verifyLocalStageEvidence(s.run, s.contract, ".")).toThrow("mixed or missing");
  });
  it.each([
    ["tool commit", (s) => (s.contract.baselineEvidence.stagingTool.commit = "d".repeat(40))],
    ["tool root", (s) => (s.run.stagingTool = { ...tool, root: "tool" })],
    ["no tool", (s) => delete s.run.stagingTool],
    ["Actions id", (s) => (s.contract.baselineEvidence.runId = 1)],
    ["relative binding", (s) => (s.run.localEvidence.receiptPath = "r")],
  ])("rejects a wrong %s before reading evidence", (_label, mutate) => {
    for (const reason of Object.keys(LOCAL_EVIDENCE_PROFILES)) {
      const s = staged(
        reason,
        reason === "cargo-producer-lock"
          ? { cargoTransition: security }
          : reason === "cargo-internal-rewire"
            ? { cargoRewire: { oldSha: "x" } }
            : {},
      );
      s.contract.outcome = Object.fromEntries(PROVENANCE_REPINS)[reason];
      mutate(s);
      expect(() => verifyLocalStageEvidence(s.run, s.contract, ".")).toThrow(
        /local Darwin evidence/,
      );
      expect(() => verifyLocalStageEvidence(s.run, s.contract, ".")).not.toThrow(missing);
    }
  });
  it("keeps exactly the four reviewed profiles", () => {
    expect(Object.keys(LOCAL_EVIDENCE_PROFILES).sort()).toEqual([
      "cargo-internal-rewire",
      "cargo-producer-lock",
      "release-bump",
      "runtime-identity",
    ]);
    for (const [reason, outcome] of PROVENANCE_REPINS)
      expect(REASONS[reason].outcome).toBe(outcome);
    expect(Object.isFrozen(LOCAL_EVIDENCE_PROFILES)).toBe(true);
    expect(() => localEvidenceProfile("toString")).toThrow("no local evidence profile");
    expect(() => localEvidenceProfile("__proto__")).toThrow("no local evidence profile");
  });
});
