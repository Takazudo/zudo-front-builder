import { createHash } from "node:crypto";
import {
  appendFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vite-plus/test";

import {
  NORMALIZED_SIDECAR,
  normalizeFixtureFiles,
  normalizeMeasurementDirectory,
  normalizerSha256,
} from "../../research/v3-island-size/normalize-identity.mjs";
import {
  CARGO_PRODUCER_LOCK_MOVEMENT,
  FINGERPRINT_HELPER_MOVEMENT,
  assertLinuxX64Provenance,
  contractDiffKeys,
  measuredTotals,
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
  darwinRepinReadmeSection,
  rewriteDarwinTestPins,
  transitionDarwinCargoLock,
  validateDarwinEvidence,
} from "../rebaseline-island-size-darwin.mjs";
import { asSchemaV2, makeCheckout, makeHelperCheckout } from "./fixtures/island-size-checkout.mjs";

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
    const next = migrateDarwinContract(contract, totalsOf(artifact), buildEvidence(args, artifact));
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
      "--reason must be one of metric-migration, release-bump, runtime-identity, fingerprint-migration, fingerprint-helper",
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
      buildEvidence(args, artifact),
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
    expect(() => migrateDarwinContract(rawMoved, totals, buildEvidence(args, artifact))).toThrow(
      "raw totals changed (packed/model)",
    );

    const missing = contractFor(root);
    delete missing.ceilings.workspace["json-api"];
    expect(checkMigrationTotals(missing, totals).join("\n")).toContain(
      "fixture set differs from the contract (workspace/json-api)",
    );

    const grown = contractFor(root);
    grown.ceilings.workspace["show-for"].gzip = totals.workspace["show-for"].gzipNormalized - 1;
    expect(() => migrateDarwinContract(grown, totals, buildEvidence(args, artifact))).toThrow(
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
    buildEvidence(args, artifact),
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
        buildEvidence({ ...args, reason }, artifact),
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
        repinnedFrom: { packageVersion: "9.9.8", baselineSourceSha: "b".repeat(40) },
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
          buildEvidence({ ...args, reason }, artifact),
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
          buildEvidence(args, artifact),
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
          buildEvidence(args, artifact),
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
        buildEvidence(args, artifact),
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
    const evidence = buildEvidence(migrationArgs, artifact);
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
      buildEvidence(migrationArgs, artifact),
      checkout,
    );
    expect(validateDarwinEvidence(v3, artifact, SOURCE_SHA, "fingerprint-migration")).toContain(
      "contract is not a schemaVersion 2 gzip-identity-normalized v1 contract",
    );
    expect(() =>
      migrateDarwinFingerprint(
        v3,
        totalsOf(artifact),
        buildEvidence(migrationArgs, artifact),
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
    const evidence = buildEvidence(migrationArgs, artifact);
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
        buildEvidence(migrationArgs, artifact),
        checkout,
      );
      v3.toolchain.packageVersion = "9.9.8";
      expect(validateDarwinEvidence(v3, artifact, SOURCE_SHA, reason)).toEqual([]);
      const evidence = buildEvidence({ ...args, reason }, artifact);
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

  it("runs end-to-end against a disposable checkout", () => {
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
    main(argv, repo.root);
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

  it("refuses end-to-end an artifact from another checkout whose graph matches", () => {
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
    expect(() => main(otherArgv, repo.root)).toThrow(
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
    oldLockSource: "old.lock",
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
      evidence ?? buildEvidence(cargoArgs, artifact),
      check,
      lock,
    );

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
      cargoTransition: {
        oldSha: OLD,
        newSha: NEW,
        added: ADDED,
        changed: [],
        removed: [],
        oldLockSource: "old.lock",
      },
      originalCheckerResult: { exitCode: 1 },
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

  it("takes the four Cargo arguments for cargo-producer-lock alone", () => {
    const cli = {
      ...cargoArgs,
      "expected-old-cargo-sha": OLD,
      "expected-new-cargo-sha": NEW,
      "old-cargo-lock": "old.lock",
      "added-packages": ADDED.join(","),
    };
    const argv = (values) => Object.entries(values).flatMap(([k, v]) => ["--" + k, v]);
    expect(parseArgs(argv(cli))).toMatchObject(cli);
    const { "added-packages": _a, ...withoutAdded } = cli;
    expect(() => parseArgs(argv(withoutAdded))).toThrow("required --added-packages");
    expect(() =>
      parseArgs(argv({ ...args, reason: "release-bump", "old-cargo-lock": "old.lock" })),
    ).toThrow("--old-cargo-lock is only for --reason cargo-producer-lock");
  });

  it("runs end to end against a disposable checkout at the new lock", () => {
    vi.spyOn(console, "log").mockImplementation(() => {});
    const repo = makeCheckout(roots, { cargoLock: newText });
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
    contract.schemaVersion = 3;
    contract.dependencyFingerprint = { ...repo.dependencyFingerprint };
    contract.fixtureSha256 = repo.fixtureSha256;
    contract.runnerSha256 = repo.runnerSha256;
    contract.toolchain.packageVersion = repo.version;
    contract.toolchain.pnpmLockSha256 = repo.pnpmLockSha256;
    contract.toolchain.cargoLockSha256 = OLD;
    const contractPath = join(repo.root, "research/v3-island-size/decision.json");
    writeFileSync(contractPath, JSON.stringify(contract, null, 2) + "\n");
    const oldPath = join(repo.root, "old.lock");
    writeFileSync(oldPath, oldText);
    const argv = Object.entries({
      ...cargoArgs,
      artifact: root,
      "expected-old-cargo-sha": OLD,
      "expected-new-cargo-sha": NEW,
      "old-cargo-lock": oldPath,
      "added-packages": ADDED.join(","),
    }).flatMap(([k, v]) => ["--" + k, v]);
    main(argv, repo.root);
    const written = JSON.parse(readFileSync(contractPath, "utf8"));
    expect(written.toolchain).toEqual({ ...contract.toolchain, cargoLockSha256: NEW });
    expect(written.ceilings).toEqual(contract.ceilings);
    expect(written.baselineEvidence.cargoTransition).toMatchObject({ oldSha: OLD, newSha: NEW });
    const pins = readFileSync(
      join(repo.root, "scripts/__tests__/island-size-budget.test.mjs"),
      "utf8",
    );
    expect(pins).toContain(JSON.stringify(NEW));
    expect(readFileSync(join(repo.root, "research/v3-island-size/README.md"), "utf8")).toContain(
      "cargo-producer-lock transition",
    );
  });
});
