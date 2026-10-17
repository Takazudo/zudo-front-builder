import { createHash } from "node:crypto";
import {
  appendFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vite-plus/test";

import {
  NORMALIZED_SIDECAR,
  normalizeFixtureFiles,
  normalizeMeasurementDirectory,
  normalizerSha256,
} from "../../research/v3-island-size/normalize-identity.mjs";
import {
  CARGO_PRODUCER_LOCK_MOVEMENT,
  DEPENDENCY_GRAPH_MOVEMENT,
  FINGERPRINT_HELPER_MOVEMENT,
  FINGERPRINT_POLICY_MOVEMENT,
  applyStage,
  assertAllowedMovement,
  assertLinuxX64Provenance,
  contractDiffKeys,
  helperExplainBytes,
  main as linuxMain,
  measuredTotals,
  rewriteTestPins,
} from "../rebaseline-island-size-linux.mjs";
import {
  buildEvidence,
  checkMigrationTotals,
  checkoutMismatches,
  darwinCargoProducerLockReadmeSection,
  darwinFingerprintReadmeSection,
  darwinReadmeSection,
  loadDarwinArtifact,
  main,
  migrateDarwinContract,
  migrateDarwinFingerprint,
  parseArgs,
  repinDarwinContract,
  repinDarwinDependencyGraph,
  repinDarwinFingerprintPolicy,
  darwinRepinReadmeSection,
  rewriteDarwinTestPins,
  transitionDarwinCargoLock,
  validateDarwinEvidence,
} from "../rebaseline-island-size-darwin.mjs";
import {
  addAmbiguousCommitPrefix,
  asSchemaV2,
  ceilingsOf,
  gitIn,
  landStaged,
  makeCheckout,
  makeGitCheckout,
  makeGraphCheckout,
  makeHelperCheckout,
  makePolicyCheckout,
  policyHelper,
  writeRuns,
} from "./fixtures/island-size-checkout.mjs";
import { moveReachableSnapshot } from "./fixtures/island-size-lock-mutations.mjs";
import { reviewedCargoLock } from "./fixtures/reviewed-cargo-transition.mjs";

const testDir = dirname(fileURLToPath(import.meta.url));
const legacy = JSON.parse(
  readFileSync(join(testDir, "fixtures/island-size-darwin-actual-gzip-v1.json"), "utf8"),
);
const SOURCE_SHA = "a".repeat(40);
const VERSION = "9.9.9";
const MODES = ["workspace", "packed"];
const FIXTURES = Object.keys(legacy.ceilings.workspace);
const roots = [];

function sha(value) {
  return createHash("sha256").update(value).digest("hex");
}

function filesFor(mode, fixture) {
  const token = sha(mode + fixture).slice(0, 16);
  if (fixture === "no-island") {
    return { "index.html": Buffer.from("<!doctype html><body>empty</body>\n") };
  }
  const entry =
    "// " +
    fixture +
    '\nfunction r(n){return{identity:{component:n,build:"' +
    token +
    '"},mount:()=>{let l={component:n,build:"' +
    token +
    '"};return l}}}\nexport {};\n';
  return {
    "index.html": Buffer.from(
      '<!doctype html><script type="module" src="/assets/islands-x.js"></script><div data-zfb-build="' +
        token +
        '"></div>\n',
    ),
    "assets/islands-x.js": Buffer.from(entry),
  };
}

function write(root, relative, bytes) {
  const path = join(root, relative);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, bytes);
}

// Builds a complete synthetic native artifact. `tweak` mutates provenance
// before the sidecars are written so the on-disk run stays self-consistent.
function makeArtifact({
  platform = { os: "darwin", arch: "arm64" },
  tweak,
  version = VERSION,
} = {}) {
  const root = mkdtempSync(join(tmpdir(), "zfb-darwin-migration-"));
  roots.push(root);
  const digests = {
    zfbBinarySha256: sha("zfb-binary"),
    esbuildBinarySha256: sha("esbuild-binary"),
    zfbTarballSha256: sha("zfb-tgz"),
    runtimeTarballSha256: sha("runtime-tgz"),
    slugifyTarballSha256: sha("slugify-tgz"),
  };
  write(root, "pack/takazudo-zfb-" + version + ".tgz", "zfb-tgz");
  write(root, "pack/takazudo-zfb-runtime-" + version + ".tgz", "runtime-tgz");
  write(root, "pack/takazudo-zfb-slugify-" + version + ".tgz", "slugify-tgz");
  write(root, "packed-package/package/package.json", '{"name":"@takazudo/zfb"}');
  write(root, "packed-runtime/package/package.json", '{"name":"@takazudo/zfb-runtime"}');
  write(root, "packed-slugify/package/package.json", '{"name":"@takazudo/zfb-slugify"}');
  write(root, "check-budget.exit-code", "1\n");
  write(root, "check-budget.log", "Island size budget rejected:\n");
  write(
    root,
    "runner-provenance.txt",
    [
      "source_sha=" + SOURCE_SHA,
      "runner=Darwin arm64",
      "sw_vers=ProductName:\t\tmacOS;ProductVersion:\t\t15.7.9;BuildVersion:\t\t24G830;",
      "rustc=rustc 1.99.0 (b940084d7 2026-09-28);host: aarch64-apple-darwin;",
      "cargo=cargo 1.99.0 (5f94df478 2026-08-27)",
      "node=" + legacy.toolchain.nodeVersion,
      "zlib=" + legacy.toolchain.zlibVersion,
      "pnpm=12.8.2",
      "zfb_binary=" + digests.zfbBinarySha256 + "  target/debug/zfb",
      "esbuild_version=" + legacy.toolchain.esbuildVersion,
      "esbuild_binary=" + digests.esbuildBinarySha256 + "  esbuild",
      "hono_version=" + legacy.toolchain.honoVersion,
      "slugify_tarball=" + digests.slugifyTarballSha256 + "  x.tgz",
      "zfb_tarball=" + digests.zfbTarballSha256 + "  y.tgz",
      "runtime_tarball=" + digests.runtimeTarballSha256 + "  z.tgz",
      "",
    ].join("\n"),
  );
  for (const mode of MODES) {
    const results = {};
    for (const fixture of FIXTURES) {
      const files = filesFor(mode, fixture);
      for (const pass of [1, 2]) {
        for (const [name, bytes] of Object.entries(files)) {
          write(root, "real-" + mode + "/" + fixture + "/pass-" + pass + "/" + name, bytes);
        }
      }
      const actual = normalizeFixtureFiles(files, mode + " " + fixture);
      results[fixture] = {
        chunks: actual.chunks.map(({ file, raw, gzip, sha256, phase, role }) => ({
          file,
          raw,
          gzip,
          sha256,
          phase,
          role,
        })),
        initial: { raw: actual.initial.raw, gzip: actual.initial.gzip },
        later: { raw: actual.later.raw, gzip: actual.later.gzip },
        shipped: { raw: actual.shipped.raw, gzip: actual.shipped.gzip },
      };
    }
    const provenance = {
      sourceSha: SOURCE_SHA,
      mode,
      platform,
      fixtureVersion: legacy.fixtureVersion,
      fixtureSha256: legacy.fixtureSha256,
      runnerSha256: legacy.runnerSha256,
      packageVersion: version,
      runtimePackageVersion: version,
      packageJsonSha256: sha('{"name":"@takazudo/zfb"}'),
      runtimePackageJsonSha256: sha('{"name":"@takazudo/zfb-runtime"}'),
      slugifyPackageJsonSha256: sha('{"name":"@takazudo/zfb-slugify"}'),
      honoPackageVersion: legacy.toolchain.honoVersion,
      esbuildVersion: legacy.toolchain.esbuildVersion,
      nodeVersion: legacy.toolchain.nodeVersion,
      zlibVersion: legacy.toolchain.zlibVersion,
      compression: legacy.compression,
      repeats: 2,
      pnpmLockSha256: sha("pnpm-lock"),
      cargoLockSha256: sha("cargo-lock"),
      zfbBinarySha256: digests.zfbBinarySha256,
      esbuildBinarySha256: digests.esbuildBinarySha256,
      zfbTarballSha256: digests.zfbTarballSha256,
      runtimeTarballSha256: digests.runtimeTarballSha256,
    };
    tweak?.(provenance, mode);
    const path = join(root, "real-" + mode, "measurement.json");
    writeFileSync(path, JSON.stringify({ provenance, results }, null, 2) + "\n");
    const { sidecar } = normalizeMeasurementDirectory(join(root, "real-" + mode));
    writeFileSync(
      join(root, "real-" + mode, NORMALIZED_SIDECAR),
      JSON.stringify(sidecar, null, 2) + "\n",
    );
  }
  return root;
}

// A schemaVersion-1 contract whose raw ceilings equal the synthetic artifact
// and whose actual-gzip ceilings are `gzipSlack` above the normalized totals.
function contractFor(root, gzipSlack = 2) {
  const contract = structuredClone(legacy);
  for (const mode of MODES) {
    const { sidecar } = normalizeMeasurementDirectory(join(root, "real-" + mode));
    for (const fixture of FIXTURES) {
      const { raw, gzipNormalized } = sidecar.results[fixture].shipped;
      contract.ceilings[mode][fixture] = {
        raw,
        gzip: gzipNormalized === 0 ? 0 : gzipNormalized + gzipSlack,
      };
    }
  }
  return contract;
}

const args = {
  artifact: "unused",
  "run-id": "7",
  "job-id": "8",
  "artifact-id": "9",
  "artifact-expires-at": "2026-11-11T07:26:59Z",
  "source-sha": SOURCE_SHA,
  reason: "metric-migration",
};

// Test fixture: a digest-only previous-contract reference, as main records for
// a contract that is not committed at HEAD.
const PREVIOUS = { sha256: sha("previous contract bytes") };
const REPINNED_FROM_KEYS = [
  "schemaVersion",
  "outcome",
  "packageVersion",
  "baselineSourceSha",
  "baselineRunId",
  "contract",
];

function evidenceFor(cliArgs, artifact, previousContract = PREVIOUS) {
  return { ...buildEvidence(cliArgs, artifact), previousContract };
}

function totalsOf(artifact) {
  return measuredTotals({
    workspace: artifact.modes.workspace.recomputed.results,
    packed: artifact.modes.packed.recomputed.results,
  });
}

afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

describe("rebaseline-island-size-darwin", () => {
  it("migrates a complete native artifact using the artifact's own provenance", () => {
    const root = makeArtifact();
    const contract = contractFor(root);
    const artifact = loadDarwinArtifact(root);
    expect(validateDarwinEvidence(contract, artifact, SOURCE_SHA)).toEqual([]);
    const next = migrateDarwinContract(contract, totalsOf(artifact), evidenceFor(args, artifact));
    expect(next.schemaVersion).toBe(2);
    expect(next.metric).toBe("gzip-identity-normalized");
    expect(next.normalizationVersion).toBe(1);
    expect(next.normalizerSha256).toBe(normalizerSha256());
    expect(next.allowance).toEqual({ raw: 0, gzipNormalized: 0 });
    expect(next.platform).toEqual({ os: "darwin", arch: "arm64" });
    expect(next.baselineSourceSha).toBe(SOURCE_SHA);
    expect(next.toolchain.packageVersion).toBe(VERSION);
    expect(next.toolchain.pnpmLockSha256).toBe(sha("pnpm-lock"));
    expect(next.toolchain.cargoLockSha256).toBe(sha("cargo-lock"));
    expect(Object.keys(next).slice(0, 4)).toEqual([
      "schemaVersion",
      "metric",
      "normalizationVersion",
      "normalizerSha256",
    ]);
    for (const mode of MODES) {
      for (const fixture of FIXTURES) {
        expect(Object.keys(next.ceilings[mode][fixture])).toEqual(["raw", "gzipNormalized"]);
        expect(next.ceilings[mode][fixture].raw).toBe(contract.ceilings[mode][fixture].raw);
        expect(next.baselineEvidence.actualGzip[mode][fixture]).toBe(
          artifact.modes[mode].recomputed.results[fixture].shipped.gzip,
        );
      }
    }
    expect(next.baselineEvidence).toMatchObject({
      event: "ci-artifact-replay",
      runId: 7,
      jobId: 8,
      artifact: { id: 9, expiresAt: "2026-11-11T07:26:59Z" },
      nativeRunner: { platform: "darwin-arm64", macOS: "15.7.9 (24G830)" },
      migratedFrom: { schemaVersion: 1, metric: "gzip" },
      originalCheckerResult: { exitCode: 1 },
    });
    expect(next.baselineEvidence).not.toHaveProperty("repinnedFrom");
    expect(next.platformStatus).toContain("not a native measurement of the current source");
    expect(next.platformStatus).toContain("#4170");
    const readme = darwinReadmeSection(next, contract);
    expect(readme).toContain("Original semantics");
    expect(readme).toContain("historical-artifact evidence");
    expect(readme).toContain("#4170");
  });

  it("accepts the four reasons and validates arguments", () => {
    expect(parseArgs(Object.entries(args).flatMap(([k, v]) => ["--" + k, v]))["run-id"]).toBe("7");
    const bad = (override) => () =>
      parseArgs(Object.entries({ ...args, ...override }).flatMap(([k, v]) => ["--" + k, v]));
    expect(bad({ reason: "release-bump" })).not.toThrow();
    expect(bad({ reason: "runtime-identity" })).not.toThrow();
    expect(bad({ reason: "fingerprint-migration" })).not.toThrow();
    expect(bad({ reason: "other" })).toThrow(
      /--reason must be one of metric-migration, release-bump, runtime-identity, fingerprint-migration, fingerprint-helper, cargo-producer-lock, dependency-graph, fingerprint-policy$/,
    );
    // The Linux-only consumer-lock reason (#4184) is not a Darwin re-pin reason.
    expect(bad({ reason: "lockfile-provenance" })).toThrow("--reason must be one of");
    expect(bad({ "source-sha": "abc" })).toThrow("40-character");
    expect(bad({ "run-id": "x" })).toThrow("positive integer");
    expect(bad({ "artifact-expires-at": "never" })).toThrow("ISO timestamp");
    expect(() => parseArgs(["--artifact", "x"])).toThrow("required --run-id");
  });

  it("rejects a Linux or unknown platform on either side", () => {
    const linuxRoot = makeArtifact({ platform: { os: "linux", arch: "x64" } });
    const linuxArtifact = loadDarwinArtifact(linuxRoot);
    expect(
      validateDarwinEvidence(contractFor(linuxRoot), linuxArtifact, SOURCE_SHA).join("\n"),
    ).toContain("measured platform is not darwin-arm64");
    const root = makeArtifact();
    const artifact = loadDarwinArtifact(root);
    const linuxContract = { ...contractFor(root), platform: { os: "linux", arch: "x64" } };
    expect(validateDarwinEvidence(linuxContract, artifact, SOURCE_SHA)).toContain(
      "contract is not the Darwin arm64 contract",
    );
    const migrated = migrateDarwinContract(
      contractFor(root),
      totalsOf(artifact),
      evidenceFor(args, artifact),
    );
    expect(validateDarwinEvidence(migrated, artifact, SOURCE_SHA)).toContain(
      "contract is not a schemaVersion 1 actual-gzip contract",
    );
    expect(() => assertLinuxX64Provenance({ platform: { os: "darwin", arch: "arm64" } })).toThrow(
      "not linux-x64",
    );
    expect(() =>
      assertLinuxX64Provenance({ platform: { os: "linux", arch: "x64" } }),
    ).not.toThrow();
  });

  it("rejects a missing normalized sidecar", () => {
    const root = makeArtifact();
    rmSync(join(root, "real-packed", NORMALIZED_SIDECAR));
    expect(() => loadDarwinArtifact(root)).toThrow(
      "missing measurement.normalized.json for packed",
    );
  });

  it("rejects a stale sidecar that disagrees with the saved bytes", () => {
    const root = makeArtifact();
    const path = join(root, "real-workspace", NORMALIZED_SIDECAR);
    const sidecar = JSON.parse(readFileSync(path, "utf8"));
    sidecar.results["event-only"].shipped.gzipNormalized -= 5;
    writeFileSync(path, JSON.stringify(sidecar));
    const errors = validateDarwinEvidence(contractFor(root), loadDarwinArtifact(root), SOURCE_SHA);
    expect(errors).toContain(
      "workspace: sidecar disagrees with the recomputation from the saved bytes",
    );
  });

  it("rejects a token shape or site count the normalizer does not recognize", () => {
    const mismatched = makeArtifact();
    for (const pass of [1, 2]) {
      const html = join(mismatched, "real-workspace/event-only/pass-" + pass + "/index.html");
      writeFileSync(
        html,
        readFileSync(html, "utf8").replace(
          /data-zfb-build="[0-9a-f]+"/,
          'data-zfb-build="0123456789abcdef"',
        ),
      );
    }
    expect(() => loadDarwinArtifact(mismatched)).toThrow("build identity site carries");

    const malformed = makeArtifact();
    for (const pass of [1, 2]) {
      const html = join(malformed, "real-packed/model/pass-" + pass + "/index.html");
      writeFileSync(
        html,
        readFileSync(html, "utf8").replace(/data-zfb-build="[0-9a-f]+"/, 'data-zfb-build="XYZ"'),
      );
    }
    expect(() => loadDarwinArtifact(malformed)).toThrow("16 lowercase hex");
  });

  it("rejects saved bytes that disagree with the claimed measurement or between passes", () => {
    const tampered = makeArtifact();
    const js = join(tampered, "real-workspace/event-only/pass-1/assets/islands-x.js");
    writeFileSync(js, readFileSync(js, "utf8") + "// extra\n");
    expect(() => loadDarwinArtifact(tampered)).toThrow(/saved repeated dist inventories differ/);

    const passEqual = makeArtifact();
    for (const pass of [1, 2]) {
      const file = join(
        passEqual,
        "real-workspace/event-only/pass-" + pass + "/assets/islands-x.js",
      );
      writeFileSync(file, readFileSync(file, "utf8") + "// extra\n");
    }
    expect(() => loadDarwinArtifact(passEqual)).toThrow(/measurement\.json claims raw/);
  });

  it("rejects inconsistent provenance, packages and runner files", () => {
    const root = makeArtifact();
    writeFileSync(join(root, "pack", "takazudo-zfb-" + VERSION + ".tgz"), "different bytes");
    writeFileSync(join(root, "packed-runtime/package/package.json"), "{}");
    const errors = validateDarwinEvidence(
      contractFor(root),
      loadDarwinArtifact(root),
      SOURCE_SHA,
    ).join("\n");
    expect(errors).toContain("saved zfb tarball is missing or differs");
    expect(errors).toContain("saved packed runtime package.json is missing or differs");
    const runnerPath = join(root, "runner-provenance.txt");
    writeFileSync(
      runnerPath,
      readFileSync(runnerPath, "utf8")
        .replace("runner=Darwin arm64", "runner=Linux x86_64")
        .replace(/zfb_tarball=[0-9a-f]+/, "zfb_tarball=" + "0".repeat(64)),
    );
    const runnerErrors = validateDarwinEvidence(
      contractFor(root),
      loadDarwinArtifact(root),
      SOURCE_SHA,
    ).join("\n");
    expect(runnerErrors).toContain("runner-provenance.txt runner is missing or inconsistent");
    expect(runnerErrors).toContain("runner-provenance.txt zfb_tarball is missing or inconsistent");

    const lockDrift = makeArtifact({
      tweak: (p, mode) => {
        if (mode === "packed") p.cargoLockSha256 = sha("other");
      },
    });
    expect(
      validateDarwinEvidence(contractFor(lockDrift), loadDarwinArtifact(lockDrift), SOURCE_SHA),
    ).toContain("packed: cargoLockSha256 differs between workspace and packed");

    const wrongSource = makeArtifact();
    expect(
      validateDarwinEvidence(
        contractFor(wrongSource),
        loadDarwinArtifact(wrongSource),
        "b".repeat(40),
      ).join("\n"),
    ).toContain("source SHA " + SOURCE_SHA);

    const wrongToolchain = makeArtifact({ tweak: (p) => (p.nodeVersion = "v22.0.0") });
    expect(
      validateDarwinEvidence(
        contractFor(wrongToolchain),
        loadDarwinArtifact(wrongToolchain),
        SOURCE_SHA,
      ),
    ).toContain("workspace: nodeVersion differs from the contract");
  });

  it("refuses changed raw totals, a missing fixture and any size increase", () => {
    const root = makeArtifact();
    const artifact = loadDarwinArtifact(root);
    const totals = totalsOf(artifact);

    const rawMoved = contractFor(root);
    rawMoved.ceilings.packed.model.raw -= 1;
    expect(() => migrateDarwinContract(rawMoved, totals, evidenceFor(args, artifact))).toThrow(
      "raw totals changed (packed/model)",
    );

    const missing = contractFor(root);
    delete missing.ceilings.workspace["json-api"];
    expect(checkMigrationTotals(missing, totals).join("\n")).toContain(
      "fixture set differs from the contract (workspace/json-api)",
    );

    const grown = contractFor(root);
    grown.ceilings.workspace["show-for"].gzip = totals.workspace["show-for"].gzipNormalized - 1;
    expect(() => migrateDarwinContract(grown, totals, evidenceFor(args, artifact))).toThrow(
      "that is a size increase, not a metric migration",
    );

    const inflated = structuredClone(totals);
    inflated.packed.model.gzipNormalized = inflated.packed.model.gzip + 1;
    expect(checkMigrationTotals(contractFor(root, 50), inflated).join("\n")).toContain(
      "packed/model: gzipNormalized exceeds actual gzip",
    );
  });

  it("keeps non-identity product bytes visible while ignoring only the token", () => {
    const files = filesFor("workspace", "event-only");
    const base = normalizeFixtureFiles(files, "base");
    const retoken = { ...files };
    const oldToken = /data-zfb-build="([0-9a-f]{16})"/.exec(files["index.html"].toString())[1];
    const swap = (buffer) => Buffer.from(buffer.toString().replaceAll(oldToken, "f".repeat(16)));
    retoken["index.html"] = swap(files["index.html"]);
    retoken["assets/islands-x.js"] = swap(files["assets/islands-x.js"]);
    const moved = normalizeFixtureFiles(retoken, "moved");
    expect(moved.shipped.gzipNormalized).toBe(base.shipped.gzipNormalized);
    expect(moved.chunks[0].sha256Normalized).toBe(base.chunks[0].sha256Normalized);
    expect(moved.chunks[0].sha256).not.toBe(base.chunks[0].sha256);

    const edited = { ...files };
    edited["assets/islands-x.js"] = Buffer.from(
      files["assets/islands-x.js"].toString().replace("export {};", "export {};/*x*/"),
    );
    const changed = normalizeFixtureFiles(edited, "edited");
    expect(changed.shipped.raw).toBeGreaterThan(base.shipped.raw);
    expect(changed.chunks[0].sha256Normalized).not.toBe(base.chunks[0].sha256Normalized);
  });
});

// A schemaVersion-2 contract one release behind the synthetic artifact, at the
// same Cargo.lock: release-bump and runtime-identity never move it.
function previousSchema2(root) {
  const artifact = loadDarwinArtifact(root);
  const migrated = migrateDarwinContract(
    contractFor(root),
    totalsOf(artifact),
    evidenceFor(args, artifact),
  );
  migrated.toolchain.packageVersion = "9.9.8";
  migrated.toolchain.pnpmLockSha256 = sha("old-pnpm-lock");
  migrated.baselineSourceSha = "b".repeat(40);
  return migrated;
}

describe("rebaseline-island-size-darwin schema-2 re-pin", () => {
  it("rejects a re-pin whose artifact does not describe the checkout", () => {
    const fingerprint = {
      policyVersion: 1,
      parser: "yaml@2.9.0",
      helperSha256: "1".repeat(64),
      sha256: "2".repeat(64),
    };
    const evidence = {
      packageVersion: "1.0.0",
      pnpmLockSha256: "a",
      cargoLockSha256: "b",
      dependencyFingerprint: { policyVersion: 1, sha256: fingerprint.sha256 },
    };
    const checkout = {
      packageVersion: "1.0.0",
      pnpmLockSha256: "a",
      cargoLockSha256: "b",
      dependencyFingerprint: fingerprint,
    };
    expect(checkoutMismatches(evidence, checkout)).toEqual([]);
    // Byte-exact lockfiles stay compared even though v3 pins the graph only.
    expect(
      checkoutMismatches(evidence, {
        packageVersion: "1.0.1",
        pnpmLockSha256: "x",
        cargoLockSha256: "y",
        dependencyFingerprint: { ...fingerprint, sha256: "3".repeat(64) },
      }),
    ).toEqual([
      "artifact measured @takazudo/zfb 1.0.0, not the checkout's 1.0.1",
      "artifact pnpm-lock.yaml hash differs from this checkout's pnpm-lock.yaml",
      "artifact Cargo.lock hash differs from this checkout's Cargo.lock",
      "artifact dependency fingerprint sha256 differs from this checkout's",
    ]);
    const { dependencyFingerprint: _f, ...withoutFingerprint } = evidence;
    expect(checkoutMismatches(withoutFingerprint, checkout)).toEqual([
      "artifact provenance must carry dependencyFingerprint with exactly policyVersion and sha256",
    ]);
    expect(
      checkoutMismatches(
        { ...evidence, runnerSha256: { "measure.mjs": "4".repeat(64) } },
        { ...checkout, runnerSha256: { "measure.mjs": "5".repeat(64) } },
      ),
    ).toEqual(["artifact runnerSha256 differs from this checkout's files"]);
  });

  for (const reason of ["release-bump", "runtime-identity"]) {
    it("re-pins only provenance for " + reason, () => {
      const root = makeArtifact();
      const artifact = loadDarwinArtifact(root);
      const contract = previousSchema2(root);
      expect(validateDarwinEvidence(contract, artifact, SOURCE_SHA, reason)).toEqual([]);
      const { contract: next, previousVersion } = repinDarwinContract(
        contract,
        totalsOf(artifact),
        evidenceFor({ ...args, reason }, artifact),
        reason,
      );
      expect(previousVersion).toBe("9.9.8");
      expect(next.ceilings).toEqual(contract.ceilings);
      expect(next.allowance).toEqual({ raw: 0, gzipNormalized: 0 });
      expect(next.schemaVersion).toBe(2);
      expect(next.outcome).toBe(
        reason === "release-bump" ? "release-version-stamp" : "runtime-identity-repin",
      );
      expect(next.baselineSourceSha).toBe(SOURCE_SHA);
      expect(next.toolchain.packageVersion).toBe(VERSION);
      expect(next.toolchain.pnpmLockSha256).toBe(sha("pnpm-lock"));
      expect(next.toolchain.cargoLockSha256).toBe(sha("cargo-lock"));
      expect(next.baselineEvidence).toMatchObject({
        event: "ci-artifact",
        runId: 7,
        artifact: { id: 9 },
      });
      expect(Object.keys(next.baselineEvidence.repinnedFrom)).toEqual(REPINNED_FROM_KEYS);
      expect(next.baselineEvidence.repinnedFrom).toEqual({
        schemaVersion: 2,
        outcome: contract.outcome,
        packageVersion: "9.9.8",
        baselineSourceSha: "b".repeat(40),
        baselineRunId: contract.baselineEvidence.runId,
        contract: PREVIOUS,
      });
      const {
        toolchain: a,
        baselineSourceSha,
        outcome,
        platformStatus,
        baselineEvidence,
        normalizerSha256: n,
        ...rest
      } = next;
      const {
        toolchain: b,
        baselineSourceSha: _s,
        outcome: _o,
        platformStatus: _p,
        baselineEvidence: _e,
        normalizerSha256: _n,
        ...restBefore
      } = contract;
      expect(rest).toEqual(restBefore);
      expect({ ...a, packageVersion: 0, pnpmLockSha256: 0, cargoLockSha256: 0 }).toEqual({
        ...b,
        packageVersion: 0,
        pnpmLockSha256: 0,
        cargoLockSha256: 0,
      });
      expect(darwinRepinReadmeSection(next, [], previousVersion, reason)).toContain(
        "Darwin v" + VERSION,
      );
      // The artifact's Cargo.lock is no longer copied over the pin unchecked.
      const stale = structuredClone(contract);
      stale.toolchain.cargoLockSha256 = sha("old-cargo-lock");
      expect(() =>
        repinDarwinContract(
          stale,
          totalsOf(artifact),
          evidenceFor({ ...args, reason }, artifact),
          reason,
        ),
      ).toThrow(
        "--reason " +
          reason +
          " cannot move Cargo.lock (the artifact and checkout carry " +
          sha("cargo-lock") +
          ", the contract pins " +
          sha("old-cargo-lock") +
          "); a reviewed additive producer-lock change needs --reason cargo-producer-lock",
      );
    });
  }

  it("re-pins twice: release wording for a version move, same-version wording after", () => {
    const root = makeArtifact();
    const artifact = loadDarwinArtifact(root);
    const previous = previousSchema2(root);
    const first = repinDarwinContract(
      previous,
      totalsOf(artifact),
      evidenceFor({ ...args, reason: "release-bump" }, artifact),
      "release-bump",
    );
    const release = first.contract;
    expect(release.outcome).toBe("release-version-stamp");
    expect(release.platformStatus).toContain(
      "v" + VERSION + " ceilings on metric gzip-identity-normalized v1, re-pinned (release-bump)",
    );
    expect(release.platformStatus).toContain("unchanged from the v9.9.8 contract");
    expect(release.baselineEvidence.note).toContain("Re-pin (release-bump) from the native");
    const releaseSection = darwinRepinReadmeSection(
      release,
      [],
      first.previousVersion,
      "release-bump",
    );
    expect(releaseSection).toContain("## Darwin v" + VERSION + " release baseline");
    expect(releaseSection).toContain("re-pinned (`release-bump`) for v" + VERSION);
    expect(releaseSection).toContain("unchanged from the v9.9.8 contract");

    // Test fixture: the first result's bytes, cited by digest only.
    const firstBytes = { sha256: sha(JSON.stringify(release, null, 2) + "\n") };
    const second = repinDarwinContract(
      release,
      totalsOf(artifact),
      evidenceFor({ ...args, reason: "runtime-identity" }, artifact, firstBytes),
      "runtime-identity",
    );
    const identity = second.contract;
    expect(Object.keys(identity.baselineEvidence.repinnedFrom)).toEqual(REPINNED_FROM_KEYS);
    expect(identity.baselineEvidence.repinnedFrom).toEqual({
      schemaVersion: 2,
      outcome: "release-version-stamp",
      packageVersion: VERSION,
      baselineSourceSha: SOURCE_SHA,
      baselineRunId: 7,
      contract: firstBytes,
    });
    expect(JSON.stringify(identity.baselineEvidence.repinnedFrom)).not.toContain("repinnedFrom");
    expect(identity.outcome).toBe("runtime-identity-repin");
    expect(identity.platformStatus).toContain("re-pinned (runtime-identity)");
    expect(identity.platformStatus).toContain("unchanged from the v" + VERSION + " contract");
    expect(identity.platformStatus).not.toMatch(/release/i);
    expect(identity.baselineEvidence.note).not.toMatch(/release/i);
    const identitySection = darwinRepinReadmeSection(
      identity,
      [],
      second.previousVersion,
      "runtime-identity",
    );
    expect(identitySection).toContain("## Darwin v" + VERSION + " runtime-identity re-pin");
    expect(identitySection).not.toMatch(/release/i);
  });

  it("rewrites the Darwin test pins on a re-pin, idempotently, touching nothing else", () => {
    const repoRoot = join(testDir, "..", "..");
    const source = readFileSync(
      join(repoRoot, "scripts/__tests__/island-size-budget.test.mjs"),
      "utf8",
    );
    const current = JSON.parse(
      readFileSync(join(repoRoot, "research/v3-island-size/decision.json"), "utf8"),
    );
    expect(rewriteDarwinTestPins(source, current)).toBe(source);

    const next = structuredClone(current);
    next.toolchain.packageVersion = "9.9.9";
    next.toolchain.pnpmLockSha256 = sha("new-pnpm");
    next.toolchain.cargoLockSha256 = sha("new-cargo");
    next.baselineSourceSha = "c".repeat(40);
    next.baselineEvidence.runId = 111;
    next.baselineEvidence.jobId = 222;
    next.baselineEvidence.artifact.id = 333;
    const rewritten = rewriteDarwinTestPins(source, next);
    for (const pin of [
      'toolchain.packageVersion).toBe("9.9.9")',
      JSON.stringify(sha("new-pnpm")),
      JSON.stringify(sha("new-cargo")),
      JSON.stringify("c".repeat(40)),
      "runId: 111,",
      "jobId: 222,",
      "artifact: { id: 333 }",
    ]) {
      expect(rewritten).toContain(pin);
    }
    expect(rewriteDarwinTestPins(rewritten, next)).toBe(rewritten);
    const outside = (text) => text.slice(0, text.indexOf('it("records the reviewed Darwin arm64'));
    expect(outside(rewritten)).toBe(outside(source));
    expect(() => rewriteDarwinTestPins("it('x')", next)).toThrow(/Darwin ceiling test not found/);
  });

  it("rejects a schemaVersion 1 contract, and schema 2 for a metric migration", () => {
    const root = makeArtifact();
    const artifact = loadDarwinArtifact(root);
    expect(validateDarwinEvidence(contractFor(root), artifact, SOURCE_SHA, "release-bump")).toEqual(
      [expect.stringContaining("not a schemaVersion 2")],
    );
    expect(validateDarwinEvidence(previousSchema2(root), artifact, SOURCE_SHA)).toContain(
      "contract is not a schemaVersion 1 actual-gzip contract",
    );
  });

  it("rejects changed raw totals in either direction", () => {
    const root = makeArtifact();
    const artifact = loadDarwinArtifact(root);
    for (const delta of [1, -1]) {
      const contract = previousSchema2(root);
      contract.ceilings.workspace["event-only"].raw += delta;
      expect(() =>
        repinDarwinContract(
          contract,
          totalsOf(artifact),
          evidenceFor(args, artifact),
          "release-bump",
        ),
      ).toThrow("raw totals changed");
    }
  });

  it("rejects changed gzipNormalized totals in either direction", () => {
    const root = makeArtifact();
    const artifact = loadDarwinArtifact(root);
    for (const delta of [1, -1]) {
      const contract = previousSchema2(root);
      contract.ceilings.packed["model"].gzipNormalized += delta;
      expect(() =>
        repinDarwinContract(
          contract,
          totalsOf(artifact),
          evidenceFor(args, artifact),
          "runtime-identity",
        ),
      ).toThrow("gzipNormalized totals changed");
    }
  });

  it("rejects a fixture-set difference", () => {
    const root = makeArtifact();
    const artifact = loadDarwinArtifact(root);
    const contract = previousSchema2(root);
    delete contract.ceilings.packed["json-api"];
    expect(() =>
      repinDarwinContract(
        contract,
        totalsOf(artifact),
        evidenceFor(args, artifact),
        "release-bump",
      ),
    ).toThrow("fixture set differs");
  });

  it("rejects a Linux artifact and a missing sidecar", () => {
    const linuxRoot = makeArtifact({ platform: { os: "linux", arch: "x64" } });
    expect(
      validateDarwinEvidence(
        previousSchema2(linuxRoot),
        loadDarwinArtifact(linuxRoot),
        SOURCE_SHA,
        "release-bump",
      ).join("\n"),
    ).toContain("measured platform is not darwin-arm64");
    const root = makeArtifact();
    rmSync(join(root, "real-workspace", NORMALIZED_SIDECAR));
    expect(() => loadDarwinArtifact(root)).toThrow("missing");
  });

  it("rejects a source SHA that is not the artifact's", () => {
    const root = makeArtifact();
    expect(
      validateDarwinEvidence(
        previousSchema2(root),
        loadDarwinArtifact(root),
        "c".repeat(40),
        "release-bump",
      ).join("\n"),
    ).toContain("source SHA");
  });
});

describe("rebaseline-island-size-darwin fingerprint-migration (#4187)", () => {
  const fingerprint = {
    policyVersion: 1,
    parser: "yaml@2.9.0",
    helperSha256: "1".repeat(64),
    sha256: "2".repeat(64),
  };
  const checkout = { dependencyFingerprint: fingerprint, dependencyFingerprintError: null };
  // The runners now record the fingerprint, so their digests moved.
  const newRunners = { "measure-real.mjs": sha("new-real"), "measure.mjs": sha("new-modeled") };
  const migrationArgs = { ...args, reason: "fingerprint-migration" };

  // A native artifact from the merged producers and a v2 contract at the same
  // version whose runner digests and lock hash predate them.
  function fixture({ tweak } = {}) {
    const root = makeArtifact({
      tweak: (p, mode) => {
        p.runnerSha256 = newRunners;
        p.dependencyFingerprint = { policyVersion: 1, sha256: fingerprint.sha256 };
        tweak?.(p, mode);
      },
    });
    const artifact = loadDarwinArtifact(root);
    const contract = previousSchema2(root);
    contract.toolchain.packageVersion = VERSION;
    contract.toolchain.cargoLockSha256 = sha("cargo-lock");
    return { root, artifact, contract };
  }

  it("migrates schemaVersion 2 to 3 from a native artifact whose v2 check failed", () => {
    const { artifact, contract } = fixture();
    expect(validateDarwinEvidence(contract, artifact, SOURCE_SHA, "fingerprint-migration")).toEqual(
      [],
    );
    // The old reasons still see the runner movement against the v2 contract.
    expect(validateDarwinEvidence(contract, artifact, SOURCE_SHA, "release-bump")).toContain(
      "workspace: runner hashes differ from the contract",
    );
    const evidence = evidenceFor(migrationArgs, artifact);
    expect(evidence.dependencyFingerprint).toEqual({
      policyVersion: 1,
      sha256: fingerprint.sha256,
    });
    expect(evidence.originalCheckerResult.exitCode).toBe(1);
    const { contract: next, gzipMoves } = migrateDarwinFingerprint(
      contract,
      totalsOf(artifact),
      evidence,
      checkout,
    );
    expect(Object.keys(next).slice(0, 6)).toEqual([
      "schemaVersion",
      "metric",
      "normalizationVersion",
      "normalizerSha256",
      "dependencyFingerprint",
      "decisionIssue",
    ]);
    expect(next.schemaVersion).toBe(3);
    expect(next.dependencyFingerprint).toEqual(fingerprint);
    expect(next.runnerSha256).toEqual(newRunners);
    expect(next.toolchain.pnpmLockSha256).toBe(sha("pnpm-lock"));
    expect(next.toolchain.cargoLockSha256).toBe(sha("cargo-lock"));
    expect(next.outcome).toBe("fingerprint-migration");
    expect(next.baselineSourceSha).toBe(SOURCE_SHA);
    expect(next.baselineEvidence).toMatchObject({
      event: "ci-artifact",
      runId: 7,
      artifact: { id: 9 },
      migratedFrom: {
        schemaVersion: 2,
        outcome: contract.outcome,
        baselineSourceSha: contract.baselineSourceSha,
        baselineRunId: contract.baselineEvidence.runId,
      },
      originalCheckerResult: { exitCode: 1 },
    });
    const {
      schemaVersion: _v,
      dependencyFingerprint: _f,
      runnerSha256: _r1,
      toolchain: a,
      baselineSourceSha: _s1,
      outcome: _o1,
      platformStatus,
      baselineEvidence,
      ...rest
    } = next;
    const {
      schemaVersion: _v2,
      runnerSha256: _r,
      toolchain: b,
      baselineSourceSha: _s,
      outcome: _o,
      platformStatus: _p,
      baselineEvidence: _e,
      ...restBefore
    } = contract;
    expect(rest).toEqual(restBefore);
    expect({ ...a, pnpmLockSha256: 0 }).toEqual({ ...b, pnpmLockSha256: 0 });
    expect(platformStatus).toContain("migrated to schemaVersion 3");
    expect(baselineEvidence.note).toContain("cannot accept a run carrying the fingerprint");
    const readme = darwinFingerprintReadmeSection(next, gzipMoves);
    expect(readme).toContain("## Darwin v" + VERSION + " dependency-fingerprint migration");
    expect(readme).toContain("`" + fingerprint.sha256 + "`");
    expect(readme).toContain("exit code 1");
    expect(readme).toContain("LOCK-FINGERPRINT-POLICY.md");
  });

  it("refuses the wrong schema and an artifact without a consistent fingerprint", () => {
    const { root, artifact, contract } = fixture();
    expect(
      validateDarwinEvidence(contractFor(root), artifact, SOURCE_SHA, "fingerprint-migration"),
    ).toContain("contract is not a schemaVersion 2 gzip-identity-normalized v1 contract");
    const { contract: v3 } = migrateDarwinFingerprint(
      contract,
      totalsOf(artifact),
      evidenceFor(migrationArgs, artifact),
      checkout,
    );
    expect(validateDarwinEvidence(v3, artifact, SOURCE_SHA, "fingerprint-migration")).toContain(
      "contract is not a schemaVersion 2 gzip-identity-normalized v1 contract",
    );
    expect(() =>
      migrateDarwinFingerprint(
        v3,
        totalsOf(artifact),
        evidenceFor(migrationArgs, artifact),
        checkout,
      ),
    ).toThrow("needs a schemaVersion 2 contract, found schemaVersion 3");

    const bare = makeArtifact();
    expect(
      validateDarwinEvidence(
        previousSchema2(bare),
        loadDarwinArtifact(bare),
        SOURCE_SHA,
        "fingerprint-migration",
      ),
    ).toContain("workspace: provenance carries no dependencyFingerprint");
    const split = fixture({
      tweak: (p, mode) => {
        if (mode === "packed")
          p.dependencyFingerprint = { policyVersion: 1, sha256: "0".repeat(64) };
      },
    });
    expect(
      validateDarwinEvidence(split.contract, split.artifact, SOURCE_SHA, "fingerprint-migration"),
    ).toContain("packed: dependencyFingerprint differs between workspace and packed");
  });

  it("refuses totals, version, normalizer, Cargo.lock or fingerprint movement", () => {
    const { artifact, contract } = fixture();
    const evidence = evidenceFor(migrationArgs, artifact);
    const totals = totalsOf(artifact);
    const run =
      (c, e = evidence, k = checkout) =>
      () =>
        migrateDarwinFingerprint(c, totals, e, k);
    const grown = structuredClone(contract);
    grown.ceilings.packed.model.gzipNormalized += 1;
    expect(run(grown)).toThrow("gzipNormalized totals changed");
    const raw = structuredClone(contract);
    raw.ceilings.workspace["event-only"].raw -= 1;
    expect(run(raw)).toThrow("raw totals changed");
    expect(
      run({ ...contract, toolchain: { ...contract.toolchain, packageVersion: "9.9.8" } }),
    ).toThrow("changes only the lock pin, but packageVersion differ");
    expect(run({ ...contract, normalizerSha256: "e".repeat(64) })).toThrow(
      "but normalizerSha256 differ",
    );
    expect(run(contract, { ...evidence, cargoLockSha256: sha("other") })).toThrow(
      "but cargoLockSha256 differ",
    );
    expect(
      run(contract, evidence, {
        dependencyFingerprint: { ...fingerprint, sha256: "3".repeat(64) },
      }),
    ).toThrow("artifact dependency fingerprint sha256 differs from this checkout's");
    expect(run(contract, evidence, { dependencyFingerprintError: "DANGLING: x" })).toThrow(
      "checkout dependency fingerprint failed: DANGLING: x",
    );
  });

  for (const reason of ["release-bump", "runtime-identity"]) {
    it(reason + " re-pins a schemaVersion 3 contract without moving its fingerprint", () => {
      const { artifact, contract } = fixture();
      const { contract: v3 } = migrateDarwinFingerprint(
        contract,
        totalsOf(artifact),
        evidenceFor(migrationArgs, artifact),
        checkout,
      );
      v3.toolchain.packageVersion = "9.9.8";
      expect(validateDarwinEvidence(v3, artifact, SOURCE_SHA, reason)).toEqual([]);
      const evidence = evidenceFor({ ...args, reason }, artifact);
      const { contract: next } = repinDarwinContract(
        v3,
        totalsOf(artifact),
        evidence,
        reason,
        checkout,
      );
      expect(next.schemaVersion).toBe(3);
      expect(next.toolchain.packageVersion).toBe(VERSION);
      const {
        toolchain: _a,
        baselineSourceSha: _s,
        outcome: _o,
        platformStatus: _p,
        baselineEvidence: _e,
        normalizerSha256: _n,
        ...rest
      } = next;
      const {
        toolchain: _b,
        baselineSourceSha: _s2,
        outcome: _o2,
        platformStatus: _p2,
        baselineEvidence: _e2,
        normalizerSha256: _n2,
        ...restBefore
      } = v3;
      expect(rest).toEqual(restBefore);
      expect(rest.dependencyFingerprint).toEqual(fingerprint);
      const moved = { dependencyFingerprint: { ...fingerprint, helperSha256: "9".repeat(64) } };
      expect(() => repinDarwinContract(v3, totalsOf(artifact), evidence, reason, moved)).toThrow(
        "cannot move the dependency fingerprint (helperSha256 differ",
      );
      // dependency-graph's allowlisted sha256 stays refused here, naming its reason.
      const graph = {
        dependencyFingerprint: { ...fingerprint, sha256: "8".repeat(64) },
      };
      expect(() =>
        repinDarwinContract(
          v3,
          totalsOf(artifact),
          { ...evidence, dependencyFingerprint: { policyVersion: 1, sha256: "8".repeat(64) } },
          reason,
          graph,
        ),
      ).toThrow(
        "cannot move the dependency fingerprint (sha256 differ from the contract); a reachable dependency change needs `--reason dependency-graph` (#4218)",
      );
      expect(DEPENDENCY_GRAPH_MOVEMENT).toContain("dependencyFingerprint.sha256");
      expect(() =>
        repinDarwinContract(
          v3,
          totalsOf(artifact),
          { ...evidence, dependencyFingerprint: undefined },
          reason,
          checkout,
        ),
      ).toThrow("must carry dependencyFingerprint");
    });
  }

  it("rewrites the v3 schema and fingerprint pins idempotently", () => {
    const repoRoot = join(testDir, "..", "..");
    const source = readFileSync(
      join(repoRoot, "scripts/__tests__/island-size-budget.test.mjs"),
      "utf8",
    );
    const current = asSchemaV2(
      JSON.parse(readFileSync(join(repoRoot, "research/v3-island-size/decision.json"), "utf8")),
    );
    const v3 = {
      ...structuredClone(current),
      schemaVersion: 3,
      dependencyFingerprint: fingerprint,
    };
    const rewritten = rewriteDarwinTestPins(source, v3);
    expect(rewritten).toContain("darwinContract.schemaVersion).toBe(3)");
    expect(rewritten).toContain(JSON.stringify(fingerprint.sha256));
    expect(rewritten).toContain("toolchain.pnpmLockSha256).toBe(");
    expect(rewriteDarwinTestPins(rewritten, v3)).toBe(rewritten);
    const outside = (text) => text.slice(0, text.indexOf('it("records the reviewed Darwin arm64'));
    expect(outside(rewritten)).toBe(outside(source));
    const unpin = (text) => text.replace(/darwinContract\.dependencyFingerprint\.sha256\)/, "x)");
    const unpinned = unpin(source);
    expect(() => rewriteDarwinTestPins(unpinned, v3)).toThrow(/missing Darwin .* pin/);
    // A schemaVersion-2 rewrite never needs or touches the fingerprint pin.
    expect(rewriteDarwinTestPins(unpinned, current)).toBe(
      unpin(rewriteDarwinTestPins(source, current)),
    );
  });

  it("runs end-to-end against a disposable checkout", async () => {
    vi.spyOn(console, "log").mockImplementation(() => {});
    const repo = makeCheckout(roots);
    const fingerprintOf = { policyVersion: 1, sha256: repo.dependencyFingerprint.sha256 };
    const root = makeArtifact({
      version: repo.version,
      tweak: (p) => {
        p.fixtureSha256 = repo.fixtureSha256;
        p.runnerSha256 = repo.runnerSha256;
        p.pnpmLockSha256 = repo.pnpmLockSha256;
        p.cargoLockSha256 = repo.cargoLockSha256;
        p.dependencyFingerprint = fingerprintOf;
      },
    });
    const contract = previousSchema2(root);
    contract.fixtureSha256 = repo.fixtureSha256;
    contract.toolchain.packageVersion = repo.version;
    contract.toolchain.cargoLockSha256 = repo.cargoLockSha256;
    const contractPath = join(repo.root, "research/v3-island-size/decision.json");
    writeFileSync(contractPath, JSON.stringify(contract, null, 2) + "\n");
    const argv = Object.entries({ ...migrationArgs, artifact: root }).flatMap(([k, v]) => [
      "--" + k,
      v,
    ]);
    await main(argv, repo.root);
    const written = JSON.parse(readFileSync(contractPath, "utf8"));
    expect(written.schemaVersion).toBe(3);
    expect(written.dependencyFingerprint).toEqual(repo.dependencyFingerprint);
    expect(written.runnerSha256).toEqual(repo.runnerSha256);
    expect(written.toolchain.pnpmLockSha256).toBe(repo.pnpmLockSha256);
    expect(written.ceilings).toEqual(contract.ceilings);
    const pins = readFileSync(
      join(repo.root, "scripts/__tests__/island-size-budget.test.mjs"),
      "utf8",
    );
    expect(pins).toContain("darwinContract.schemaVersion).toBe(3)");
    expect(pins).toContain(JSON.stringify(repo.dependencyFingerprint.sha256));
    expect(readFileSync(join(repo.root, "research/v3-island-size/README.md"), "utf8")).toContain(
      "dependency-fingerprint migration",
    );
  });

  it("refuses end-to-end an artifact from another checkout whose graph matches", async () => {
    const repo = makeCheckout(roots);
    const fingerprintOf = { policyVersion: 1, sha256: repo.dependencyFingerprint.sha256 };
    const other = makeArtifact({
      version: repo.version,
      tweak: (p) => {
        p.fixtureSha256 = repo.fixtureSha256;
        p.runnerSha256 = repo.runnerSha256;
        p.pnpmLockSha256 = sha("other-lock");
        p.cargoLockSha256 = repo.cargoLockSha256;
        p.dependencyFingerprint = fingerprintOf;
      },
    });
    const contract = previousSchema2(other);
    contract.fixtureSha256 = repo.fixtureSha256;
    contract.toolchain.packageVersion = repo.version;
    contract.toolchain.cargoLockSha256 = repo.cargoLockSha256;
    const contractPath = join(repo.root, "research/v3-island-size/decision.json");
    writeFileSync(contractPath, JSON.stringify(contract, null, 2) + "\n");
    const otherArgv = Object.entries({ ...migrationArgs, artifact: other }).flatMap(([k, v]) => [
      "--" + k,
      v,
    ]);
    await expect(main(otherArgv, repo.root)).rejects.toThrow(
      "artifact pnpm-lock.yaml hash differs from this checkout's pnpm-lock.yaml",
    );
  });
});

// The shared re-pin and its full refusal matrix are covered in
// rebaseline-island-size-linux.test.mjs; these cases prove the Darwin entry
// point reaches the same artifact-less path and rewrites both contracts.
describe("rebaseline-island-size-darwin fingerprint-helper (#4198)", () => {
  const helperArgv = (ref = "HEAD") => ["--reason", "fingerprint-helper", "--base-ref", ref];

  it("takes only --base-ref and refuses every Darwin artifact argument", () => {
    expect(parseArgs(helperArgv("main"))).toEqual({
      reason: "fingerprint-helper",
      "base-ref": "main",
    });
    for (const key of Object.keys(args).filter((key) => key !== "reason")) {
      expect(() => parseArgs([...helperArgv(), "--" + key, args[key]])).toThrow(
        "--reason fingerprint-helper takes no artifact (no native run); remove --" + key,
      );
    }
    for (const key of ["contract", "readme", "force"]) {
      expect(() => parseArgs([...helperArgv(), "--" + key, "x"])).toThrow(
        "--reason fingerprint-helper takes only --base-ref; remove --" + key,
      );
    }
    expect(() => parseArgs(helperArgv().slice(0, 2))).toThrow("required --base-ref");
    expect(() =>
      parseArgs(Object.entries({ ...args, "base-ref": "main" }).flatMap(([k, v]) => ["--" + k, v])),
    ).toThrow("--base-ref is only for --reason fingerprint-helper");
    expect(() => repinDarwinContract(legacy, {}, {}, "fingerprint-helper", {})).toThrow(
      "unknown re-pin --reason fingerprint-helper",
    );
  });

  it("re-pins both contracts end to end, moving only the allowed keys", async () => {
    vi.spyOn(console, "log").mockImplementation(() => {});
    const repo = makeHelperCheckout(roots);
    appendFileSync(repo.helperPath, "// maintenance comment\n");
    const newHelper = sha(readFileSync(repo.helperPath));
    await main(helperArgv(), repo.root);
    for (const [name, file] of [
      ["darwin", "decision.json"],
      ["linux", "decision-linux-x64.json"],
    ]) {
      const written = JSON.parse(
        readFileSync(join(repo.root, "research/v3-island-size", file), "utf8"),
      );
      expect(contractDiffKeys(repo.contracts[name], written)).toEqual(FINGERPRINT_HELPER_MOVEMENT);
      expect(written.dependencyFingerprint.helperSha256).toBe(newHelper);
      expect(written.dependencyFingerprint.sha256).toBe(repo.dependencyFingerprint.sha256);
      expect(JSON.stringify(written.ceilings)).toBe(JSON.stringify(repo.contracts[name].ceilings));
      expect(JSON.stringify(written.baselineEvidence)).toBe(
        JSON.stringify(repo.contracts[name].baselineEvidence),
      );
    }
  });

  it("refuses contracts that disagree on helperSha256 and writes nothing", async () => {
    const repo = makeHelperCheckout(roots);
    appendFileSync(repo.helperPath, "// maintenance comment\n");
    const path = join(repo.root, "research/v3-island-size/decision.json");
    const darwin = structuredClone(repo.contracts.darwin);
    darwin.dependencyFingerprint.helperSha256 = "9".repeat(64);
    writeFileSync(path, JSON.stringify(darwin, null, 2) + "\n");
    await expect(main(helperArgv(), repo.root)).rejects.toThrow(
      /contracts disagree on helperSha256 .*; fix the contract that is wrong first/,
    );
    expect(JSON.parse(readFileSync(path, "utf8"))).toEqual(darwin);
  });
});

// The proof's full refusal matrix is covered in rebaseline-island-size-linux.test.mjs;
// these cases prove the Darwin entry point binds the same evidence and writes
// only the Cargo hash and its evidence.
describe("rebaseline-island-size-darwin cargo-producer-lock (#4214)", () => {
  const lockDir = join(testDir, "fixtures/cargo-producer-lock");
  const oldText = readFileSync(join(lockDir, "old.lock"), "utf8");
  const newText = readFileSync(join(lockDir, "new.lock"), "utf8");
  const OLD = sha(oldText);
  const NEW = sha(newText);
  const ADDED = ["ctor@0.2.9", "zfb-css-node@0.0.0"];
  const fingerprint = {
    policyVersion: 1,
    parser: "yaml@2.9.0",
    helperSha256: "1".repeat(64),
    sha256: "2".repeat(64),
  };
  const checkout = { dependencyFingerprint: fingerprint, dependencyFingerprintError: null };
  const cargoArgs = { ...args, reason: "cargo-producer-lock" };
  const cargoLock = {
    expectedOldSha: OLD,
    expectedNewSha: NEW,
    oldText,
    newText,
    added: ADDED,
    // Test fixture: the verified git source of the old lock (readCargoLockEvidence).
    oldLockSource: { commit: "a".repeat(40), path: "Cargo.lock", sha256: OLD },
  };

  // A schemaVersion-3 contract at the artifact's version and pnpm lock, pinning
  // the old Cargo.lock, and a native artifact measured at the new one.
  function fixture({ tweak } = {}) {
    const root = makeArtifact({
      tweak: (p, mode) => {
        p.cargoLockSha256 = NEW;
        p.dependencyFingerprint = { policyVersion: 1, sha256: fingerprint.sha256 };
        tweak?.(p, mode);
      },
    });
    const artifact = loadDarwinArtifact(root);
    const contract = previousSchema2(root);
    contract.schemaVersion = 3;
    contract.dependencyFingerprint = { ...fingerprint };
    contract.toolchain.packageVersion = VERSION;
    contract.toolchain.pnpmLockSha256 = sha("pnpm-lock");
    contract.toolchain.cargoLockSha256 = OLD;
    return { root, artifact, contract };
  }
  const run = (contract, artifact, { evidence, check = checkout, lock = cargoLock } = {}) =>
    transitionDarwinCargoLock(
      contract,
      totalsOf(artifact),
      evidence ?? evidenceFor(cargoArgs, artifact),
      check,
      lock,
    );

  it("keeps native platform, exact source and total gates for the reviewed non-additive lock", () => {
    const lock = reviewedCargoLock();
    const { artifact, contract } = fixture({
      tweak: (p) => {
        p.cargoLockSha256 = lock.expectedNewSha;
      },
    });
    contract.toolchain.cargoLockSha256 = lock.expectedOldSha;
    expect(validateDarwinEvidence(contract, artifact, SOURCE_SHA, "cargo-producer-lock")).toEqual(
      [],
    );
    const { contract: next, gzipMoves } = run(contract, artifact, { lock });
    expect(next.ceilings).toEqual(contract.ceilings);
    expect(next.allowance).toEqual(contract.allowance);
    expect(next.platformStatus).toContain("reviewed non-additive");
    expect(darwinCargoProducerLockReadmeSection(next, gzipMoves)).toContain("46 changed records");
    const other = fixture({
      tweak: (p) => {
        p.cargoLockSha256 = lock.expectedNewSha;
        p.platform = { os: "linux", arch: "x64" };
      },
    });
    expect(
      validateDarwinEvidence(contract, other.artifact, SOURCE_SHA, "cargo-producer-lock").length,
    ).toBeGreaterThan(0);
    expect(
      validateDarwinEvidence(contract, artifact, "f".repeat(40), "cargo-producer-lock").length,
    ).toBeGreaterThan(0);
    const grown = structuredClone(contract);
    grown.ceilings.packed.model.gzipNormalized += 1;
    expect(() => run(grown, artifact, { lock })).toThrow(/gzipNormalized totals changed/);
  });

  it("moves only the Cargo hash and its evidence", () => {
    const { artifact, contract } = fixture();
    expect(validateDarwinEvidence(contract, artifact, SOURCE_SHA, "cargo-producer-lock")).toEqual(
      [],
    );
    const { contract: next, gzipMoves } = run(contract, artifact);
    expect(next.outcome).toBe("cargo-producer-lock-transition");
    expect(next.toolchain).toEqual({ ...contract.toolchain, cargoLockSha256: NEW });
    expect(next.ceilings).toEqual(contract.ceilings);
    expect(next.dependencyFingerprint).toEqual(fingerprint);
    expect(next.baselineEvidence).toMatchObject({
      event: "ci-artifact",
      runId: 7,
      artifact: { id: 9 },
      originalCheckerResult: { exitCode: 1 },
    });
    expect(next.baselineEvidence.cargoTransition).toEqual({
      oldSha: OLD,
      newSha: NEW,
      added: ADDED,
      changed: [],
      removed: [],
      oldLockSource: cargoLock.oldLockSource,
    });
    expect(Object.keys(next.baselineEvidence.repinnedFrom)).toEqual(REPINNED_FROM_KEYS);
    expect(next.baselineEvidence.repinnedFrom).toEqual({
      schemaVersion: 3,
      outcome: contract.outcome,
      packageVersion: VERSION,
      baselineSourceSha: contract.baselineSourceSha,
      baselineRunId: contract.baselineEvidence.runId,
      contract: PREVIOUS,
    });
    for (const key of contractDiffKeys(contract, next)) {
      expect(
        CARGO_PRODUCER_LOCK_MOVEMENT.some(
          (allowed) => key === allowed || key.startsWith(allowed + "."),
        ),
      ).toBe(true);
    }
    expect(next.platformStatus).toContain("only the Cargo.lock hash moved");
    const readme = darwinCargoProducerLockReadmeSection(next, gzipMoves);
    expect(readme).toContain("## Darwin v" + VERSION + " cargo-producer-lock transition");
    expect(readme).toContain("`" + OLD + "` to `" + NEW + "`");
    expect(readme).toContain("exit code 1");
    expect(readme).toContain(
      "proved against `Cargo.lock` at commit `" + "a".repeat(40) + "` (SHA-256 `" + OLD + "`).",
    );
    expect(() =>
      run(contract, artifact, {
        evidence: { ...evidenceFor(cargoArgs, artifact), previousContract: undefined },
      }),
    ).toThrow("a re-pin needs the previous contract reference");
    expect(() =>
      run(contract, artifact, { lock: { ...cargoLock, oldLockSource: "old.lock" } }),
    ).toThrow("the old Cargo.lock source must be its verified git blob");
  });

  it("refuses schemaVersion 2, moved totals, version, pnpm lock or fingerprint", () => {
    const { artifact, contract } = fixture();
    const v2 = asSchemaV2(contract);
    expect(validateDarwinEvidence(v2, artifact, SOURCE_SHA, "cargo-producer-lock")).toContain(
      "contract is not a schemaVersion 3 gzip-identity-normalized v1 contract",
    );
    expect(() => run(v2, artifact)).toThrow(
      "--reason cargo-producer-lock needs a schemaVersion 3 contract, found schemaVersion 2",
    );
    const grown = structuredClone(contract);
    grown.ceilings.packed.model.gzipNormalized += 1;
    expect(() => run(grown, artifact)).toThrow("gzipNormalized totals changed");
    const raw = structuredClone(contract);
    raw.ceilings.workspace["event-only"].raw -= 1;
    expect(() => run(raw, artifact)).toThrow("raw totals changed");
    for (const [key, value] of [
      ["packageVersion", "9.9.8"],
      ["pnpmLockSha256", sha("other-pnpm")],
    ]) {
      const moved = structuredClone(contract);
      moved.toolchain[key] = value;
      expect(() => run(moved, artifact)).toThrow(
        "allows only Cargo.lock to move, but " + key + " differ",
      );
    }
    expect(() => run({ ...contract, normalizerSha256: "e".repeat(64) }, artifact)).toThrow(
      "but normalizerSha256 differ",
    );
    expect(() =>
      run(contract, artifact, {
        check: { dependencyFingerprint: { ...fingerprint, helperSha256: "9".repeat(64) } },
      }),
    ).toThrow("but dependencyFingerprint.helperSha256 differ");
    expect(() =>
      run(contract, artifact, {
        check: { dependencyFingerprint: { ...fingerprint, sha256: "3".repeat(64) } },
      }),
    ).toThrow("artifact dependency fingerprint sha256 differs from this checkout's");
  });

  it("refuses a non-additive or unallowlisted lock move and a wrong SHA", () => {
    const { artifact, contract } = fixture();
    expect(() =>
      run(contract, artifact, { lock: { ...cargoLock, added: ["ctor@0.2.9"] } }),
    ).toThrow("unexpected additions zfb-css-node@0.0.0");
    expect(() =>
      run(contract, artifact, { lock: { ...cargoLock, expectedOldSha: "1".repeat(64) } }),
    ).toThrow("the contract pins Cargo.lock " + OLD + ", not --expected-old-cargo-sha");
    const other = fixture({ tweak: (p) => (p.cargoLockSha256 = sha("other-cargo")) });
    expect(() => run(other.contract, other.artifact)).toThrow(
      "the artifact measured Cargo.lock " + sha("other-cargo") + ", not --expected-new-cargo-sha",
    );
  });

  it("takes the Cargo arguments for cargo-producer-lock alone", () => {
    const cli = {
      ...cargoArgs,
      "expected-old-cargo-sha": OLD,
      "expected-new-cargo-sha": NEW,
      "old-cargo-ref": "main",
      "added-packages": ADDED.join(","),
    };
    const argv = (values) => Object.entries(values).flatMap(([k, v]) => ["--" + k, v]);
    expect(parseArgs(argv(cli))).toMatchObject(cli);
    expect(parseArgs(argv({ ...cli, "old-cargo-lock": "old.lock" }))).toMatchObject({
      "old-cargo-lock": "old.lock",
    });
    for (const key of ["added-packages", "old-cargo-ref"]) {
      const { [key]: _drop, ...without } = cli;
      expect(() => parseArgs(argv(without))).toThrow("required --" + key);
    }
    expect(() => parseArgs(argv({ ...cli, "old-cargo-ref": "main:../Cargo.lock" }))).toThrow(
      "--old-cargo-ref path must not contain a .. segment",
    );
    expect(parseArgs(argv({ ...cli, "previous-contract-ref": "main" }))).toMatchObject({
      "previous-contract-ref": "main",
    });
    expect(() =>
      parseArgs(
        argv({ ...args, reason: "fingerprint-migration", "previous-contract-ref": "main" }),
      ),
    ).toThrow(
      "--previous-contract-ref is only for --reason release-bump, runtime-identity, cargo-producer-lock, dependency-graph",
    );
    expect(() =>
      parseArgs(argv({ ...args, reason: "release-bump", "old-cargo-lock": "old.lock" })),
    ).toThrow("--old-cargo-lock is only for --reason cargo-producer-lock");
  });
});

// End-to-end CLI runs against one git-backed test-fixture checkout, built once
// and restored after every case: its only commit holds the OLD Cargo.lock and
// the contract pinning it (plus a copy for --contract); the working tree
// carries the NEW lock. The shared refusal logic is also covered on Linux.
describe("rebaseline-island-size-darwin git-backed provenance (#4219)", () => {
  const gitRoots = [];
  const lockDir = join(testDir, "fixtures/cargo-producer-lock");
  const oldText = readFileSync(join(lockDir, "old.lock"), "utf8");
  const newText = readFileSync(join(lockDir, "new.lock"), "utf8");
  const OLD = sha(oldText);
  const NEW = sha(newText);
  const ADDED = ["ctor@0.2.9", "zfb-css-node@0.0.0"];
  const CONTRACT = "research/v3-island-size/decision.json";
  const COPY = "research/v3-island-size/decision-copy.json";
  const TEST = "scripts/__tests__/island-size-budget.test.mjs";
  const README = "research/v3-island-size/README.md";
  const OUTPUTS = [CONTRACT, COPY, TEST, README];
  let repo;
  let artifactDir;
  let pinned;
  let ambiguous;
  let saved;
  const at = (relative) => join(repo.root, relative);
  const read = (relative) => readFileSync(at(relative));
  const written = (file = CONTRACT) => JSON.parse(read(file).toString("utf8"));

  beforeAll(() => {
    repo = makeGitCheckout(gitRoots, {
      oldCargoLock: oldText,
      newCargoLock: newText,
      prepare: (checkout) => {
        artifactDir = makeArtifact({
          version: checkout.version,
          tweak: (p) => {
            p.fixtureSha256 = checkout.fixtureSha256;
            p.runnerSha256 = checkout.runnerSha256;
            p.pnpmLockSha256 = checkout.pnpmLockSha256;
            p.cargoLockSha256 = checkout.cargoLockSha256;
            p.dependencyFingerprint = {
              policyVersion: 1,
              sha256: checkout.dependencyFingerprint.sha256,
            };
          },
        });
        // Kept for the whole describe, not removed by the per-test cleanup.
        gitRoots.push(roots.splice(roots.indexOf(artifactDir), 1)[0]);
        pinned = previousSchema2(artifactDir);
        pinned.schemaVersion = 3;
        pinned.dependencyFingerprint = { ...checkout.dependencyFingerprint };
        pinned.fixtureSha256 = checkout.fixtureSha256;
        pinned.runnerSha256 = checkout.runnerSha256;
        pinned.toolchain.packageVersion = checkout.version;
        pinned.toolchain.pnpmLockSha256 = checkout.pnpmLockSha256;
        pinned.toolchain.cargoLockSha256 = OLD;
        for (const file of [CONTRACT, COPY]) {
          writeFileSync(join(checkout.root, file), JSON.stringify(pinned, null, 2) + "\n");
        }
      },
    });
    repo.git("branch", "dup");
    repo.git("tag", "dup");
    ambiguous = addAmbiguousCommitPrefix(repo.root);
    saved = Object.fromEntries([...OUTPUTS, "Cargo.lock"].map((file) => [file, read(file)]));
  });
  afterEach(() => {
    for (const [file, bytes] of Object.entries(saved)) writeFileSync(at(file), bytes);
    rmSync(at("old.lock"), { force: true });
    vi.restoreAllMocks();
  });
  afterAll(() => {
    for (const dir of gitRoots.splice(0)) rmSync(dir, { recursive: true, force: true });
  });

  const cargoArgv = (extra = {}) =>
    Object.entries({
      ...args,
      artifact: artifactDir,
      reason: "cargo-producer-lock",
      "expected-old-cargo-sha": OLD,
      "expected-new-cargo-sha": NEW,
      "old-cargo-ref": "HEAD",
      "added-packages": ADDED.join(","),
      ...extra,
    }).flatMap(([key, value]) => ["--" + key, value]);
  const quiet = () => {
    vi.spyOn(console, "log").mockImplementation(() => {});
    return vi.spyOn(console, "error").mockImplementation(() => {});
  };
  const NOTICE =
    "previous contract bytes are not at HEAD; recorded digest only — no retrievable citation";

  it("records the old lock's git blob and the HEAD citation of the previous contract", async () => {
    const notices = quiet();
    writeFileSync(at("notes.txt"), "test fixture: unrelated untracked file\n");
    try {
      await main(cargoArgv(), repo.root);
    } finally {
      rmSync(at("notes.txt"), { force: true });
    }
    const next = written();
    expect(next.toolchain).toEqual({ ...pinned.toolchain, cargoLockSha256: NEW });
    expect(next.baselineEvidence.cargoTransition.oldLockSource).toEqual({
      commit: repo.commit,
      path: "Cargo.lock",
      sha256: OLD,
    });
    expect(Object.keys(next.baselineEvidence.repinnedFrom)).toEqual(REPINNED_FROM_KEYS);
    expect(next.baselineEvidence.repinnedFrom).toEqual({
      schemaVersion: 3,
      outcome: pinned.outcome,
      packageVersion: pinned.toolchain.packageVersion,
      baselineSourceSha: pinned.baselineSourceSha,
      baselineRunId: pinned.baselineEvidence.runId,
      contract: { sha256: sha(saved[CONTRACT]), commit: repo.commit, path: CONTRACT },
    });
    expect(notices).not.toHaveBeenCalled();
    expect(read(TEST).toString("utf8")).toContain(JSON.stringify(NEW));
    const readme = read(README).toString("utf8");
    expect(readme).toContain("`" + OLD + "` to `" + NEW + "`");
    expect(readme).toContain(
      "proved against `Cargo.lock` at commit `" + repo.commit + "` (SHA-256 `" + OLD + "`).",
    );
  });

  it("cites a custom --contract path, and only its digest outside the checkout", async () => {
    const notices = quiet();
    await main(cargoArgv({ contract: at(COPY) }), repo.root);
    expect(written(COPY).baselineEvidence.repinnedFrom.contract).toEqual({
      sha256: sha(saved[COPY]),
      commit: repo.commit,
      path: COPY,
    });
    expect(read(CONTRACT).equals(saved[CONTRACT])).toBe(true);
    expect(notices).not.toHaveBeenCalled();

    const outside = join(mkdtempSync(join(tmpdir(), "zfb-darwin-contract-")), "decision.json");
    gitRoots.push(dirname(outside));
    writeFileSync(outside, saved[CONTRACT]);
    for (const file of [CONTRACT, TEST, README]) writeFileSync(at(file), saved[file]);
    await main(cargoArgv({ contract: outside }), repo.root);
    expect(
      JSON.parse(readFileSync(outside, "utf8")).baselineEvidence.repinnedFrom.contract,
    ).toEqual({ sha256: sha(saved[CONTRACT]) });
    expect(notices.mock.calls.flat().join("\n")).toContain(NOTICE);
  });

  // Each refusal leaves the contract, the budget test and the README byte-identical.
  it.each([
    ["an option-like ref", { "old-cargo-ref": "-x" }, null, /--old-cargo-ref must name a git ref/],
    [
      "an unresolvable ref",
      { "old-cargo-ref": "no-such-ref" },
      null,
      /--old-cargo-ref no-such-ref does not resolve to exactly one commit in /,
    ],
    [
      "an ambiguous short SHA",
      () => ({ "old-cargo-ref": ambiguous }),
      null,
      /short object ID [0-9a-f]{4} is ambiguous/,
    ],
    [
      "an ambiguous branch and tag name",
      { "old-cargo-ref": "dup" },
      null,
      /refname 'dup' is ambiguous/,
    ],
    [
      "a tree ref",
      { "old-cargo-ref": "HEAD^{tree}" },
      null,
      /the object dereferences to tree type/,
    ],
    [
      "a blob ref",
      () => ({ "old-cargo-ref": repo.git("rev-parse", "HEAD:Cargo.lock") }),
      null,
      /the object dereferences to blob type/,
    ],
    ["an empty path", { "old-cargo-ref": "HEAD:" }, null, /--old-cargo-ref has an empty path/],
    [
      "an absolute path",
      { "old-cargo-ref": "HEAD:/Cargo.lock" },
      null,
      /path must be relative to the repository root/,
    ],
    [
      "a .. path",
      { "old-cargo-ref": "HEAD:../Cargo.lock" },
      null,
      /must not contain a \.\. segment/,
    ],
    [
      "a missing blob",
      { "old-cargo-ref": "HEAD:no-such.lock" },
      null,
      /[0-9a-f]{40}:no-such\.lock does not exist/,
    ],
    [
      "a blob whose hash is not --expected-old-cargo-sha",
      { "old-cargo-ref": "HEAD:pnpm-lock.yaml" },
      null,
      /[0-9a-f]{40}:pnpm-lock\.yaml hashes to [0-9a-f]{64}, not --expected-old-cargo-sha/,
    ],
    [
      "a corroboration file that differs from the blob",
      () => ({ "old-cargo-lock": at("old.lock") }),
      () => writeFileSync(at("old.lock"), newText),
      /--old-cargo-lock differs from [0-9a-f]{40}:Cargo\.lock/,
    ],
    [
      "an explicit --previous-contract-ref that is not the input bytes",
      { "previous-contract-ref": "HEAD" },
      () => writeFileSync(at(CONTRACT), JSON.stringify(pinned, null, 4) + "\n"),
      /--previous-contract-ref HEAD: [0-9a-f]{40}:research\/v3-island-size\/decision\.json is not byte-identical/,
    ],
    [
      "an unresolvable --previous-contract-ref (no HEAD fallback)",
      { "previous-contract-ref": "no-such-ref" },
      null,
      /--previous-contract-ref no-such-ref does not resolve to exactly one commit/,
    ],
    [
      "a missing test-pin anchor",
      {},
      () =>
        writeFileSync(
          at(TEST),
          saved[TEST].toString("utf8").replace("records the reviewed Darwin arm64 ceilings", "x"),
        ),
      /Darwin ceiling test not found/,
    ],
    [
      "a missing Cargo pin inside the anchored test",
      {},
      () =>
        writeFileSync(
          at(TEST),
          saved[TEST]
            .toString("utf8")
            .replace("darwinContract.toolchain.cargoLockSha256)", "darwinContract.x)"),
        ),
      /missing Darwin .*cargoLockSha256.* pin in /,
    ],
  ])("refuses %s and writes nothing", async (_name, extra, setup, message) => {
    quiet();
    setup?.();
    const before = OUTPUTS.map(read);
    await expect(
      main(cargoArgv(typeof extra === "function" ? extra() : extra), repo.root),
    ).rejects.toThrow(message);
    OUTPUTS.forEach((file, i) => expect(read(file).equals(before[i])).toBe(true));
  });

  it("resolves refs in the supplied linked worktree, at its detached HEAD", async () => {
    quiet();
    const dir = mkdtempSync(join(tmpdir(), "zfb-darwin-linked-"));
    gitRoots.push(dir);
    const tree = join(dir, "wt");
    repo.git("worktree", "add", "-q", "--detach", tree, "HEAD");
    try {
      const linked = gitIn(tree);
      writeFileSync(join(tree, "notes.txt"), "test fixture: linked-worktree commit\n");
      linked("add", "notes.txt");
      linked("commit", "-qm", "test fixture: linked worktree");
      const head = linked("rev-parse", "HEAD");
      expect(head).not.toBe(repo.commit);
      writeFileSync(join(tree, "Cargo.lock"), newText);
      await main(cargoArgv(), tree);
      const next = JSON.parse(readFileSync(join(tree, CONTRACT), "utf8"));
      expect(next.baselineEvidence.cargoTransition.oldLockSource.commit).toBe(head);
      expect(next.baselineEvidence.repinnedFrom.contract).toEqual({
        sha256: sha(saved[CONTRACT]),
        commit: head,
        path: CONTRACT,
      });
      expect(read(CONTRACT).equals(saved[CONTRACT])).toBe(true);
    } finally {
      repo.git("worktree", "remove", "--force", tree);
    }
  });

  it("records only the digest, with a notice, for a reformatted contract", async () => {
    const notices = quiet();
    const reformatted = JSON.stringify(pinned, null, 4) + "\n";
    writeFileSync(at(CONTRACT), reformatted);
    await main(cargoArgv(), repo.root);
    expect(written().baselineEvidence.repinnedFrom.contract).toEqual({ sha256: sha(reformatted) });
    expect(notices.mock.calls.flat().join("\n")).toContain(NOTICE);
  });
});

// End-to-end CLI runs against one test-fixture checkout — synthetic prior
// graph, not native evidence: its pnpm-lock.yaml has one reachable snapshot
// moved, both contracts pin the real lock's (prior) fingerprint, and each
// platform's artifact was measured at the checkout carrying the new graph.
// The shared refusal matrix is covered in rebaseline-island-size-linux.test.mjs.
describe("rebaseline-island-size-darwin dependency-graph and paired staging (#4218)", () => {
  const graphRoots = [];
  const DARWIN = "research/v3-island-size/decision.json";
  const LINUX = "research/v3-island-size/decision-linux-x64.json";
  const TEST = "scripts/__tests__/island-size-budget.test.mjs";
  const README = "research/v3-island-size/README.md";
  const INVENTORY = "research/v3-island-size/lock-fingerprint-inventory.json";
  const OUTPUTS = [DARWIN, LINUX, TEST, README, INVENTORY];
  const LINUX_SOURCE = "b".repeat(40);
  let repo;
  let darwinArtifact;
  let linuxArtifact;
  let darwinPinned;
  let linuxPinned;
  let movedLock;
  let saved;
  const at = (relative) => join(repo.root, relative);
  const read = (relative) => readFileSync(at(relative));
  const contractOf = (file) => JSON.parse(read(file).toString("utf8"));
  const contractText = (value) => JSON.stringify(value, null, 2) + "\n";
  const tempDir = (prefix) => {
    const dir = mkdtempSync(join(tmpdir(), prefix));
    graphRoots.push(dir);
    return dir;
  };

  // A native Darwin artifact measured at the checkout; `tweak` adjusts provenance.
  function darwinArtifactAt(tweak) {
    const dir = makeArtifact({
      version: repo.version,
      tweak: (p, mode) => {
        p.fixtureSha256 = repo.fixtureSha256;
        p.runnerSha256 = repo.runnerSha256;
        p.pnpmLockSha256 = repo.pnpmLockSha256;
        p.cargoLockSha256 = repo.cargoLockSha256;
        p.dependencyFingerprint = { policyVersion: 1, sha256: repo.dependencyFingerprint.sha256 };
        tweak?.(p, mode);
      },
    });
    // Kept for the whole describe, not removed by the per-test cleanup.
    graphRoots.push(roots.splice(roots.indexOf(dir), 1)[0]);
    return dir;
  }

  beforeAll(() => {
    const version = JSON.parse(
      readFileSync(join(testDir, "../../packages/zfb/package.json"), "utf8"),
    ).version;
    const moved = moveReachableSnapshot(
      readFileSync(join(testDir, "../../pnpm-lock.yaml"), "utf8"),
      version,
    );
    movedLock = moved.lockText;
    repo = makeGraphCheckout(graphRoots, { pnpmLock: movedLock });
    darwinArtifact = darwinArtifactAt();
    linuxArtifact = tempDir("zfb-graph-linux-artifact-");
    const liveOf = (file) => JSON.parse(readFileSync(join(testDir, "../..", file), "utf8"));
    writeRuns(linuxArtifact, Object.keys(liveOf(LINUX).ceilings.workspace), (mode) => ({
      sourceSha: LINUX_SOURCE,
      mode,
      platform: { os: "linux", arch: "x64" },
      fixtureVersion: liveOf(LINUX).fixtureVersion,
      fixtureSha256: repo.fixtureSha256,
      runnerSha256: repo.runnerSha256,
      packageVersion: repo.version,
      runtimePackageVersion: repo.version,
      honoPackageVersion: liveOf(LINUX).toolchain.honoVersion,
      esbuildVersion: liveOf(LINUX).toolchain.esbuildVersion,
      nodeVersion: liveOf(LINUX).toolchain.nodeVersion,
      zlibVersion: liveOf(LINUX).toolchain.zlibVersion,
      compression: liveOf(LINUX).compression,
      repeats: 2,
      pnpmLockSha256: repo.pnpmLockSha256,
      dependencyFingerprint: { policyVersion: 1, sha256: repo.dependencyFingerprint.sha256 },
      cargoLockSha256: repo.cargoLockSha256,
    }));
    const pin = (contract, artifactDir) => {
      contract.ceilings = ceilingsOf(artifactDir);
      contract.fixtureSha256 = { ...repo.fixtureSha256 };
      contract.runnerSha256 = { ...repo.runnerSha256 };
      contract.toolchain.packageVersion = repo.version;
      contract.toolchain.pnpmLockSha256 = repo.priorPnpmLockSha256;
      contract.toolchain.cargoLockSha256 = repo.cargoLockSha256;
      contract.dependencyFingerprint = { ...repo.priorFingerprint };
      return contract;
    };
    darwinPinned = pin(liveOf(DARWIN), darwinArtifact);
    darwinPinned.fixtureVersion = legacy.fixtureVersion;
    darwinPinned.compression = legacy.compression;
    darwinPinned.normalizerSha256 = normalizerSha256();
    for (const key of ["nodeVersion", "zlibVersion", "esbuildVersion", "honoVersion"]) {
      darwinPinned.toolchain[key] = legacy.toolchain[key];
    }
    linuxPinned = pin(liveOf(LINUX), linuxArtifact);
    writeFileSync(at(DARWIN), contractText(darwinPinned));
    writeFileSync(at(LINUX), contractText(linuxPinned));
    saved = Object.fromEntries(OUTPUTS.map((file) => [file, read(file)]));
  });
  afterEach(() => {
    for (const [file, bytes] of Object.entries(saved)) writeFileSync(at(file), bytes);
    vi.restoreAllMocks();
  });
  afterAll(() => {
    for (const dir of graphRoots.splice(0)) rmSync(dir, { recursive: true, force: true });
  });

  const darwinArgv = (extra = {}) =>
    Object.entries({
      ...args,
      artifact: darwinArtifact,
      reason: "dependency-graph",
      stage: join(tempDir("zfb-graph-stage-"), "stage"),
      ...extra,
    })
      .filter(([, value]) => value !== undefined)
      .flatMap(([key, value]) => ["--" + key, value]);
  const linuxArgv = (extra = {}) =>
    Object.entries({
      artifact: linuxArtifact,
      "run-id": "71",
      "job-id": "72",
      // Linux measured the PR merge source of the probe head Darwin measured natively.
      "head-sha": SOURCE_SHA,
      "source-sha": LINUX_SOURCE,
      reason: "dependency-graph",
      ...extra,
    })
      .filter(([, value]) => value !== undefined)
      .flatMap(([key, value]) => ["--" + key, value]);
  const quiet = () => {
    vi.spyOn(console, "log").mockImplementation(() => {});
    vi.spyOn(console, "error").mockImplementation(() => {});
  };
  const unchanged = (before) =>
    OUTPUTS.forEach((file, i) => expect(read(file).equals(before[i])).toBe(true));
  const live = () => OUTPUTS.map((file) => saved[file]);

  it("re-pins only the fingerprint sha256 end to end and rewrites the Darwin pins", async () => {
    quiet();
    const stage = join(tempDir("zfb-graph-stage-"), "stage");
    await main(darwinArgv({ stage }), repo.root);
    unchanged(live());
    landStaged(stage, repo.root);
    const next = contractOf(DARWIN);
    const moved = contractDiffKeys(darwinPinned, next);
    expect(moved.filter((key) => !key.startsWith("baselineEvidence."))).toEqual([
      "baselineSourceSha",
      "dependencyFingerprint.sha256",
      "outcome",
      "platformStatus",
      "toolchain.pnpmLockSha256",
    ]);
    expect(() =>
      assertAllowedMovement("dependency-graph", DEPENDENCY_GRAPH_MOVEMENT, darwinPinned, next),
    ).not.toThrow();
    expect(next.dependencyFingerprint.sha256).toBe(repo.dependencyFingerprint.sha256);
    expect(next.toolchain.pnpmLockSha256).toBe(repo.pnpmLockSha256);
    expect(next.outcome).toBe("dependency-graph-repin");
    expect(next.baselineSourceSha).toBe(SOURCE_SHA);
    expect(next.baselineEvidence).toMatchObject({
      event: "ci-artifact",
      runId: 7,
      artifact: { id: 9 },
      originalCheckerResult: { exitCode: 1 },
      fingerprintTransition: {
        policyVersion: 1,
        oldSha256: repo.priorFingerprint.sha256,
        newSha256: repo.dependencyFingerprint.sha256,
      },
    });
    expect(Object.keys(next.baselineEvidence.repinnedFrom)).toEqual(REPINNED_FROM_KEYS);
    expect(next.platformStatus).toContain(
      "equal totals alone do not prove byte-identical product code",
    );
    expect(read(LINUX).equals(saved[LINUX])).toBe(true);
    expect(read(INVENTORY).equals(helperExplainBytes(repo.root))).toBe(true);
    expect(read(TEST).toString("utf8")).toBe(
      rewriteDarwinTestPins(saved[TEST].toString("utf8"), next),
    );
    expect(read(TEST).toString("utf8")).toContain(
      JSON.stringify(repo.dependencyFingerprint.sha256),
    );
    const readme = read(README).toString("utf8");
    expect(readme.startsWith(saved[README].toString("utf8"))).toBe(true);
    expect(readme).toContain("Linux x64 moves in the same paired change");
    expect(readme).toContain("## Darwin v" + repo.version + " dependency-graph re-pin");
    expect(readme).toContain(
      "`" + repo.priorFingerprint.sha256 + "` to `" + repo.dependencyFingerprint.sha256 + "`",
    );
  });

  it("refuses an unchanged fingerprint and a moved helperSha256 in the pure re-pin", () => {
    const artifact = loadDarwinArtifact(darwinArtifact);
    const evidence = evidenceFor({ ...args, reason: "dependency-graph" }, artifact);
    const checkout = { dependencyFingerprint: repo.dependencyFingerprint };
    const run =
      (contract, check = checkout) =>
      () =>
        repinDarwinDependencyGraph(contract, totalsOf(artifact), evidence, check);
    expect(run({ ...darwinPinned, dependencyFingerprint: repo.dependencyFingerprint })).toThrow(
      "--reason dependency-graph: the contract already pins this fingerprint; nothing to re-pin",
    );
    expect(
      run({
        ...darwinPinned,
        dependencyFingerprint: {
          ...darwinPinned.dependencyFingerprint,
          helperSha256: "1".repeat(64),
        },
      }),
    ).toThrow(/but helperSha256 differ from the contract; .*`fingerprint-policy` \(#4218\)/);
    expect(run(asSchemaV2(darwinPinned))).toThrow(
      "--reason dependency-graph needs a schemaVersion 3 contract, found schemaVersion 2",
    );
    expect(run({ ...darwinPinned, allowance: { raw: 0, gzipNormalized: 1 } })).toThrow(
      "moves only dependencyFingerprint.sha256, but allowance differ",
    );
    expect(
      run(darwinPinned, {
        dependencyFingerprint: { ...repo.dependencyFingerprint, sha256: "3".repeat(64) },
      }),
    ).toThrow("artifact dependency fingerprint sha256 differs from this checkout's");
  });

  const edit = (file, change) => () => {
    const contract = structuredClone(file === DARWIN ? darwinPinned : linuxPinned);
    change(contract);
    writeFileSync(at(file), contractText(contract));
  };
  it.each([
    [
      "an unchanged sha256",
      edit(DARWIN, (c) => (c.dependencyFingerprint = { ...repo.dependencyFingerprint })),
      {},
      "the contract already pins this fingerprint; nothing to re-pin",
    ],
    [
      "raw +1",
      edit(DARWIN, (c) => (c.ceilings.workspace.model.raw -= 1)),
      {},
      "raw totals changed (workspace/model)",
    ],
    [
      "gzipNormalized -1",
      edit(DARWIN, (c) => (c.ceilings.packed.model.gzipNormalized += 1)),
      {},
      "gzipNormalized totals changed (packed/model -1)",
    ],
    [
      "a changed Cargo.lock",
      edit(DARWIN, (c) => (c.toolchain.cargoLockSha256 = "0".repeat(64))),
      {},
      "--reason dependency-graph cannot move Cargo.lock",
    ],
    [
      "a changed package version",
      edit(DARWIN, (c) => (c.toolchain.packageVersion = "0.0.1")),
      {},
      "but packageVersion differ from the contract",
    ],
    [
      "a Hono bump",
      edit(DARWIN, (c) => (c.toolchain.honoVersion = "0.0.0-test")),
      {},
      "honoPackageVersion differs from the contract",
    ],
    [
      "a schemaVersion 2 contract",
      edit(DARWIN, (c) => {
        delete c.dependencyFingerprint;
        c.schemaVersion = 2;
      }),
      {},
      "contract is not a schemaVersion 3 gzip-identity-normalized v1 contract",
    ],
    [
      "a sidecar whose results were edited",
      () => {
        const path = join(darwinArtifact, "real-packed", NORMALIZED_SIDECAR);
        const sidecar = JSON.parse(readFileSync(path, "utf8"));
        sidecar.results.model.shipped.gzipNormalized -= 1;
        writeFileSync(path, JSON.stringify(sidecar));
      },
      {},
      "real-packed: measurement.normalized.json results disagree with the recomputation from the saved bytes",
    ],
    [
      "a sidecar whose normalizer.sha256 was tampered",
      () => {
        const path = join(darwinArtifact, "real-packed", NORMALIZED_SIDECAR);
        const sidecar = JSON.parse(readFileSync(path, "utf8"));
        sidecar.normalizer.sha256 = "0".repeat(64);
        writeFileSync(path, JSON.stringify(sidecar));
      },
      {},
      "real-packed: measurement.normalized.json normalizer.sha256 differs from the current normalizer; refusing",
    ],
    [
      "a --source-sha that is not the artifact's",
      null,
      { "source-sha": "d".repeat(40) },
      "source SHA " + SOURCE_SHA + " is not " + "d".repeat(40),
    ],
    [
      "a cargo-only flag",
      null,
      { "old-cargo-ref": "HEAD" },
      "--old-cargo-ref is only for --reason cargo-producer-lock",
    ],
    [
      "a missing Darwin fingerprint pin",
      () =>
        writeFileSync(
          at(TEST),
          saved[TEST]
            .toString("utf8")
            .replace("darwinContract.dependencyFingerprint.sha256)", "darwinContract.x)"),
        ),
      {},
      /missing Darwin .*dependencyFingerprint.* pin in /,
    ],
    [
      "--contract with --stage",
      null,
      { stage: join(tmpdir(), "zfb-unused-stage"), contract: "x.json" },
      "--stage stages the checkout's own files; remove --contract",
    ],
  ])("refuses %s end to end and writes nothing", async (_name, setup, extra, message) => {
    quiet();
    const sidecarPath = join(darwinArtifact, "real-packed", NORMALIZED_SIDECAR);
    const sidecarBytes = readFileSync(sidecarPath);
    setup?.();
    const before = OUTPUTS.map(read);
    try {
      await expect(main(darwinArgv(extra), repo.root)).rejects.toThrow(message);
    } finally {
      writeFileSync(sidecarPath, sidecarBytes);
    }
    unchanged(before);
  });

  it("refuses to run without --stage, in parseArgs, before reading or writing anything", async () => {
    const message =
      "--reason dependency-graph re-pins both platforms together and only stages: pass --stage <dir> for Linux x64 and Darwin arm64 into the same directory, then --apply <dir> (nothing was written)";
    expect(() => parseArgs(darwinArgv({ stage: undefined }))).toThrow(message);
    const before = OUTPUTS.map(read);
    await expect(main(darwinArgv({ stage: undefined }), repo.root)).rejects.toThrow(message);
    await expect(linuxMain(linuxArgv({ stage: undefined }), repo.root)).rejects.toThrow(message);
    unchanged(before);
  });

  it("refuses end to end an artifact whose fingerprint is not the checkout's", async () => {
    quiet();
    const other = darwinArtifactAt((p) => (p.dependencyFingerprint.sha256 = "0".repeat(64)));
    const before = OUTPUTS.map(read);
    await expect(main(darwinArgv({ artifact: other }), repo.root)).rejects.toThrow(
      "artifact dependency fingerprint sha256 differs from this checkout's",
    );
    unchanged(before);
  });

  // Stages Linux then Darwin into a fresh directory; returns it.
  async function stagePair() {
    const stage = join(tempDir("zfb-graph-pair-"), "stage");
    await linuxMain(linuxArgv({ stage }), repo.root);
    await main(darwinArgv({ stage }), repo.root);
    return stage;
  }

  it("stages both platforms into one directory and applies them in one step", async () => {
    quiet();
    const stage = await stagePair();
    unchanged(live());
    const staged = (file) => readFileSync(join(stage, file));
    const linuxNext = JSON.parse(staged(LINUX).toString("utf8"));
    const darwinNext = JSON.parse(staged(DARWIN).toString("utf8"));
    expect(linuxNext.dependencyFingerprint.sha256).toBe(repo.dependencyFingerprint.sha256);
    expect(darwinNext.dependencyFingerprint.sha256).toBe(repo.dependencyFingerprint.sha256);
    // Both platforms' rewrites survive: Darwin composed on Linux's staged copies.
    expect(staged(TEST).toString("utf8")).toBe(
      rewriteDarwinTestPins(rewriteTestPins(saved[TEST].toString("utf8"), linuxNext), darwinNext),
    );
    const readme = staged(README).toString("utf8");
    const linuxAt = readme.indexOf("dependency-graph re-pin\n\nThe Linux contract");
    const darwinAt = readme.indexOf("## Darwin v" + repo.version + " dependency-graph re-pin");
    expect(readme.startsWith(saved[README].toString("utf8"))).toBe(true);
    expect(linuxAt).toBeGreaterThan(0);
    expect(darwinAt).toBeGreaterThan(linuxAt);
    expect(staged(INVENTORY).equals(helperExplainBytes(repo.root))).toBe(true);
    const manifest = JSON.parse(staged("stage-manifest.json").toString("utf8"));
    expect(manifest.runs.map((run) => run.platform)).toEqual(["linux-x64", "darwin-arm64"]);
    expect(manifest.runs[1]).toMatchObject({
      reason: "dependency-graph",
      platform: "darwin-arm64",
      sourceSha: SOURCE_SHA,
      runId: 7,
      artifactId: 9,
    });
    expect(Object.keys(manifest.destinations)).toEqual([...OUTPUTS].sort());
    for (const file of OUTPUTS) {
      expect(manifest.destinations[file].live).toBe(sha(saved[file]));
    }

    applyStage(stage, repo.root, () => {});
    for (const file of OUTPUTS) expect(read(file).equals(staged(file))).toBe(true);
    // A second apply finds every live file moved: nothing is written twice.
    await expect(main(["--apply", stage], repo.root)).rejects.toThrow(
      /moved since staging; nothing was applied/,
    );
  });

  // No temporary file is left beside any destination.
  const noTemps = () => {
    for (const dir of new Set(OUTPUTS.map((file) => dirname(at(file))))) {
      expect(readdirSync(dir).filter((name) => name.includes(".apply-"))).toEqual([]);
    }
  };

  it("changes nothing when writing a temporary file fails during --apply", async () => {
    quiet();
    const stage = await stagePair();
    let writes = 0;
    const io = {
      writeFileSync(path, bytes) {
        if (++writes === OUTPUTS.length) throw new Error("injected write failure");
        writeFileSync(path, bytes);
      },
      renameSync,
    };
    expect(() => applyStage(stage, repo.root, () => {}, io)).toThrow(
      /could not write a temporary file \(injected write failure\); nothing was applied/,
    );
    unchanged(live());
    noTemps();
    applyStage(stage, repo.root, () => {});
    for (const file of OUTPUTS)
      expect(read(file).equals(readFileSync(join(stage, file)))).toBe(true);
  });

  it("restores the destinations already renamed when a rename fails during --apply", async () => {
    quiet();
    const stage = await stagePair();
    let renames = 0;
    const io = {
      writeFileSync,
      renameSync(from, to) {
        if (++renames === 3) throw new Error("injected rename failure");
        renameSync(from, to);
      },
    };
    expect(() => applyStage(stage, repo.root, () => {}, io)).toThrow(
      /renaming .* into place failed \(injected rename failure\); restored the 2 destination\(s\) already renamed, so nothing was applied/,
    );
    unchanged(live());
    noTemps();
  });

  it("refuses --apply when a live destination moved after staging", async () => {
    quiet();
    const stage = await stagePair();
    appendFileSync(at(README), "\nmanual edit after staging\n");
    const before = OUTPUTS.map(read);
    await expect(main(["--apply", stage], repo.root)).rejects.toThrow(
      "research/v3-island-size/README.md moved since staging; nothing was applied",
    );
    unchanged(before);
  });

  it("refuses --apply when a staged file was edited", async () => {
    quiet();
    const stage = await stagePair();
    appendFileSync(join(stage, DARWIN), " ");
    await expect(linuxMain(["--apply", stage], repo.root)).rejects.toThrow(
      "the staged research/v3-island-size/decision.json changed after staging; refusing",
    );
    unchanged(live());
    writeFileSync(join(stage, "stray.txt"), "x");
    await expect(main(["--apply", stage], repo.root)).rejects.toThrow(
      /changed after staging|unlisted files/,
    );
    unchanged(live());
  });

  it("applies nothing when the Darwin stage is refused after Linux staged", async () => {
    quiet();
    const stage = join(tempDir("zfb-graph-half-"), "stage");
    await linuxMain(linuxArgv({ stage }), repo.root);
    await expect(
      main(darwinArgv({ stage, "source-sha": "d".repeat(40) }), repo.root),
    ).rejects.toThrow("is not " + "d".repeat(40));
    await expect(main(["--apply", stage], repo.root)).rejects.toThrow(
      "--reason dependency-graph is not staged for darwin-arm64",
    );
    unchanged(live());
    // Staging the same platform twice into one directory is refused.
    await expect(linuxMain(linuxArgv({ stage }), repo.root)).rejects.toThrow(
      "research/v3-island-size/decision-linux-x64.json is already staged",
    );
  });

  // Paired-run consistency (#4245 finding 1): the two runs must describe one
  // transition and one measured candidate.
  const manifestOf = (stage) =>
    JSON.parse(readFileSync(join(stage, "stage-manifest.json"), "utf8"));

  it("records each paired run's transition identity and accepts the probe head or the merge source", async () => {
    quiet();
    const stage = await stagePair();
    const [linuxRun, darwinRun] = manifestOf(stage).runs;
    const transition = {
      from: {
        packageVersion: repo.version,
        cargoLockSha256: repo.cargoLockSha256,
        dependencyFingerprint: repo.priorFingerprint,
      },
      to: {
        packageVersion: repo.version,
        cargoLockSha256: repo.cargoLockSha256,
        pnpmLockSha256: repo.pnpmLockSha256,
        dependencyFingerprint: {
          ...repo.priorFingerprint,
          sha256: repo.dependencyFingerprint.sha256,
        },
      },
    };
    expect(linuxRun).toEqual({
      reason: "dependency-graph",
      platform: "linux-x64",
      sourceSha: LINUX_SOURCE,
      headSha: SOURCE_SHA,
      runId: 71,
      artifactId: null,
      previousContract: { sha256: sha(saved[LINUX]) },
      transition,
    });
    expect(darwinRun).toEqual({
      reason: "dependency-graph",
      platform: "darwin-arm64",
      sourceSha: SOURCE_SHA,
      runId: 7,
      artifactId: 9,
      previousContract: { sha256: sha(saved[DARWIN]) },
      transition,
    });
    // Darwin measured the Linux probe head: allowed.
    applyStage(stage, repo.root, () => {});
    for (const file of OUTPUTS)
      expect(read(file).equals(readFileSync(join(stage, file)))).toBe(true);
  });

  // Rewrites a staged manifest, standing for a stage directory whose runs were
  // recorded by hand or by an earlier script: --apply must re-check them.
  const editRuns = (stage, edit) => {
    const manifest = manifestOf(stage);
    edit(manifest.runs);
    writeFileSync(join(stage, "stage-manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
  };
  const disagree = (problem) =>
    "--reason dependency-graph paired runs disagree: " +
    problem +
    "; discard the stage directory and stage both platforms from one checkout and one measured candidate";

  const OTHER_HEAD = "c".repeat(40);
  const sourceProblem = () =>
    disagree(
      "darwin-arm64 measured source " +
        SOURCE_SHA +
        " is neither the linux-x64 measured source " +
        LINUX_SOURCE +
        " nor its probe head " +
        OTHER_HEAD,
    );

  it("refuses to stage a Darwin source that is neither the Linux merge source nor its probe head", async () => {
    quiet();
    const problem = sourceProblem();
    const stage = join(tempDir("zfb-graph-pair-"), "stage");
    await linuxMain(linuxArgv({ stage, "head-sha": OTHER_HEAD }), repo.root);
    const half = readdirSync(stage, { recursive: true }).sort();
    await expect(main(darwinArgv({ stage }), repo.root)).rejects.toThrow(
      problem + " (nothing was staged)",
    );
    expect(readdirSync(stage, { recursive: true }).sort()).toEqual(half);
    unchanged(live());
  });

  it("re-checks the recorded sources on --apply: the Linux merge source is allowed, no other", async () => {
    quiet();
    const forged = await stagePair();
    editRuns(forged, (runs) => (runs[0].headSha = OTHER_HEAD));
    await expect(main(["--apply", forged], repo.root)).rejects.toThrow(
      sourceProblem() + " (nothing was applied)",
    );
    unchanged(live());
    editRuns(forged, (runs) => (runs[1].sourceSha = LINUX_SOURCE));
    applyStage(forged, repo.root, () => {});
    for (const file of OUTPUTS) {
      expect(read(file).equals(readFileSync(join(forged, file)))).toBe(true);
    }
  });

  it("refuses paired runs that staged different prior identities", async () => {
    quiet();
    const stale = structuredClone(darwinPinned);
    stale.dependencyFingerprint.sha256 = "f".repeat(64);
    const problem = disagree(
      "transition from.dependencyFingerprint differs (linux-x64 " +
        JSON.stringify(repo.priorFingerprint) +
        ", darwin-arm64 " +
        JSON.stringify({ ...repo.priorFingerprint, sha256: "f".repeat(64) }) +
        ")",
    );
    const stage = join(tempDir("zfb-graph-pair-"), "stage");
    await linuxMain(linuxArgv({ stage }), repo.root);
    writeFileSync(at(DARWIN), contractText(stale));
    await expect(main(darwinArgv({ stage }), repo.root)).rejects.toThrow(
      problem + " (nothing was staged)",
    );
    writeFileSync(at(DARWIN), saved[DARWIN]);
    const forged = await stagePair();
    editRuns(
      forged,
      (runs) => (runs[1].transition.from.dependencyFingerprint.sha256 = "f".repeat(64)),
    );
    await expect(main(["--apply", forged], repo.root)).rejects.toThrow(
      problem + " (nothing was applied)",
    );
    unchanged(live());
  });

  it("refuses paired runs staged from checkouts with different Cargo.lock files", async () => {
    quiet();
    // A second checkout of the same graph whose Cargo.lock differs; every
    // destination Linux staged is byte-identical there, so the live hashes agree.
    const cargoLock = readFileSync(join(repo.root, "Cargo.lock"), "utf8") + "\n";
    const other = makeGraphCheckout(graphRoots, { pnpmLock: movedLock, cargoLock });
    for (const file of [LINUX, TEST, README, INVENTORY]) {
      writeFileSync(join(other.root, file), read(file));
    }
    const otherDarwin = structuredClone(darwinPinned);
    otherDarwin.toolchain.cargoLockSha256 = other.cargoLockSha256;
    writeFileSync(join(other.root, DARWIN), contractText(otherDarwin));
    const otherArtifact = darwinArtifactAt((p) => (p.cargoLockSha256 = other.cargoLockSha256));
    const stage = join(tempDir("zfb-graph-pair-"), "stage");
    await linuxMain(linuxArgv({ stage }), repo.root);
    const before = OUTPUTS.map((file) => readFileSync(join(other.root, file)));
    await expect(main(darwinArgv({ stage, artifact: otherArtifact }), other.root)).rejects.toThrow(
      disagree(
        "transition from.cargoLockSha256 differs (linux-x64 " +
          repo.cargoLockSha256 +
          ", darwin-arm64 " +
          other.cargoLockSha256 +
          "); transition to.cargoLockSha256 differs (linux-x64 " +
          repo.cargoLockSha256 +
          ", darwin-arm64 " +
          other.cargoLockSha256 +
          ")",
      ) + " (nothing was staged)",
    );
    await expect(main(["--apply", stage], other.root)).rejects.toThrow(
      "--reason dependency-graph is not staged for darwin-arm64",
    );
    OUTPUTS.forEach((file, i) =>
      expect(readFileSync(join(other.root, file)).equals(before[i])).toBe(true),
    );
  });

  it("refuses --apply for a stage directory that mixes reasons", async () => {
    quiet();
    const stage = await stagePair();
    const manifest = manifestOf(stage);
    manifest.runs.push({ ...manifest.runs[0], reason: "release-bump" });
    writeFileSync(join(stage, "stage-manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
    await expect(main(["--apply", stage], repo.root)).rejects.toThrow(
      "the stage directory mixes --reason dependency-graph, release-bump; stage one reason per directory (nothing was applied)",
    );
    manifest.runs.pop();
    manifest.runs[1].reason = "fingerprint-policy";
    writeFileSync(join(stage, "stage-manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
    await expect(main(["--apply", stage], repo.root)).rejects.toThrow(
      "the stage directory mixes --reason dependency-graph, fingerprint-policy",
    );
    unchanged(live());
  });

  it("refuses --apply for a paired run staged without a transition identity", async () => {
    quiet();
    const stage = await stagePair();
    const manifest = manifestOf(stage);
    delete manifest.runs[1].transition;
    writeFileSync(join(stage, "stage-manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
    await expect(main(["--apply", stage], repo.root)).rejects.toThrow(
      "darwin-arm64 run records no transition identity",
    );
    unchanged(live());
  });

  // Partial staging (#4245 finding 3): a write that fails mid-commit leaves a
  // partial stage directory, which neither --apply nor a later --stage accepts.
  const failingStageIo = (failAt) => {
    let writes = 0;
    return {
      mkdirSync,
      writeFileSync(path, bytes) {
        if (++writes === failAt) throw new Error("injected stage write failure");
        writeFileSync(path, bytes);
      },
    };
  };
  const RECOVERY = "discard the stage directory and stage again from the start";
  const stagingFailed =
    "staging failed (injected stage write failure); the stage directory may now hold a partial stage, which --apply and --stage refuse: " +
    RECOVERY;

  it("refuses a first stage that failed mid-write, before its manifest, and recovers from a fresh one", async () => {
    quiet();
    const stage = join(tempDir("zfb-graph-partial-"), "stage");
    await expect(linuxMain(linuxArgv({ stage }), repo.root, failingStageIo(2))).rejects.toThrow(
      stagingFailed,
    );
    expect(readdirSync(stage, { recursive: true }).sort()).toEqual([
      "research",
      "research/v3-island-size",
      "research/v3-island-size/decision-linux-x64.json",
      "scripts",
      "scripts/__tests__",
    ]);
    await expect(main(["--apply", stage], repo.root)).rejects.toThrow(
      "no stage-manifest.json in the stage directory",
    );
    const noManifest =
      "the stage directory is not empty and has no stage-manifest.json; refusing (" +
      RECOVERY +
      ")";
    await expect(linuxMain(linuxArgv({ stage }), repo.root)).rejects.toThrow(noManifest);
    await expect(main(darwinArgv({ stage }), repo.root)).rejects.toThrow(noManifest);
    unchanged(live());
    // The recovery the errors name: discard the directory and stage both platforms again.
    rmSync(stage, { recursive: true, force: true });
    await linuxMain(linuxArgv({ stage }), repo.root);
    await main(darwinArgv({ stage }), repo.root);
    applyStage(stage, repo.root, () => {});
    for (const file of OUTPUTS)
      expect(read(file).equals(readFileSync(join(stage, file)))).toBe(true);
  });

  it("refuses a second stage that failed after writing a new file", async () => {
    quiet();
    const stage = join(tempDir("zfb-graph-partial-"), "stage");
    await linuxMain(linuxArgv({ stage }), repo.root);
    await expect(main(darwinArgv({ stage }), repo.root, failingStageIo(2))).rejects.toThrow(
      stagingFailed,
    );
    const unlisted = "unlisted files in the stage directory: " + DARWIN + " (" + RECOVERY + ")";
    await expect(main(["--apply", stage], repo.root)).rejects.toThrow(unlisted);
    await expect(main(darwinArgv({ stage }), repo.root)).rejects.toThrow(unlisted);
    await expect(linuxMain(linuxArgv({ stage }), repo.root)).rejects.toThrow(unlisted);
    unchanged(live());
  });

  it("refuses a second stage whose manifest write failed", async () => {
    quiet();
    const stage = join(tempDir("zfb-graph-partial-"), "stage");
    await linuxMain(linuxArgv({ stage }), repo.root);
    // Every output is written; only the manifest naming the Darwin run is not.
    await expect(main(darwinArgv({ stage }), repo.root, failingStageIo(5))).rejects.toThrow(
      stagingFailed,
    );
    const changed = "the staged " + README + " changed after staging; refusing (" + RECOVERY + ")";
    await expect(main(["--apply", stage], repo.root)).rejects.toThrow(changed);
    await expect(main(darwinArgv({ stage }), repo.root)).rejects.toThrow(changed);
    unchanged(live());
  });

  it("refuses --apply combined with any other argument and a stage directory that aliases the checkout", async () => {
    quiet();
    expect(() => parseArgs(["--apply", "x", "--reason", "dependency-graph"])).toThrow(
      "--apply takes no other argument; remove --reason",
    );
    await expect(main(darwinArgv({ stage: repo.root }), repo.root)).rejects.toThrow(
      /resolves to or contains the checkout/,
    );
    await expect(main(["--apply", repo.root], repo.root)).rejects.toThrow(
      /resolves to or contains the checkout/,
    );
    unchanged(live());
  });
});

// End-to-end CLI runs against one test-fixture checkout — synthetic prior
// policy, not native evidence: the checkout's COPY of lock-fingerprint.mjs
// computes policy 2 (the live helper stays at policy 1), both contracts pin
// the live helper's policy-1 fingerprint of the same lock, and each
// platform's artifact was measured at the checkout carrying the policy-2
// helper. The shared refusal matrix is covered in
// rebaseline-island-size-linux.test.mjs.
describe("rebaseline-island-size-darwin fingerprint-policy (#4218)", () => {
  const policyRoots = [];
  const DARWIN = "research/v3-island-size/decision.json";
  const LINUX = "research/v3-island-size/decision-linux-x64.json";
  const TEST = "scripts/__tests__/island-size-budget.test.mjs";
  const README = "research/v3-island-size/README.md";
  const INVENTORY = "research/v3-island-size/lock-fingerprint-inventory.json";
  const POLICY = "research/v3-island-size/LOCK-FINGERPRINT-POLICY.md";
  const OUTPUTS = [DARWIN, LINUX, TEST, README, INVENTORY, POLICY];
  const LINUX_SOURCE = "b".repeat(40);
  let repo;
  let darwinArtifact;
  let linuxArtifact;
  let darwinPinned;
  let saved;
  const at = (relative) => join(repo.root, relative);
  const read = (relative) => readFileSync(at(relative));
  const contractOf = (file) => JSON.parse(read(file).toString("utf8"));
  const contractText = (value) => JSON.stringify(value, null, 2) + "\n";
  const liveOf = (file) => JSON.parse(readFileSync(join(testDir, "../..", file), "utf8"));
  const policyTempDir = () => {
    const dir = mkdtempSync(join(tmpdir(), "zfb-policy-stage-"));
    policyRoots.push(dir);
    return dir;
  };

  function darwinArtifactAt(tweak) {
    const dir = makeArtifact({
      version: repo.version,
      tweak: (p, mode) => {
        p.fixtureSha256 = repo.fixtureSha256;
        p.runnerSha256 = repo.runnerSha256;
        p.pnpmLockSha256 = repo.pnpmLockSha256;
        p.cargoLockSha256 = repo.cargoLockSha256;
        p.dependencyFingerprint = {
          policyVersion: repo.dependencyFingerprint.policyVersion,
          sha256: repo.dependencyFingerprint.sha256,
        };
        tweak?.(p, mode);
      },
    });
    // Kept for the whole describe, not removed by the per-test cleanup.
    policyRoots.push(roots.splice(roots.indexOf(dir), 1)[0]);
    return dir;
  }

  beforeAll(() => {
    repo = makePolicyCheckout(policyRoots, { editHelper: policyHelper.v2 });
    darwinArtifact = darwinArtifactAt();
    linuxArtifact = mkdtempSync(join(tmpdir(), "zfb-policy-linux-artifact-"));
    policyRoots.push(linuxArtifact);
    const linuxLive = liveOf(LINUX);
    writeRuns(linuxArtifact, Object.keys(linuxLive.ceilings.workspace), (mode) => ({
      sourceSha: LINUX_SOURCE,
      mode,
      platform: { os: "linux", arch: "x64" },
      fixtureVersion: linuxLive.fixtureVersion,
      fixtureSha256: repo.fixtureSha256,
      runnerSha256: repo.runnerSha256,
      packageVersion: repo.version,
      runtimePackageVersion: repo.version,
      honoPackageVersion: linuxLive.toolchain.honoVersion,
      esbuildVersion: linuxLive.toolchain.esbuildVersion,
      nodeVersion: linuxLive.toolchain.nodeVersion,
      zlibVersion: linuxLive.toolchain.zlibVersion,
      compression: linuxLive.compression,
      repeats: 2,
      pnpmLockSha256: repo.pnpmLockSha256,
      dependencyFingerprint: {
        policyVersion: repo.dependencyFingerprint.policyVersion,
        sha256: repo.dependencyFingerprint.sha256,
      },
      cargoLockSha256: repo.cargoLockSha256,
    }));
    const pin = (contract, artifactDir) => {
      contract.ceilings = ceilingsOf(artifactDir);
      contract.fixtureSha256 = { ...repo.fixtureSha256 };
      contract.runnerSha256 = { ...repo.runnerSha256 };
      contract.toolchain.packageVersion = repo.version;
      contract.toolchain.pnpmLockSha256 = repo.pnpmLockSha256;
      contract.toolchain.cargoLockSha256 = repo.cargoLockSha256;
      contract.dependencyFingerprint = { ...repo.priorFingerprint };
      return contract;
    };
    darwinPinned = pin(liveOf(DARWIN), darwinArtifact);
    darwinPinned.fixtureVersion = legacy.fixtureVersion;
    darwinPinned.compression = legacy.compression;
    darwinPinned.normalizerSha256 = normalizerSha256();
    for (const key of ["nodeVersion", "zlibVersion", "esbuildVersion", "honoVersion"]) {
      darwinPinned.toolchain[key] = legacy.toolchain[key];
    }
    writeFileSync(at(DARWIN), contractText(darwinPinned));
    writeFileSync(at(LINUX), contractText(pin(liveOf(LINUX), linuxArtifact)));
    saved = Object.fromEntries(OUTPUTS.map((file) => [file, read(file)]));
  });
  afterEach(() => {
    for (const [file, bytes] of Object.entries(saved)) writeFileSync(at(file), bytes);
    vi.restoreAllMocks();
  });
  afterAll(() => {
    for (const dir of policyRoots.splice(0)) rmSync(dir, { recursive: true, force: true });
  });

  const darwinArgv = (extra = {}) =>
    Object.entries({
      ...args,
      artifact: darwinArtifact,
      reason: "fingerprint-policy",
      stage: join(policyTempDir(), "stage"),
      ...extra,
    })
      .filter(([, value]) => value !== undefined)
      .flatMap(([key, value]) => ["--" + key, value]);
  const linuxArgv = (extra = {}) =>
    Object.entries({
      artifact: linuxArtifact,
      "run-id": "71",
      "job-id": "72",
      // Linux measured the PR merge source of the probe head Darwin measured natively.
      "head-sha": SOURCE_SHA,
      "source-sha": LINUX_SOURCE,
      reason: "fingerprint-policy",
      ...extra,
    }).flatMap(([key, value]) => ["--" + key, value]);
  const quiet = () => {
    vi.spyOn(console, "log").mockImplementation(() => {});
    vi.spyOn(console, "error").mockImplementation(() => {});
  };
  const unchanged = (before) =>
    OUTPUTS.forEach((file, i) => expect(read(file).equals(before[i])).toBe(true));

  it("re-pins the four fingerprint keys end to end and rewrites the policy identity line", async () => {
    quiet();
    const stage = join(policyTempDir(), "stage");
    await main(darwinArgv({ stage }), repo.root);
    unchanged(OUTPUTS.map((file) => saved[file]));
    landStaged(stage, repo.root);
    const next = contractOf(DARWIN);
    expect(
      contractDiffKeys(darwinPinned, next).filter((key) => !key.startsWith("baselineEvidence.")),
    ).toEqual([
      "baselineSourceSha",
      "dependencyFingerprint.helperSha256",
      "dependencyFingerprint.policyVersion",
      "dependencyFingerprint.sha256",
      "outcome",
      "platformStatus",
    ]);
    expect(() =>
      assertAllowedMovement("fingerprint-policy", FINGERPRINT_POLICY_MOVEMENT, darwinPinned, next),
    ).not.toThrow();
    expect(next.dependencyFingerprint).toEqual(repo.dependencyFingerprint);
    expect(next.outcome).toBe("fingerprint-policy");
    expect(next.baselineEvidence).toMatchObject({
      event: "ci-artifact",
      runId: 7,
      artifact: { id: 9 },
      originalCheckerResult: { exitCode: 1 },
      fingerprintPolicyTransition: { from: repo.priorFingerprint, to: repo.dependencyFingerprint },
    });
    expect(Object.keys(next.baselineEvidence.repinnedFrom)).toEqual(REPINNED_FROM_KEYS);
    expect(read(LINUX).equals(saved[LINUX])).toBe(true);
    expect(read(INVENTORY).equals(helperExplainBytes(repo.root))).toBe(true);
    expect(read(POLICY).toString("utf8")).toContain(
      'Policy identity: `policyVersion: 2`, `parser: "yaml@2.9.0"`',
    );
    expect(read(TEST).toString("utf8")).toBe(
      rewriteDarwinTestPins(saved[TEST].toString("utf8"), next),
    );
    const readme = read(README).toString("utf8");
    expect(readme).toContain("## Darwin v" + repo.version + " fingerprint-policy re-pin");
    expect(readme).toContain("Linux x64 moves in the same paired change");
  });

  it("refuses the version-step and checkout-helper violations in the pure re-pin", () => {
    const artifact = loadDarwinArtifact(darwinArtifact);
    const evidence = evidenceFor({ ...args, reason: "fingerprint-policy" }, artifact);
    const checkoutHelper = {
      policyVersion: repo.dependencyFingerprint.policyVersion,
      parser: repo.dependencyFingerprint.parser,
      helperSha256: repo.dependencyFingerprint.helperSha256,
    };
    const checkout = { dependencyFingerprint: repo.dependencyFingerprint, checkoutHelper };
    const run =
      (fingerprint, check = checkout) =>
      () =>
        repinDarwinFingerprintPolicy(
          { ...darwinPinned, dependencyFingerprint: fingerprint },
          totalsOf(artifact),
          evidence,
          check,
        );
    expect(run(repo.priorFingerprint)).not.toThrow();
    expect(run(repo.dependencyFingerprint)).toThrow(
      "--reason fingerprint-policy: policyVersion did not move; use fingerprint-helper or dependency-graph",
    );
    expect(run({ ...repo.priorFingerprint, policyVersion: 0 })).toThrow(
      "policyVersion must increase by exactly one",
    );
    expect(
      run(repo.priorFingerprint, {
        ...checkout,
        checkoutHelper: { ...checkoutHelper, policyVersion: 3 },
      }),
    ).toThrow("does not match the helper's exported FINGERPRINT_POLICY_VERSION 3");
    expect(run({ ...repo.dependencyFingerprint, helperSha256: "1".repeat(64) })).toThrow(
      "helperSha256 moved without a policyVersion step",
    );
    expect(() =>
      repinDarwinFingerprintPolicy(
        { ...darwinPinned, allowance: { raw: 0, gzipNormalized: 1 } },
        totalsOf(artifact),
        evidence,
        checkout,
      ),
    ).toThrow(
      "--reason fingerprint-policy moves only the dependency fingerprint, together with a policyVersion step, but allowance differ",
    );
  });

  const edit = (change) => () => {
    const contract = structuredClone(darwinPinned);
    change(contract);
    writeFileSync(at(DARWIN), contractText(contract));
  };
  it.each([
    [
      "+0",
      edit((c) => (c.dependencyFingerprint = { ...repo.dependencyFingerprint })),
      {},
      "policyVersion did not move; use fingerprint-helper or dependency-graph",
    ],
    [
      "-1",
      edit((c) => (c.dependencyFingerprint.policyVersion = 3)),
      {},
      "policyVersion must increase by exactly one",
    ],
    [
      "raw +1",
      edit((c) => (c.ceilings.workspace.model.raw -= 1)),
      {},
      "raw totals changed (workspace/model)",
    ],
    [
      "a changed Cargo.lock",
      edit((c) => (c.toolchain.cargoLockSha256 = "0".repeat(64))),
      {},
      "--reason fingerprint-policy cannot move Cargo.lock",
    ],
    [
      "a Hono bump",
      edit((c) => (c.toolchain.honoVersion = "0.0.0-test")),
      {},
      "honoPackageVersion differs from the contract",
    ],
    [
      "a missing policy identity line",
      () =>
        writeFileSync(at(POLICY), saved[POLICY].toString("utf8").replace("Policy identity:", "x:")),
      {},
      "policy identity line not found in LOCK-FINGERPRINT-POLICY.md",
    ],
    [
      "a cargo-only flag",
      null,
      { "added-packages": "ctor@0.2.9" },
      "--added-packages is only for --reason cargo-producer-lock",
    ],
    [
      "a --source-sha that is not the artifact's",
      null,
      { "source-sha": "d".repeat(40) },
      "source SHA " + SOURCE_SHA + " is not " + "d".repeat(40),
    ],
  ])("refuses %s end to end and writes nothing", async (_name, setup, extra, message) => {
    quiet();
    setup?.();
    const before = OUTPUTS.map(read);
    await expect(main(darwinArgv(extra), repo.root)).rejects.toThrow(message);
    unchanged(before);
  });

  it("refuses to run without --stage, in parseArgs, before reading or writing anything", async () => {
    const message =
      "--reason fingerprint-policy re-pins both platforms together and only stages: pass --stage <dir> for Linux x64 and Darwin arm64 into the same directory, then --apply <dir> (nothing was written)";
    expect(() => parseArgs(darwinArgv({ stage: undefined }))).toThrow(message);
    const before = OUTPUTS.map(read);
    await expect(main(darwinArgv({ stage: undefined }), repo.root)).rejects.toThrow(message);
    unchanged(before);
  });

  it("refuses end to end an artifact at policyVersion 1 against the policy-2 checkout", async () => {
    quiet();
    const old = darwinArtifactAt((p) => (p.dependencyFingerprint.policyVersion = 1));
    const before = OUTPUTS.map(read);
    await expect(main(darwinArgv({ artifact: old }), repo.root)).rejects.toThrow(
      "artifact dependency fingerprint policyVersion differs from this checkout's",
    );
    unchanged(before);
  });

  it("stages both platforms into one directory, policy document included, and applies them together", async () => {
    quiet();
    const stage = join(mkdtempSync(join(tmpdir(), "zfb-policy-pair-")), "stage");
    policyRoots.push(dirname(stage));
    await linuxMain(linuxArgv({ stage }), repo.root);
    await expect(main(["--apply", stage], repo.root)).rejects.toThrow(
      "--reason fingerprint-policy is not staged for darwin-arm64",
    );
    await main(darwinArgv({ stage }), repo.root);
    unchanged(OUTPUTS.map((file) => saved[file]));
    const staged = (file) => readFileSync(join(stage, file));
    const linuxNext = JSON.parse(staged(LINUX).toString("utf8"));
    const darwinNext = JSON.parse(staged(DARWIN).toString("utf8"));
    expect(linuxNext.dependencyFingerprint).toEqual(repo.dependencyFingerprint);
    expect(darwinNext.dependencyFingerprint).toEqual(repo.dependencyFingerprint);
    expect(staged(POLICY).toString("utf8")).toBe(
      saved[POLICY]
        .toString("utf8")
        .replace(
          'Policy identity: `policyVersion: 1`, `parser: "yaml@2.9.0"`',
          'Policy identity: `policyVersion: 2`, `parser: "yaml@2.9.0"`',
        ),
    );
    const readme = staged(README).toString("utf8");
    expect(
      readme.indexOf("## Darwin v" + repo.version + " fingerprint-policy re-pin"),
    ).toBeGreaterThan(readme.indexOf("fingerprint-policy re-pin\n\nThe Linux contract"));
    const manifest = JSON.parse(staged("stage-manifest.json").toString("utf8"));
    expect(manifest.runs.map((run) => run.platform + " " + run.reason)).toEqual([
      "linux-x64 fingerprint-policy",
      "darwin-arm64 fingerprint-policy",
    ]);
    expect(Object.keys(manifest.destinations)).toEqual([...OUTPUTS].sort());
    applyStage(stage, repo.root, () => {});
    for (const file of OUTPUTS) expect(read(file).equals(staged(file))).toBe(true);
  });
});
